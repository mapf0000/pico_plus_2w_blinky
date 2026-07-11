use super::model::TransferRecord;
use super::protocol::{TransferControlEvent, decode_chunk_envelope};
use super::storage::{storage_queue_chunk_persist, storage_queue_clear_transfer_chunks};
use super::{
    ChunkCounters, ChunkState, FinalizeError, FinalizePlan, TransferState, TransferStore,
    TransferView,
};
use crc32fast::Hasher as Crc32Hasher;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

const EVENT_VERSION: u16 = 1;
const EWMA_ALPHA: f64 = 0.2;
const RATE_EPSILON: f64 = 1e-3;

impl TransferStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply_text_event(&mut self, event_json: &str, now_ms: f64) -> Result<(), String> {
        let event: TransferControlEvent = serde_json::from_str(event_json)
            .map_err(|err| format!("transfer event parse: {err}"))?;

        match event {
            TransferControlEvent::Open {
                version,
                transfer_id,
                file_name,
                total_size,
                chunk_size,
                chunk_count,
                sha256,
            } => {
                if version != EVENT_VERSION {
                    return Err(format!("unsupported transfer event version {version}"));
                }
                if chunk_size == 0 && chunk_count > 0 {
                    return Err("invalid transfer metadata: chunk_size=0".to_string());
                }

                storage_queue_clear_transfer_chunks(transfer_id);

                let record = TransferRecord {
                    transfer_id,
                    file_name,
                    total_size,
                    chunk_size,
                    chunk_count,
                    sha256_hex: sha256,
                    received_size: 0,
                    finished_chunks: 0,
                    failed_chunks: 0,
                    retrying_chunks: 0,
                    status: TransferState::Open,
                    updated_at_ms: now_ms,
                    smoothed_rate_bps: 0.0,
                    eta_total_secs: None,
                    last_rate_sample_ms: now_ms,
                    last_rate_sample_bytes: 0,
                    next_expected_chunk: 0,
                    rolling_sha256: Sha256::new(),
                    chunk_states: HashMap::new(),
                };
                self.transfers.insert(transfer_id, record);
            }
            TransferControlEvent::Progress {
                version,
                transfer_id,
                received_size,
                total_size,
                finished_chunks,
                chunk_count,
            } => {
                if version != EVENT_VERSION {
                    return Err(format!("unsupported transfer event version {version}"));
                }
                if let Some(record) = self.transfers.get_mut(&transfer_id) {
                    record.total_size = total_size;
                    record.chunk_count = chunk_count;
                    record.received_size = record.received_size.max(received_size.min(total_size));
                    record.finished_chunks =
                        record.finished_chunks.max(finished_chunks.min(chunk_count));
                    if record.status == TransferState::Open {
                        record.status = TransferState::Downloading;
                    }
                    update_transfer_rate(record, now_ms);
                }
            }
            TransferControlEvent::ChunkStatus {
                version,
                transfer_id,
                chunk_index,
                status,
                attempts: _attempts,
                error,
            } => {
                if version != EVENT_VERSION {
                    return Err(format!("unsupported transfer event version {version}"));
                }
                if let Some(record) = self.transfers.get_mut(&transfer_id) {
                    if chunk_index >= record.chunk_count {
                        return Ok(());
                    }

                    let mut next_state = map_chunk_status(&status);
                    if error.is_some() && next_state != ChunkState::Finished {
                        next_state = ChunkState::Retrying;
                    }
                    update_chunk_state(record, chunk_index, next_state);
                    record.updated_at_ms = now_ms;
                }
            }
            TransferControlEvent::Finished {
                version,
                transfer_id,
            } => {
                if version != EVENT_VERSION {
                    return Err(format!("unsupported transfer event version {version}"));
                }
                if let Some(record) = self.transfers.get_mut(&transfer_id) {
                    record.status = TransferState::Finished;
                    record.updated_at_ms = now_ms;
                    record.eta_total_secs = Some(0.0);
                }
            }
            TransferControlEvent::Failed {
                version,
                transfer_id,
                reason,
            } => {
                if version != EVENT_VERSION {
                    return Err(format!("unsupported transfer event version {version}"));
                }
                if let Some(record) = self.transfers.get_mut(&transfer_id) {
                    record.status = TransferState::Failed;
                    record.updated_at_ms = now_ms;
                    record.eta_total_secs = None;
                    if !reason.is_empty() {
                        record.failed_chunks = record.failed_chunks.max(1);
                    }
                }
            }
            TransferControlEvent::Aborted {
                version,
                transfer_id,
                reason_code,
                detail,
            } => {
                if version != EVENT_VERSION {
                    return Err(format!("unsupported transfer event version {version}"));
                }
                if let Some(record) = self.transfers.get_mut(&transfer_id) {
                    record.status = TransferState::Aborted;
                    record.updated_at_ms = now_ms;
                    if reason_code > 0 || !detail.is_empty() {
                        record.failed_chunks = record.failed_chunks.max(1);
                    }
                }
            }
        }

        Ok(())
    }

    pub fn apply_binary_chunk(&mut self, frame: &[u8], now_ms: f64) -> Result<(), String> {
        let envelope = decode_chunk_envelope(frame)?;

        let mut crc32 = Crc32Hasher::new();
        crc32.update(&envelope.payload);
        let checksum = crc32.finalize();
        if checksum != envelope.crc32 {
            return Err(format!(
                "crc32 mismatch for transfer {} chunk {}",
                envelope.transfer_id, envelope.chunk_index
            ));
        }

        let Some(record) = self.transfers.get_mut(&envelope.transfer_id) else {
            return Err(format!(
                "chunk for unknown transfer {}",
                envelope.transfer_id
            ));
        };

        if envelope.chunk_index >= record.chunk_count {
            return Err(format!(
                "chunk index {} out of bounds for transfer {}",
                envelope.chunk_index, envelope.transfer_id
            ));
        }

        if envelope.chunk_index != record.next_expected_chunk {
            return Err(format!(
                "out-of-order chunk for transfer {}: got {}, expected {}",
                envelope.transfer_id, envelope.chunk_index, record.next_expected_chunk
            ));
        }

        let expected_offset = envelope.chunk_index as u64 * record.chunk_size as u64;
        if envelope.offset != expected_offset {
            return Err(format!(
                "offset mismatch for transfer {} chunk {}: got {}, expected {}",
                envelope.transfer_id, envelope.chunk_index, envelope.offset, expected_offset
            ));
        }

        let expected_len = expected_chunk_len(record, envelope.chunk_index)?;
        if envelope.payload_len as usize != expected_len {
            return Err(format!(
                "chunk length mismatch for transfer {} chunk {}: got {}, expected {}",
                envelope.transfer_id, envelope.chunk_index, envelope.payload_len, expected_len
            ));
        }

        storage_queue_chunk_persist(
            envelope.transfer_id,
            envelope.chunk_index,
            &envelope.payload,
        );

        update_chunk_state(record, envelope.chunk_index, ChunkState::Finished);

        record.received_size = record
            .received_size
            .saturating_add(envelope.payload.len() as u64)
            .min(record.total_size);
        record.finished_chunks = record.finished_chunks.saturating_add(1);
        record.next_expected_chunk = record.next_expected_chunk.saturating_add(1);
        record.rolling_sha256.update(&envelope.payload);

        if matches!(
            record.status,
            TransferState::Open | TransferState::Downloading
        ) {
            record.status = TransferState::Downloading;
        }

        update_transfer_rate(record, now_ms);
        Ok(())
    }

    pub fn tick(&mut self, now_ms: f64) {
        for record in self.transfers.values_mut() {
            record.updated_at_ms = now_ms;
            update_transfer_eta(record);
        }
    }

    pub fn snapshots(&self) -> Vec<TransferView> {
        let mut rows = self
            .transfers
            .values()
            .map(|record| {
                let finished = record.finished_chunks.min(record.chunk_count);
                let failed = record
                    .failed_chunks
                    .min(record.chunk_count.saturating_sub(finished));
                let retrying = record
                    .retrying_chunks
                    .min(record.chunk_count.saturating_sub(finished + failed));
                let downloading = if record.status == TransferState::Downloading
                    && finished < record.chunk_count
                {
                    1
                } else {
                    0
                }
                .min(
                    record
                        .chunk_count
                        .saturating_sub(finished + failed + retrying),
                );
                let open = record
                    .chunk_count
                    .saturating_sub(finished + failed + retrying + downloading);

                TransferView {
                    transfer_id: record.transfer_id,
                    file_name: record.file_name.clone(),
                    total_size: record.total_size,
                    received_size: record.received_size,
                    chunk_count: record.chunk_count,
                    finished_chunks: record.finished_chunks,
                    failed_chunks: record.failed_chunks,
                    status: record.status.clone(),
                    smoothed_rate_bps: record.smoothed_rate_bps,
                    eta_total_secs: record.eta_total_secs,
                    counters: ChunkCounters {
                        open,
                        downloading,
                        finished,
                        retrying,
                        failed,
                    },
                    updated_at_ms: record.updated_at_ms,
                }
            })
            .collect::<Vec<_>>();

        rows.sort_by(|left, right| {
            right
                .updated_at_ms
                .partial_cmp(&left.updated_at_ms)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        rows
    }

    pub fn prepare_finalize(&mut self, transfer_id: u64) -> Result<FinalizePlan, String> {
        let Some(record) = self.transfers.get_mut(&transfer_id) else {
            return Err(format!("unknown transfer {transfer_id}"));
        };

        if record.next_expected_chunk != record.chunk_count {
            return Err(format!(
                "transfer {} has missing chunks: got {} of {}",
                transfer_id, record.next_expected_chunk, record.chunk_count
            ));
        }
        if record.received_size != record.total_size {
            return Err(format!(
                "transfer {} size mismatch: received {} of {} bytes",
                transfer_id, record.received_size, record.total_size
            ));
        }

        record.status = TransferState::Verifying;
        record.updated_at_ms = web_now_ms();

        let computed_sha256_hex = hex_lower(&record.rolling_sha256.clone().finalize());

        Ok(FinalizePlan {
            transfer_id,
            file_name: record.file_name.clone(),
            chunk_count: record.chunk_count,
            expected_sha256_hex: record.sha256_hex.clone(),
            computed_sha256_hex,
        })
    }

    pub fn finish_finalize(&mut self, transfer_id: u64, result: &Result<(), FinalizeError>) {
        if let Some(record) = self.transfers.get_mut(&transfer_id) {
            match result {
                Ok(()) => {
                    record.status = TransferState::Finished;
                    record.eta_total_secs = Some(0.0);
                }
                Err(err) => {
                    if err.is_verification() {
                        record.status = TransferState::Failed;
                        record.failed_chunks = record.failed_chunks.saturating_add(1);
                        record.eta_total_secs = None;
                    } else {
                        // Keep completed transfers retryable after save-dialog or browser errors.
                        record.status = TransferState::Finished;
                        record.eta_total_secs = Some(0.0);
                    }
                }
            }
            record.updated_at_ms = web_now_ms();
        }
    }
}

fn web_now_ms() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|window| window.performance())
            .map(|perf| perf.now())
            .unwrap_or(0.0)
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        0.0
    }
}

fn map_chunk_status(status: &str) -> ChunkState {
    match status {
        "open" => ChunkState::Open,
        "downloading" => ChunkState::Downloading,
        "finished" => ChunkState::Finished,
        "retrying" => ChunkState::Retrying,
        "failed" => ChunkState::Failed,
        _ => ChunkState::Open,
    }
}

fn update_chunk_state(record: &mut TransferRecord, chunk_index: u32, next: ChunkState) {
    if let Some(previous) = record.chunk_states.remove(&chunk_index) {
        decrement_chunk_counter(record, previous);
    }

    match next {
        ChunkState::Retrying | ChunkState::Failed => {
            increment_chunk_counter(record, next);
            record.chunk_states.insert(chunk_index, next);
        }
        ChunkState::Open | ChunkState::Downloading | ChunkState::Finished => {}
    }
}

fn increment_chunk_counter(record: &mut TransferRecord, state: ChunkState) {
    match state {
        ChunkState::Retrying => {
            record.retrying_chunks = record.retrying_chunks.saturating_add(1);
        }
        ChunkState::Failed => {
            record.failed_chunks = record.failed_chunks.saturating_add(1);
        }
        ChunkState::Open | ChunkState::Downloading | ChunkState::Finished => {}
    }
}

fn decrement_chunk_counter(record: &mut TransferRecord, state: ChunkState) {
    match state {
        ChunkState::Retrying => {
            record.retrying_chunks = record.retrying_chunks.saturating_sub(1);
        }
        ChunkState::Failed => {
            record.failed_chunks = record.failed_chunks.saturating_sub(1);
        }
        ChunkState::Open | ChunkState::Downloading | ChunkState::Finished => {}
    }
}

fn expected_chunk_len(record: &TransferRecord, chunk_index: u32) -> Result<usize, String> {
    if chunk_index >= record.chunk_count {
        return Err(format!(
            "chunk index {} out of bounds for transfer {}",
            chunk_index, record.transfer_id
        ));
    }
    if record.chunk_count == 0 {
        return Ok(0);
    }
    if chunk_index + 1 == record.chunk_count {
        let consumed = chunk_index as u64 * record.chunk_size as u64;
        return usize::try_from(record.total_size.saturating_sub(consumed))
            .map_err(|_| "chunk size overflow".to_string());
    }
    Ok(record.chunk_size as usize)
}

fn update_transfer_rate(record: &mut TransferRecord, now_ms: f64) {
    let delta_ms = (now_ms - record.last_rate_sample_ms).max(0.0);
    if delta_ms <= 0.0 {
        return;
    }

    let delta_bytes = record
        .received_size
        .saturating_sub(record.last_rate_sample_bytes) as f64;
    let instant_rate = delta_bytes / (delta_ms / 1000.0).max(RATE_EPSILON);

    if record.smoothed_rate_bps <= RATE_EPSILON {
        record.smoothed_rate_bps = instant_rate;
    } else {
        record.smoothed_rate_bps =
            (EWMA_ALPHA * instant_rate) + ((1.0 - EWMA_ALPHA) * record.smoothed_rate_bps);
    }

    record.last_rate_sample_ms = now_ms;
    record.last_rate_sample_bytes = record.received_size;
    record.updated_at_ms = now_ms;

    update_transfer_eta(record);
}

fn update_transfer_eta(record: &mut TransferRecord) {
    let remaining = record.total_size.saturating_sub(record.received_size) as f64;
    if remaining <= 0.0 {
        record.eta_total_secs = Some(0.0);
        return;
    }

    if record.smoothed_rate_bps > RATE_EPSILON {
        record.eta_total_secs = Some(remaining / record.smoothed_rate_bps.max(RATE_EPSILON));
    } else {
        record.eta_total_secs = None;
    }
}

pub(super) fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{:02x}", byte);
    }
    out
}

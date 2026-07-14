use super::model::TransferRecord;
use super::protocol::TransferControlEvent;
use super::secure::{SecureCloseData, SecureOpenData};
use super::storage::{storage_queue_chunk_persist, storage_queue_clear_transfer_chunks};
use super::{
    ChunkCounters, ChunkState, FinalizeError, FinalizePlan, TransferState, TransferStore,
    TransferView,
};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

const EVENT_VERSION: u16 = transfer_protocol::TRANSFER_PROTOCOL_VERSION;
const EWMA_ALPHA: f64 = 0.2;
const RATE_EPSILON: f64 = 1e-3;

impl TransferStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply_secure_open(&mut self, open: SecureOpenData, now_ms: f64) -> Result<(), String> {
        if open.chunk_size == 0 || open.chunk_size as usize > transfer_protocol::MAX_PLAINTEXT_CHUNK
        {
            return Err("invalid encrypted transfer chunk size".into());
        }
        let expected_chunks = if open.total_size == 0 {
            0
        } else {
            open.total_size.div_ceil(open.chunk_size as u64)
        };
        if expected_chunks != u64::from(open.chunk_count) {
            return Err("encrypted manifest has inconsistent size and chunk count".into());
        }
        if open.file_name.is_empty() || open.file_name.len() > transfer_protocol::MAX_FILE_NAME_LEN
        {
            return Err("encrypted manifest contains an invalid file name".into());
        }

        storage_queue_clear_transfer_chunks(open.transfer_id);
        self.transfers.insert(
            open.transfer_id,
            TransferRecord {
                transfer_id: open.transfer_id,
                file_name: open.file_name,
                total_size: open.total_size,
                chunk_size: open.chunk_size,
                chunk_count: open.chunk_count,
                sha256_hex: String::new(),
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
            },
        );
        Ok(())
    }

    pub fn apply_secure_chunk(
        &mut self,
        transfer_id: u64,
        chunk_index: u32,
        plaintext: &[u8],
        now_ms: f64,
    ) -> Result<(), String> {
        let Some(record) = self.transfers.get_mut(&transfer_id) else {
            return Err(format!("chunk for unknown transfer {transfer_id}"));
        };
        if chunk_index != record.next_expected_chunk {
            record.status = TransferState::Failed;
            record.failed_chunks = record.failed_chunks.saturating_add(1);
            storage_queue_clear_transfer_chunks(transfer_id);
            return Err(format!(
                "out-of-order chunk for transfer {transfer_id}: got {chunk_index}, expected {}",
                record.next_expected_chunk
            ));
        }
        let expected_len = expected_chunk_len(record, chunk_index)?;
        if plaintext.len() != expected_len {
            record.status = TransferState::Failed;
            record.failed_chunks = record.failed_chunks.saturating_add(1);
            storage_queue_clear_transfer_chunks(transfer_id);
            return Err(format!(
                "chunk length mismatch for transfer {transfer_id} chunk {chunk_index}: got {}, expected {expected_len}",
                plaintext.len()
            ));
        }

        storage_queue_chunk_persist(transfer_id, chunk_index, plaintext);
        update_chunk_state(record, chunk_index, ChunkState::Finished);
        record.next_expected_chunk = record.next_expected_chunk.saturating_add(1);
        record.received_size = (u64::from(chunk_index) * u64::from(record.chunk_size))
            .saturating_add(plaintext.len() as u64)
            .min(record.total_size);
        record.finished_chunks = record.next_expected_chunk;
        record.rolling_sha256.update(plaintext);
        record.status = TransferState::Downloading;
        update_transfer_rate(record, now_ms);
        Ok(())
    }

    pub fn apply_secure_close(
        &mut self,
        close: SecureCloseData,
        now_ms: f64,
    ) -> Result<[u8; 32], String> {
        let Some(record) = self.transfers.get_mut(&close.transfer_id) else {
            return Err(format!("close for unknown transfer {}", close.transfer_id));
        };
        if close.total_size != record.total_size || close.chunk_count != record.chunk_count {
            record.status = TransferState::Failed;
            record.failed_chunks = record.failed_chunks.saturating_add(1);
            storage_queue_clear_transfer_chunks(close.transfer_id);
            return Err("authenticated close does not match the manifest".into());
        }
        if record.next_expected_chunk != record.chunk_count
            || record.received_size != record.total_size
        {
            record.status = TransferState::Failed;
            record.failed_chunks = record.failed_chunks.saturating_add(1);
            storage_queue_clear_transfer_chunks(close.transfer_id);
            return Err("authenticated close arrived before all chunks".into());
        }
        let computed: [u8; 32] = record.rolling_sha256.clone().finalize().into();
        if computed != close.sha256 {
            record.status = TransferState::Failed;
            record.failed_chunks = record.failed_chunks.saturating_add(1);
            storage_queue_clear_transfer_chunks(close.transfer_id);
            return Err("end-to-end SHA-256 verification failed".into());
        }
        record.sha256_hex = hex_lower(&computed);
        record.status = TransferState::Finished;
        record.updated_at_ms = now_ms;
        record.eta_total_secs = Some(0.0);
        Ok(computed)
    }

    pub fn fail_secure_transfer(&mut self, transfer_id: u64, now_ms: f64) {
        storage_queue_clear_transfer_chunks(transfer_id);
        if let Some(record) = self.transfers.get_mut(&transfer_id) {
            record.status = TransferState::Failed;
            record.failed_chunks = record.failed_chunks.saturating_add(1);
            record.updated_at_ms = now_ms;
            record.eta_total_secs = None;
        }
    }

    pub fn apply_text_event(&mut self, event_json: &str, now_ms: f64) -> Result<(), String> {
        let event: TransferControlEvent = serde_json::from_str(event_json)
            .map_err(|err| format!("transfer event parse: {err}"))?;

        match event {
            TransferControlEvent::Open { version } => {
                if version != EVENT_VERSION {
                    return Err(format!("unsupported transfer event version {version}"));
                }
                return Err("rejected unauthenticated plaintext transfer manifest".into());
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
                    if chunk_count != record.chunk_count || total_size < record.total_size {
                        return Err(
                            "relay progress is inconsistent with the authenticated manifest".into(),
                        );
                    }
                    record.received_size = record
                        .received_size
                        .max(received_size.min(record.total_size));
                    record.finished_chunks = record
                        .finished_chunks
                        .max(finished_chunks.min(record.chunk_count));
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
                    if record.next_expected_chunk == record.chunk_count
                        && !record.sha256_hex.is_empty()
                    {
                        record.status = TransferState::Finished;
                    }
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
                storage_queue_clear_transfer_chunks(transfer_id);
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
                storage_queue_clear_transfer_chunks(transfer_id);
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
                        storage_queue_clear_transfer_chunks(transfer_id);
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

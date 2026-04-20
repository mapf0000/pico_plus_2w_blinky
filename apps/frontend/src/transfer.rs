use crc32fast::Hasher as Crc32Hasher;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsValue, prelude::*};

const EVENT_VERSION: u16 = 1;
const EWMA_ALPHA: f64 = 0.2;
const RATE_EPSILON: f64 = 1e-3;
const CHUNK_HEADER_LEN: usize = 8 + 4 + 8 + 2 + 4;
const MAX_BLOB_FALLBACK_BYTES: f64 = 128.0 * 1024.0 * 1024.0;

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(module = "/src/idb.js")]
extern "C" {
    #[wasm_bindgen(js_name = queueChunkPersist)]
    fn js_queue_chunk_persist(transfer_id: u64, chunk_index: u32, payload: &[u8]);

    #[wasm_bindgen(js_name = queueClearTransferChunks)]
    fn js_queue_clear_transfer_chunks(transfer_id: u64);

    #[wasm_bindgen(catch, js_name = waitForChunkPersistQueue)]
    async fn js_wait_for_chunk_persist_queue() -> Result<JsValue, JsValue>;

    #[wasm_bindgen(catch, js_name = downloadTransferFromDb)]
    async fn js_download_transfer_from_db(
        transfer_id: u64,
        chunk_count: u32,
        file_name: &str,
        max_blob_bytes: f64,
    ) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(catch, js_name = clearTransferChunks)]
    async fn js_clear_transfer_chunks(transfer_id: u64) -> Result<JsValue, JsValue>;
}

#[cfg(target_arch = "wasm32")]
fn storage_queue_chunk_persist(transfer_id: u64, chunk_index: u32, payload: &[u8]) {
    js_queue_chunk_persist(transfer_id, chunk_index, payload);
}

#[cfg(target_arch = "wasm32")]
fn storage_queue_clear_transfer_chunks(transfer_id: u64) {
    js_queue_clear_transfer_chunks(transfer_id);
}

#[cfg(target_arch = "wasm32")]
async fn storage_wait_for_chunk_persist_queue() -> Result<(), String> {
    js_wait_for_chunk_persist_queue()
        .await
        .map(|_| ())
        .map_err(js_error_to_string)
}

#[cfg(target_arch = "wasm32")]
async fn storage_download_transfer_from_db(
    transfer_id: u64,
    chunk_count: u32,
    file_name: &str,
    max_blob_bytes: f64,
) -> Result<(), String> {
    js_download_transfer_from_db(transfer_id, chunk_count, file_name, max_blob_bytes)
        .await
        .map(|_| ())
        .map_err(js_error_to_string)
}

#[cfg(target_arch = "wasm32")]
async fn storage_clear_transfer_chunks(transfer_id: u64) -> Result<(), String> {
    js_clear_transfer_chunks(transfer_id)
        .await
        .map(|_| ())
        .map_err(js_error_to_string)
}

#[cfg(not(target_arch = "wasm32"))]
mod native_storage {
    use std::collections::HashMap;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    type ChunkKey = (u64, u32);
    type ChunkMap = HashMap<ChunkKey, Vec<u8>>;

    static CHUNKS: OnceLock<Mutex<ChunkMap>> = OnceLock::new();

    fn chunks() -> &'static Mutex<ChunkMap> {
        CHUNKS.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn lock_chunks() -> MutexGuard<'static, ChunkMap> {
        match chunks().lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub fn queue_chunk_persist(transfer_id: u64, chunk_index: u32, payload: &[u8]) {
        let mut db = lock_chunks();
        db.insert((transfer_id, chunk_index), payload.to_vec());
    }

    pub fn queue_clear_transfer_chunks(transfer_id: u64) {
        let mut db = lock_chunks();
        db.retain(|(existing_transfer_id, _), _| *existing_transfer_id != transfer_id);
    }

    pub async fn wait_for_chunk_persist_queue() -> Result<(), String> {
        Ok(())
    }

    pub async fn download_transfer_from_db(
        transfer_id: u64,
        chunk_count: u32,
        _file_name: &str,
        _max_blob_bytes: f64,
    ) -> Result<(), String> {
        let db = lock_chunks();
        for chunk_index in 0..chunk_count {
            if !db.contains_key(&(transfer_id, chunk_index)) {
                return Err(format!("Missing chunk {chunk_index}"));
            }
        }
        Ok(())
    }

    pub async fn clear_transfer_chunks(transfer_id: u64) -> Result<(), String> {
        queue_clear_transfer_chunks(transfer_id);
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn storage_queue_chunk_persist(transfer_id: u64, chunk_index: u32, payload: &[u8]) {
    native_storage::queue_chunk_persist(transfer_id, chunk_index, payload);
}

#[cfg(not(target_arch = "wasm32"))]
fn storage_queue_clear_transfer_chunks(transfer_id: u64) {
    native_storage::queue_clear_transfer_chunks(transfer_id);
}

#[cfg(not(target_arch = "wasm32"))]
async fn storage_wait_for_chunk_persist_queue() -> Result<(), String> {
    native_storage::wait_for_chunk_persist_queue().await
}

#[cfg(not(target_arch = "wasm32"))]
async fn storage_download_transfer_from_db(
    transfer_id: u64,
    chunk_count: u32,
    file_name: &str,
    max_blob_bytes: f64,
) -> Result<(), String> {
    native_storage::download_transfer_from_db(transfer_id, chunk_count, file_name, max_blob_bytes)
        .await
}

#[cfg(not(target_arch = "wasm32"))]
async fn storage_clear_transfer_chunks(transfer_id: u64) -> Result<(), String> {
    native_storage::clear_transfer_chunks(transfer_id).await
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferState {
    Open,
    Downloading,
    Verifying,
    Finished,
    Failed,
    Aborted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkState {
    Open,
    Downloading,
    Finished,
    Retrying,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkCounters {
    pub open: u32,
    pub downloading: u32,
    pub finished: u32,
    pub retrying: u32,
    pub failed: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TransferView {
    pub transfer_id: u64,
    pub file_name: String,
    pub total_size: u64,
    pub received_size: u64,
    pub chunk_count: u32,
    pub finished_chunks: u32,
    pub failed_chunks: u32,
    pub status: TransferState,
    pub smoothed_rate_bps: f64,
    pub eta_total_secs: Option<f64>,
    pub counters: ChunkCounters,
    pub updated_at_ms: f64,
}

#[derive(Debug)]
struct TransferRecord {
    transfer_id: u64,
    file_name: String,
    total_size: u64,
    chunk_size: u16,
    chunk_count: u32,
    sha256_hex: String,
    received_size: u64,
    finished_chunks: u32,
    failed_chunks: u32,
    retrying_chunks: u32,
    status: TransferState,
    updated_at_ms: f64,
    smoothed_rate_bps: f64,
    eta_total_secs: Option<f64>,
    last_rate_sample_ms: f64,
    last_rate_sample_bytes: u64,
    next_expected_chunk: u32,
    rolling_sha256: Sha256,
    chunk_states: HashMap<u32, ChunkState>,
}

#[derive(Debug, Default)]
pub struct TransferStore {
    transfers: HashMap<u64, TransferRecord>,
}

#[derive(Debug, Clone)]
pub struct FinalizePlan {
    pub transfer_id: u64,
    pub file_name: String,
    pub chunk_count: u32,
    pub expected_sha256_hex: String,
    pub computed_sha256_hex: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalizeErrorKind {
    Verification,
    Download,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalizeError {
    kind: FinalizeErrorKind,
    message: String,
}

impl FinalizeError {
    fn verification(message: impl Into<String>) -> Self {
        Self {
            kind: FinalizeErrorKind::Verification,
            message: message.into(),
        }
    }

    fn download(message: impl Into<String>) -> Self {
        Self {
            kind: FinalizeErrorKind::Download,
            message: message.into(),
        }
    }

    fn is_verification(&self) -> bool {
        self.kind == FinalizeErrorKind::Verification
    }
}

impl std::fmt::Display for FinalizeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for FinalizeError {}

#[derive(Debug, Deserialize)]
#[serde(tag = "event_type")]
enum TransferControlEvent {
    #[serde(rename = "transfer/open")]
    Open {
        version: u16,
        transfer_id: u64,
        file_name: String,
        total_size: u64,
        chunk_size: u16,
        chunk_count: u32,
        sha256: String,
    },
    #[serde(rename = "transfer/progress")]
    Progress {
        version: u16,
        transfer_id: u64,
        received_size: u64,
        total_size: u64,
        finished_chunks: u32,
        chunk_count: u32,
    },
    #[serde(rename = "transfer/chunk_status")]
    ChunkStatus {
        version: u16,
        transfer_id: u64,
        chunk_index: u32,
        status: String,
        attempts: u32,
        #[serde(default)]
        error: Option<String>,
    },
    #[serde(rename = "transfer/finished")]
    Finished { version: u16, transfer_id: u64 },
    #[serde(rename = "transfer/failed")]
    Failed {
        version: u16,
        transfer_id: u64,
        reason: String,
    },
    #[serde(rename = "transfer/aborted")]
    Aborted {
        version: u16,
        transfer_id: u64,
        reason_code: u8,
        detail: String,
    },
}

struct ChunkEnvelope {
    transfer_id: u64,
    chunk_index: u32,
    offset: u64,
    payload_len: u16,
    crc32: u32,
    payload: Vec<u8>,
}

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

pub async fn finalize_and_download(plan: &FinalizePlan) -> Result<(), FinalizeError> {
    storage_wait_for_chunk_persist_queue()
        .await
        .map_err(FinalizeError::download)?;

    verify_finalize_plan(plan).map_err(FinalizeError::verification)?;

    storage_download_transfer_from_db(
        plan.transfer_id,
        plan.chunk_count,
        &plan.file_name,
        MAX_BLOB_FALLBACK_BYTES,
    )
    .await
    .map_err(FinalizeError::download)?;

    storage_clear_transfer_chunks(plan.transfer_id)
        .await
        .map_err(FinalizeError::download)?;

    Ok(())
}

fn verify_finalize_plan(plan: &FinalizePlan) -> Result<(), String> {
    if !plan.expected_sha256_hex.is_empty()
        && !plan
            .expected_sha256_hex
            .eq_ignore_ascii_case(&plan.computed_sha256_hex)
    {
        return Err("sha256 mismatch".to_string());
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn js_error_to_string(value: JsValue) -> String {
    value
        .as_string()
        .or_else(|| {
            js_sys::JSON::stringify(&value)
                .ok()
                .and_then(|v| v.as_string())
        })
        .unwrap_or_else(|| "javascript error".to_string())
}

fn web_now_ms() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        return web_sys::window()
            .and_then(|window| window.performance())
            .map(|perf| perf.now())
            .unwrap_or(0.0);
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

fn decode_chunk_envelope(frame: &[u8]) -> Result<ChunkEnvelope, String> {
    if frame.len() < CHUNK_HEADER_LEN {
        return Err("binary frame too short".to_string());
    }

    let transfer_id = u64::from_le_bytes([
        frame[0], frame[1], frame[2], frame[3], frame[4], frame[5], frame[6], frame[7],
    ]);
    let chunk_index = u32::from_le_bytes([frame[8], frame[9], frame[10], frame[11]]);
    let offset = u64::from_le_bytes([
        frame[12], frame[13], frame[14], frame[15], frame[16], frame[17], frame[18], frame[19],
    ]);
    let payload_len = u16::from_le_bytes([frame[20], frame[21]]);
    let crc32 = u32::from_le_bytes([frame[22], frame[23], frame[24], frame[25]]);

    let expected = CHUNK_HEADER_LEN + payload_len as usize;
    if frame.len() != expected {
        return Err(format!(
            "binary payload length mismatch: expected {expected}, got {}",
            frame.len()
        ));
    }

    let payload = frame[CHUNK_HEADER_LEN..].to_vec();

    Ok(ChunkEnvelope {
        transfer_id,
        chunk_index,
        offset,
        payload_len,
        crc32,
        payload,
    })
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{:02x}", byte);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_binary_chunk_roundtrip() {
        let payload = b"hello";
        let mut crc32 = Crc32Hasher::new();
        crc32.update(payload);
        let crc = crc32.finalize();

        let mut frame = Vec::new();
        frame.extend_from_slice(&7u64.to_le_bytes());
        frame.extend_from_slice(&3u32.to_le_bytes());
        frame.extend_from_slice(&42u64.to_le_bytes());
        frame.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        frame.extend_from_slice(&crc.to_le_bytes());
        frame.extend_from_slice(payload);

        let chunk = decode_chunk_envelope(&frame).expect("decode chunk envelope");
        assert_eq!(chunk.transfer_id, 7);
        assert_eq!(chunk.chunk_index, 3);
        assert_eq!(chunk.offset, 42);
        assert_eq!(chunk.payload, payload);
        assert_eq!(chunk.crc32, crc);
    }

    #[test]
    fn open_and_progress_updates_state() {
        let mut store = TransferStore::new();
        store
            .apply_text_event(
                r#"{"event_type":"transfer/open","version":1,"transfer_id":1,"file_name":"a.bin","total_size":10,"chunk_size":5,"chunk_count":2,"sha256":""}"#,
                1000.0,
            )
            .unwrap();
        store
            .apply_text_event(
                r#"{"event_type":"transfer/progress","version":1,"transfer_id":1,"received_size":5,"total_size":10,"finished_chunks":1,"chunk_count":2}"#,
                1500.0,
            )
            .unwrap();

        let rows = store.snapshots();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].received_size, 5);
        assert_eq!(rows[0].finished_chunks, 1);
        assert_eq!(rows[0].status, TransferState::Downloading);
    }

    #[test]
    fn ewma_and_eta_are_computed() {
        let mut store = TransferStore::new();
        store
            .apply_text_event(
                r#"{"event_type":"transfer/open","version":1,"transfer_id":5,"file_name":"a.bin","total_size":100,"chunk_size":10,"chunk_count":10,"sha256":""}"#,
                0.0,
            )
            .unwrap();

        let payload = vec![1u8; 10];
        let mut crc = Crc32Hasher::new();
        crc.update(&payload);
        let crc = crc.finalize();

        let mut frame = Vec::new();
        frame.extend_from_slice(&5u64.to_le_bytes());
        frame.extend_from_slice(&0u32.to_le_bytes());
        frame.extend_from_slice(&0u64.to_le_bytes());
        frame.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        frame.extend_from_slice(&crc.to_le_bytes());
        frame.extend_from_slice(&payload);

        store.apply_binary_chunk(&frame, 1000.0).unwrap();
        let rows = store.snapshots();
        assert!(rows[0].smoothed_rate_bps > 0.0);
        assert!(rows[0].eta_total_secs.is_some());
    }

    #[test]
    fn rejects_out_of_order_binary_chunks() {
        let mut store = TransferStore::new();
        store
            .apply_text_event(
                r#"{"event_type":"transfer/open","version":1,"transfer_id":9,"file_name":"x.bin","total_size":20,"chunk_size":10,"chunk_count":2,"sha256":""}"#,
                0.0,
            )
            .unwrap();

        let payload = vec![1u8; 10];
        let mut crc = Crc32Hasher::new();
        crc.update(&payload);
        let crc = crc.finalize();

        let mut frame = Vec::new();
        frame.extend_from_slice(&9u64.to_le_bytes());
        frame.extend_from_slice(&1u32.to_le_bytes());
        frame.extend_from_slice(&10u64.to_le_bytes());
        frame.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        frame.extend_from_slice(&crc.to_le_bytes());
        frame.extend_from_slice(&payload);

        let err = store.apply_binary_chunk(&frame, 1000.0).unwrap_err();
        assert!(err.contains("out-of-order chunk"));
    }

    #[test]
    fn finalize_plan_uses_streamed_sha256() {
        let payload = b"hello-world";
        let expected_sha256 = {
            let mut h = Sha256::new();
            h.update(payload);
            hex_lower(&h.finalize())
        };

        let mut store = TransferStore::new();
        store
            .apply_text_event(
                &format!(
                    "{{\"event_type\":\"transfer/open\",\"version\":1,\"transfer_id\":11,\"file_name\":\"x.bin\",\"total_size\":{},\"chunk_size\":{},\"chunk_count\":1,\"sha256\":\"{}\"}}",
                    payload.len(),
                    payload.len(),
                    expected_sha256
                ),
                0.0,
            )
            .unwrap();

        let mut crc = Crc32Hasher::new();
        crc.update(payload);
        let crc = crc.finalize();

        let mut frame = Vec::new();
        frame.extend_from_slice(&11u64.to_le_bytes());
        frame.extend_from_slice(&0u32.to_le_bytes());
        frame.extend_from_slice(&0u64.to_le_bytes());
        frame.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        frame.extend_from_slice(&crc.to_le_bytes());
        frame.extend_from_slice(payload);
        store.apply_binary_chunk(&frame, 1000.0).unwrap();

        let plan = store.prepare_finalize(11).unwrap();
        assert_eq!(plan.computed_sha256_hex, expected_sha256);
        verify_finalize_plan(&plan).unwrap();
    }

    #[test]
    fn download_errors_keep_completed_transfer_retryable() {
        let mut store = TransferStore::new();
        store
            .apply_text_event(
                r#"{"event_type":"transfer/open","version":1,"transfer_id":21,"file_name":"x.bin","total_size":4,"chunk_size":4,"chunk_count":1,"sha256":""}"#,
                0.0,
            )
            .unwrap();
        store
            .apply_text_event(
                r#"{"event_type":"transfer/finished","version":1,"transfer_id":21}"#,
                10.0,
            )
            .unwrap();

        store.finish_finalize(21, &Err(FinalizeError::download("picker canceled")));

        let rows = store.snapshots();
        assert_eq!(rows[0].status, TransferState::Finished);
    }

    #[test]
    fn verification_errors_mark_transfer_failed() {
        let mut store = TransferStore::new();
        store
            .apply_text_event(
                r#"{"event_type":"transfer/open","version":1,"transfer_id":22,"file_name":"x.bin","total_size":4,"chunk_size":4,"chunk_count":1,"sha256":""}"#,
                0.0,
            )
            .unwrap();

        store.finish_finalize(22, &Err(FinalizeError::verification("sha256 mismatch")));

        let rows = store.snapshots();
        assert_eq!(rows[0].status, TransferState::Failed);
        assert_eq!(rows[0].failed_chunks, 1);
    }
}

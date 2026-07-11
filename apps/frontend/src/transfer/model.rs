use sha2::Sha256;
use std::collections::HashMap;

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
pub(super) struct TransferRecord {
    pub(super) transfer_id: u64,
    pub(super) file_name: String,
    pub(super) total_size: u64,
    pub(super) chunk_size: u16,
    pub(super) chunk_count: u32,
    pub(super) sha256_hex: String,
    pub(super) received_size: u64,
    pub(super) finished_chunks: u32,
    pub(super) failed_chunks: u32,
    pub(super) retrying_chunks: u32,
    pub(super) status: TransferState,
    pub(super) updated_at_ms: f64,
    pub(super) smoothed_rate_bps: f64,
    pub(super) eta_total_secs: Option<f64>,
    pub(super) last_rate_sample_ms: f64,
    pub(super) last_rate_sample_bytes: u64,
    pub(super) next_expected_chunk: u32,
    pub(super) rolling_sha256: Sha256,
    pub(super) chunk_states: HashMap<u32, ChunkState>,
}

#[derive(Debug, Default)]
pub struct TransferStore {
    pub(super) transfers: HashMap<u64, TransferRecord>,
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
    pub(super) fn verification(message: impl Into<String>) -> Self {
        Self {
            kind: FinalizeErrorKind::Verification,
            message: message.into(),
        }
    }

    pub(super) fn download(message: impl Into<String>) -> Self {
        Self {
            kind: FinalizeErrorKind::Download,
            message: message.into(),
        }
    }

    pub(super) fn is_verification(&self) -> bool {
        self.kind == FinalizeErrorKind::Verification
    }
}

impl std::fmt::Display for FinalizeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for FinalizeError {}

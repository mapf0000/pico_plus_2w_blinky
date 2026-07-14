use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(tag = "event_type")]
pub(super) enum TransferControlEvent {
    #[serde(rename = "transfer/open")]
    Open { version: u16 },
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

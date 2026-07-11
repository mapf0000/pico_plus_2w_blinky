use serde::Deserialize;

const CHUNK_HEADER_LEN: usize = 8 + 4 + 8 + 2 + 4;

#[derive(Debug, Deserialize)]
#[serde(tag = "event_type")]
pub(super) enum TransferControlEvent {
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

pub(super) struct ChunkEnvelope {
    pub(super) transfer_id: u64,
    pub(super) chunk_index: u32,
    pub(super) offset: u64,
    pub(super) payload_len: u16,
    pub(super) crc32: u32,
    pub(super) payload: Vec<u8>,
}

pub(super) fn decode_chunk_envelope(frame: &[u8]) -> Result<ChunkEnvelope, String> {
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

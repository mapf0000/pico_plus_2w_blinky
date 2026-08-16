use anyhow::{Result, bail};
use std::collections::HashMap;
use transfer_protocol::{
    FILE_TAG_LEN, SESSION_ID_LEN, decode_secure_chunk, decode_secure_close, decode_secure_open,
};

pub const TAG_FILE_OPEN: u8 = 20;
pub const TAG_FILE_CHUNK: u8 = 21;
pub const TAG_FILE_ACK: u8 = 22;
pub const TAG_FILE_CLOSE: u8 = 23;
pub const TAG_FILE_RESULT: u8 = 24;
pub const TAG_FILE_ABORT: u8 = 25;

const NONE_CONTIGUOUS_CHUNK: u32 = u32::MAX;
const ACK_WINDOW_CREDIT: u16 = 8;

#[derive(Debug)]
struct Transfer {
    session_id: [u8; SESSION_ID_LEN],
    chunk_size: u16,
    chunk_count: u32,
    next_chunk: u32,
    received_size: u64,
}

#[derive(Debug)]
pub enum RelayEvent {
    Open {
        transfer_id: u64,
        binary: Vec<u8>,
        ack: Vec<u8>,
        progress: String,
    },
    Chunk {
        transfer_id: u64,
        binary: Option<Vec<u8>>,
        ack: Vec<u8>,
        progress: Option<String>,
    },
    Close {
        transfer_id: u64,
        binary: Vec<u8>,
        result: Vec<u8>,
        finished: String,
    },
    Abort {
        transfer_id: u64,
        result: Vec<u8>,
        event: String,
    },
}

#[derive(Default)]
pub struct Relay {
    transfers: HashMap<u64, Transfer>,
}

impl Relay {
    pub fn handle(&mut self, tag: u8, payload: &[u8]) -> Result<Option<RelayEvent>> {
        match tag {
            TAG_FILE_OPEN => self.open(payload).map(Some),
            TAG_FILE_CHUNK => self.chunk(payload).map(Some),
            TAG_FILE_CLOSE => self.close(payload).map(Some),
            TAG_FILE_ABORT => self.abort(payload).map(Some),
            _ => Ok(None),
        }
    }

    pub fn cancel(&mut self, transfer_id: u64) {
        self.transfers.remove(&transfer_id);
    }

    fn open(&mut self, payload: &[u8]) -> Result<RelayEvent> {
        let open =
            decode_secure_open(payload).map_err(|_| anyhow::anyhow!("malformed FILE_OPEN"))?;
        let transfer = Transfer {
            session_id: open.session_id,
            chunk_size: open.chunk_size,
            chunk_count: open.chunk_count,
            next_chunk: 0,
            received_size: 0,
        };
        self.transfers.insert(open.transfer_id, transfer);
        Ok(RelayEvent::Open {
            transfer_id: open.transfer_id,
            binary: binary(transfer_protocol::WS_BINARY_KIND_SECURE_OPEN, payload),
            ack: encode_ack(open.transfer_id, None, 0),
            progress: progress(open.transfer_id, 0, open.chunk_size, open.chunk_count, 0),
        })
    }

    fn chunk(&mut self, payload: &[u8]) -> Result<RelayEvent> {
        let chunk =
            decode_secure_chunk(payload).map_err(|_| anyhow::anyhow!("malformed FILE_CHUNK"))?;
        let transfer = self
            .transfers
            .get_mut(&chunk.transfer_id)
            .ok_or_else(|| anyhow::anyhow!("FILE_CHUNK references unknown transfer"))?;
        if chunk.session_id != transfer.session_id || chunk.chunk_index >= transfer.chunk_count {
            bail!("invalid FILE_CHUNK envelope");
        }
        if chunk.chunk_index < transfer.next_chunk {
            return Ok(RelayEvent::Chunk {
                transfer_id: chunk.transfer_id,
                binary: None,
                ack: encode_ack(
                    chunk.transfer_id,
                    transfer.next_chunk.checked_sub(1),
                    transfer.received_size,
                ),
                progress: None,
            });
        }
        if chunk.chunk_index != transfer.next_chunk {
            bail!("out-of-order FILE_CHUNK");
        }
        let plaintext_len = chunk.ciphertext.len().saturating_sub(FILE_TAG_LEN);
        if chunk.chunk_index + 1 < transfer.chunk_count
            && plaintext_len != transfer.chunk_size as usize
        {
            bail!("short non-final FILE_CHUNK");
        }
        if plaintext_len > transfer.chunk_size as usize {
            bail!("oversized FILE_CHUNK");
        }
        transfer.next_chunk += 1;
        transfer.received_size += plaintext_len as u64;
        let emit_progress =
            transfer.next_chunk == transfer.chunk_count || transfer.next_chunk.is_multiple_of(64);
        Ok(RelayEvent::Chunk {
            transfer_id: chunk.transfer_id,
            binary: Some(binary(
                transfer_protocol::WS_BINARY_KIND_SECURE_CHUNK,
                payload,
            )),
            ack: encode_ack(
                chunk.transfer_id,
                transfer.next_chunk.checked_sub(1),
                transfer.received_size,
            ),
            progress: emit_progress.then(|| {
                progress(
                    chunk.transfer_id,
                    transfer.received_size,
                    transfer.chunk_size,
                    transfer.chunk_count,
                    transfer.next_chunk,
                )
            }),
        })
    }

    fn close(&mut self, payload: &[u8]) -> Result<RelayEvent> {
        let close =
            decode_secure_close(payload).map_err(|_| anyhow::anyhow!("malformed FILE_CLOSE"))?;
        let transfer = self
            .transfers
            .remove(&close.transfer_id)
            .ok_or_else(|| anyhow::anyhow!("FILE_CLOSE references unknown transfer"))?;
        if close.session_id != transfer.session_id || transfer.next_chunk != transfer.chunk_count {
            bail!("FILE_CLOSE arrived before all chunks");
        }
        Ok(RelayEvent::Close {
            transfer_id: close.transfer_id,
            binary: binary(transfer_protocol::WS_BINARY_KIND_SECURE_CLOSE, payload),
            result: encode_result(close.transfer_id, 0, "encrypted relay complete"),
            finished: format!(
                "{{\"event_type\":\"transfer/finished\",\"version\":2,\"transfer_id\":{}}}",
                close.transfer_id
            ),
        })
    }

    fn abort(&mut self, payload: &[u8]) -> Result<RelayEvent> {
        if payload.len() < 11 {
            bail!("malformed FILE_ABORT");
        }
        let transfer_id = u64::from_le_bytes(payload[0..8].try_into()?);
        let reason = payload[8];
        let detail_len = u16::from_le_bytes(payload[9..11].try_into()?) as usize;
        if payload.len() != 11 + detail_len {
            bail!("malformed FILE_ABORT detail");
        }
        let detail = std::str::from_utf8(&payload[11..])?;
        self.transfers.remove(&transfer_id);
        Ok(RelayEvent::Abort {
            transfer_id,
            result: encode_result(transfer_id, 3, "encrypted transfer aborted"),
            event: serde_json::json!({
                "event_type": "transfer/aborted",
                "version": 2,
                "transfer_id": transfer_id,
                "reason_code": reason,
                "detail": detail,
            })
            .to_string(),
        })
    }
}

pub fn encode_unavailable_abort(transfer_id: u64) -> Vec<u8> {
    let detail = b"secure browser relay unavailable";
    let mut payload = Vec::with_capacity(11 + detail.len());
    payload.extend_from_slice(&transfer_id.to_le_bytes());
    payload.push(5);
    payload.extend_from_slice(&(detail.len() as u16).to_le_bytes());
    payload.extend_from_slice(detail);
    payload
}

fn binary(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(1 + payload.len());
    output.push(kind);
    output.extend_from_slice(payload);
    output
}

fn encode_ack(transfer_id: u64, highest: Option<u32>, offset: u64) -> Vec<u8> {
    let mut payload = Vec::with_capacity(22);
    payload.extend_from_slice(&transfer_id.to_le_bytes());
    payload.extend_from_slice(&highest.unwrap_or(NONE_CONTIGUOUS_CHUNK).to_le_bytes());
    payload.extend_from_slice(&offset.to_le_bytes());
    payload.extend_from_slice(&ACK_WINDOW_CREDIT.to_le_bytes());
    payload
}

fn encode_result(transfer_id: u64, code: u8, detail: &str) -> Vec<u8> {
    let detail = detail.as_bytes();
    let mut payload = Vec::with_capacity(11 + detail.len());
    payload.extend_from_slice(&transfer_id.to_le_bytes());
    payload.push(code);
    payload.extend_from_slice(&(detail.len() as u16).to_le_bytes());
    payload.extend_from_slice(detail);
    payload
}

fn progress(
    transfer_id: u64,
    received_size: u64,
    chunk_size: u16,
    chunk_count: u32,
    finished_chunks: u32,
) -> String {
    format!(
        "{{\"event_type\":\"transfer/progress\",\"version\":2,\"transfer_id\":{transfer_id},\"received_size\":{received_size},\"total_size\":{},\"finished_chunks\":{finished_chunks},\"chunk_count\":{chunk_count}}}",
        u64::from(chunk_size) * u64::from(chunk_count)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use transfer_protocol::{FILE_SALT_LEN, encode_secure_open};

    #[test]
    fn open_is_forwarded_and_acked() {
        let mut encoded = [0u8; transfer_protocol::TLV_MAX_PAYLOAD];
        let len = encode_secure_open(
            &mut encoded,
            &[3; SESSION_ID_LEN],
            42,
            &[4; FILE_SALT_LEN],
            128,
            2,
            &[5; 32],
        )
        .unwrap();
        let mut relay = Relay::default();
        let RelayEvent::Open { binary, ack, .. } = relay
            .handle(TAG_FILE_OPEN, &encoded[..len])
            .unwrap()
            .unwrap()
        else {
            panic!("unexpected relay event");
        };
        assert_eq!(binary[0], transfer_protocol::WS_BINARY_KIND_SECURE_OPEN);
        assert_eq!(&ack[0..8], &42u64.to_le_bytes());
        assert_eq!(&ack[8..12], &u32::MAX.to_le_bytes());
        assert_eq!(&ack[20..22], &ACK_WINDOW_CREDIT.to_le_bytes());
    }
}

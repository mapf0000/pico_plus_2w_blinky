use crate::tlv::{self, Frame};
use anyhow::{Context, Result, bail};
use bytes::{BufMut, Bytes, BytesMut};
use crc32fast::Hasher as Crc32Hasher;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::mpsc;
use tokio::time::{Duration, timeout};

pub const TAG_FILE_OPEN: u8 = 20;
pub const TAG_FILE_CHUNK: u8 = 21;
pub const TAG_FILE_ACK: u8 = 22;
pub const TAG_FILE_CLOSE: u8 = 23;
pub const TAG_FILE_RESULT: u8 = 24;
pub const TAG_FILE_ABORT: u8 = 25;
#[allow(dead_code)] // reserved for protocol parity with firmware
pub const TAG_FILE_HEARTBEAT: u8 = 26;

pub const PROTOCOL_VERSION: u16 = 1;

const CHUNK_FIELD_OVERHEAD: usize = core::mem::size_of::<u64>() + // transfer_id
    core::mem::size_of::<u32>() + // chunk_index
    core::mem::size_of::<u64>() + // offset
    core::mem::size_of::<u16>() + // payload_len
    core::mem::size_of::<u32>(); // chunk_crc32

pub const MAX_CHUNK_DATA_LEN: usize = tlv::MAX_PAYLOAD_LEN - CHUNK_FIELD_OVERHEAD;

const DEFAULT_CHUNK_SIZE: usize = MAX_CHUNK_DATA_LEN;
const ACK_TIMEOUT: Duration = Duration::from_secs(5);
const RESULT_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_CHUNK_RETRIES: usize = 5;
const NONE_CONTIGUOUS_CHUNK: u32 = u32::MAX;
const DEFAULT_ABORT_REASON: u8 = 1;
const INITIAL_WINDOW_CREDIT: u16 = 8;
const MAX_WINDOW_CREDIT: u16 = 64;

static NEXT_TRANSFER_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("unexpected end of payload")]
    UnexpectedEof,
    #[error("payload too large")]
    PayloadTooLarge,
    #[error("invalid field: {0}")]
    InvalidField(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TransferId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkIndex(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ByteCount(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProtocolVersion(pub u16);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FileResultCode {
    Ok,
    HashMismatch,
    SizeMismatch,
    Aborted,
    InternalError,
    Unknown(u8),
}

impl FileResultCode {
    #[cfg(test)]
    pub fn as_u8(self) -> u8 {
        match self {
            Self::Ok => 0,
            Self::HashMismatch => 1,
            Self::SizeMismatch => 2,
            Self::Aborted => 3,
            Self::InternalError => 4,
            Self::Unknown(code) => code,
        }
    }
}

impl From<u8> for FileResultCode {
    fn from(value: u8) -> Self {
        match value {
            0 => Self::Ok,
            1 => Self::HashMismatch,
            2 => Self::SizeMismatch,
            3 => Self::Aborted,
            4 => Self::InternalError,
            code => Self::Unknown(code),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileOpen {
    pub protocol_version: ProtocolVersion,
    pub transfer_id: TransferId,
    pub total_size: ByteCount,
    pub chunk_size: u16,
    pub chunk_count: u32,
    pub sha256: [u8; 32],
    pub file_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChunk {
    pub transfer_id: TransferId,
    pub chunk_index: ChunkIndex,
    pub offset: ByteCount,
    pub payload: Bytes,
    pub chunk_crc32: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileAck {
    pub transfer_id: TransferId,
    pub highest_contiguous_chunk: Option<ChunkIndex>,
    pub next_expected_offset: ByteCount,
    pub window_credit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileClose {
    pub transfer_id: TransferId,
    pub sent_chunk_count: u32,
    pub sent_total_size: ByteCount,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileResult {
    pub transfer_id: TransferId,
    pub result_code: FileResultCode,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileAbort {
    pub transfer_id: TransferId,
    pub reason_code: u8,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub enum TransferFeedback {
    Ack(FileAck),
    Result(FileResult),
    Abort(FileAbort),
}

pub fn decode_feedback(frame: &Frame) -> Result<Option<TransferFeedback>> {
    let feedback = match frame.tag {
        TAG_FILE_ACK => TransferFeedback::Ack(decode_file_ack(&frame.payload)?),
        TAG_FILE_RESULT => TransferFeedback::Result(decode_file_result(&frame.payload)?),
        TAG_FILE_ABORT => TransferFeedback::Abort(decode_file_abort(&frame.payload)?),
        _ => return Ok(None),
    };
    Ok(Some(feedback))
}

pub async fn send_files(
    outbound: &mpsc::Sender<Frame>,
    feedback_rx: &mut mpsc::Receiver<TransferFeedback>,
    files: &[PathBuf],
) -> Result<()> {
    for path in files {
        send_single_file(outbound, feedback_rx, path)
            .await
            .with_context(|| format!("transfer failed for {}", path.display()))?;
    }

    Ok(())
}

pub fn encode_file_open(message: &FileOpen) -> Result<Bytes, ProtocolError> {
    let name = message.file_name.as_bytes();
    let name_len: u16 = name
        .len()
        .try_into()
        .map_err(|_| ProtocolError::InvalidField("file_name_len"))?;

    let mut buf = BytesMut::with_capacity(2 + 8 + 8 + 2 + 4 + 32 + 2 + name.len());
    buf.put_u16_le(message.protocol_version.0);
    buf.put_u64_le(message.transfer_id.0);
    buf.put_u64_le(message.total_size.0);
    buf.put_u16_le(message.chunk_size);
    buf.put_u32_le(message.chunk_count);
    buf.extend_from_slice(&message.sha256);
    buf.put_u16_le(name_len);
    buf.extend_from_slice(name);
    Ok(buf.freeze())
}

#[cfg(test)]
pub fn decode_file_open(payload: &[u8]) -> Result<FileOpen, ProtocolError> {
    let mut cursor = payload;
    let protocol_version = ProtocolVersion(take_u16(&mut cursor)?);
    let transfer_id = TransferId(take_u64(&mut cursor)?);
    let total_size = ByteCount(take_u64(&mut cursor)?);
    let chunk_size = take_u16(&mut cursor)?;
    let chunk_count = take_u32(&mut cursor)?;
    let sha256 = take_array_32(&mut cursor)?;
    let name_len = take_u16(&mut cursor)? as usize;
    let name_bytes = take_bytes(&mut cursor, name_len)?;
    let file_name = std::str::from_utf8(name_bytes)
        .map_err(|_| ProtocolError::InvalidField("file_name_utf8"))?
        .to_string();

    if chunk_size == 0 {
        return Err(ProtocolError::InvalidField("chunk_size"));
    }

    Ok(FileOpen {
        protocol_version,
        transfer_id,
        total_size,
        chunk_size,
        chunk_count,
        sha256,
        file_name,
    })
}

pub fn encode_file_chunk(message: &FileChunk) -> Result<Bytes, ProtocolError> {
    if message.payload.len() > MAX_CHUNK_DATA_LEN {
        return Err(ProtocolError::PayloadTooLarge);
    }
    let payload_len: u16 = message
        .payload
        .len()
        .try_into()
        .map_err(|_| ProtocolError::PayloadTooLarge)?;

    let mut buf = BytesMut::with_capacity(CHUNK_FIELD_OVERHEAD + message.payload.len());
    buf.put_u64_le(message.transfer_id.0);
    buf.put_u32_le(message.chunk_index.0);
    buf.put_u64_le(message.offset.0);
    buf.put_u16_le(payload_len);
    buf.extend_from_slice(&message.payload);
    buf.put_u32_le(message.chunk_crc32);
    Ok(buf.freeze())
}

#[cfg(test)]
pub fn decode_file_chunk(payload: &[u8]) -> Result<FileChunk, ProtocolError> {
    let mut cursor = payload;
    let transfer_id = TransferId(take_u64(&mut cursor)?);
    let chunk_index = ChunkIndex(take_u32(&mut cursor)?);
    let offset = ByteCount(take_u64(&mut cursor)?);
    let payload_len = take_u16(&mut cursor)? as usize;
    if payload_len > MAX_CHUNK_DATA_LEN {
        return Err(ProtocolError::PayloadTooLarge);
    }
    let payload_bytes = take_bytes(&mut cursor, payload_len)?;
    let chunk_crc32 = take_u32(&mut cursor)?;

    Ok(FileChunk {
        transfer_id,
        chunk_index,
        offset,
        payload: Bytes::copy_from_slice(payload_bytes),
        chunk_crc32,
    })
}

#[cfg(test)]
pub fn encode_file_ack(message: &FileAck) -> Bytes {
    let mut buf = BytesMut::with_capacity(8 + 4 + 8 + 2);
    buf.put_u64_le(message.transfer_id.0);
    buf.put_u32_le(
        message
            .highest_contiguous_chunk
            .map_or(NONE_CONTIGUOUS_CHUNK, |index| index.0),
    );
    buf.put_u64_le(message.next_expected_offset.0);
    buf.put_u16_le(message.window_credit);
    buf.freeze()
}

pub fn decode_file_ack(payload: &[u8]) -> Result<FileAck, ProtocolError> {
    let mut cursor = payload;
    let transfer_id = TransferId(take_u64(&mut cursor)?);
    let highest_raw = take_u32(&mut cursor)?;
    let highest_contiguous_chunk = if highest_raw == NONE_CONTIGUOUS_CHUNK {
        None
    } else {
        Some(ChunkIndex(highest_raw))
    };
    let next_expected_offset = ByteCount(take_u64(&mut cursor)?);
    let window_credit = take_u16(&mut cursor)?;

    Ok(FileAck {
        transfer_id,
        highest_contiguous_chunk,
        next_expected_offset,
        window_credit,
    })
}

pub fn encode_file_close(message: &FileClose) -> Bytes {
    let mut buf = BytesMut::with_capacity(8 + 4 + 8);
    buf.put_u64_le(message.transfer_id.0);
    buf.put_u32_le(message.sent_chunk_count);
    buf.put_u64_le(message.sent_total_size.0);
    buf.freeze()
}

#[cfg(test)]
pub fn decode_file_close(payload: &[u8]) -> Result<FileClose, ProtocolError> {
    let mut cursor = payload;
    Ok(FileClose {
        transfer_id: TransferId(take_u64(&mut cursor)?),
        sent_chunk_count: take_u32(&mut cursor)?,
        sent_total_size: ByteCount(take_u64(&mut cursor)?),
    })
}

#[cfg(test)]
pub fn encode_file_result(message: &FileResult) -> Result<Bytes, ProtocolError> {
    let detail = message.detail.as_bytes();
    let detail_len: u16 = detail
        .len()
        .try_into()
        .map_err(|_| ProtocolError::InvalidField("detail_len"))?;

    let mut buf = BytesMut::with_capacity(8 + 1 + 2 + detail.len());
    buf.put_u64_le(message.transfer_id.0);
    buf.put_u8(message.result_code.as_u8());
    buf.put_u16_le(detail_len);
    buf.extend_from_slice(detail);
    Ok(buf.freeze())
}

pub fn decode_file_result(payload: &[u8]) -> Result<FileResult, ProtocolError> {
    let mut cursor = payload;
    let transfer_id = TransferId(take_u64(&mut cursor)?);
    let result_code = FileResultCode::from(take_u8(&mut cursor)?);
    let detail_len = take_u16(&mut cursor)? as usize;
    let detail_bytes = take_bytes(&mut cursor, detail_len)?;
    let detail = std::str::from_utf8(detail_bytes)
        .map_err(|_| ProtocolError::InvalidField("detail_utf8"))?
        .to_string();

    Ok(FileResult {
        transfer_id,
        result_code,
        detail,
    })
}

pub fn encode_file_abort(message: &FileAbort) -> Result<Bytes, ProtocolError> {
    let detail = message.detail.as_bytes();
    let detail_len: u16 = detail
        .len()
        .try_into()
        .map_err(|_| ProtocolError::InvalidField("detail_len"))?;

    let mut buf = BytesMut::with_capacity(8 + 1 + 2 + detail.len());
    buf.put_u64_le(message.transfer_id.0);
    buf.put_u8(message.reason_code);
    buf.put_u16_le(detail_len);
    buf.extend_from_slice(detail);
    Ok(buf.freeze())
}

pub fn decode_file_abort(payload: &[u8]) -> Result<FileAbort, ProtocolError> {
    let mut cursor = payload;
    let transfer_id = TransferId(take_u64(&mut cursor)?);
    let reason_code = take_u8(&mut cursor)?;
    let detail_len = take_u16(&mut cursor)? as usize;
    let detail_bytes = take_bytes(&mut cursor, detail_len)?;
    let detail = std::str::from_utf8(detail_bytes)
        .map_err(|_| ProtocolError::InvalidField("detail_utf8"))?
        .to_string();

    Ok(FileAbort {
        transfer_id,
        reason_code,
        detail,
    })
}

async fn send_single_file(
    outbound: &mpsc::Sender<Frame>,
    feedback_rx: &mut mpsc::Receiver<TransferFeedback>,
    path: &Path,
) -> Result<()> {
    let open = build_open_message(path).await?;

    outbound
        .send(Frame::new(TAG_FILE_OPEN, encode_file_open(&open)?))
        .await
        .context("send FILE_OPEN")?;

    let mut file = File::open(path)
        .await
        .with_context(|| format!("open {}", path.display()))?;

    let mut next_chunk_to_send = 0u32;
    let mut highest_acked: Option<u32> = None;
    let mut window_credit = INITIAL_WINDOW_CREDIT;
    let mut in_flight: BTreeMap<u32, usize> = BTreeMap::new();

    while highest_acked.map_or(0, |value| value.saturating_add(1)) < open.chunk_count {
        while next_chunk_to_send < open.chunk_count
            && in_flight.len() < window_credit.max(1).min(MAX_WINDOW_CREDIT) as usize
        {
            send_chunk(outbound, &mut file, &open, next_chunk_to_send).await?;
            in_flight.insert(next_chunk_to_send, 0);
            next_chunk_to_send = next_chunk_to_send.saturating_add(1);
        }

        let feedback = timeout(ACK_TIMEOUT, feedback_rx.recv()).await;

        match feedback {
            Ok(Some(TransferFeedback::Ack(ack))) if ack.transfer_id == open.transfer_id => {
                window_credit = ack.window_credit.max(1).min(MAX_WINDOW_CREDIT);
                let acked = ack
                    .highest_contiguous_chunk
                    .map(|value| value.0)
                    .or(highest_acked);
                if let Some(acked_index) = acked {
                    highest_acked = Some(highest_acked.map_or(acked_index, |v| v.max(acked_index)));
                    in_flight.retain(|chunk_index, _| *chunk_index > acked_index);
                }
            }
            Ok(Some(TransferFeedback::Result(result)))
                if result.transfer_id == open.transfer_id =>
            {
                if result.result_code == FileResultCode::Ok {
                    highest_acked = Some(open.chunk_count.saturating_sub(1));
                    in_flight.clear();
                } else {
                    bail!(
                        "transfer failed early: {:?} ({})",
                        result.result_code,
                        result.detail
                    );
                }
            }
            Ok(Some(TransferFeedback::Abort(abort))) if abort.transfer_id == open.transfer_id => {
                bail!("transfer aborted by device: {}", abort.detail);
            }
            Ok(Some(_)) => {}
            Ok(None) => bail!("feedback channel closed"),
            Err(_) => {
                if let Some((&oldest_chunk, retries)) = in_flight.iter_mut().next() {
                    *retries = retries.saturating_add(1);
                    if *retries > MAX_CHUNK_RETRIES {
                        let abort = FileAbort {
                            transfer_id: open.transfer_id,
                            reason_code: DEFAULT_ABORT_REASON,
                            detail: format!("ack timeout for chunk {oldest_chunk}"),
                        };
                        let _ = outbound
                            .send(Frame::new(TAG_FILE_ABORT, encode_file_abort(&abort)?))
                            .await;
                        bail!("ack timeout for chunk {}", oldest_chunk);
                    }
                    send_chunk(outbound, &mut file, &open, oldest_chunk).await?;
                } else if next_chunk_to_send < open.chunk_count {
                    send_chunk(outbound, &mut file, &open, next_chunk_to_send).await?;
                    in_flight.insert(next_chunk_to_send, 0);
                    next_chunk_to_send = next_chunk_to_send.saturating_add(1);
                } else {
                    bail!("ack timeout with no in-flight chunks");
                }
            }
        }
    }

    let close = FileClose {
        transfer_id: open.transfer_id,
        sent_chunk_count: open.chunk_count,
        sent_total_size: open.total_size,
    };

    outbound
        .send(Frame::new(TAG_FILE_CLOSE, encode_file_close(&close)))
        .await
        .context("send FILE_CLOSE")?;

    wait_for_result(feedback_rx, open.transfer_id).await
}

async fn send_chunk(
    outbound: &mpsc::Sender<Frame>,
    file: &mut File,
    open: &FileOpen,
    chunk_index: u32,
) -> Result<()> {
    let offset = chunk_index as u64 * open.chunk_size as u64;
    let remaining = open.total_size.0.saturating_sub(offset);
    let payload_len = remaining.min(open.chunk_size as u64) as usize;
    let payload = read_chunk(file, offset, payload_len)
        .await
        .with_context(|| format!("read chunk {chunk_index}"))?;

    let mut crc32 = Crc32Hasher::new();
    crc32.update(&payload);
    let chunk = FileChunk {
        transfer_id: open.transfer_id,
        chunk_index: ChunkIndex(chunk_index),
        offset: ByteCount(offset),
        payload: Bytes::from(payload),
        chunk_crc32: crc32.finalize(),
    };

    outbound
        .send(Frame::new(TAG_FILE_CHUNK, encode_file_chunk(&chunk)?))
        .await
        .with_context(|| format!("send FILE_CHUNK {chunk_index}"))?;

    Ok(())
}

async fn wait_for_result(
    feedback_rx: &mut mpsc::Receiver<TransferFeedback>,
    transfer_id: TransferId,
) -> Result<()> {
    let result = timeout(RESULT_TIMEOUT, wait_for_terminal(feedback_rx, transfer_id))
        .await
        .context("timeout waiting for FILE_RESULT")??;

    if result.result_code == FileResultCode::Ok {
        return Ok(());
    }

    bail!(
        "transfer ended with {:?}: {}",
        result.result_code,
        result.detail
    )
}

async fn wait_for_terminal(
    feedback_rx: &mut mpsc::Receiver<TransferFeedback>,
    transfer_id: TransferId,
) -> Result<FileResult> {
    loop {
        let feedback = feedback_rx
            .recv()
            .await
            .context("feedback channel closed")?;
        match feedback {
            TransferFeedback::Result(result) if result.transfer_id == transfer_id => {
                return Ok(result);
            }
            TransferFeedback::Abort(abort) if abort.transfer_id == transfer_id => {
                bail!("transfer aborted: {}", abort.detail);
            }
            _ => {}
        }
    }
}

async fn read_chunk(file: &mut File, offset: u64, len: usize) -> Result<Vec<u8>> {
    file.seek(std::io::SeekFrom::Start(offset)).await?;
    let mut payload = vec![0u8; len];
    file.read_exact(&mut payload).await?;
    Ok(payload)
}

async fn build_open_message(path: &Path) -> Result<FileOpen> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| anyhow::anyhow!("invalid file name"))?
        .to_string();

    let metadata = tokio::fs::metadata(path).await?;
    let total_size = metadata.len();

    let chunk_size = DEFAULT_CHUNK_SIZE
        .min(MAX_CHUNK_DATA_LEN)
        .try_into()
        .expect("chunk size fits in u16");
    let chunk_count = if total_size == 0 {
        0
    } else {
        ((total_size + chunk_size as u64 - 1) / chunk_size as u64) as u32
    };

    let sha256 = compute_sha256(path).await?;

    Ok(FileOpen {
        protocol_version: ProtocolVersion(PROTOCOL_VERSION),
        transfer_id: next_transfer_id(),
        total_size: ByteCount(total_size),
        chunk_size,
        chunk_count,
        sha256,
        file_name,
    })
}

async fn compute_sha256(path: &Path) -> Result<[u8; 32]> {
    let mut file = File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 4096];

    loop {
        let count = file.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }

    Ok(hasher.finalize().into())
}

fn next_transfer_id() -> TransferId {
    TransferId(NEXT_TRANSFER_ID.fetch_add(1, Ordering::Relaxed))
}

fn take_bytes<'a>(cursor: &mut &'a [u8], len: usize) -> Result<&'a [u8], ProtocolError> {
    if cursor.len() < len {
        return Err(ProtocolError::UnexpectedEof);
    }
    let (head, tail) = cursor.split_at(len);
    *cursor = tail;
    Ok(head)
}

fn take_u8(cursor: &mut &[u8]) -> Result<u8, ProtocolError> {
    Ok(take_bytes(cursor, 1)?[0])
}

fn take_u16(cursor: &mut &[u8]) -> Result<u16, ProtocolError> {
    let bytes = take_bytes(cursor, 2)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn take_u32(cursor: &mut &[u8]) -> Result<u32, ProtocolError> {
    let bytes = take_bytes(cursor, 4)?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn take_u64(cursor: &mut &[u8]) -> Result<u64, ProtocolError> {
    let bytes = take_bytes(cursor, 8)?;
    Ok(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

#[cfg(test)]
fn take_array_32(cursor: &mut &[u8]) -> Result<[u8; 32], ProtocolError> {
    let bytes = take_bytes(cursor, 32)?;
    let mut out = [0u8; 32];
    out.copy_from_slice(bytes);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_open() -> FileOpen {
        FileOpen {
            protocol_version: ProtocolVersion(PROTOCOL_VERSION),
            transfer_id: TransferId(42),
            total_size: ByteCount(9999),
            chunk_size: 1024,
            chunk_count: 10,
            sha256: [7u8; 32],
            file_name: "capture.bin".to_string(),
        }
    }

    #[test]
    fn file_open_roundtrip() {
        let open = sample_open();
        let encoded = encode_file_open(&open).unwrap();
        let decoded = decode_file_open(&encoded).unwrap();
        assert_eq!(decoded, open);
    }

    #[test]
    fn file_chunk_roundtrip() {
        let chunk = FileChunk {
            transfer_id: TransferId(7),
            chunk_index: ChunkIndex(3),
            offset: ByteCount(3072),
            payload: Bytes::from_static(b"abc"),
            chunk_crc32: 0x1234_5678,
        };
        let encoded = encode_file_chunk(&chunk).unwrap();
        let decoded = decode_file_chunk(&encoded).unwrap();
        assert_eq!(decoded, chunk);
    }

    #[test]
    fn file_ack_roundtrip_none_highest() {
        let ack = FileAck {
            transfer_id: TransferId(11),
            highest_contiguous_chunk: None,
            next_expected_offset: ByteCount(0),
            window_credit: 4,
        };
        let encoded = encode_file_ack(&ack);
        let decoded = decode_file_ack(&encoded).unwrap();
        assert_eq!(decoded, ack);
    }

    #[test]
    fn file_result_roundtrip_unknown_code() {
        let payload = {
            let mut bytes = BytesMut::new();
            bytes.put_u64_le(99);
            bytes.put_u8(77);
            bytes.put_u16_le(2);
            bytes.extend_from_slice(b"ok");
            bytes.freeze()
        };

        let decoded = decode_file_result(&payload).unwrap();
        assert_eq!(decoded.transfer_id, TransferId(99));
        assert_eq!(decoded.result_code, FileResultCode::Unknown(77));
        assert_eq!(decoded.detail, "ok");
    }

    #[test]
    fn reject_oversized_chunk_payload() {
        let chunk = FileChunk {
            transfer_id: TransferId(1),
            chunk_index: ChunkIndex(0),
            offset: ByteCount(0),
            payload: Bytes::from(vec![0u8; MAX_CHUNK_DATA_LEN + 1]),
            chunk_crc32: 0,
        };

        let err = encode_file_chunk(&chunk).unwrap_err();
        assert!(matches!(err, ProtocolError::PayloadTooLarge));
    }

    #[test]
    fn decode_file_open_rejects_short_payload() {
        let err = decode_file_open(&[0u8; 10]).unwrap_err();
        assert!(matches!(err, ProtocolError::UnexpectedEof));
    }

    #[test]
    fn decode_file_chunk_rejects_short_payload() {
        let err = decode_file_chunk(&[0u8; 6]).unwrap_err();
        assert!(matches!(err, ProtocolError::UnexpectedEof));
    }
}

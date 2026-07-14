use crate::tlv::{self, Frame};
use anyhow::{Context, Result, bail};
use bytes::{BufMut, Bytes, BytesMut};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tokio::time::{Duration, timeout};
use transfer_crypto::{FileCipher, SessionMaster, encode_close, encode_manifest, fill_random};
use transfer_protocol::{
    FILE_SALT_LEN, MAX_FILE_NAME_LEN, MAX_PLAINTEXT_CHUNK, RECORD_CLOSE, RECORD_MANIFEST,
    SESSION_ID_LEN, encode_secure_chunk, encode_secure_close, encode_secure_open,
};
use zeroize::{Zeroize, Zeroizing};

pub const TAG_FILE_OPEN: u8 = 20;
pub const TAG_FILE_CHUNK: u8 = 21;
pub const TAG_FILE_ACK: u8 = 22;
pub const TAG_FILE_CLOSE: u8 = 23;
pub const TAG_FILE_RESULT: u8 = 24;
pub const TAG_FILE_ABORT: u8 = 25;
#[allow(dead_code)]
pub const TAG_FILE_HEARTBEAT: u8 = 26;

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
    #[error("invalid field: {0}")]
    InvalidField(&'static str),
    #[error("trailing payload bytes")]
    TrailingData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TransferId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkIndex(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ByteCount(pub u64);

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
pub struct FileAck {
    pub transfer_id: TransferId,
    pub highest_contiguous_chunk: Option<ChunkIndex>,
    pub next_expected_offset: ByteCount,
    pub window_credit: u16,
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
    BrowserReceipt {
        transfer_id: TransferId,
        sha256: [u8; 32],
    },
}

pub struct TransferRequest {
    pub path: PathBuf,
    pub session_id: [u8; SESSION_ID_LEN],
    pub session_master: SessionMaster,
}

struct OpenTransfer {
    transfer_id: TransferId,
    session_id: [u8; SESSION_ID_LEN],
    total_size: u64,
    chunk_size: u16,
    chunk_count: u32,
    cipher: FileCipher,
    encoded_open: Bytes,
}

struct InFlight {
    frame: Frame,
    retries: usize,
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

pub async fn send_request(
    outbound: &mpsc::Sender<Frame>,
    feedback_rx: &mut mpsc::Receiver<TransferFeedback>,
    request: TransferRequest,
) -> Result<()> {
    send_single_file(outbound, feedback_rx, request).await
}

async fn send_single_file(
    outbound: &mpsc::Sender<Frame>,
    feedback_rx: &mut mpsc::Receiver<TransferFeedback>,
    mut request: TransferRequest,
) -> Result<()> {
    let mut file = File::open(&request.path)
        .await
        .context("open transfer source")?;
    let metadata = file
        .metadata()
        .await
        .context("read transfer source metadata")?;
    if !metadata.is_file() {
        bail!("transfer source is not a regular file");
    }
    let initial_modified = metadata.modified().ok();
    let open = build_open(&request.path, metadata.len(), &request)?;
    request.session_master.zeroize();

    outbound
        .send(Frame::new(TAG_FILE_OPEN, open.encoded_open.clone()))
        .await
        .context("send encrypted FILE_OPEN")?;

    let mut next_chunk_to_send = 0u32;
    let mut highest_acked: Option<u32> = None;
    let mut window_credit = INITIAL_WINDOW_CREDIT;
    let mut in_flight: BTreeMap<u32, InFlight> = BTreeMap::new();
    let mut hasher = Sha256::new();
    let mut plaintext_bytes = 0u64;

    while highest_acked.map_or(0, |value| value.saturating_add(1)) < open.chunk_count {
        while next_chunk_to_send < open.chunk_count
            && in_flight.len() < window_credit.clamp(1, MAX_WINDOW_CREDIT) as usize
        {
            let frame = match read_encrypt_chunk(
                &mut file,
                &open,
                next_chunk_to_send,
                &mut hasher,
                &mut plaintext_bytes,
            )
            .await
            {
                Ok(frame) => frame,
                Err(error) => {
                    send_abort(
                        outbound,
                        open.transfer_id,
                        "source read or encryption failed",
                    )
                    .await;
                    return Err(error);
                }
            };
            outbound
                .send(frame.clone())
                .await
                .context("send encrypted chunk")?;
            in_flight.insert(next_chunk_to_send, InFlight { frame, retries: 0 });
            next_chunk_to_send = next_chunk_to_send.saturating_add(1);
        }

        match timeout(ACK_TIMEOUT, feedback_rx.recv()).await {
            Ok(Some(TransferFeedback::Ack(ack))) if ack.transfer_id == open.transfer_id => {
                if let Err(error) = validate_ack(&ack, &open, next_chunk_to_send, highest_acked) {
                    send_abort(outbound, open.transfer_id, "invalid relay ACK").await;
                    return Err(error);
                }
                window_credit = ack.window_credit.clamp(1, MAX_WINDOW_CREDIT);
                let acked = ack
                    .highest_contiguous_chunk
                    .map(|value| value.0)
                    .or(highest_acked);
                if let Some(acked_index) = acked {
                    highest_acked = Some(highest_acked.map_or(acked_index, |v| v.max(acked_index)));
                    in_flight.retain(|chunk_index, _| *chunk_index > acked_index);
                }
            }
            Ok(Some(TransferFeedback::Abort(abort))) if abort.transfer_id == open.transfer_id => {
                bail!("encrypted transfer aborted by relay: {}", abort.detail);
            }
            Ok(Some(TransferFeedback::Result(result)))
                if result.transfer_id == open.transfer_id =>
            {
                if result.result_code != FileResultCode::Ok {
                    bail!("encrypted transfer failed early: {:?}", result.result_code);
                }
            }
            Ok(Some(_)) => {}
            Ok(None) => bail!("transfer feedback channel closed"),
            Err(_) => {
                let Some((_, oldest)) = in_flight.iter_mut().next() else {
                    bail!("ACK timeout with no encrypted chunk in flight");
                };
                oldest.retries = oldest.retries.saturating_add(1);
                if oldest.retries > MAX_CHUNK_RETRIES {
                    send_abort(outbound, open.transfer_id, "ACK retry limit reached").await;
                    bail!("ACK timeout retry limit reached");
                }
                // Retransmit the exact ciphertext. Re-encryption with a reused nonce is forbidden.
                outbound
                    .send(oldest.frame.clone())
                    .await
                    .context("retransmit encrypted chunk")?;
            }
        }
    }

    if plaintext_bytes != open.total_size {
        send_abort(outbound, open.transfer_id, "source size changed").await;
        bail!("transfer source size changed while reading");
    }
    let final_metadata = match file
        .metadata()
        .await
        .context("recheck transfer source metadata")
    {
        Ok(metadata) => metadata,
        Err(error) => {
            send_abort(outbound, open.transfer_id, "source metadata recheck failed").await;
            return Err(error);
        }
    };
    if final_metadata.len() != open.total_size {
        send_abort(outbound, open.transfer_id, "source size changed").await;
        bail!("transfer source changed during transfer");
    }
    if initial_modified.is_some() && final_metadata.modified().ok() != initial_modified {
        send_abort(
            outbound,
            open.transfer_id,
            "source modification time changed",
        )
        .await;
        bail!("transfer source modification time changed during transfer");
    }

    let sha256: [u8; 32] = hasher.finalize().into();
    let close_plaintext = Zeroizing::new(encode_close(open.total_size, open.chunk_count, &sha256));
    let close_ciphertext = open
        .cipher
        .seal(RECORD_CLOSE, 0, close_plaintext.as_slice())
        .context("encrypt FILE_CLOSE")?;
    let mut close_payload = [0; tlv::MAX_PAYLOAD_LEN];
    let close_len = encode_secure_close(
        &mut close_payload,
        &open.session_id,
        open.transfer_id.0,
        &close_ciphertext,
    )
    .map_err(|_| anyhow::anyhow!("encrypted close exceeds TLV limit"))?;
    outbound
        .send(Frame::new(
            TAG_FILE_CLOSE,
            Bytes::copy_from_slice(&close_payload[..close_len]),
        ))
        .await
        .context("send encrypted FILE_CLOSE")?;

    wait_for_completion(feedback_rx, open.transfer_id, &sha256).await
}

fn validate_ack(
    ack: &FileAck,
    open: &OpenTransfer,
    next_chunk_to_send: u32,
    highest_acked: Option<u32>,
) -> Result<()> {
    let expected_offset = match ack.highest_contiguous_chunk {
        Some(ChunkIndex(index)) => {
            if index >= open.chunk_count || index >= next_chunk_to_send {
                bail!("relay ACK references an unsent chunk");
            }
            if highest_acked.is_some_and(|previous| index < previous) {
                bail!("relay ACK regressed");
            }
            (u64::from(index) + 1)
                .saturating_mul(u64::from(open.chunk_size))
                .min(open.total_size)
        }
        None => {
            if highest_acked.is_some() {
                bail!("relay ACK lost its contiguous position");
            }
            0
        }
    };
    if ack.next_expected_offset.0 != expected_offset {
        bail!("relay ACK offset is inconsistent");
    }
    Ok(())
}

fn build_open(path: &Path, total_size: u64, request: &TransferRequest) -> Result<OpenTransfer> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty() && name.len() <= MAX_FILE_NAME_LEN)
        .ok_or_else(|| anyhow::anyhow!("invalid transfer file name"))?;
    let chunk_size = u16::try_from(MAX_PLAINTEXT_CHUNK).expect("chunk limit fits u16");
    let chunk_count_u64 = if total_size == 0 {
        0
    } else {
        total_size.div_ceil(chunk_size as u64)
    };
    let chunk_count = u32::try_from(chunk_count_u64).context("file has too many chunks")?;
    let transfer_id = next_transfer_id();
    let mut file_salt = [0; FILE_SALT_LEN];
    fill_random(&mut file_salt).context("generate file salt")?;
    let cipher = FileCipher::new(
        &request.session_master,
        &request.session_id,
        transfer_id.0,
        &file_salt,
        chunk_size,
        chunk_count,
    )
    .context("derive per-file key")?;
    let manifest = Zeroizing::new(encode_manifest(file_name, total_size)?);
    let encrypted_manifest = cipher
        .seal(RECORD_MANIFEST, 0, manifest.as_slice())
        .context("encrypt file manifest")?;
    let mut open_payload = [0; tlv::MAX_PAYLOAD_LEN];
    let open_len = encode_secure_open(
        &mut open_payload,
        &request.session_id,
        transfer_id.0,
        &file_salt,
        chunk_size,
        chunk_count,
        &encrypted_manifest,
    )
    .map_err(|_| anyhow::anyhow!("encrypted manifest exceeds TLV limit"))?;
    Ok(OpenTransfer {
        transfer_id,
        session_id: request.session_id,
        total_size,
        chunk_size,
        chunk_count,
        cipher,
        encoded_open: Bytes::copy_from_slice(&open_payload[..open_len]),
    })
}

async fn read_encrypt_chunk(
    file: &mut File,
    open: &OpenTransfer,
    chunk_index: u32,
    hasher: &mut Sha256,
    plaintext_bytes: &mut u64,
) -> Result<Frame> {
    let remaining = open.total_size.saturating_sub(*plaintext_bytes);
    let expected = remaining.min(open.chunk_size as u64) as usize;
    let mut plaintext = Zeroizing::new(vec![0; expected]);
    file.read_exact(plaintext.as_mut_slice())
        .await
        .with_context(|| format!("read source chunk {chunk_index}"))?;
    hasher.update(plaintext.as_slice());
    *plaintext_bytes = plaintext_bytes.saturating_add(expected as u64);
    let ciphertext = open
        .cipher
        .seal_chunk(chunk_index, plaintext.as_slice())
        .with_context(|| format!("encrypt chunk {chunk_index}"))?;
    let mut payload = [0; tlv::MAX_PAYLOAD_LEN];
    let len = encode_secure_chunk(
        &mut payload,
        &open.session_id,
        open.transfer_id.0,
        chunk_index,
        &ciphertext,
    )
    .map_err(|_| anyhow::anyhow!("encrypted chunk exceeds TLV limit"))?;
    Ok(Frame::new(
        TAG_FILE_CHUNK,
        Bytes::copy_from_slice(&payload[..len]),
    ))
}

async fn wait_for_completion(
    feedback_rx: &mut mpsc::Receiver<TransferFeedback>,
    transfer_id: TransferId,
    expected_sha256: &[u8; 32],
) -> Result<()> {
    timeout(RESULT_TIMEOUT, async {
        let mut relay_ok = false;
        let mut browser_ok = false;
        while !relay_ok || !browser_ok {
            match feedback_rx
                .recv()
                .await
                .context("feedback channel closed")?
            {
                TransferFeedback::Result(result) if result.transfer_id == transfer_id => {
                    if result.result_code != FileResultCode::Ok {
                        bail!(
                            "relay rejected encrypted transfer: {:?}",
                            result.result_code
                        );
                    }
                    relay_ok = true;
                }
                TransferFeedback::BrowserReceipt {
                    transfer_id: id,
                    sha256,
                } if id == transfer_id => {
                    if &sha256 != expected_sha256 {
                        bail!("browser receipt hash mismatch");
                    }
                    browser_ok = true;
                }
                TransferFeedback::Abort(abort) if abort.transfer_id == transfer_id => {
                    bail!("encrypted transfer aborted: {}", abort.detail);
                }
                _ => {}
            }
        }
        Ok(())
    })
    .await
    .context("timeout waiting for authenticated browser receipt")?
}

async fn send_abort(outbound: &mpsc::Sender<Frame>, transfer_id: TransferId, detail: &str) {
    let abort = FileAbort {
        transfer_id,
        reason_code: DEFAULT_ABORT_REASON,
        detail: detail.to_string(),
    };
    if let Ok(payload) = encode_file_abort(&abort) {
        let _ = outbound.send(Frame::new(TAG_FILE_ABORT, payload)).await;
    }
}

pub fn decode_file_ack(payload: &[u8]) -> Result<FileAck, ProtocolError> {
    let mut cursor = payload;
    let transfer_id = TransferId(take_u64(&mut cursor)?);
    let highest_raw = take_u32(&mut cursor)?;
    let highest_contiguous_chunk =
        (highest_raw != NONE_CONTIGUOUS_CHUNK).then_some(ChunkIndex(highest_raw));
    let next_expected_offset = ByteCount(take_u64(&mut cursor)?);
    let window_credit = take_u16(&mut cursor)?;
    require_empty(cursor)?;
    Ok(FileAck {
        transfer_id,
        highest_contiguous_chunk,
        next_expected_offset,
        window_credit,
    })
}

pub fn decode_file_result(payload: &[u8]) -> Result<FileResult, ProtocolError> {
    let mut cursor = payload;
    let transfer_id = TransferId(take_u64(&mut cursor)?);
    let result_code = FileResultCode::from(take_u8(&mut cursor)?);
    let detail_len = take_u16(&mut cursor)? as usize;
    let detail = std::str::from_utf8(take_bytes(&mut cursor, detail_len)?)
        .map_err(|_| ProtocolError::InvalidField("detail_utf8"))?
        .to_string();
    require_empty(cursor)?;
    Ok(FileResult {
        transfer_id,
        result_code,
        detail,
    })
}

pub fn encode_file_abort(message: &FileAbort) -> Result<Bytes, ProtocolError> {
    let detail = message.detail.as_bytes();
    let detail_len =
        u16::try_from(detail.len()).map_err(|_| ProtocolError::InvalidField("detail_len"))?;
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
    let detail = std::str::from_utf8(take_bytes(&mut cursor, detail_len)?)
        .map_err(|_| ProtocolError::InvalidField("detail_utf8"))?
        .to_string();
    require_empty(cursor)?;
    Ok(FileAbort {
        transfer_id,
        reason_code,
        detail,
    })
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
    Ok(u16::from_le_bytes(
        take_bytes(cursor, 2)?.try_into().unwrap(),
    ))
}

fn take_u32(cursor: &mut &[u8]) -> Result<u32, ProtocolError> {
    Ok(u32::from_le_bytes(
        take_bytes(cursor, 4)?.try_into().unwrap(),
    ))
}

fn take_u64(cursor: &mut &[u8]) -> Result<u64, ProtocolError> {
    Ok(u64::from_le_bytes(
        take_bytes(cursor, 8)?.try_into().unwrap(),
    ))
}

fn require_empty(cursor: &[u8]) -> Result<(), ProtocolError> {
    if cursor.is_empty() {
        Ok(())
    } else {
        Err(ProtocolError::TrailingData)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use transfer_crypto::{decode_close, decode_manifest};
    use transfer_protocol::{
        RECORD_CLOSE, RECORD_MANIFEST, decode_secure_chunk, decode_secure_close, decode_secure_open,
    };

    #[test]
    fn ack_decoder_rejects_trailing_data() {
        let mut bytes = BytesMut::new();
        bytes.put_u64_le(4);
        bytes.put_u32_le(NONE_CONTIGUOUS_CHUNK);
        bytes.put_u64_le(0);
        bytes.put_u16_le(8);
        assert!(decode_file_ack(&bytes).is_ok());
        bytes.put_u8(0);
        assert!(matches!(
            decode_file_ack(&bytes),
            Err(ProtocolError::TrailingData)
        ));
    }

    #[test]
    fn abort_roundtrip() {
        let abort = FileAbort {
            transfer_id: TransferId(7),
            reason_code: 3,
            detail: "stopped".into(),
        };
        let encoded = encode_file_abort(&abort).unwrap();
        assert_eq!(decode_file_abort(&encoded).unwrap(), abort);
    }

    #[tokio::test]
    async fn streams_authenticated_ciphertext_and_waits_for_browser_receipt() {
        let content = (0..3_000)
            .map(|index| ((index * 31 + 7) % 251) as u8)
            .collect::<Vec<_>>();
        let path = std::env::temp_dir().join(format!(
            "pico-secure-transfer-{}-{}.bin",
            std::process::id(),
            NEXT_TRANSFER_ID.load(Ordering::Relaxed)
        ));
        std::fs::write(&path, &content).unwrap();

        let session_id = [0x41; SESSION_ID_LEN];
        let master_bytes = [0x52; 32];
        let request = TransferRequest {
            path: path.clone(),
            session_id,
            session_master: Zeroizing::new(master_bytes),
        };
        let (outbound_tx, mut outbound_rx) = mpsc::channel(16);
        let (feedback_tx, mut feedback_rx) = mpsc::channel(16);
        let sender =
            tokio::spawn(
                async move { send_request(&outbound_tx, &mut feedback_rx, request).await },
            );

        let mut cipher = None;
        let mut transfer_id = 0;
        let mut received = Vec::new();
        let mut expected_chunks = 0;
        while let Some(frame) = outbound_rx.recv().await {
            match frame.tag {
                TAG_FILE_OPEN => {
                    let open = decode_secure_open(&frame.payload).unwrap();
                    transfer_id = open.transfer_id;
                    expected_chunks = open.chunk_count;
                    let file_cipher = FileCipher::new(
                        &master_bytes,
                        &session_id,
                        open.transfer_id,
                        &open.file_salt,
                        open.chunk_size,
                        open.chunk_count,
                    )
                    .unwrap();
                    let manifest = file_cipher
                        .open(RECORD_MANIFEST, 0, open.ciphertext)
                        .unwrap();
                    let (size, name) = decode_manifest(&manifest).unwrap();
                    assert_eq!(size, content.len() as u64);
                    assert!(name.ends_with(".bin"));
                    assert!(
                        !frame
                            .payload
                            .windows(content.len())
                            .any(|part| part == content)
                    );
                    cipher = Some(file_cipher);
                }
                TAG_FILE_CHUNK => {
                    let chunk = decode_secure_chunk(&frame.payload).unwrap();
                    let plaintext = cipher
                        .as_ref()
                        .unwrap()
                        .open_chunk(chunk.chunk_index, chunk.ciphertext)
                        .unwrap();
                    assert_ne!(chunk.ciphertext, plaintext);
                    received.extend_from_slice(&plaintext);
                    feedback_tx
                        .send(TransferFeedback::Ack(FileAck {
                            transfer_id: TransferId(chunk.transfer_id),
                            highest_contiguous_chunk: Some(ChunkIndex(chunk.chunk_index)),
                            next_expected_offset: ByteCount(received.len() as u64),
                            window_credit: 8,
                        }))
                        .await
                        .unwrap();
                }
                TAG_FILE_CLOSE => {
                    let close = decode_secure_close(&frame.payload).unwrap();
                    let plaintext = cipher
                        .as_ref()
                        .unwrap()
                        .open(RECORD_CLOSE, 0, close.ciphertext)
                        .unwrap();
                    let (size, chunks, digest) = decode_close(&plaintext).unwrap();
                    assert_eq!(size, content.len() as u64);
                    assert_eq!(chunks, expected_chunks);
                    assert_eq!(received, content);
                    assert_eq!(digest, Sha256::digest(&content).as_slice());
                    feedback_tx
                        .send(TransferFeedback::Result(FileResult {
                            transfer_id: TransferId(transfer_id),
                            result_code: FileResultCode::Ok,
                            detail: String::new(),
                        }))
                        .await
                        .unwrap();
                    feedback_tx
                        .send(TransferFeedback::BrowserReceipt {
                            transfer_id: TransferId(transfer_id),
                            sha256: digest,
                        })
                        .await
                        .unwrap();
                    break;
                }
                other => panic!("unexpected outbound tag {other}"),
            }
        }

        sender.await.unwrap().unwrap();
        std::fs::remove_file(path).unwrap();
    }
}

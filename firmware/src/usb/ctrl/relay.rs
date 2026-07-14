use super::*;

mod events;

use events::{
    emit_transfer_aborted, emit_transfer_finished, emit_transfer_progress, forward_secure_chunk,
    forward_secure_close, forward_secure_open,
};
pub(super) use events::{forward_filesystem_page, forward_secure_session};
use transfer_protocol::{
    FILE_SALT_LEN, FILE_TAG_LEN, MAX_PLAINTEXT_CHUNK, SESSION_ID_LEN, SecureChunk, SecureOpen,
    decode_secure_chunk, decode_secure_close, decode_secure_open,
};

#[derive(Clone)]
struct RelayTransferState {
    transfer_id: u64,
    session_id: [u8; SESSION_ID_LEN],
    file_salt: [u8; FILE_SALT_LEN],
    chunk_size: u16,
    chunk_count: u32,
    highest_contiguous_chunk: Option<u32>,
    received_size: u64,
    finished_chunks: u32,
    last_progress_emit_finished_chunks: u32,
}

impl RelayTransferState {
    fn approximate_total_size(&self) -> u64 {
        self.chunk_size as u64 * self.chunk_count as u64
    }
}

pub(super) struct RelayState {
    transfers: Vec<RelayTransferState, MAX_RELAY_TRANSFERS>,
}

impl RelayState {
    pub(super) const fn new() -> Self {
        Self {
            transfers: Vec::new(),
        }
    }

    fn index_of(&self, transfer_id: u64) -> Option<usize> {
        self.transfers
            .iter()
            .position(|state| state.transfer_id == transfer_id)
    }

    fn get_mut(&mut self, transfer_id: u64) -> Option<&mut RelayTransferState> {
        let index = self.index_of(transfer_id)?;
        self.transfers.get_mut(index)
    }

    fn remove(&mut self, transfer_id: u64) -> Option<RelayTransferState> {
        let index = self.index_of(transfer_id)?;
        Some(self.transfers.swap_remove(index))
    }

    fn insert(&mut self, state: RelayTransferState) -> Result<(), ()> {
        self.transfers.push(state).map_err(|_| ())
    }
}

pub(super) async fn handle_file_open<'d, D>(
    class: &mut CdcAcmClass<'d, D>,
    max_packet: usize,
    relay: &mut RelayState,
    payload: &[u8],
) -> Result<(), EndpointError>
where
    D: Driver<'d>,
{
    let open = match decode_secure_open(payload) {
        Ok(open) => open,
        Err(_) => {
            log::warn!("usb: rejected malformed encrypted FILE_OPEN");
            return Ok(());
        }
    };

    if let Some(existing) = relay.get_mut(open.transfer_id) {
        if open_matches(existing, &open) {
            send_file_ack(
                class,
                max_packet,
                existing.transfer_id,
                existing.highest_contiguous_chunk,
                existing.received_size,
                DEFAULT_ACK_WINDOW_CREDIT,
            )
            .await?;
        } else {
            send_file_abort(
                class,
                max_packet,
                open.transfer_id,
                FILE_ABORT_REASON_METADATA_MISMATCH,
                "duplicate encrypted open mismatch",
            )
            .await?;
        }
        return Ok(());
    }

    let state = RelayTransferState {
        transfer_id: open.transfer_id,
        session_id: open.session_id,
        file_salt: open.file_salt,
        chunk_size: open.chunk_size,
        chunk_count: open.chunk_count,
        highest_contiguous_chunk: None,
        received_size: 0,
        finished_chunks: 0,
        last_progress_emit_finished_chunks: 0,
    };
    if relay.insert(state.clone()).is_err() {
        send_file_abort(
            class,
            max_packet,
            open.transfer_id,
            FILE_ABORT_REASON_CAPACITY,
            "too many active encrypted transfers",
        )
        .await?;
        return Ok(());
    }

    TRANSFER_ID.store(state.transfer_id, Ordering::Release);
    TRANSFER_TOTAL_SIZE.store(state.approximate_total_size(), Ordering::Release);
    TRANSFER_RECEIVED_SIZE.store(0, Ordering::Release);
    TRANSFER_CHUNK_COUNT.store(state.chunk_count, Ordering::Release);
    TRANSFER_FINISHED_CHUNKS.store(0, Ordering::Release);
    set_transfer_view_state(TransferViewState::Open);

    if !forward_secure_open(payload).await {
        let _ = relay.remove(open.transfer_id);
        send_file_abort(
            class,
            max_packet,
            open.transfer_id,
            FILE_ABORT_REASON_CAPACITY,
            "secure browser relay unavailable",
        )
        .await?;
        return Ok(());
    }
    emit_transfer_progress(&state).await;
    send_file_ack(
        class,
        max_packet,
        state.transfer_id,
        None,
        0,
        DEFAULT_ACK_WINDOW_CREDIT,
    )
    .await
}

pub(super) async fn handle_file_chunk<'d, D>(
    class: &mut CdcAcmClass<'d, D>,
    max_packet: usize,
    relay: &mut RelayState,
    payload: &[u8],
) -> Result<(), EndpointError>
where
    D: Driver<'d>,
{
    let chunk = match decode_secure_chunk(payload) {
        Ok(chunk) => chunk,
        Err(_) => {
            log::warn!("usb: rejected malformed encrypted FILE_CHUNK");
            return Ok(());
        }
    };
    let Some(state) = relay.get_mut(chunk.transfer_id) else {
        send_file_abort(
            class,
            max_packet,
            chunk.transfer_id,
            FILE_ABORT_REASON_UNKNOWN_TRANSFER,
            "unknown encrypted transfer",
        )
        .await?;
        return Ok(());
    };
    if chunk.session_id != state.session_id || !valid_chunk(state, &chunk) {
        send_file_abort(
            class,
            max_packet,
            chunk.transfer_id,
            FILE_ABORT_REASON_INVALID_CHUNK,
            "invalid encrypted chunk envelope",
        )
        .await?;
        return Ok(());
    }

    if let Some(highest) = state.highest_contiguous_chunk
        && chunk.chunk_index <= highest
    {
        send_file_ack(
            class,
            max_packet,
            state.transfer_id,
            state.highest_contiguous_chunk,
            state.received_size,
            DEFAULT_ACK_WINDOW_CREDIT,
        )
        .await?;
        return Ok(());
    }

    let expected_index = state.highest_contiguous_chunk.map_or(0, |value| value + 1);
    if chunk.chunk_index != expected_index {
        send_file_ack(
            class,
            max_packet,
            state.transfer_id,
            state.highest_contiguous_chunk,
            state.received_size,
            0,
        )
        .await?;
        return Ok(());
    }

    if !forward_secure_chunk(payload).await {
        let transfer_id = state.transfer_id;
        let _ = relay.remove(transfer_id);
        send_file_abort(
            class,
            max_packet,
            transfer_id,
            FILE_ABORT_REASON_CAPACITY,
            "secure browser relay unavailable",
        )
        .await?;
        return Ok(());
    }

    let plaintext_len = chunk.ciphertext.len().saturating_sub(FILE_TAG_LEN) as u64;
    state.highest_contiguous_chunk = Some(chunk.chunk_index);
    state.received_size = state.received_size.saturating_add(plaintext_len);
    state.finished_chunks = state.finished_chunks.saturating_add(1);
    TRANSFER_RECEIVED_SIZE.store(state.received_size, Ordering::Release);
    TRANSFER_FINISHED_CHUNKS.store(state.finished_chunks, Ordering::Release);
    set_transfer_view_state(TransferViewState::Progress);

    if state.finished_chunks == state.chunk_count
        || state
            .finished_chunks
            .saturating_sub(state.last_progress_emit_finished_chunks)
            >= PROGRESS_EMIT_EVERY_CHUNKS
    {
        state.last_progress_emit_finished_chunks = state.finished_chunks;
        emit_transfer_progress(state).await;
    }

    send_file_ack(
        class,
        max_packet,
        state.transfer_id,
        state.highest_contiguous_chunk,
        state.received_size,
        DEFAULT_ACK_WINDOW_CREDIT,
    )
    .await
}

pub(super) async fn handle_file_close<'d, D>(
    class: &mut CdcAcmClass<'d, D>,
    max_packet: usize,
    relay: &mut RelayState,
    payload: &[u8],
) -> Result<(), EndpointError>
where
    D: Driver<'d>,
{
    let close = match decode_secure_close(payload) {
        Ok(close) => close,
        Err(_) => {
            log::warn!("usb: rejected malformed encrypted FILE_CLOSE");
            return Ok(());
        }
    };
    let Some(state) = relay.get_mut(close.transfer_id) else {
        send_file_abort(
            class,
            max_packet,
            close.transfer_id,
            FILE_ABORT_REASON_UNKNOWN_TRANSFER,
            "unknown encrypted transfer",
        )
        .await?;
        return Ok(());
    };
    if close.session_id != state.session_id || state.finished_chunks != state.chunk_count {
        send_file_result(
            class,
            max_packet,
            close.transfer_id,
            FILE_RESULT_SIZE_MISMATCH,
            "encrypted chunk count mismatch",
        )
        .await?;
        return Ok(());
    }
    if !forward_secure_close(payload).await {
        send_file_abort(
            class,
            max_packet,
            close.transfer_id,
            FILE_ABORT_REASON_CAPACITY,
            "secure browser relay unavailable",
        )
        .await?;
        return Ok(());
    }
    let transfer_id = close.transfer_id;
    let _ = relay.remove(transfer_id);
    set_transfer_view_state(TransferViewState::Finished);
    send_file_result(
        class,
        max_packet,
        transfer_id,
        FILE_RESULT_OK,
        "encrypted relay complete",
    )
    .await?;
    emit_transfer_finished(transfer_id).await;
    Ok(())
}

pub(super) async fn handle_file_abort<'d, D>(
    class: &mut CdcAcmClass<'d, D>,
    max_packet: usize,
    relay: &mut RelayState,
    payload: &[u8],
) -> Result<(), EndpointError>
where
    D: Driver<'d>,
{
    let Some((transfer_id, reason_code, detail)) = parse_abort(payload) else {
        return Ok(());
    };
    let _ = relay.remove(transfer_id);
    set_transfer_view_state(TransferViewState::Aborted);
    send_file_result(
        class,
        max_packet,
        transfer_id,
        FILE_RESULT_ABORTED,
        "encrypted transfer aborted",
    )
    .await?;
    emit_transfer_aborted(transfer_id, reason_code, detail).await;
    Ok(())
}

fn open_matches(state: &RelayTransferState, open: &SecureOpen<'_>) -> bool {
    state.session_id == open.session_id
        && state.file_salt == open.file_salt
        && state.chunk_size == open.chunk_size
        && state.chunk_count == open.chunk_count
}

fn valid_chunk(state: &RelayTransferState, chunk: &SecureChunk<'_>) -> bool {
    if chunk.chunk_index >= state.chunk_count {
        return false;
    }
    let plaintext_len = chunk.ciphertext.len().saturating_sub(FILE_TAG_LEN);
    if chunk.chunk_index + 1 < state.chunk_count {
        plaintext_len == state.chunk_size as usize
    } else {
        plaintext_len <= state.chunk_size as usize
    }
}

fn parse_abort(payload: &[u8]) -> Option<(u64, u8, &str)> {
    if payload.len() < 11 {
        return None;
    }
    let transfer_id = u64::from_le_bytes(payload[0..8].try_into().ok()?);
    let reason = payload[8];
    let len = u16::from_le_bytes(payload[9..11].try_into().ok()?) as usize;
    if payload.len() != 11 + len {
        return None;
    }
    Some((
        transfer_id,
        reason,
        core::str::from_utf8(&payload[11..]).ok()?,
    ))
}

async fn send_file_ack<'d, D>(
    class: &mut CdcAcmClass<'d, D>,
    max_packet: usize,
    transfer_id: u64,
    highest_contiguous_chunk: Option<u32>,
    next_expected_offset: u64,
    window_credit: u16,
) -> Result<(), EndpointError>
where
    D: Driver<'d>,
{
    let mut payload = [0; 22];
    payload[0..8].copy_from_slice(&transfer_id.to_le_bytes());
    payload[8..12].copy_from_slice(
        &highest_contiguous_chunk
            .unwrap_or(NONE_CONTIGUOUS_CHUNK)
            .to_le_bytes(),
    );
    payload[12..20].copy_from_slice(&next_expected_offset.to_le_bytes());
    payload[20..22].copy_from_slice(&window_credit.to_le_bytes());
    send_tlv(class, max_packet, TAG_FILE_ACK, &payload).await
}

async fn send_file_result<'d, D>(
    class: &mut CdcAcmClass<'d, D>,
    max_packet: usize,
    transfer_id: u64,
    code: u8,
    detail: &str,
) -> Result<(), EndpointError>
where
    D: Driver<'d>,
{
    let detail = detail.as_bytes();
    let detail_len = detail.len().min(u16::MAX as usize);
    let mut payload: Vec<u8, MAX_PAYLOAD_LEN> = Vec::new();
    let _ = payload.extend_from_slice(&transfer_id.to_le_bytes());
    let _ = payload.push(code);
    let _ = payload.extend_from_slice(&(detail_len as u16).to_le_bytes());
    let _ = payload.extend_from_slice(&detail[..detail_len]);
    send_tlv(class, max_packet, TAG_FILE_RESULT, payload.as_slice()).await
}

async fn send_file_abort<'d, D>(
    class: &mut CdcAcmClass<'d, D>,
    max_packet: usize,
    transfer_id: u64,
    reason_code: u8,
    detail: &str,
) -> Result<(), EndpointError>
where
    D: Driver<'d>,
{
    let detail = detail.as_bytes();
    let detail_len = detail.len().min(u16::MAX as usize);
    let mut payload: Vec<u8, MAX_PAYLOAD_LEN> = Vec::new();
    let _ = payload.extend_from_slice(&transfer_id.to_le_bytes());
    let _ = payload.push(reason_code);
    let _ = payload.extend_from_slice(&(detail_len as u16).to_le_bytes());
    let _ = payload.extend_from_slice(&detail[..detail_len]);
    send_tlv(class, max_packet, TAG_FILE_ABORT, payload.as_slice()).await
}

const _: () = assert!(MAX_PLAINTEXT_CHUNK <= u16::MAX as usize);

use super::*;

mod events;
mod protocol;

pub(super) use events::forward_filesystem_page;
use events::{
    build_ws_chunk_envelope, emit_chunk_status, emit_transfer_aborted, emit_transfer_failed,
    emit_transfer_finished, emit_transfer_open, emit_transfer_progress,
};
use protocol::{crc32_ieee, parse_file_abort, parse_file_chunk, parse_file_close, parse_file_open};

#[derive(Clone)]
struct RelayTransferState {
    transfer_id: u64,
    protocol_version: u16,
    total_size: u64,
    chunk_size: u16,
    chunk_count: u32,
    sha256: [u8; 32],
    file_name: String<MAX_TRANSFER_FILE_NAME>,
    highest_contiguous_chunk: Option<u32>,
    next_expected_offset: u64,
    received_size: u64,
    finished_chunks: u32,
    last_progress_emit_finished_chunks: u32,
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

struct IncomingOpen<'a> {
    protocol_version: u16,
    transfer_id: u64,
    total_size: u64,
    chunk_size: u16,
    chunk_count: u32,
    sha256: [u8; 32],
    file_name: &'a str,
}

struct IncomingChunk<'a> {
    transfer_id: u64,
    chunk_index: u32,
    offset: u64,
    payload: &'a [u8],
    chunk_crc32: u32,
}

struct IncomingClose {
    transfer_id: u64,
    sent_chunk_count: u32,
    sent_total_size: u64,
}

struct IncomingAbort<'a> {
    transfer_id: u64,
    reason_code: u8,
    detail: &'a str,
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
    let Some(open) = parse_file_open(payload) else {
        return Ok(());
    };

    if open.protocol_version != 1 {
        send_file_abort(
            class,
            max_packet,
            open.transfer_id,
            FILE_ABORT_REASON_PROTOCOL,
            "unsupported protocol version",
        )
        .await?;
        emit_transfer_failed(open.transfer_id, "unsupported protocol version").await;
        return Ok(());
    }

    if open.chunk_size == 0 || open.chunk_size as usize > FILE_CHUNK_MAX_DATA {
        send_file_abort(
            class,
            max_packet,
            open.transfer_id,
            FILE_ABORT_REASON_PROTOCOL,
            "invalid chunk size",
        )
        .await?;
        emit_transfer_failed(open.transfer_id, "invalid chunk size").await;
        return Ok(());
    }

    if let Some(existing) = relay.get_mut(open.transfer_id) {
        if metadata_matches(existing, &open) {
            send_file_ack(
                class,
                max_packet,
                existing.transfer_id,
                existing.highest_contiguous_chunk,
                existing.next_expected_offset,
                DEFAULT_ACK_WINDOW_CREDIT,
            )
            .await?;
            return Ok(());
        }

        send_file_abort(
            class,
            max_packet,
            open.transfer_id,
            FILE_ABORT_REASON_METADATA_MISMATCH,
            "duplicate open with mismatched metadata",
        )
        .await?;
        emit_transfer_failed(open.transfer_id, "duplicate open mismatch").await;
        return Ok(());
    }

    let mut file_name: String<MAX_TRANSFER_FILE_NAME> = String::new();
    if file_name.push_str(open.file_name).is_err() {
        send_file_abort(
            class,
            max_packet,
            open.transfer_id,
            FILE_ABORT_REASON_PROTOCOL,
            "file name too long",
        )
        .await?;
        emit_transfer_failed(open.transfer_id, "file name too long").await;
        return Ok(());
    }

    let state = RelayTransferState {
        transfer_id: open.transfer_id,
        protocol_version: open.protocol_version,
        total_size: open.total_size,
        chunk_size: open.chunk_size,
        chunk_count: open.chunk_count,
        sha256: open.sha256,
        file_name,
        highest_contiguous_chunk: None,
        next_expected_offset: 0,
        received_size: 0,
        finished_chunks: 0,
        last_progress_emit_finished_chunks: 0,
    };

    TRANSFER_ID.store(state.transfer_id, Ordering::Release);
    TRANSFER_TOTAL_SIZE.store(state.total_size, Ordering::Release);
    TRANSFER_RECEIVED_SIZE.store(0, Ordering::Release);
    TRANSFER_CHUNK_COUNT.store(state.chunk_count, Ordering::Release);
    TRANSFER_FINISHED_CHUNKS.store(0, Ordering::Release);
    set_transfer_view_state(TransferViewState::Open);

    if relay.insert(state.clone()).is_err() {
        send_file_abort(
            class,
            max_packet,
            open.transfer_id,
            FILE_ABORT_REASON_CAPACITY,
            "too many active transfers",
        )
        .await?;
        emit_transfer_failed(open.transfer_id, "too many active transfers").await;
        return Ok(());
    }

    emit_transfer_open(&state).await;
    emit_transfer_progress(&state).await;

    send_file_ack(
        class,
        max_packet,
        state.transfer_id,
        state.highest_contiguous_chunk,
        state.next_expected_offset,
        DEFAULT_ACK_WINDOW_CREDIT,
    )
    .await?;

    Ok(())
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
    let Some(chunk) = parse_file_chunk(payload) else {
        return Ok(());
    };

    let Some(state) = relay.get_mut(chunk.transfer_id) else {
        send_file_abort(
            class,
            max_packet,
            chunk.transfer_id,
            FILE_ABORT_REASON_UNKNOWN_TRANSFER,
            "unknown transfer id",
        )
        .await?;
        emit_transfer_failed(chunk.transfer_id, "unknown transfer id").await;
        return Ok(());
    };

    if chunk.chunk_index >= state.chunk_count {
        emit_chunk_status(
            state.transfer_id,
            chunk.chunk_index,
            "failed",
            1,
            Some("chunk index out of bounds"),
        )
        .await;
        send_file_abort(
            class,
            max_packet,
            state.transfer_id,
            FILE_ABORT_REASON_INVALID_CHUNK,
            "chunk index out of bounds",
        )
        .await?;
        return Ok(());
    }

    if chunk.payload.len() > state.chunk_size as usize {
        emit_chunk_status(
            state.transfer_id,
            chunk.chunk_index,
            "failed",
            1,
            Some("payload larger than chunk size"),
        )
        .await;
        send_file_abort(
            class,
            max_packet,
            state.transfer_id,
            FILE_ABORT_REASON_INVALID_CHUNK,
            "payload larger than chunk size",
        )
        .await?;
        return Ok(());
    }

    let expected_offset = chunk.chunk_index as u64 * state.chunk_size as u64;
    if chunk.offset != expected_offset {
        emit_chunk_status(
            state.transfer_id,
            chunk.chunk_index,
            "failed",
            1,
            Some("offset mismatch"),
        )
        .await;
        send_file_abort(
            class,
            max_packet,
            state.transfer_id,
            FILE_ABORT_REASON_INVALID_CHUNK,
            "offset mismatch",
        )
        .await?;
        return Ok(());
    }

    if chunk.offset.saturating_add(chunk.payload.len() as u64) > state.total_size {
        emit_chunk_status(
            state.transfer_id,
            chunk.chunk_index,
            "failed",
            1,
            Some("chunk exceeds total size"),
        )
        .await;
        send_file_abort(
            class,
            max_packet,
            state.transfer_id,
            FILE_ABORT_REASON_INVALID_CHUNK,
            "chunk exceeds total size",
        )
        .await?;
        return Ok(());
    }

    let expected_next = state
        .highest_contiguous_chunk
        .map_or(0u32, |value| value.saturating_add(1));

    if chunk.chunk_index < expected_next {
        emit_chunk_status(
            state.transfer_id,
            chunk.chunk_index,
            "retrying",
            2,
            Some("duplicate chunk"),
        )
        .await;
        send_file_ack(
            class,
            max_packet,
            state.transfer_id,
            state.highest_contiguous_chunk,
            state.next_expected_offset,
            DEFAULT_ACK_WINDOW_CREDIT,
        )
        .await?;
        return Ok(());
    }

    if chunk.chunk_index > expected_next {
        emit_chunk_status(
            state.transfer_id,
            chunk.chunk_index,
            "retrying",
            1,
            Some("out-of-order chunk"),
        )
        .await;
        send_file_ack(
            class,
            max_packet,
            state.transfer_id,
            state.highest_contiguous_chunk,
            state.next_expected_offset,
            0,
        )
        .await?;
        return Ok(());
    }

    let crc32 = crc32_ieee(chunk.payload);
    if crc32 != chunk.chunk_crc32 {
        emit_chunk_status(
            state.transfer_id,
            chunk.chunk_index,
            "retrying",
            1,
            Some("crc32 mismatch"),
        )
        .await;
        send_file_ack(
            class,
            max_packet,
            state.transfer_id,
            state.highest_contiguous_chunk,
            state.next_expected_offset,
            DEFAULT_ACK_WINDOW_CREDIT,
        )
        .await?;
        return Ok(());
    }

    if matches!(transfer_relay_mode(), TransferRelayMode::RelayToBrowser) {
        if let Some(binary) = build_ws_chunk_envelope(&chunk) {
            if let Err(err) = ws::send_transfer_binary(binary).await {
                let transfer_id = state.transfer_id;
                let detail = err.detail();
                let _ = relay.remove(transfer_id);
                send_file_abort(
                    class,
                    max_packet,
                    transfer_id,
                    FILE_ABORT_REASON_CAPACITY,
                    detail,
                )
                .await?;
                emit_transfer_failed(transfer_id, detail).await;
                return Ok(());
            }
        } else {
            let transfer_id = state.transfer_id;
            let _ = relay.remove(transfer_id);
            send_file_abort(
                class,
                max_packet,
                transfer_id,
                FILE_ABORT_REASON_CAPACITY,
                "binary envelope overflow",
            )
            .await?;
            emit_transfer_failed(transfer_id, "binary envelope overflow").await;
            return Ok(());
        }
    }

    state.highest_contiguous_chunk = Some(chunk.chunk_index);
    state.next_expected_offset = chunk.offset + chunk.payload.len() as u64;
    state.received_size = state
        .received_size
        .saturating_add(chunk.payload.len() as u64);
    state.finished_chunks = state.finished_chunks.saturating_add(1);
    TRANSFER_RECEIVED_SIZE.store(state.received_size, Ordering::Release);
    TRANSFER_FINISHED_CHUNKS.store(state.finished_chunks, Ordering::Release);
    set_transfer_view_state(TransferViewState::Progress);

    let should_emit_progress = state.finished_chunks == state.chunk_count
        || state
            .finished_chunks
            .saturating_sub(state.last_progress_emit_finished_chunks)
            >= PROGRESS_EMIT_EVERY_CHUNKS;
    if should_emit_progress {
        state.last_progress_emit_finished_chunks = state.finished_chunks;
        emit_transfer_progress(state).await;
    }

    send_file_ack(
        class,
        max_packet,
        state.transfer_id,
        state.highest_contiguous_chunk,
        state.next_expected_offset,
        DEFAULT_ACK_WINDOW_CREDIT,
    )
    .await?;

    Ok(())
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
    let Some(close) = parse_file_close(payload) else {
        return Ok(());
    };

    let Some(state) = relay.remove(close.transfer_id) else {
        send_file_abort(
            class,
            max_packet,
            close.transfer_id,
            FILE_ABORT_REASON_UNKNOWN_TRANSFER,
            "close for unknown transfer",
        )
        .await?;
        emit_transfer_failed(close.transfer_id, "close for unknown transfer").await;
        return Ok(());
    };

    if close.sent_chunk_count != state.chunk_count
        || close.sent_total_size != state.total_size
        || state.received_size != state.total_size
        || state.finished_chunks != state.chunk_count
    {
        send_file_result(
            class,
            max_packet,
            state.transfer_id,
            FILE_RESULT_SIZE_MISMATCH,
            "size mismatch",
        )
        .await?;
        emit_transfer_failed(state.transfer_id, "size mismatch").await;
        return Ok(());
    }

    send_file_result(class, max_packet, state.transfer_id, FILE_RESULT_OK, "ok").await?;
    emit_transfer_finished(state.transfer_id).await;
    set_transfer_view_state(TransferViewState::Finished);
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
    let Some(abort) = parse_file_abort(payload) else {
        return Ok(());
    };

    let _ = relay.remove(abort.transfer_id);
    emit_transfer_aborted(abort.transfer_id, abort.reason_code, abort.detail).await;
    set_transfer_view_state(TransferViewState::Aborted);
    send_file_result(
        class,
        max_packet,
        abort.transfer_id,
        FILE_RESULT_ABORTED,
        abort.detail,
    )
    .await?;

    Ok(())
}

fn metadata_matches(existing: &RelayTransferState, incoming: &IncomingOpen<'_>) -> bool {
    existing.protocol_version == incoming.protocol_version
        && existing.total_size == incoming.total_size
        && existing.chunk_size == incoming.chunk_size
        && existing.chunk_count == incoming.chunk_count
        && existing.sha256 == incoming.sha256
        && existing.file_name.as_str() == incoming.file_name
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
    let mut payload = [0u8; 8 + 4 + 8 + 2];
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
    result_code: u8,
    detail: &str,
) -> Result<(), EndpointError>
where
    D: Driver<'d>,
{
    let mut payload: Vec<u8, 256> = Vec::new();
    let detail_bytes = detail.as_bytes();
    let detail_len = core::cmp::min(detail_bytes.len(), u16::MAX as usize);

    let _ = payload.extend_from_slice(&transfer_id.to_le_bytes());
    let _ = payload.push(result_code);
    let _ = payload.extend_from_slice(&(detail_len as u16).to_le_bytes());
    let _ = payload.extend_from_slice(&detail_bytes[..detail_len]);

    send_tlv(class, max_packet, TAG_FILE_RESULT, &payload).await
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
    let mut payload: Vec<u8, 256> = Vec::new();
    let detail_bytes = detail.as_bytes();
    let detail_len = core::cmp::min(detail_bytes.len(), u16::MAX as usize);

    let _ = payload.extend_from_slice(&transfer_id.to_le_bytes());
    let _ = payload.push(reason_code);
    let _ = payload.extend_from_slice(&(detail_len as u16).to_le_bytes());
    let _ = payload.extend_from_slice(&detail_bytes[..detail_len]);

    send_tlv(class, max_packet, TAG_FILE_ABORT, &payload).await
}

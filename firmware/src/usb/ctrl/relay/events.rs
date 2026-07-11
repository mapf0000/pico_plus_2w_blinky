use super::*;

pub(super) async fn emit_transfer_open(state: &RelayTransferState) {
    let mut sha256_hex: String<64> = String::new();
    for byte in state.sha256 {
        let _ = write!(&mut sha256_hex, "{:02x}", byte);
    }
    let escaped_name = escape_json_str(state.file_name.as_str());

    let mut event: String<{ ws::TRANSFER_TEXT_MAX }> = String::new();
    let _ = write!(
        &mut event,
        "{{\"event_type\":\"transfer/open\",\"version\":1,\"transfer_id\":{},\"file_name\":\"{}\",\"total_size\":{},\"chunk_size\":{},\"chunk_count\":{},\"sha256\":\"{}\"}}",
        state.transfer_id,
        escaped_name.as_str(),
        state.total_size,
        state.chunk_size,
        state.chunk_count,
        sha256_hex.as_str()
    );
    let _ = ws::send_transfer_text(event).await;
}

pub(super) async fn emit_transfer_progress(state: &RelayTransferState) {
    let mut event: String<{ ws::TRANSFER_TEXT_MAX }> = String::new();
    let _ = write!(
        &mut event,
        "{{\"event_type\":\"transfer/progress\",\"version\":1,\"transfer_id\":{},\"received_size\":{},\"total_size\":{},\"finished_chunks\":{},\"chunk_count\":{}}}",
        state.transfer_id,
        state.received_size,
        state.total_size,
        state.finished_chunks,
        state.chunk_count
    );
    let _ = ws::queue_transfer_text(event);
}

pub(super) async fn emit_chunk_status(
    transfer_id: u64,
    chunk_index: u32,
    status: &str,
    attempts: u32,
    error: Option<&str>,
) {
    let mut event: String<{ ws::TRANSFER_TEXT_MAX }> = String::new();
    let escaped_error = error.map(escape_json_str);
    match escaped_error {
        Some(err) => {
            let _ = write!(
                &mut event,
                "{{\"event_type\":\"transfer/chunk_status\",\"version\":1,\"transfer_id\":{},\"chunk_index\":{},\"status\":\"{}\",\"attempts\":{},\"error\":\"{}\"}}",
                transfer_id,
                chunk_index,
                status,
                attempts,
                err.as_str()
            );
        }
        None => {
            let _ = write!(
                &mut event,
                "{{\"event_type\":\"transfer/chunk_status\",\"version\":1,\"transfer_id\":{},\"chunk_index\":{},\"status\":\"{}\",\"attempts\":{}}}",
                transfer_id, chunk_index, status, attempts,
            );
        }
    }
    let _ = ws::queue_transfer_text(event);
}

pub(super) async fn emit_transfer_finished(transfer_id: u64) {
    let mut event: String<{ ws::TRANSFER_TEXT_MAX }> = String::new();
    let _ = write!(
        &mut event,
        "{{\"event_type\":\"transfer/finished\",\"version\":1,\"transfer_id\":{}}}",
        transfer_id
    );
    let _ = ws::send_transfer_text(event).await;
}

pub(super) async fn emit_transfer_failed(transfer_id: u64, reason: &str) {
    set_transfer_view_state(TransferViewState::Failed);
    let reason = escape_json_str(reason);
    let mut event: String<{ ws::TRANSFER_TEXT_MAX }> = String::new();
    let _ = write!(
        &mut event,
        "{{\"event_type\":\"transfer/failed\",\"version\":1,\"transfer_id\":{},\"reason\":\"{}\"}}",
        transfer_id,
        reason.as_str(),
    );
    let _ = ws::send_transfer_text(event).await;
}

pub(super) async fn emit_transfer_aborted(transfer_id: u64, reason_code: u8, detail: &str) {
    let detail = escape_json_str(detail);
    let mut event: String<{ ws::TRANSFER_TEXT_MAX }> = String::new();
    let _ = write!(
        &mut event,
        "{{\"event_type\":\"transfer/aborted\",\"version\":1,\"transfer_id\":{},\"reason_code\":{},\"detail\":\"{}\"}}",
        transfer_id,
        reason_code,
        detail.as_str(),
    );
    let _ = ws::send_transfer_text(event).await;
}

pub(super) fn build_ws_chunk_envelope(
    chunk: &IncomingChunk<'_>,
) -> Option<Vec<u8, { ws::TRANSFER_BINARY_MAX }>> {
    let payload_len: u16 = chunk.payload.len().try_into().ok()?;
    let mut out: Vec<u8, { ws::TRANSFER_BINARY_MAX }> = Vec::new();
    out.push(ws::WS_BINARY_KIND_TRANSFER).ok()?;
    out.extend_from_slice(&chunk.transfer_id.to_le_bytes())
        .ok()?;
    out.extend_from_slice(&chunk.chunk_index.to_le_bytes())
        .ok()?;
    out.extend_from_slice(&chunk.offset.to_le_bytes()).ok()?;
    out.extend_from_slice(&payload_len.to_le_bytes()).ok()?;
    out.extend_from_slice(&chunk.chunk_crc32.to_le_bytes())
        .ok()?;
    out.extend_from_slice(chunk.payload).ok()?;
    Some(out)
}

pub(in crate::usb::ctrl) fn forward_filesystem_page(payload: &[u8]) -> bool {
    if payload.len() < 10 {
        return false;
    }
    let version = u16::from_le_bytes([payload[0], payload[1]]);
    if version != FS_PROTOCOL_VERSION {
        return false;
    }

    let mut out: Vec<u8, { ws::TRANSFER_BINARY_MAX }> = Vec::new();
    if out.push(ws::WS_BINARY_KIND_FILESYSTEM).is_err() || out.extend_from_slice(payload).is_err() {
        return false;
    }
    ws::queue_transfer_binary(out).is_ok()
}

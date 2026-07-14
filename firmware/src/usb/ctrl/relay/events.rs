use super::*;

pub(super) async fn forward_secure_open(payload: &[u8]) -> bool {
    forward_binary(transfer_protocol::WS_BINARY_KIND_SECURE_OPEN, payload).await
}

pub(super) async fn forward_secure_chunk(payload: &[u8]) -> bool {
    forward_binary(transfer_protocol::WS_BINARY_KIND_SECURE_CHUNK, payload).await
}

pub(super) async fn forward_secure_close(payload: &[u8]) -> bool {
    forward_binary(transfer_protocol::WS_BINARY_KIND_SECURE_CLOSE, payload).await
}

pub(in crate::usb::ctrl) fn forward_secure_session(payload: &[u8]) -> bool {
    let mut out: Vec<u8, { ws::TRANSFER_BINARY_MAX }> = Vec::new();
    if out.push(transfer_protocol::WS_BINARY_KIND_SESSION).is_err()
        || out.extend_from_slice(payload).is_err()
    {
        return false;
    }
    ws::queue_transfer_binary(out).is_ok()
}

async fn forward_binary(kind: u8, payload: &[u8]) -> bool {
    let mut out: Vec<u8, { ws::TRANSFER_BINARY_MAX }> = Vec::new();
    if out.push(kind).is_err() || out.extend_from_slice(payload).is_err() {
        return false;
    }
    ws::send_transfer_binary(out).await.is_ok()
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

pub(super) async fn emit_transfer_progress(state: &RelayTransferState) {
    let mut event: String<{ ws::TRANSFER_TEXT_MAX }> = String::new();
    let _ = write!(
        &mut event,
        "{{\"event_type\":\"transfer/progress\",\"version\":2,\"transfer_id\":{},\"received_size\":{},\"total_size\":{},\"finished_chunks\":{},\"chunk_count\":{}}}",
        state.transfer_id,
        state.received_size,
        state.approximate_total_size(),
        state.finished_chunks,
        state.chunk_count
    );
    let _ = ws::queue_transfer_text(event);
}

pub(super) async fn emit_transfer_finished(transfer_id: u64) {
    let mut event: String<{ ws::TRANSFER_TEXT_MAX }> = String::new();
    let _ = write!(
        &mut event,
        "{{\"event_type\":\"transfer/finished\",\"version\":2,\"transfer_id\":{}}}",
        transfer_id
    );
    let _ = ws::send_transfer_text(event).await;
}

pub(super) async fn emit_transfer_aborted(transfer_id: u64, reason_code: u8, detail: &str) {
    let detail = escape_json_str(detail);
    let mut event: String<{ ws::TRANSFER_TEXT_MAX }> = String::new();
    let _ = write!(
        &mut event,
        "{{\"event_type\":\"transfer/aborted\",\"version\":2,\"transfer_id\":{},\"reason_code\":{},\"detail\":\"{}\"}}",
        transfer_id,
        reason_code,
        detail.as_str(),
    );
    let _ = ws::send_transfer_text(event).await;
}

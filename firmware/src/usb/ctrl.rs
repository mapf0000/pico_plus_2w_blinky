use core::fmt::Write as _;

use embassy_futures::select::{Either, select};
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, channel::Channel};
use embassy_usb::class::cdc_acm::CdcAcmClass;
use embassy_usb::driver::{Driver, EndpointError};
use heapless::{String, Vec};
use portable_atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};

use crate::http::routes::ws;
use crate::http::util::escape_json_str;

pub enum CtrlCommand {
    RequestStatus,
    Execute {
        command: &'static str,
    },
    RequestDbCredentials {
        prompt: &'static str,
    },
    StartTransfer {
        path: String<MAX_TRANSFER_PATH_LEN>,
    },
    SetTransferDefault {
        path: String<MAX_TRANSFER_PATH_LEN>,
    },
    StartTransferDefault,
    ListDirectory {
        request_id: u64,
        cursor: u32,
        entry_limit: u16,
        flags: u8,
        path: String<MAX_TRANSFER_PATH_LEN>,
    },
    CancelDirectoryList {
        request_id: u64,
    },
}

pub static CTRL_CHAN: Channel<ThreadModeRawMutex, CtrlCommand, 8> = Channel::new();
pub static CTRL_READY: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferRelayMode {
    RelayToBrowser,
    SimulationDrop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferViewState {
    Idle,
    Open,
    Progress,
    Finished,
    Failed,
    Aborted,
}

#[derive(Clone, Copy, Debug)]
pub struct TransferViewSnapshot {
    pub mode: TransferRelayMode,
    pub state: TransferViewState,
    pub transfer_id: u64,
    pub total_size: u64,
    pub received_size: u64,
    pub chunk_count: u32,
    pub finished_chunks: u32,
}

const MODE_RELAY_TO_BROWSER: u8 = 0;
const MODE_SIMULATION_DROP: u8 = 1;

const VIEW_STATE_IDLE: u8 = 0;
const VIEW_STATE_OPEN: u8 = 1;
const VIEW_STATE_PROGRESS: u8 = 2;
const VIEW_STATE_FINISHED: u8 = 3;
const VIEW_STATE_FAILED: u8 = 4;
const VIEW_STATE_ABORTED: u8 = 5;

static TRANSFER_MODE: AtomicU8 = AtomicU8::new(MODE_RELAY_TO_BROWSER);
static TRANSFER_STATE: AtomicU8 = AtomicU8::new(VIEW_STATE_IDLE);
static TRANSFER_ID: AtomicU64 = AtomicU64::new(0);
static TRANSFER_TOTAL_SIZE: AtomicU64 = AtomicU64::new(0);
static TRANSFER_RECEIVED_SIZE: AtomicU64 = AtomicU64::new(0);
static TRANSFER_CHUNK_COUNT: AtomicU32 = AtomicU32::new(0);
static TRANSFER_FINISHED_CHUNKS: AtomicU32 = AtomicU32::new(0);

pub fn transfer_relay_mode() -> TransferRelayMode {
    mode_from_raw(TRANSFER_MODE.load(Ordering::Acquire))
}

pub fn set_transfer_relay_mode(mode: TransferRelayMode) {
    TRANSFER_MODE.store(mode_to_raw(mode), Ordering::Release);
}

pub fn toggle_transfer_relay_mode() -> TransferRelayMode {
    let mode = match transfer_relay_mode() {
        TransferRelayMode::RelayToBrowser => TransferRelayMode::SimulationDrop,
        TransferRelayMode::SimulationDrop => TransferRelayMode::RelayToBrowser,
    };
    set_transfer_relay_mode(mode);
    mode
}

pub fn transfer_view_snapshot() -> TransferViewSnapshot {
    TransferViewSnapshot {
        mode: transfer_relay_mode(),
        state: view_state_from_raw(TRANSFER_STATE.load(Ordering::Acquire)),
        transfer_id: TRANSFER_ID.load(Ordering::Acquire),
        total_size: TRANSFER_TOTAL_SIZE.load(Ordering::Acquire),
        received_size: TRANSFER_RECEIVED_SIZE.load(Ordering::Acquire),
        chunk_count: TRANSFER_CHUNK_COUNT.load(Ordering::Acquire),
        finished_chunks: TRANSFER_FINISHED_CHUNKS.load(Ordering::Acquire),
    }
}

#[inline]
fn mode_from_raw(raw: u8) -> TransferRelayMode {
    match raw {
        MODE_SIMULATION_DROP => TransferRelayMode::SimulationDrop,
        _ => TransferRelayMode::RelayToBrowser,
    }
}

#[inline]
fn mode_to_raw(mode: TransferRelayMode) -> u8 {
    match mode {
        TransferRelayMode::RelayToBrowser => MODE_RELAY_TO_BROWSER,
        TransferRelayMode::SimulationDrop => MODE_SIMULATION_DROP,
    }
}

#[inline]
fn view_state_from_raw(raw: u8) -> TransferViewState {
    match raw {
        VIEW_STATE_OPEN => TransferViewState::Open,
        VIEW_STATE_PROGRESS => TransferViewState::Progress,
        VIEW_STATE_FINISHED => TransferViewState::Finished,
        VIEW_STATE_FAILED => TransferViewState::Failed,
        VIEW_STATE_ABORTED => TransferViewState::Aborted,
        _ => TransferViewState::Idle,
    }
}

#[inline]
fn set_transfer_view_state(state: TransferViewState) {
    let raw = match state {
        TransferViewState::Idle => VIEW_STATE_IDLE,
        TransferViewState::Open => VIEW_STATE_OPEN,
        TransferViewState::Progress => VIEW_STATE_PROGRESS,
        TransferViewState::Finished => VIEW_STATE_FINISHED,
        TransferViewState::Failed => VIEW_STATE_FAILED,
        TransferViewState::Aborted => VIEW_STATE_ABORTED,
    };
    TRANSFER_STATE.store(raw, Ordering::Release);
}

const TAG_EXECUTE: u8 = 1;
const TAG_DEBUG_MSG: u8 = 2;
const TAG_REQUEST_AGENT_STATUS: u8 = 7;
const TAG_AGENT_STATUS: u8 = 8;
const TAG_DB_CREDENTIALS_REQUEST: u8 = 11;
const TAG_DB_CREDENTIALS_RESPONSE: u8 = 12;

const TAG_FILE_OPEN: u8 = 20;
const TAG_FILE_CHUNK: u8 = 21;
const TAG_FILE_ACK: u8 = 22;
const TAG_FILE_CLOSE: u8 = 23;
const TAG_FILE_RESULT: u8 = 24;
const TAG_FILE_ABORT: u8 = 25;
const TAG_FILE_HEARTBEAT: u8 = 26;
const TAG_FILE_START_REQUEST: u8 = 27;
const TAG_FILE_SET_DEFAULT_PATH: u8 = 28;
const TAG_FS_LIST_REQUEST: u8 = 29;
const TAG_FS_LIST_PAGE: u8 = 30;
const TAG_FS_LIST_CANCEL: u8 = 31;

const FS_PROTOCOL_VERSION: u16 = 1;

const FILE_RESULT_OK: u8 = 0;
const FILE_RESULT_HASH_MISMATCH: u8 = 1;
const FILE_RESULT_SIZE_MISMATCH: u8 = 2;
const FILE_RESULT_ABORTED: u8 = 3;
const FILE_RESULT_INTERNAL_ERROR: u8 = 4;

const FILE_ABORT_REASON_PROTOCOL: u8 = 1;
const FILE_ABORT_REASON_METADATA_MISMATCH: u8 = 2;
const FILE_ABORT_REASON_INVALID_CHUNK: u8 = 3;
const FILE_ABORT_REASON_UNKNOWN_TRANSFER: u8 = 4;
const FILE_ABORT_REASON_CAPACITY: u8 = 5;

const TLV_HEADER_LEN: usize = 5;
const MAX_PAYLOAD_LEN: usize = 2048;
const LOCAL_BUF_LEN: usize = 64;
const HANDSHAKE_PAYLOAD: &[u8] = b"handshake";

const MAX_TRANSFER_FILE_NAME: usize = 96;
pub const MAX_TRANSFER_PATH_LEN: usize = 512;
const MAX_RELAY_TRANSFERS: usize = 4;
const FILE_CHUNK_OVERHEAD: usize = 8 + 4 + 8 + 2 + 4;
const FILE_CHUNK_MAX_DATA: usize = MAX_PAYLOAD_LEN - FILE_CHUNK_OVERHEAD;
const NONE_CONTIGUOUS_CHUNK: u32 = u32::MAX;
const DEFAULT_ACK_WINDOW_CREDIT: u16 = 8;
const PROGRESS_EMIT_EVERY_CHUNKS: u32 = 16;

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

struct RelayState {
    transfers: Vec<RelayTransferState, MAX_RELAY_TRANSFERS>,
}

impl RelayState {
    const fn new() -> Self {
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

struct TlvStreamDecoder {
    header: [u8; TLV_HEADER_LEN],
    header_len: usize,
    payload: [u8; MAX_PAYLOAD_LEN],
    payload_len: usize,
    payload_read: usize,
    tag: u8,
    reading_payload: bool,
}

impl TlvStreamDecoder {
    const fn new() -> Self {
        Self {
            header: [0; TLV_HEADER_LEN],
            header_len: 0,
            payload: [0; MAX_PAYLOAD_LEN],
            payload_len: 0,
            payload_read: 0,
            tag: 0,
            reading_payload: false,
        }
    }

    fn push_byte(&mut self, byte: u8) -> Option<(u8, usize)> {
        if self.reading_payload {
            self.payload[self.payload_read] = byte;
            self.payload_read += 1;
            if self.payload_read == self.payload_len {
                self.reading_payload = false;
                return Some((self.tag, self.payload_len));
            }
            return None;
        }

        self.header[self.header_len] = byte;
        self.header_len += 1;

        if self.header_len < TLV_HEADER_LEN {
            return None;
        }

        let payload_len = u32::from_le_bytes([
            self.header[1],
            self.header[2],
            self.header[3],
            self.header[4],
        ]) as usize;

        if payload_len > MAX_PAYLOAD_LEN {
            self.header.copy_within(1..TLV_HEADER_LEN, 0);
            self.header_len = TLV_HEADER_LEN - 1;
            return None;
        }

        self.tag = self.header[0];
        self.payload_len = payload_len;
        self.payload_read = 0;
        self.header_len = 0;

        if payload_len == 0 {
            return Some((self.tag, 0));
        }

        self.reading_payload = true;
        None
    }

    fn payload(&self, len: usize) -> &[u8] {
        &self.payload[..len]
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

pub async fn run_ctrl<'d, D>(mut class: CdcAcmClass<'d, D>) -> !
where
    D: Driver<'d>,
{
    class.wait_connection().await;
    CTRL_READY.store(true, Ordering::SeqCst);
    log::info!("usb: CDC control ready");

    let max_packet = class.max_packet_size() as usize;
    let mut packet = [0u8; LOCAL_BUF_LEN];
    let mut decoder = TlvStreamDecoder::new();
    let mut relay = RelayState::new();

    loop {
        match select(CTRL_CHAN.receive(), class.read_packet(&mut packet)).await {
            Either::First(cmd) => {
                if let Err(err) = handle_command(&mut class, max_packet, cmd).await {
                    log::warn!("usb: control send error: {:?}", err);
                }
            }
            Either::Second(result) => match result {
                Ok(count) => {
                    if count == 0 {
                        continue;
                    }
                    if let Err(err) = handle_incoming_bytes(
                        &mut class,
                        max_packet,
                        &mut decoder,
                        &mut relay,
                        &packet[..count],
                    )
                    .await
                    {
                        log::warn!("usb: control rx error: {:?}", err);
                    }
                }
                Err(err) => {
                    log::warn!("usb: control read error: {:?}", err);
                }
            },
        }
    }
}

async fn handle_command<'d, D>(
    class: &mut CdcAcmClass<'d, D>,
    max_packet: usize,
    cmd: CtrlCommand,
) -> Result<(), EndpointError>
where
    D: Driver<'d>,
{
    match cmd {
        CtrlCommand::RequestStatus => {
            send_tlv(class, max_packet, TAG_REQUEST_AGENT_STATUS, &[]).await
        }
        CtrlCommand::Execute { command } => {
            let payload = command.as_bytes();
            send_tlv(class, max_packet, TAG_EXECUTE, payload).await
        }
        CtrlCommand::RequestDbCredentials { prompt } => {
            let payload = prompt.as_bytes();
            send_tlv(class, max_packet, TAG_DB_CREDENTIALS_REQUEST, payload).await
        }
        CtrlCommand::StartTransfer { path } => {
            let payload = path.as_bytes();
            send_tlv(class, max_packet, TAG_FILE_START_REQUEST, payload).await
        }
        CtrlCommand::SetTransferDefault { path } => {
            let payload = path.as_bytes();
            send_tlv(class, max_packet, TAG_FILE_SET_DEFAULT_PATH, payload).await
        }
        CtrlCommand::StartTransferDefault => {
            send_tlv(class, max_packet, TAG_FILE_START_REQUEST, &[]).await
        }
        CtrlCommand::ListDirectory {
            request_id,
            cursor,
            entry_limit,
            flags,
            path,
        } => {
            let mut payload: Vec<u8, MAX_PAYLOAD_LEN> = Vec::new();
            let path_bytes = path.as_bytes();
            let path_len = path_bytes.len() as u16;
            let _ = payload.extend_from_slice(&FS_PROTOCOL_VERSION.to_le_bytes());
            let _ = payload.extend_from_slice(&request_id.to_le_bytes());
            let _ = payload.extend_from_slice(&cursor.to_le_bytes());
            let _ = payload.extend_from_slice(&entry_limit.to_le_bytes());
            let _ = payload.push(flags);
            let _ = payload.extend_from_slice(&path_len.to_le_bytes());
            let _ = payload.extend_from_slice(path_bytes);
            send_tlv(class, max_packet, TAG_FS_LIST_REQUEST, payload.as_slice()).await
        }
        CtrlCommand::CancelDirectoryList { request_id } => {
            send_tlv(
                class,
                max_packet,
                TAG_FS_LIST_CANCEL,
                &request_id.to_le_bytes(),
            )
            .await
        }
    }
}

async fn send_tlv<'d, D>(
    class: &mut CdcAcmClass<'d, D>,
    max_packet: usize,
    tag: u8,
    payload: &[u8],
) -> Result<(), EndpointError>
where
    D: Driver<'d>,
{
    if payload.len() > MAX_PAYLOAD_LEN {
        log::warn!("usb: payload too large ({} bytes)", payload.len());
        return Ok(());
    }

    let header = tlv_header(tag, payload.len() as u32);
    let total_len = TLV_HEADER_LEN + payload.len();

    if total_len <= max_packet && total_len <= LOCAL_BUF_LEN {
        let mut buf = [0u8; LOCAL_BUF_LEN];
        buf[..TLV_HEADER_LEN].copy_from_slice(&header);
        buf[TLV_HEADER_LEN..total_len].copy_from_slice(payload);
        class.write_packet(&buf[..total_len]).await?;
        return Ok(());
    }

    class.write_packet(&header).await?;
    write_payload_chunks(class, max_packet, payload).await?;
    Ok(())
}

async fn handle_incoming_bytes<'d, D>(
    class: &mut CdcAcmClass<'d, D>,
    max_packet: usize,
    decoder: &mut TlvStreamDecoder,
    relay: &mut RelayState,
    bytes: &[u8],
) -> Result<(), EndpointError>
where
    D: Driver<'d>,
{
    for byte in bytes {
        if let Some((tag, payload_len)) = decoder.push_byte(*byte) {
            let payload = decoder.payload(payload_len);
            handle_host_frame(class, max_packet, relay, tag, payload).await?;
        }
    }
    Ok(())
}

async fn handle_host_frame<'d, D>(
    class: &mut CdcAcmClass<'d, D>,
    max_packet: usize,
    relay: &mut RelayState,
    tag: u8,
    payload: &[u8],
) -> Result<(), EndpointError>
where
    D: Driver<'d>,
{
    match tag {
        TAG_AGENT_STATUS => {
            let name = core::str::from_utf8(payload).unwrap_or("<invalid utf-8>");
            log::info!("usb: host agent status: {}", name);
        }
        TAG_REQUEST_AGENT_STATUS => {
            if payload == HANDSHAKE_PAYLOAD {
                send_tlv(class, max_packet, TAG_DEBUG_MSG, b"handshake-ok").await?;
                log::info!("usb: handshake ok");
                return Ok(());
            }
            if payload.is_empty() {
                send_tlv(class, max_packet, TAG_DEBUG_MSG, b"probe-ok").await?;
            }
        }
        TAG_DB_CREDENTIALS_RESPONSE => {
            if payload.is_empty() {
                log::warn!("usb: db credential prompt canceled");
                return Ok(());
            }
            let Some(split_at) = payload.iter().position(|byte| *byte == 0) else {
                log::warn!("usb: invalid db credential payload");
                return Ok(());
            };
            let (user_bytes, pass_bytes) = payload.split_at(split_at);
            let user = core::str::from_utf8(user_bytes).unwrap_or("<invalid utf-8>");
            let pass_len = pass_bytes.len().saturating_sub(1);
            log::info!(
                "usb: db credentials received for user={} (password length={})",
                user,
                pass_len
            );
        }
        TAG_FILE_OPEN => {
            handle_file_open(class, max_packet, relay, payload).await?;
        }
        TAG_FILE_CHUNK => {
            handle_file_chunk(class, max_packet, relay, payload).await?;
        }
        TAG_FILE_CLOSE => {
            handle_file_close(class, max_packet, relay, payload).await?;
        }
        TAG_FILE_ABORT => {
            handle_file_abort(class, max_packet, relay, payload).await?;
        }
        TAG_FILE_HEARTBEAT => {
            log::debug!("usb: transfer heartbeat");
        }
        TAG_FS_LIST_PAGE => {
            if !forward_filesystem_page(payload) {
                log::warn!("usb: failed to forward filesystem page");
            }
        }
        TAG_EXECUTE
        | TAG_DEBUG_MSG
        | TAG_DB_CREDENTIALS_REQUEST
        | TAG_FILE_ACK
        | TAG_FILE_RESULT
        | TAG_FS_LIST_REQUEST
        | TAG_FS_LIST_CANCEL => {
            log::debug!("usb: unhandled tag={} len={}", tag, payload.len());
        }
        _ => {
            log::debug!("usb: unknown tag={} len={}", tag, payload.len());
        }
    }

    Ok(())
}

async fn handle_file_open<'d, D>(
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

async fn handle_file_chunk<'d, D>(
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
            if let Err(err) = ws::queue_transfer_binary(binary) {
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

async fn handle_file_close<'d, D>(
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

async fn handle_file_abort<'d, D>(
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

async fn emit_transfer_open(state: &RelayTransferState) {
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
    let _ = ws::queue_transfer_text(event);
}

async fn emit_transfer_progress(state: &RelayTransferState) {
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

async fn emit_chunk_status(
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

async fn emit_transfer_finished(transfer_id: u64) {
    let mut event: String<{ ws::TRANSFER_TEXT_MAX }> = String::new();
    let _ = write!(
        &mut event,
        "{{\"event_type\":\"transfer/finished\",\"version\":1,\"transfer_id\":{}}}",
        transfer_id
    );
    let _ = ws::queue_transfer_text(event);
}

async fn emit_transfer_failed(transfer_id: u64, reason: &str) {
    set_transfer_view_state(TransferViewState::Failed);
    let reason = escape_json_str(reason);
    let mut event: String<{ ws::TRANSFER_TEXT_MAX }> = String::new();
    let _ = write!(
        &mut event,
        "{{\"event_type\":\"transfer/failed\",\"version\":1,\"transfer_id\":{},\"reason\":\"{}\"}}",
        transfer_id,
        reason.as_str(),
    );
    let _ = ws::queue_transfer_text(event);
}

async fn emit_transfer_aborted(transfer_id: u64, reason_code: u8, detail: &str) {
    let detail = escape_json_str(detail);
    let mut event: String<{ ws::TRANSFER_TEXT_MAX }> = String::new();
    let _ = write!(
        &mut event,
        "{{\"event_type\":\"transfer/aborted\",\"version\":1,\"transfer_id\":{},\"reason_code\":{},\"detail\":\"{}\"}}",
        transfer_id,
        reason_code,
        detail.as_str(),
    );
    let _ = ws::queue_transfer_text(event);
}

fn build_ws_chunk_envelope(
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

fn forward_filesystem_page(payload: &[u8]) -> bool {
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

fn parse_file_open(payload: &[u8]) -> Option<IncomingOpen<'_>> {
    let mut cursor = payload;
    let protocol_version = take_u16(&mut cursor)?;
    let transfer_id = take_u64(&mut cursor)?;
    let total_size = take_u64(&mut cursor)?;
    let chunk_size = take_u16(&mut cursor)?;
    let chunk_count = take_u32(&mut cursor)?;
    let sha256 = take_array_32(&mut cursor)?;
    let file_name_len = take_u16(&mut cursor)? as usize;
    let file_name_bytes = take_bytes(&mut cursor, file_name_len)?;
    let file_name = core::str::from_utf8(file_name_bytes).ok()?;

    Some(IncomingOpen {
        protocol_version,
        transfer_id,
        total_size,
        chunk_size,
        chunk_count,
        sha256,
        file_name,
    })
}

fn parse_file_chunk(payload: &[u8]) -> Option<IncomingChunk<'_>> {
    let mut cursor = payload;
    let transfer_id = take_u64(&mut cursor)?;
    let chunk_index = take_u32(&mut cursor)?;
    let offset = take_u64(&mut cursor)?;
    let payload_len = take_u16(&mut cursor)? as usize;
    if payload_len > FILE_CHUNK_MAX_DATA {
        return None;
    }
    let payload = take_bytes(&mut cursor, payload_len)?;
    let chunk_crc32 = take_u32(&mut cursor)?;

    Some(IncomingChunk {
        transfer_id,
        chunk_index,
        offset,
        payload,
        chunk_crc32,
    })
}

fn parse_file_close(payload: &[u8]) -> Option<IncomingClose> {
    let mut cursor = payload;
    Some(IncomingClose {
        transfer_id: take_u64(&mut cursor)?,
        sent_chunk_count: take_u32(&mut cursor)?,
        sent_total_size: take_u64(&mut cursor)?,
    })
}

fn parse_file_abort(payload: &[u8]) -> Option<IncomingAbort<'_>> {
    let mut cursor = payload;
    let transfer_id = take_u64(&mut cursor)?;
    let reason_code = take_u8(&mut cursor)?;
    let detail_len = take_u16(&mut cursor)? as usize;
    let detail_bytes = take_bytes(&mut cursor, detail_len)?;
    let detail = core::str::from_utf8(detail_bytes).ok()?;

    Some(IncomingAbort {
        transfer_id,
        reason_code,
        detail,
    })
}

fn crc32_ieee(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc ^= *byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

async fn write_payload_chunks<'d, D>(
    class: &mut CdcAcmClass<'d, D>,
    max_packet: usize,
    payload: &[u8],
) -> Result<(), EndpointError>
where
    D: Driver<'d>,
{
    let mut offset = 0;
    while offset < payload.len() {
        let remaining = payload.len() - offset;
        let chunk_len = remaining.min(max_packet);
        let chunk = &payload[offset..offset + chunk_len];
        class.write_packet(chunk).await?;
        offset += chunk_len;
    }

    if payload.len() % max_packet == 0 {
        class.write_packet(&[]).await?;
    }

    Ok(())
}

fn tlv_header(tag: u8, len: u32) -> [u8; TLV_HEADER_LEN] {
    let len_bytes = len.to_le_bytes();
    [tag, len_bytes[0], len_bytes[1], len_bytes[2], len_bytes[3]]
}

fn take_bytes<'a>(cursor: &mut &'a [u8], len: usize) -> Option<&'a [u8]> {
    if cursor.len() < len {
        return None;
    }
    let (head, tail) = cursor.split_at(len);
    *cursor = tail;
    Some(head)
}

fn take_u8(cursor: &mut &[u8]) -> Option<u8> {
    Some(take_bytes(cursor, 1)?[0])
}

fn take_u16(cursor: &mut &[u8]) -> Option<u16> {
    let bytes = take_bytes(cursor, 2)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn take_u32(cursor: &mut &[u8]) -> Option<u32> {
    let bytes = take_bytes(cursor, 4)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn take_u64(cursor: &mut &[u8]) -> Option<u64> {
    let bytes = take_bytes(cursor, 8)?;
    Some(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

fn take_array_32(cursor: &mut &[u8]) -> Option<[u8; 32]> {
    let bytes = take_bytes(cursor, 32)?;
    let mut out = [0u8; 32];
    out.copy_from_slice(bytes);
    Some(out)
}

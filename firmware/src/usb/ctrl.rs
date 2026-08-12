use core::fmt::Write as _;

use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Instant, WithTimeout};
use embassy_usb::class::cdc_acm::CdcAcmClass;
use embassy_usb::driver::{Driver, EndpointError};
use heapless::{String, Vec};
use portable_atomic::Ordering;

use crate::http::transfer;
use crate::http::util::escape_json_str;

#[allow(
    clippy::large_enum_variant,
    reason = "the bounded no_std command channel intentionally owns payloads and paths without heap indirection"
)]
pub enum CtrlCommand {
    RequestStatus,
    Execute {
        command: &'static str,
    },
    RequestDbCredentials {
        prompt: &'static str,
    },
    SecureTransfer {
        payload: Vec<u8, MAX_SECURE_TRANSFER_FRAME>,
    },
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

mod relay;
mod view;
use relay::{
    RelayState, forward_filesystem_page, forward_secure_session, handle_file_abort,
    handle_file_chunk, handle_file_close, handle_file_open,
};
pub use view::{
    CTRL_CHAN, CTRL_READY, TransferRelayMode, TransferViewSnapshot, TransferViewState,
    set_transfer_relay_mode, transfer_relay_mode, transfer_view_snapshot,
};
use view::{
    TRANSFER_CHUNK_COUNT, TRANSFER_FINISHED_CHUNKS, TRANSFER_ID, TRANSFER_RECEIVED_SIZE,
    TRANSFER_TOTAL_SIZE, set_transfer_view_state,
};

const TAG_EXECUTE: u8 = 1;
const TAG_DEBUG_MSG: u8 = 2;
const TAG_REQUEST_AGENT_STATUS: u8 = 7;
const TAG_AGENT_STATUS: u8 = 8;
const TAG_HOST_OS: u8 = 34;
const TAG_DB_CREDENTIALS_REQUEST: u8 = 11;
const TAG_DB_CREDENTIALS_RESPONSE: u8 = 12;

const TAG_FILE_OPEN: u8 = 20;
const TAG_FILE_CHUNK: u8 = 21;
const TAG_FILE_ACK: u8 = 22;
const TAG_FILE_CLOSE: u8 = 23;
const TAG_FILE_RESULT: u8 = 24;
const TAG_FILE_ABORT: u8 = 25;
const TAG_FILE_HEARTBEAT: u8 = 26;
const TAG_FS_LIST_REQUEST: u8 = 29;
const TAG_FS_LIST_PAGE: u8 = 30;
const TAG_FS_LIST_CANCEL: u8 = 31;
const TAG_TRANSFER_SESSION_TO_HOST: u8 = transfer_protocol::TAG_TRANSFER_SESSION_TO_HOST;
const TAG_TRANSFER_SESSION_TO_BROWSER: u8 = transfer_protocol::TAG_TRANSFER_SESSION_TO_BROWSER;
const TAG_USB_BENCHMARK_START: u8 = transfer_protocol::TAG_USB_BENCHMARK_START;
const TAG_USB_BENCHMARK_DATA: u8 = transfer_protocol::TAG_USB_BENCHMARK_DATA;
const TAG_USB_BENCHMARK_FINISH: u8 = transfer_protocol::TAG_USB_BENCHMARK_FINISH;
const TAG_USB_BENCHMARK_RESULT: u8 = transfer_protocol::TAG_USB_BENCHMARK_RESULT;
const TAG_USB_RAW_BENCHMARK_START: u8 = transfer_protocol::TAG_USB_RAW_BENCHMARK_START;
const TAG_USB_RAW_BENCHMARK_RESULT: u8 = transfer_protocol::TAG_USB_RAW_BENCHMARK_RESULT;

const FS_PROTOCOL_VERSION: u16 = crate::capabilities::FILESYSTEM_PROTOCOL_VERSION;
const FILE_RESULT_OK: u8 = 0;
const FILE_RESULT_SIZE_MISMATCH: u8 = 2;
const FILE_RESULT_ABORTED: u8 = 3;

const FILE_ABORT_REASON_METADATA_MISMATCH: u8 = 2;
const FILE_ABORT_REASON_INVALID_CHUNK: u8 = 3;
const FILE_ABORT_REASON_UNKNOWN_TRANSFER: u8 = 4;
const FILE_ABORT_REASON_CAPACITY: u8 = 5;

const TLV_HEADER_LEN: usize = 5;
const MAX_PAYLOAD_LEN: usize = 2048;
const LOCAL_BUF_LEN: usize = 64;
const HANDSHAKE_PAYLOAD: &[u8] = b"handshake";

pub const MAX_TRANSFER_PATH_LEN: usize = 512;
pub const MAX_SECURE_TRANSFER_FRAME: usize = transfer_protocol::MAX_SECURE_SESSION_FRAME;
const MAX_RELAY_TRANSFERS: usize = 4;
const NONE_CONTIGUOUS_CHUNK: u32 = u32::MAX;
const DEFAULT_ACK_WINDOW_CREDIT: u16 = 8;
const PROGRESS_EMIT_EVERY_CHUNKS: u32 = 64;

struct UsbBenchmarkState {
    active: bool,
    token: u32,
    ack_every: u16,
    expected_sequence: u32,
    bytes: u64,
    checksum: u32,
    started_at_us: Option<u64>,
}

impl UsbBenchmarkState {
    const fn new() -> Self {
        Self {
            active: false,
            token: 0,
            ack_every: 0,
            expected_sequence: 0,
            bytes: 0,
            checksum: 0,
            started_at_us: None,
        }
    }

    fn start(&mut self, payload: &[u8]) -> u8 {
        if payload.len() != 8 {
            self.active = false;
            return transfer_protocol::USB_BENCHMARK_STATUS_ERROR;
        }
        let version = u16::from_le_bytes([payload[0], payload[1]]);
        let ack_every = u16::from_le_bytes([payload[6], payload[7]]);
        if version != transfer_protocol::USB_BENCHMARK_VERSION || ack_every == 0 {
            self.active = false;
            return transfer_protocol::USB_BENCHMARK_STATUS_ERROR;
        }
        self.active = true;
        self.token = u32::from_le_bytes([payload[2], payload[3], payload[4], payload[5]]);
        self.ack_every = ack_every;
        self.expected_sequence = 0;
        self.bytes = 0;
        self.checksum = 0;
        self.started_at_us = None;
        transfer_protocol::USB_BENCHMARK_STATUS_STARTED
    }

    fn ingest(&mut self, payload: &[u8]) -> Option<u8> {
        if !self.active || payload.len() < transfer_protocol::USB_BENCHMARK_DATA_HEADER_LEN {
            return Some(transfer_protocol::USB_BENCHMARK_STATUS_ERROR);
        }
        let token = u32::from_le_bytes(payload[0..4].try_into().ok()?);
        let sequence = u32::from_le_bytes(payload[4..8].try_into().ok()?);
        if token != self.token || sequence != self.expected_sequence {
            self.active = false;
            return Some(transfer_protocol::USB_BENCHMARK_STATUS_ERROR);
        }
        let data = &payload[transfer_protocol::USB_BENCHMARK_DATA_HEADER_LEN..];
        if self.started_at_us.is_none() {
            self.started_at_us = Some(Instant::now().as_micros());
        }
        self.bytes = self.bytes.saturating_add(data.len() as u64);
        self.checksum = data.iter().fold(self.checksum, |checksum, byte| {
            checksum.wrapping_add(u32::from(*byte))
        });
        self.expected_sequence = self.expected_sequence.saturating_add(1);
        self.expected_sequence
            .is_multiple_of(u32::from(self.ack_every))
            .then_some(transfer_protocol::USB_BENCHMARK_STATUS_PROGRESS)
    }

    fn finish(&mut self, payload: &[u8]) -> u8 {
        let valid = self.active
            && payload.len() == 8
            && u32::from_le_bytes(payload[0..4].try_into().unwrap_or_default()) == self.token
            && u32::from_le_bytes(payload[4..8].try_into().unwrap_or_default())
                == self.expected_sequence;
        self.active = false;
        if valid {
            transfer_protocol::USB_BENCHMARK_STATUS_COMPLETE
        } else {
            transfer_protocol::USB_BENCHMARK_STATUS_ERROR
        }
    }

    fn encode_result(&self, status: u8) -> [u8; transfer_protocol::USB_BENCHMARK_RESULT_LEN] {
        let mut out = [0; transfer_protocol::USB_BENCHMARK_RESULT_LEN];
        out[0..2].copy_from_slice(&transfer_protocol::USB_BENCHMARK_VERSION.to_le_bytes());
        out[2] = status;
        out[3..7].copy_from_slice(&self.token.to_le_bytes());
        out[7..11].copy_from_slice(&self.expected_sequence.saturating_sub(1).to_le_bytes());
        out[11..15].copy_from_slice(&self.expected_sequence.to_le_bytes());
        out[15..23].copy_from_slice(&self.bytes.to_le_bytes());
        let elapsed_us = self.started_at_us.map_or(0, |started| {
            Instant::now().as_micros().saturating_sub(started)
        });
        out[23..31].copy_from_slice(&elapsed_us.to_le_bytes());
        out[31..35].copy_from_slice(&self.checksum.to_le_bytes());
        out
    }
}

struct RawUsbBenchmarkState {
    active: bool,
    token: u32,
    expected_bytes: u64,
    bytes: u64,
    packets: u32,
    full_packets: u32,
    short_packets: u32,
    started_at_us: Option<u64>,
}

impl RawUsbBenchmarkState {
    const fn new() -> Self {
        Self {
            active: false,
            token: 0,
            expected_bytes: 0,
            bytes: 0,
            packets: 0,
            full_packets: 0,
            short_packets: 0,
            started_at_us: None,
        }
    }

    fn start(&mut self, payload: &[u8]) -> u8 {
        if payload.len() != transfer_protocol::USB_RAW_BENCHMARK_START_LEN {
            self.active = false;
            return transfer_protocol::USB_BENCHMARK_STATUS_ERROR;
        }
        let version = u16::from_le_bytes([payload[0], payload[1]]);
        let expected_bytes = u64::from_le_bytes(payload[6..14].try_into().unwrap_or_default());
        if version != transfer_protocol::USB_RAW_BENCHMARK_VERSION
            || expected_bytes == 0
            || expected_bytes > transfer_protocol::USB_RAW_BENCHMARK_MAX_BYTES
        {
            self.active = false;
            return transfer_protocol::USB_BENCHMARK_STATUS_ERROR;
        }

        self.active = true;
        self.token = u32::from_le_bytes(payload[2..6].try_into().unwrap_or_default());
        self.expected_bytes = expected_bytes;
        self.bytes = 0;
        self.packets = 0;
        self.full_packets = 0;
        self.short_packets = 0;
        self.started_at_us = None;
        transfer_protocol::USB_BENCHMARK_STATUS_STARTED
    }

    fn ingest_packet(&mut self, packet: &[u8], max_packet: usize) -> Option<u8> {
        if !self.active || packet.is_empty() {
            return None;
        }
        if self.started_at_us.is_none() {
            self.started_at_us = Some(Instant::now().as_micros());
        }

        let Some(next_bytes) = self.bytes.checked_add(packet.len() as u64) else {
            self.active = false;
            return Some(transfer_protocol::USB_BENCHMARK_STATUS_ERROR);
        };
        self.packets = self.packets.saturating_add(1);
        if packet.len() == max_packet {
            self.full_packets = self.full_packets.saturating_add(1);
        } else {
            self.short_packets = self.short_packets.saturating_add(1);
        }
        self.bytes = next_bytes;

        if self.bytes > self.expected_bytes {
            self.active = false;
            return Some(transfer_protocol::USB_BENCHMARK_STATUS_ERROR);
        }
        if self.bytes == self.expected_bytes {
            self.active = false;
            return Some(transfer_protocol::USB_BENCHMARK_STATUS_COMPLETE);
        }
        None
    }

    fn encode_result(&self, status: u8) -> [u8; transfer_protocol::USB_RAW_BENCHMARK_RESULT_LEN] {
        let mut out = [0; transfer_protocol::USB_RAW_BENCHMARK_RESULT_LEN];
        out[0..2].copy_from_slice(&transfer_protocol::USB_RAW_BENCHMARK_VERSION.to_le_bytes());
        out[2] = status;
        out[3..7].copy_from_slice(&self.token.to_le_bytes());
        out[7..15].copy_from_slice(&self.bytes.to_le_bytes());
        let elapsed_us = self.started_at_us.map_or(0, |started| {
            Instant::now().as_micros().saturating_sub(started)
        });
        out[15..23].copy_from_slice(&elapsed_us.to_le_bytes());
        out[23..27].copy_from_slice(&self.packets.to_le_bytes());
        out[27..31].copy_from_slice(&self.full_packets.to_le_bytes());
        out[31..35].copy_from_slice(&self.short_packets.to_le_bytes());
        out
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

pub async fn run_ctrl<'d, D>(mut class: CdcAcmClass<'d, D>) -> !
where
    D: Driver<'d>,
{
    class.wait_connection().await;
    crate::capabilities::clear_host_agent();
    CTRL_READY.store(true, Ordering::SeqCst);
    log::info!("usb: CDC control ready");

    let max_packet = class.max_packet_size() as usize;
    let mut packet = [0u8; LOCAL_BUF_LEN];
    let mut decoder = TlvStreamDecoder::new();
    let mut relay = RelayState::new();
    let mut benchmark = UsbBenchmarkState::new();
    let mut raw_benchmark = RawUsbBenchmarkState::new();

    loop {
        if raw_benchmark.active {
            match class
                .read_packet(&mut packet)
                .with_timeout(Duration::from_secs(5))
                .await
            {
                Ok(Ok(count)) if count > 0 => {
                    if let Some(status) = raw_benchmark.ingest_packet(&packet[..count], max_packet)
                    {
                        let result = raw_benchmark.encode_result(status);
                        if let Err(err) = send_tlv(
                            &mut class,
                            max_packet,
                            TAG_USB_RAW_BENCHMARK_RESULT,
                            &result,
                        )
                        .await
                        {
                            log::warn!("usb: raw benchmark result send error: {:?}", err);
                        }
                    }
                }
                Ok(Ok(_)) => {}
                Ok(Err(err)) => {
                    raw_benchmark.active = false;
                    log::warn!("usb: raw benchmark read error: {:?}", err);
                }
                Err(_) => {
                    raw_benchmark.active = false;
                    let result =
                        raw_benchmark.encode_result(transfer_protocol::USB_BENCHMARK_STATUS_ERROR);
                    if let Err(err) = send_tlv(
                        &mut class,
                        max_packet,
                        TAG_USB_RAW_BENCHMARK_RESULT,
                        &result,
                    )
                    .await
                    {
                        log::warn!("usb: raw benchmark timeout result send error: {:?}", err);
                    }
                }
            }
            continue;
        }

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
                        &mut benchmark,
                        &mut raw_benchmark,
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
        CtrlCommand::SecureTransfer { payload } => {
            send_tlv(
                class,
                max_packet,
                TAG_TRANSFER_SESSION_TO_HOST,
                payload.as_slice(),
            )
            .await
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
    benchmark: &mut UsbBenchmarkState,
    raw_benchmark: &mut RawUsbBenchmarkState,
    bytes: &[u8],
) -> Result<(), EndpointError>
where
    D: Driver<'d>,
{
    for byte in bytes {
        if let Some((tag, payload_len)) = decoder.push_byte(*byte) {
            let payload = decoder.payload(payload_len);
            handle_host_frame(
                class,
                max_packet,
                relay,
                benchmark,
                raw_benchmark,
                tag,
                payload,
            )
            .await?;
        }
    }
    Ok(())
}

async fn handle_host_frame<'d, D>(
    class: &mut CdcAcmClass<'d, D>,
    max_packet: usize,
    relay: &mut RelayState,
    benchmark: &mut UsbBenchmarkState,
    raw_benchmark: &mut RawUsbBenchmarkState,
    tag: u8,
    payload: &[u8],
) -> Result<(), EndpointError>
where
    D: Driver<'d>,
{
    match tag {
        TAG_USB_BENCHMARK_START => {
            let status = benchmark.start(payload);
            let result = benchmark.encode_result(status);
            send_tlv(class, max_packet, TAG_USB_BENCHMARK_RESULT, &result).await?;
        }
        TAG_USB_BENCHMARK_DATA => {
            if let Some(status) = benchmark.ingest(payload) {
                let result = benchmark.encode_result(status);
                send_tlv(class, max_packet, TAG_USB_BENCHMARK_RESULT, &result).await?;
            }
        }
        TAG_USB_BENCHMARK_FINISH => {
            let status = benchmark.finish(payload);
            let result = benchmark.encode_result(status);
            send_tlv(class, max_packet, TAG_USB_BENCHMARK_RESULT, &result).await?;
        }
        TAG_USB_RAW_BENCHMARK_START => {
            let status = raw_benchmark.start(payload);
            let result = raw_benchmark.encode_result(status);
            send_tlv(class, max_packet, TAG_USB_RAW_BENCHMARK_RESULT, &result).await?;
        }
        TAG_AGENT_STATUS => {
            crate::capabilities::record_host_agent_status(payload);
            let snapshot = crate::capabilities::host_agent_snapshot();
            log::info!(
                "usb: host agent status: version={} host={}",
                snapshot.version.as_str(),
                snapshot.hostname.as_str()
            );
            let _ = transfer::queue_text(crate::capabilities::hello_json());
        }
        TAG_HOST_OS => {
            crate::capabilities::record_host_os(payload);
            log::info!(
                "usb: detected host OS: {}",
                crate::capabilities::host_agent_snapshot().host_os.as_str()
            );
        }
        TAG_REQUEST_AGENT_STATUS => {
            crate::capabilities::mark_host_agent_seen();
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
        TAG_TRANSFER_SESSION_TO_BROWSER => {
            if !forward_secure_session(payload) {
                log::warn!("usb: failed to forward secure transfer session message");
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

    if payload.len().is_multiple_of(max_packet) {
        class.write_packet(&[]).await?;
    }

    Ok(())
}

fn tlv_header(tag: u8, len: u32) -> [u8; TLV_HEADER_LEN] {
    let len_bytes = len.to_le_bytes();
    [tag, len_bytes[0], len_bytes[1], len_bytes[2], len_bytes[3]]
}

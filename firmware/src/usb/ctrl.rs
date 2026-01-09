use core::sync::atomic::{AtomicBool, Ordering};

use embassy_futures::select::{select, Either};
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, channel::Channel};
use embassy_usb::class::cdc_acm::CdcAcmClass;
use embassy_usb::driver::{Driver, EndpointError};

pub enum CtrlCommand {
    RequestStatus,
    Execute { command: &'static str },
}

pub static CTRL_CHAN: Channel<ThreadModeRawMutex, CtrlCommand, 8> = Channel::new();
pub static CTRL_READY: AtomicBool = AtomicBool::new(false);

const TAG_EXECUTE: u8 = 1;
const TAG_DEBUG_MSG: u8 = 2;
const TAG_REQUEST_AGENT_STATUS: u8 = 7;
const TAG_AGENT_STATUS: u8 = 8;
const TLV_HEADER_LEN: usize = 5;
const MAX_PAYLOAD_LEN: usize = 2048;
const LOCAL_BUF_LEN: usize = 64;
const RX_BUF_LEN: usize = 128;
const HANDSHAKE_PAYLOAD: &[u8] = b"handshake";

pub async fn run_ctrl<'d, D>(mut class: CdcAcmClass<'d, D>) -> !
where
    D: Driver<'d>,
{
    class.wait_connection().await;
    CTRL_READY.store(true, Ordering::SeqCst);
    log::info!("usb: CDC control ready");

    let max_packet = class.max_packet_size() as usize;
    let mut packet = [0u8; LOCAL_BUF_LEN];
    let mut rx_buf = [0u8; RX_BUF_LEN];
    let mut rx_len = 0usize;
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
                    if rx_len + count > RX_BUF_LEN {
                        rx_len = 0;
                    }
                    let end = (rx_len + count).min(RX_BUF_LEN);
                    let copy_len = end.saturating_sub(rx_len);
                    rx_buf[rx_len..end].copy_from_slice(&packet[..copy_len]);
                    rx_len = end;
                    if let Err(err) =
                        handle_incoming(&mut class, max_packet, &mut rx_buf, &mut rx_len).await
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
        CtrlCommand::RequestStatus => send_tlv(class, max_packet, TAG_REQUEST_AGENT_STATUS, &[]).await,
        CtrlCommand::Execute { command } => {
            let payload = command.as_bytes();
            send_tlv(class, max_packet, TAG_EXECUTE, payload).await
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

async fn handle_incoming<'d, D>(
    class: &mut CdcAcmClass<'d, D>,
    max_packet: usize,
    buf: &mut [u8; RX_BUF_LEN],
    len: &mut usize,
) -> Result<(), EndpointError>
where
    D: Driver<'d>,
{
    loop {
        if *len < TLV_HEADER_LEN {
            break;
        }
        let tag = buf[0];
        let payload_len =
            u32::from_le_bytes([buf[1], buf[2], buf[3], buf[4]]) as usize;
        if payload_len > MAX_PAYLOAD_LEN {
            drain(buf, len, 1);
            continue;
        }
        let total = TLV_HEADER_LEN + payload_len;
        if *len < total {
            break;
        }
        let payload = &buf[TLV_HEADER_LEN..total];
        handle_host_frame(class, max_packet, tag, payload).await?;
        drain(buf, len, total);
    }
    Ok(())
}

async fn handle_host_frame<'d, D>(
    class: &mut CdcAcmClass<'d, D>,
    max_packet: usize,
    tag: u8,
    payload: &[u8],
) -> Result<(), EndpointError>
where
    D: Driver<'d>,
{
    if tag == TAG_AGENT_STATUS {
        let name = core::str::from_utf8(payload).unwrap_or("<invalid utf-8>");
        log::info!("usb: host agent status: {}", name);
        return Ok(());
    }
    if tag == TAG_REQUEST_AGENT_STATUS {
        if payload == HANDSHAKE_PAYLOAD {
            send_tlv(class, max_packet, TAG_DEBUG_MSG, b"handshake-ok").await?;
            log::info!("usb: handshake ok");
            return Ok(());
        }
        if payload.is_empty() {
            send_tlv(class, max_packet, TAG_DEBUG_MSG, b"probe-ok").await?;
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

    if payload.len() % max_packet == 0 {
        class.write_packet(&[]).await?;
    }

    Ok(())
}

fn tlv_header(tag: u8, len: u32) -> [u8; TLV_HEADER_LEN] {
    let len_bytes = len.to_le_bytes();
    [tag, len_bytes[0], len_bytes[1], len_bytes[2], len_bytes[3]]
}

fn drain(buf: &mut [u8; RX_BUF_LEN], len: &mut usize, count: usize) {
    if count >= *len {
        *len = 0;
        return;
    }
    let remaining = *len - count;
    buf.copy_within(count..*len, 0);
    *len = remaining;
}

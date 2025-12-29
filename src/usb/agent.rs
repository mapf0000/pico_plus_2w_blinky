use core::sync::atomic::{AtomicBool, Ordering};

use agent_proto::{encode_frame, Frame, Tag, TlvDecoder, MAX_FRAME, MAX_PAYLOAD};
use embassy_futures::select::{select3, Either3};
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, channel::Channel};
use embassy_time::{Duration, Ticker};
use embassy_usb::class::cdc_acm::CdcAcmClass;
use embassy_usb::driver::Driver;
use heapless::{String, Vec};

use crate::log_buffer;

pub enum AgentCommand {
    Execute { command: Vec<u8, { MAX_PAYLOAD }> },
}

pub static AGENT_CHAN: Channel<ThreadModeRawMutex, AgentCommand, 4> = Channel::new();
pub static HOST_CONNECTED: AtomicBool = AtomicBool::new(false);

const STATUS_TICK_MS: u64 = 500;
const STATUS_INTERVAL_TICKS: u8 = 4; // 2s

pub async fn run_agent<'d, D>(class: CdcAcmClass<'d, D>) -> !
where
    D: Driver<'d>,
{
    let (mut sender, mut receiver) = class.split();
    sender.wait_connection().await;
    receiver.wait_connection().await;
    HOST_CONNECTED.store(false, Ordering::SeqCst);
    log::info!("usb: agent CDC connected");

    let mut decoder = TlvDecoder::new(true);
    let mut read_buf = [0u8; 64];
    let mut frame_buf = [0u8; MAX_FRAME];
    let mut ticker = Ticker::every(Duration::from_millis(STATUS_TICK_MS));
    let mut status_ticks = 0u8;
    let mut last_log_gen = log_buffer::generation();

    loop {
        match select3(receiver.read_packet(&mut read_buf), AGENT_CHAN.receive(), ticker.next()).await
        {
            Either3::First(read_res) => match read_res {
                Ok(n) => {
                    if n == 0 {
                        continue;
                    }
                    decoder.push_bytes(&read_buf[..n], |frame| {
                        handle_frame(frame);
                    });
                }
                Err(err) => {
                    log::warn!("usb: agent CDC read error: {:?}", err);
                    HOST_CONNECTED.store(false, Ordering::SeqCst);
                    sender.wait_connection().await;
                    receiver.wait_connection().await;
                }
            },
            Either3::Second(cmd) => {
                if let Err(err) = handle_command(cmd, &mut sender, &mut frame_buf).await {
                    log::warn!("usb: agent send error: {:?}", err);
                }
            }
            Either3::Third(_) => {
                status_ticks = status_ticks.wrapping_add(1);
                if status_ticks >= STATUS_INTERVAL_TICKS {
                    status_ticks = 0;
                    let _ = send_empty(Tag::RequestAgentStatus, &mut sender, &mut frame_buf).await;
                }
                flush_logs(&mut sender, &mut frame_buf, &mut last_log_gen).await;
            }
        }
    }
}

fn handle_frame(frame: Frame<'_>) {
    match Tag::from_u8(frame.tag) {
        Some(Tag::AgentStatus) => {
            HOST_CONNECTED.store(true, Ordering::SeqCst);
            if let Ok(name) = core::str::from_utf8(frame.payload) {
                log::info!("host: connected ({name})");
            } else {
                log::info!("host: connected (non-utf8 name)");
            }
        }
        Some(Tag::ExecuteResult) => {
            if let Ok(text) = core::str::from_utf8(frame.payload) {
                log::info!("exec: {text}");
            } else {
                log::info!("exec: <{} bytes>", frame.payload.len());
            }
        }
        Some(Tag::DebugMsg) => {
            // Device -> host only; ignore if host echoes back.
        }
        Some(Tag::RequestAgentStatus | Tag::Execute | Tag::MicPcmData) => {
            // Host should not send these; ignore.
        }
        Some(Tag::WsConnect | Tag::WsData | Tag::WsDisconnect | Tag::WsDataRecv) => {
            // VNC deferred.
        }
        None => {
            log::debug!("usb: agent unknown tag {}", frame.tag);
        }
    }
}

async fn handle_command<'d, D>(
    cmd: AgentCommand,
    sender: &mut embassy_usb::class::cdc_acm::Sender<'d, D>,
    frame_buf: &mut [u8; MAX_FRAME],
) -> Result<(), embassy_usb::driver::EndpointError>
where
    D: Driver<'d>,
{
    match cmd {
        AgentCommand::Execute { command } => send_frame(Tag::Execute, &command, sender, frame_buf).await,
    }
}

async fn flush_logs<'d, D>(
    sender: &mut embassy_usb::class::cdc_acm::Sender<'d, D>,
    frame_buf: &mut [u8; MAX_FRAME],
    last_gen: &mut u32,
) where
    D: Driver<'d>,
{
    let current = log_buffer::generation();
    if current == *last_gen {
        return;
    }
    let mut lines: Vec<String<{ log_buffer::LOG_LINE_MAX }>, { log_buffer::LOG_CAPACITY }> =
        Vec::new();
    log_buffer::snapshot(&mut lines);
    let delta = current.wrapping_sub(*last_gen) as usize;
    *last_gen = current;

    let start = lines.len().saturating_sub(delta);
    for line in lines.iter().skip(start) {
        let _ = send_frame(Tag::DebugMsg, line.as_bytes(), sender, frame_buf).await;
    }
}

async fn send_empty<'d, D>(
    tag: Tag,
    sender: &mut embassy_usb::class::cdc_acm::Sender<'d, D>,
    frame_buf: &mut [u8; MAX_FRAME],
) -> Result<(), embassy_usb::driver::EndpointError>
where
    D: Driver<'d>,
{
    send_frame(tag, &[], sender, frame_buf).await
}

async fn send_frame<'d, D>(
    tag: Tag,
    payload: &[u8],
    sender: &mut embassy_usb::class::cdc_acm::Sender<'d, D>,
    frame_buf: &mut [u8; MAX_FRAME],
) -> Result<(), embassy_usb::driver::EndpointError>
where
    D: Driver<'d>,
{
    let len = match encode_frame(tag.as_u8(), payload, frame_buf) {
        Ok(len) => len,
        Err(_) => return Ok(()),
    };
    write_all(sender, &frame_buf[..len]).await
}

async fn write_all<'d, D>(
    sender: &mut embassy_usb::class::cdc_acm::Sender<'d, D>,
    buf: &[u8],
) -> Result<(), embassy_usb::driver::EndpointError>
where
    D: Driver<'d>,
{
    let max = sender.max_packet_size() as usize;
    let mut offset = 0;
    while offset < buf.len() {
        let end = core::cmp::min(offset + max, buf.len());
        sender.write_packet(&buf[offset..end]).await?;
        offset = end;
    }
    if buf.len() % max == 0 {
        sender.write_packet(&[]).await?;
    }
    Ok(())
}

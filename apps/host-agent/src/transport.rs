use crate::config::Config;
use crate::tlv;
use anyhow::{bail, Context, Result};
use bytes::{Bytes, BytesMut};
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::time::{timeout, Instant};
use tokio_serial::{DataBits, Parity, SerialPort, SerialPortBuilderExt, SerialPortType, StopBits};
use tracing::{debug, error, info};

#[cfg(all(unix, any(test, feature = "test-port-fd")))]
use serialport::TTYPort;
#[cfg(all(unix, any(test, feature = "test-port-fd")))]
use std::os::unix::io::FromRawFd;

const BAUD_RATE: u32 = 115_200;
const READ_CHUNK: usize = 1024;
const INBOUND_QUEUE: usize = 64;
const OUTBOUND_QUEUE: usize = 64;
const PROBE_READ_CHUNK: usize = 256;

const TAG_EXECUTE: u8 = 1;
const TAG_DEBUG_MSG: u8 = 2;
const TAG_WS_CONNECT: u8 = 3;
const TAG_WS_DATA: u8 = 4;
const TAG_WS_DISCONNECT: u8 = 5;
const TAG_WS_DATA_RECV: u8 = 6;
const TAG_REQUEST_AGENT_STATUS: u8 = 7;
const TAG_AGENT_STATUS: u8 = 8;
const TAG_EXECUTE_RESULT: u8 = 9;
const TAG_MIC_PCM_DATA: u8 = 10;

const CACHE_DIR_NAME: &str = "host-agent";
const CACHE_FILE_NAME: &str = "port";

#[derive(Debug)]
pub enum Event {
    Frame(tlv::Frame),
    Raw(Bytes),
}

pub async fn select_port(config: &Config) -> Result<String> {
    if let Some(port) = &config.port {
        return Ok(port.clone());
    }

    let ports = tokio_serial::available_ports().context("list serial ports")?;
    if ports.is_empty() {
        bail!("no serial ports found");
    }

    let filtered = filter_ports(&ports, config.vid, config.pid);
    if (config.vid.is_some() || config.pid.is_some()) && filtered.is_empty() {
        let listing = list_ports(&ports);
        bail!("no serial ports match vid/pid; available ports: {listing}");
    }

    let mut candidates = if config.vid.is_some() || config.pid.is_some() {
        filtered.clone()
    } else {
        ports.clone()
    };
    candidates = prefer_callout_ports(&candidates);

    if let Some(cached) = load_cached_port() {
        if let Some(info) = candidates.iter().find(|info| info.port_name == cached) {
            if let Ok(outcome) =
                probe_port(info, Duration::from_millis(config.probe_timeout_ms)).await
            {
                if matches!(outcome, ProbeOutcome::Control | ProbeOutcome::Quiet) {
                    info!(port = %cached, "using cached serial port");
                    return Ok(cached);
                }
            }
        }
    }

    if candidates.len() == 1 {
        return Ok(candidates[0].port_name.clone());
    }

    let usb_candidates = candidates
        .iter()
        .filter(|info| matches!(info.port_type, SerialPortType::UsbPort(_)))
        .cloned()
        .collect::<Vec<_>>();

    if usb_candidates.len() == 1 {
        return Ok(usb_candidates[0].port_name.clone());
    }

    if usb_candidates.len() > 1 {
        if let Some(selected) =
            probe_control_port(&usb_candidates, config.probe_timeout_ms).await?
        {
            store_cached_port(&selected);
            return Ok(selected);
        }
    }

    if let Some(selected) = pick_port(&candidates) {
        return Ok(selected);
    }

    let listing = list_ports(&candidates);
    if config.vid.is_some() || config.pid.is_some() {
        bail!("multiple serial ports match vid/pid; use --port: {listing}");
    }
    bail!("multiple serial ports found; use --port or vid/pid: {listing}")
}

pub async fn spawn(
    port: String,
    raw: bool,
) -> Result<(mpsc::Receiver<Event>, mpsc::Sender<tlv::Frame>)> {
    let stream = open_stream(&port).with_context(|| format!("open serial port {port}"))?;
    spawn_stream(stream, raw).await
}

#[cfg(all(unix, any(test, feature = "test-port-fd")))]
pub async fn spawn_fd(
    fd: i32,
    raw: bool,
) -> Result<(mpsc::Receiver<Event>, mpsc::Sender<tlv::Frame>)> {
    let tty = unsafe { TTYPort::from_raw_fd(fd) };
    let mut stream = tokio_serial::SerialStream::try_from(tty)?;
    let _ = stream.set_baud_rate(BAUD_RATE);
    let _ = stream.set_data_bits(DataBits::Eight);
    let _ = stream.set_parity(Parity::None);
    let _ = stream.set_stop_bits(StopBits::One);
    spawn_stream(stream, raw).await
}

async fn spawn_stream(
    stream: tokio_serial::SerialStream,
    raw: bool,
) -> Result<(mpsc::Receiver<Event>, mpsc::Sender<tlv::Frame>)> {
    let stream = stream;

    let (reader, writer) = tokio::io::split(stream);
    let (in_tx, in_rx) = mpsc::channel(INBOUND_QUEUE);
    let (out_tx, out_rx) = mpsc::channel(OUTBOUND_QUEUE);

    tokio::spawn(async move {
        if let Err(err) = read_loop(reader, in_tx, raw).await {
            error!(error = %err, "serial read loop failed");
        }
    });

    tokio::spawn(async move {
        if let Err(err) = write_loop(writer, out_rx).await {
            error!(error = %err, "serial write loop failed");
        }
    });

    Ok((in_rx, out_tx))
}

async fn read_loop(
    mut reader: tokio::io::ReadHalf<tokio_serial::SerialStream>,
    inbound: mpsc::Sender<Event>,
    raw: bool,
) -> Result<()> {
    let mut buffer = BytesMut::with_capacity(4096);
    let mut temp = [0u8; READ_CHUNK];
    let inbound = inbound;

    loop {
        let count = reader.read(&mut temp).await?;
        if count == 0 {
            info!("serial port closed");
            return Ok(());
        }

        if raw {
            let raw_bytes = Bytes::copy_from_slice(&temp[..count]);
            if inbound.send(Event::Raw(raw_bytes)).await.is_err() {
                return Ok(());
            }
        }

        buffer.extend_from_slice(&temp[..count]);
        while let Some(frame) = tlv::decode_next(&mut buffer) {
            if inbound.send(Event::Frame(frame)).await.is_err() {
                return Ok(());
            }
        }
    }
}

async fn write_loop(
    mut writer: tokio::io::WriteHalf<tokio_serial::SerialStream>,
    mut outbound: mpsc::Receiver<tlv::Frame>,
) -> Result<()> {
    let mut buffer = BytesMut::with_capacity(4096);
    while let Some(frame) = outbound.recv().await {
        buffer.clear();
        tlv::write_frame(&frame, &mut buffer)?;
        writer.write_all(&buffer).await?;
        debug!(tag = frame.tag, len = frame.payload.len(), "sent frame");
    }
    Ok(())
}

fn filter_ports(
    ports: &[tokio_serial::SerialPortInfo],
    vid: Option<u16>,
    pid: Option<u16>,
) -> Vec<tokio_serial::SerialPortInfo> {
    if vid.is_none() && pid.is_none() {
        return ports.to_vec();
    }

    ports
        .iter()
        .filter(|info| match &info.port_type {
            SerialPortType::UsbPort(usb) => {
                if let Some(vid) = vid {
                    if usb.vid != vid {
                        return false;
                    }
                }
                if let Some(pid) = pid {
                    if usb.pid != pid {
                        return false;
                    }
                }
                true
            }
            _ => false,
        })
        .cloned()
        .collect()
}

fn pick_port(ports: &[tokio_serial::SerialPortInfo]) -> Option<String> {
    if ports.len() == 1 {
        return Some(ports[0].port_name.clone());
    }

    let cu_ports: Vec<_> = ports
        .iter()
        .filter(|info| info.port_name.starts_with("/dev/cu."))
        .collect();

    if cu_ports.len() == 1 {
        return Some(cu_ports[0].port_name.clone());
    }

    None
}

fn list_ports(ports: &[tokio_serial::SerialPortInfo]) -> String {
    let mut names = ports
        .iter()
        .map(|info| match &info.port_type {
            SerialPortType::UsbPort(usb) => format!(
                "{} (vid={:04x} pid={:04x})",
                info.port_name, usb.vid, usb.pid
            ),
            _ => info.port_name.clone(),
        })
        .collect::<Vec<_>>();
    names.sort();
    names.join(", ")
}

fn prefer_callout_ports(ports: &[tokio_serial::SerialPortInfo]) -> Vec<tokio_serial::SerialPortInfo> {
    #[cfg(target_os = "macos")]
    {
        let cu_ports: Vec<_> = ports
            .iter()
            .filter(|info| info.port_name.starts_with("/dev/cu."))
            .cloned()
            .collect();
        if !cu_ports.is_empty() {
            return cu_ports;
        }
    }
    ports.to_vec()
}

fn is_known_tag(tag: u8) -> bool {
    matches!(
        tag,
        TAG_EXECUTE
            | TAG_DEBUG_MSG
            | TAG_WS_CONNECT
            | TAG_WS_DATA
            | TAG_WS_DISCONNECT
            | TAG_WS_DATA_RECV
            | TAG_REQUEST_AGENT_STATUS
            | TAG_AGENT_STATUS
            | TAG_EXECUTE_RESULT
            | TAG_MIC_PCM_DATA
    )
}

fn is_control_response(tag: u8) -> bool {
    tag == TAG_AGENT_STATUS
}

fn open_stream(port: &str) -> Result<tokio_serial::SerialStream> {
    let builder = tokio_serial::new(port, BAUD_RATE)
        .data_bits(DataBits::Eight)
        .parity(Parity::None)
        .stop_bits(StopBits::One)
        .timeout(Duration::from_secs(1));
    let mut stream = builder.open_native_async()?;
    let _ = stream.write_data_terminal_ready(false);
    let _ = stream.write_request_to_send(false);
    Ok(stream)
}

#[derive(Debug, Clone, Copy)]
enum ProbeOutcome {
    Control,
    Quiet,
    Noisy,
}

async fn probe_control_port(
    ports: &[tokio_serial::SerialPortInfo],
    timeout_ms: u64,
) -> Result<Option<String>> {
    let timeout = Duration::from_millis(timeout_ms);
    let mut quiet = Vec::new();

    for info in ports {
        match probe_port(info, timeout).await {
            Ok(ProbeOutcome::Control) => {
                info!(port = %info.port_name, "probe matched control port");
                return Ok(Some(info.port_name.clone()));
            }
            Ok(ProbeOutcome::Quiet) => {
                quiet.push(info.port_name.clone());
            }
            Ok(ProbeOutcome::Noisy) => {
                debug!(port = %info.port_name, "probe saw noise (logger?)");
            }
            Err(err) => {
                debug!(port = %info.port_name, error = %err, "probe failed");
            }
        }
    }

    if quiet.len() == 1 {
        info!(port = %quiet[0], "probe picked quiet port");
        return Ok(Some(quiet[0].clone()));
    }

    Ok(None)
}

async fn probe_port(
    info: &tokio_serial::SerialPortInfo,
    probe_timeout: Duration,
) -> Result<ProbeOutcome> {
    let mut stream = open_stream(&info.port_name)?;
    let mut buf = [0u8; PROBE_READ_CHUNK];
    let mut acc = BytesMut::with_capacity(512);
    let mut saw_data = false;
    let deadline = Instant::now() + probe_timeout;

    send_status_request(&mut stream).await?;

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match timeout(remaining, stream.read(&mut buf)).await {
            Ok(Ok(count)) => {
                if count == 0 {
                    break;
                }
                saw_data = true;
                acc.extend_from_slice(&buf[..count]);
                while let Some(frame) = tlv::decode_next(&mut acc) {
                    if is_control_response(frame.tag) {
                        return Ok(ProbeOutcome::Control);
                    }
                    if is_known_tag(frame.tag) {
                        return Ok(ProbeOutcome::Control);
                    }
                }
            }
            Ok(Err(err)) => return Err(err.into()),
            Err(_) => break,
        }
    }

    if saw_data {
        Ok(ProbeOutcome::Noisy)
    } else {
        Ok(ProbeOutcome::Quiet)
    }
}

async fn send_status_request(stream: &mut tokio_serial::SerialStream) -> Result<()> {
    let header = [TAG_REQUEST_AGENT_STATUS, 0, 0, 0, 0];
    let _ = stream.write_all(&header).await;
    Ok(())
}

fn cache_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let mut path = PathBuf::from(home);
    #[cfg(target_os = "macos")]
    {
        path.push("Library");
        path.push("Caches");
    }
    #[cfg(not(target_os = "macos"))]
    {
        path.push(".cache");
    }
    path.push(CACHE_DIR_NAME);
    path.push(CACHE_FILE_NAME);
    Some(path)
}

fn load_cached_port() -> Option<String> {
    let path = cache_path()?;
    let data = std::fs::read_to_string(path).ok()?;
    let port = data.trim();
    if port.is_empty() {
        None
    } else {
        Some(port.to_string())
    }
}

fn store_cached_port(port: &str) {
    let Some(path) = cache_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    let _ = std::fs::write(path, port.as_bytes());
}

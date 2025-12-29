use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent_proto::{encode_frame, Frame, Tag, TlvDecoder, MAX_FRAME, MAX_PAYLOAD};
use clap::Parser;
use gethostname::gethostname;
use serialport::{SerialPortInfo, SerialPortType, UsbPortInfo};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, Mutex};
use tokio::time::sleep;
use tokio_serial::{DataBits, Parity, SerialPortBuilderExt, StopBits};
use tracing::{debug, info, warn};

const BAUD_RATE: u32 = 115_200;
const OUTBOUND_QUEUE: usize = 32;
const EXECUTE_RESULT_MAX: usize = 8 * 1024;
const EXECUTE_CHUNK: usize = 2048;
const EXECUTE_PACE_MS: u64 = 10;
const RAW_PREVIEW_MAX: usize = 128;

#[derive(Parser, Debug)]
#[command(author, version, about = "Host agent daemon for USB CDC TLV protocol")]
struct Args {
    /// Serial port path, e.g. /dev/cu.usbmodem*
    #[arg(long)]
    port: Option<String>,
    /// USB VID (hex or decimal), also accepted as vid=....
    #[arg(long)]
    vid: Option<String>,
    /// USB PID (hex or decimal), also accepted as pid=....
    #[arg(long)]
    pid: Option<String>,
    /// Working directory for Execute
    #[arg(long)]
    cwd: Option<PathBuf>,
    /// Dump raw bytes as hex
    #[arg(long)]
    raw: bool,
    /// Enable verbose debug logging
    #[arg(long)]
    debug: bool,
}

#[derive(Clone, Debug)]
struct Config {
    port: Option<String>,
    vid: Option<u16>,
    pid: Option<u16>,
    cwd: Option<PathBuf>,
    raw: bool,
    debug: bool,
}

#[derive(Clone)]
struct SessionCtx {
    tx_out: mpsc::Sender<Vec<u8>>,
    exec_lock: Arc<Mutex<()>>,
    mic_writer: Arc<Mutex<MicWriter>>,
    host_name: Arc<Vec<u8>>,
    cwd: Option<PathBuf>,
    raw: bool,
    pace: Duration,
}

#[derive(thiserror::Error, Debug)]
enum AgentError {
    #[error("missing serial port; provide --port or vid/pid")]
    MissingPort,
    #[error("invalid vid/pid: {0}")]
    BadId(String),
    #[error("serial port error: {0}")]
    Serial(#[from] serialport::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("encode error: {0:?}")]
    Encode(agent_proto::EncodeError),
}

struct MicWriter {
    path: PathBuf,
    file: Option<tokio::fs::File>,
}

impl From<agent_proto::EncodeError> for AgentError {
    fn from(err: agent_proto::EncodeError) -> Self {
        AgentError::Encode(err)
    }
}

impl MicWriter {
    fn new(path: PathBuf) -> Self {
        Self { path, file: None }
    }

    async fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        if self.file.is_none() {
            let file = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
                .await?;
            self.file = Some(file);
        }
        if let Some(file) = self.file.as_mut() {
            file.write_all(data).await?;
        }
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), AgentError> {
    let args = Args::parse_from(normalize_args(std::env::args()));
    let config = parse_config(args)?;
    init_tracing(config.debug);

    run(config).await
}

fn normalize_args<I>(args: I) -> Vec<String>
where
    I: IntoIterator<Item = String>,
{
    let mut out = Vec::new();
    let mut iter = args.into_iter();
    if let Some(program) = iter.next() {
        out.push(program);
    }
    for arg in iter {
        if let Some((k, v)) = arg.split_once('=') {
            if matches!(k, "vid" | "pid" | "cwd") && !arg.starts_with("--") {
                out.push(format!("--{k}={v}"));
                continue;
            }
        }
        out.push(arg);
    }
    out
}

fn parse_config(args: Args) -> Result<Config, AgentError> {
    let vid = match args.vid {
        Some(v) => Some(parse_u16(&v).map_err(|e| AgentError::BadId(e))?),
        None => None,
    };
    let pid = match args.pid {
        Some(v) => Some(parse_u16(&v).map_err(|e| AgentError::BadId(e))?),
        None => None,
    };
    Ok(Config {
        port: args.port,
        vid,
        pid,
        cwd: args.cwd,
        raw: args.raw,
        debug: args.debug,
    })
}

fn parse_u16(input: &str) -> Result<u16, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("empty value".to_string());
    }
    let (radix, digits) = if let Some(hex) = trimmed.strip_prefix("0x") {
        (16, hex)
    } else if trimmed
        .chars()
        .any(|c| matches!(c, 'a'..='f' | 'A'..='F'))
    {
        (16, trimmed)
    } else {
        (10, trimmed)
    };
    u16::from_str_radix(digits, radix).map_err(|e| format!("{input}: {e}"))
}

fn init_tracing(debug: bool) {
    let mut filter = tracing_subscriber::EnvFilter::from_default_env();
    if debug {
        filter = filter.add_directive("agentd=debug".parse().unwrap());
    } else {
        filter = filter.add_directive("agentd=info".parse().unwrap());
    }
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

async fn run(config: Config) -> Result<(), AgentError> {
    let host_name = gethostname().to_string_lossy().into_owned();
    let mut host_name_bytes = host_name.into_bytes();
    if host_name_bytes.len() > MAX_PAYLOAD {
        host_name_bytes.truncate(MAX_PAYLOAD);
    }

    let exec_lock = Arc::new(Mutex::new(()));
    let mic_writer = Arc::new(Mutex::new(MicWriter::new(PathBuf::from("mic.pcm"))));
    let pace = Duration::from_millis(EXECUTE_PACE_MS);

    let mut backoff = Duration::from_millis(200);
    loop {
        let port_name = match select_port(&config)? {
            Some(port) => port,
            None => {
                info!("waiting for device (vid/pid match)...");
                sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(5));
                continue;
            }
        };

        info!("connecting to {}", port_name);
        match open_serial(&port_name) {
            Ok(port) => {
                backoff = Duration::from_millis(200);
                let (tx_out, rx_out) = mpsc::channel(OUTBOUND_QUEUE);
                let ctx = Arc::new(SessionCtx {
                    tx_out,
                    exec_lock: exec_lock.clone(),
                    mic_writer: mic_writer.clone(),
                    host_name: Arc::new(host_name_bytes.clone()),
                    cwd: config.cwd.clone(),
                    raw: config.raw,
                    pace,
                });
                if let Err(err) = run_session(port, rx_out, ctx).await {
                    warn!("session ended: {err}");
                }
            }
            Err(err) => {
                warn!("open failed: {err}");
            }
        }

        sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(5));
    }
}

fn select_port(config: &Config) -> Result<Option<String>, AgentError> {
    if let Some(port) = &config.port {
        return Ok(Some(port.clone()));
    }
    let vid = config.vid.ok_or(AgentError::MissingPort)?;
    let pid = config.pid.ok_or(AgentError::MissingPort)?;

    let ports = serialport::available_ports()?;
    let mut candidates: Vec<SerialPortInfo> = ports
        .into_iter()
        .filter(|info| match &info.port_type {
            SerialPortType::UsbPort(usb) => usb.vid == vid && usb.pid == pid,
            _ => false,
        })
        .collect();
    if candidates.is_empty() {
        return Ok(None);
    }
    candidates.sort_by_key(score_port);
    Ok(candidates.last().map(|p| p.port_name.clone()))
}

fn score_port(info: &SerialPortInfo) -> u8 {
    let mut score = 0u8;
    if info.port_name.contains("cu.") {
        score = score.saturating_add(2);
    }
    if let SerialPortType::UsbPort(UsbPortInfo {
        interface,
        manufacturer,
        product,
        serial_number,
        ..
    }) = &info.port_type
    {
        if let Some(iface) = interface {
            if *iface >= 2 {
                score = score.saturating_add(4);
            }
            score = score.saturating_add(*iface);
        }
        if product
            .as_ref()
            .map(|p| p.to_ascii_lowercase().contains("agent"))
            .unwrap_or(false)
        {
            score = score.saturating_add(4);
        }
        if manufacturer
            .as_ref()
            .map(|p| p.to_ascii_lowercase().contains("agent"))
            .unwrap_or(false)
        {
            score = score.saturating_add(2);
        }
        if serial_number.is_some() {
            score = score.saturating_add(1);
        }
    }
    score
}

fn open_serial(port: &str) -> Result<tokio_serial::SerialStream, serialport::Error> {
    tokio_serial::new(port, BAUD_RATE)
        .data_bits(DataBits::Eight)
        .parity(Parity::None)
        .stop_bits(StopBits::One)
        .open_native_async()
}

async fn run_session(
    port: tokio_serial::SerialStream,
    mut rx_out: mpsc::Receiver<Vec<u8>>,
    ctx: Arc<SessionCtx>,
) -> Result<(), AgentError> {
    let (mut reader, mut writer) = tokio::io::split(port);

    let write_task = tokio::spawn(async move {
        while let Some(frame) = rx_out.recv().await {
            if let Err(err) = writer.write_all(&frame).await {
                return Err(err);
            }
        }
        Ok::<(), std::io::Error>(())
    });

    queue_frame(&ctx.tx_out, Tag::AgentStatus.as_u8(), &ctx.host_name).await?;

    let mut decoder = TlvDecoder::new(true);
    let mut buf = [0u8; 1024];
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        if ctx.raw {
            info!("raw rx: {}", hex_preview(&buf[..n]));
        }
        let ctx_clone = ctx.clone();
        decoder.push_bytes(&buf[..n], |frame| {
            handle_frame(frame, &ctx_clone);
        });
    }

    write_task.abort();
    let _ = write_task.await;
    Ok(())
}

fn handle_frame(frame: Frame<'_>, ctx: &Arc<SessionCtx>) {
    match Tag::from_u8(frame.tag) {
        Some(Tag::RequestAgentStatus) => {
            let ctx = ctx.clone();
            tokio::spawn(async move {
                if let Err(err) =
                    queue_frame(&ctx.tx_out, Tag::AgentStatus.as_u8(), &ctx.host_name).await
                {
                    warn!("status send failed: {err}");
                }
            });
        }
        Some(Tag::Execute) => {
            let payload = frame.payload.to_vec();
            let ctx = ctx.clone();
            tokio::spawn(async move {
                handle_execute(payload, ctx).await;
            });
        }
        Some(Tag::DebugMsg) => {
            if let Ok(text) = std::str::from_utf8(frame.payload) {
                info!("device: {text}");
            } else {
                info!("device: <non-utf8 {} bytes>", frame.payload.len());
            }
        }
        Some(Tag::MicPcmData) => {
            let payload = frame.payload.to_vec();
            let ctx = ctx.clone();
            tokio::spawn(async move {
                let mut writer = ctx.mic_writer.lock().await;
                if let Err(err) = writer.write(&payload).await {
                    warn!("mic.pcm write failed: {err}");
                }
            });
        }
        Some(Tag::ExecuteResult) => {
            if let Ok(text) = std::str::from_utf8(frame.payload) {
                info!("exec: {text}");
            } else {
                info!("exec: <{} bytes>", frame.payload.len());
            }
        }
        Some(Tag::WsConnect | Tag::WsData | Tag::WsDisconnect | Tag::WsDataRecv) => {
            debug!("vnc tag {} ignored (not implemented)", frame.tag);
        }
        Some(Tag::AgentStatus) => {
            debug!("unexpected AgentStatus from device");
        }
        None => {
            debug!("unknown tag {}", frame.tag);
        }
    }
}

async fn handle_execute(payload: Vec<u8>, ctx: Arc<SessionCtx>) {
    let Ok(command) = std::str::from_utf8(&payload) else {
        warn!("execute payload is not utf-8");
        return;
    };
    let _guard = ctx.exec_lock.lock().await;
    info!("execute: {command}");

    let mut cmd = tokio::process::Command::new("/bin/sh");
    cmd.arg("-lc").arg(command);
    if let Some(cwd) = &ctx.cwd {
        cmd.current_dir(cwd);
    }
    let output = match cmd.output().await {
        Ok(output) => output,
        Err(err) => {
            warn!("execute failed: {err}");
            return;
        }
    };

    let mut data = output.stdout;
    data.extend_from_slice(&output.stderr);
    if data.len() > EXECUTE_RESULT_MAX {
        data.truncate(EXECUTE_RESULT_MAX);
    }

    if let Err(err) = send_execute_result(data, &ctx).await {
        warn!("execute result send failed: {err}");
    }
}

async fn send_execute_result(data: Vec<u8>, ctx: &SessionCtx) -> Result<(), AgentError> {
    if data.is_empty() {
        return Ok(());
    }
    for chunk in data.chunks(EXECUTE_CHUNK) {
        queue_frame(&ctx.tx_out, Tag::ExecuteResult.as_u8(), chunk).await?;
        if !ctx.pace.is_zero() {
            sleep(ctx.pace).await;
        }
    }
    Ok(())
}

async fn queue_frame(
    tx: &mpsc::Sender<Vec<u8>>,
    tag: u8,
    payload: &[u8],
) -> Result<(), AgentError> {
    let mut buf = [0u8; MAX_FRAME];
    let len = encode_frame(tag, payload, &mut buf)?;
    tx.send(buf[..len].to_vec()).await.map_err(|e| {
        AgentError::Io(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            format!("send failed: {e}"),
        ))
    })
}

fn hex_preview(data: &[u8]) -> String {
    let mut out = String::new();
    let mut first = true;
    for byte in data.iter().take(RAW_PREVIEW_MAX) {
        if !first {
            out.push(' ');
        }
        first = false;
        let _ = std::fmt::Write::write_fmt(&mut out, format_args!("{byte:02X}"));
    }
    if data.len() > RAW_PREVIEW_MAX {
        out.push_str(" ...");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_u16_accepts_decimal() {
        assert_eq!(parse_u16("1234").unwrap(), 1234);
    }

    #[test]
    fn parse_u16_accepts_hex_prefix() {
        assert_eq!(parse_u16("0x10").unwrap(), 16);
    }

    #[test]
    fn parse_u16_accepts_hex_letters() {
        assert_eq!(parse_u16("cafe").unwrap(), 0xCAFE);
        assert_eq!(parse_u16("BEEF").unwrap(), 0xBEEF);
    }

    #[test]
    fn normalize_args_rewrites_key_value_pairs() {
        let args = vec![
            "agentd".to_string(),
            "vid=cafe".to_string(),
            "pid=403f".to_string(),
            "cwd=/tmp".to_string(),
            "--debug".to_string(),
        ];
        let out = normalize_args(args);
        assert_eq!(out[0], "agentd");
        assert!(out.contains(&"--vid=cafe".to_string()));
        assert!(out.contains(&"--pid=403f".to_string()));
        assert!(out.contains(&"--cwd=/tmp".to_string()));
        assert!(out.contains(&"--debug".to_string()));
    }
}

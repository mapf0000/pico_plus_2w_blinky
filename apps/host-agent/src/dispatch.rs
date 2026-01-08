use crate::config::Config;
use crate::tlv::{self, Frame};
use crate::transport::Event;
use anyhow::Result;
use bytes::Bytes;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::Path;
use tokio::sync::mpsc;
use tokio::process::Command;
use tokio::time::{sleep, Duration};
use tracing::{debug, info};

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
const MAX_EXEC_OUTPUT: usize = 8 * 1024;
const EXEC_CHUNK_DELAY_MS: u64 = 20;

pub async fn run(
    mut inbound: mpsc::Receiver<Event>,
    outbound: mpsc::Sender<Frame>,
    config: &Config,
) -> Result<()> {
    let mut outbound = outbound;
    let mut debug_sink = DebugSink::new(config.debug_log.as_deref())?;

    while let Some(event) = inbound.recv().await {
        match event {
            Event::Raw(bytes) => {
                if config.raw {
                    info!(len = bytes.len(), raw = %format_hex(&bytes), "raw data");
                }
            }
            Event::Frame(frame) => {
                handle_frame(frame, &mut outbound, &mut debug_sink).await?;
            }
        }
    }

    Ok(())
}

async fn handle_frame(
    frame: Frame,
    outbound: &mut mpsc::Sender<Frame>,
    debug_sink: &mut DebugSink,
) -> Result<()> {
    match frame.tag {
        TAG_REQUEST_AGENT_STATUS => {
            let hostname = hostname::get().unwrap_or_else(|_| "unknown".into());
            let payload = Bytes::from(hostname.to_string_lossy().into_owned());
            let response = Frame::new(TAG_AGENT_STATUS, payload);
            outbound.send(response).await?;
            info!("sent agent status");
        }
        TAG_DEBUG_MSG => {
            let message = String::from_utf8_lossy(&frame.payload);
            info!(message = %message, "device debug");
            debug_sink.write_line(&message)?;
        }
        TAG_EXECUTE => {
            handle_execute(frame, outbound).await?;
        }
        TAG_WS_CONNECT
        | TAG_WS_DATA
        | TAG_WS_DISCONNECT
        | TAG_WS_DATA_RECV
        | TAG_EXECUTE_RESULT
        | TAG_MIC_PCM_DATA => {
            debug!(tag = frame.tag, len = frame.payload.len(), "unhandled frame");
        }
        _ => {
            debug!(tag = frame.tag, len = frame.payload.len(), "unknown frame");
        }
    }

    Ok(())
}

async fn handle_execute(frame: Frame, outbound: &mut mpsc::Sender<Frame>) -> Result<()> {
    let command = String::from_utf8_lossy(&frame.payload);
    info!(command = %command, "execute request");

    let output = match run_command(&command).await {
        Ok(output) => output,
        Err(err) => format!("execute error: {err}").into_bytes(),
    };

    send_execute_result(outbound, output).await?;
    Ok(())
}

async fn send_execute_result(outbound: &mut mpsc::Sender<Frame>, mut output: Vec<u8>) -> Result<()> {
    if output.len() > MAX_EXEC_OUTPUT {
        output.truncate(MAX_EXEC_OUTPUT);
        info!(len = output.len(), "execute output truncated");
    }

    if output.is_empty() {
        outbound
            .send(Frame::new(TAG_EXECUTE_RESULT, Bytes::new()))
            .await?;
        return Ok(());
    }

    let mut iter = output.chunks(tlv::MAX_PAYLOAD_LEN).peekable();
    while let Some(chunk) = iter.next() {
        outbound
            .send(Frame::new(TAG_EXECUTE_RESULT, Bytes::copy_from_slice(chunk)))
            .await?;
        if iter.peek().is_some() {
            sleep(Duration::from_millis(EXEC_CHUNK_DELAY_MS)).await;
        }
    }

    Ok(())
}

async fn run_command(command: &str) -> Result<Vec<u8>> {
    let mut cmd = build_command(command);
    let output = cmd.output().await?;
    Ok(combine_output(output))
}

#[cfg(target_os = "windows")]
fn build_command(command: &str) -> Command {
    let mut cmd = Command::new("cmd");
    cmd.arg("/C").arg(command);
    cmd
}

#[cfg(not(target_os = "windows"))]
fn build_command(command: &str) -> Command {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(command);
    cmd
}

fn combine_output(output: std::process::Output) -> Vec<u8> {
    let mut data = output.stdout;
    if !output.stderr.is_empty() {
        if !data.is_empty() && !data.ends_with(b"\n") {
            data.push(b'\n');
        }
        data.extend_from_slice(&output.stderr);
    }
    data
}

struct DebugSink {
    file: Option<std::fs::File>,
}

impl DebugSink {
    fn new(path: Option<&Path>) -> Result<Self> {
        let file = match path {
            Some(path) => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                Some(
                    OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)?,
                )
            }
            None => None,
        };
        Ok(Self { file })
    }

    fn write_line(&mut self, message: &str) -> Result<()> {
        if let Some(file) = self.file.as_mut() {
            if message.ends_with('\n') {
                file.write_all(message.as_bytes())?;
            } else {
                file.write_all(message.as_bytes())?;
                file.write_all(b"\n")?;
            }
        }
        Ok(())
    }
}

fn format_hex(bytes: &Bytes) -> String {
    bytes
        .iter()
        .map(|byte| format!("{:02x}", byte))
        .collect::<Vec<_>>()
        .join(" ")
}

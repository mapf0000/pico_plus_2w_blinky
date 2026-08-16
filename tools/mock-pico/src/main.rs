#[cfg(unix)]
mod pty;
#[cfg(unix)]
mod relay;
#[cfg(unix)]
mod websocket;

#[cfg(not(unix))]
fn main() {
    eprintln!("mock-pico currently requires a Unix host for its pseudo-terminal");
    std::process::exit(1);
}

#[cfg(unix)]
mod unix {
    use super::relay::{
        Relay, RelayEvent, TAG_FILE_ABORT, TAG_FILE_ACK, TAG_FILE_RESULT, encode_unavailable_abort,
    };
    use super::websocket::{self, Inbound, Outbound};
    use anyhow::{Context, Result, bail, ensure};
    use clap::Parser;
    use serde_json::{Value, json};
    use std::os::fd::AsRawFd;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::process::{Child, Command};
    use tokio::sync::mpsc;
    use tokio_serial::SerialStream;

    const TAG_DEBUG_MSG: u8 = 2;
    const TAG_REQUEST_AGENT_STATUS: u8 = 7;
    const TAG_AGENT_STATUS: u8 = 8;
    const TAG_HOST_OS: u8 = 34;
    const TAG_FS_LIST_REQUEST: u8 = 29;
    const TAG_FS_LIST_PAGE: u8 = 30;
    const TAG_FS_LIST_CANCEL: u8 = 31;
    const WS_BINARY_KIND_FILESYSTEM: u8 = 2;
    const TLV_HEADER_LEN: usize = 5;
    const AGENT_STATUS_MAGIC: &[u8] = b"PICOAGENT\0";
    const WEBSOCKET_PROTOCOL_VERSION: u16 = 2;

    #[derive(Parser, Debug)]
    #[command(
        author,
        version,
        about = "Local Pico firmware simulator for frontend and host-agent development"
    )]
    struct Args {
        #[arg(long, default_value = "127.0.0.1:9001")]
        listen: String,

        #[arg(long, value_name = "PATH")]
        host_agent: Option<PathBuf>,

        #[arg(long, value_name = "PATH")]
        agent_cwd: Option<PathBuf>,

        #[arg(long, conflicts_with = "host_agent")]
        no_host_agent: bool,

        #[arg(long)]
        quiet_agent: bool,
    }

    #[derive(Clone, Debug)]
    struct TlvFrame {
        tag: u8,
        payload: Vec<u8>,
    }

    #[derive(Clone, Debug, Default)]
    struct AgentInfo {
        present: bool,
        version: Option<String>,
        hostname: Option<String>,
        host_os: Option<String>,
    }

    #[derive(Debug)]
    struct DeviceState {
        agent: AgentInfo,
        usb_enabled: bool,
        usb_ready: bool,
        manufacturer: String,
        product: String,
        websocket: Option<(u64, mpsc::Sender<Outbound>)>,
    }

    impl Default for DeviceState {
        fn default() -> Self {
            Self {
                agent: AgentInfo::default(),
                usb_enabled: false,
                usb_ready: false,
                manufacturer: "Pico Endpoint Mock".to_string(),
                product: "Pico Plus 2 W (simulated)".to_string(),
                websocket: None,
            }
        }
    }

    #[derive(Clone)]
    struct Shared {
        state: Arc<Mutex<DeviceState>>,
        relay: Arc<Mutex<Relay>>,
        serial: mpsc::Sender<TlvFrame>,
        next_websocket: Arc<AtomicU64>,
    }

    impl Shared {
        fn new(serial: mpsc::Sender<TlvFrame>) -> Self {
            Self {
                state: Arc::new(Mutex::new(DeviceState::default())),
                relay: Arc::new(Mutex::new(Relay::default())),
                serial,
                next_websocket: Arc::new(AtomicU64::new(1)),
            }
        }

        fn hello(&self) -> Value {
            let state = self.state.lock().expect("device state lock");
            let privileged = if state.agent.present && state.agent.version.is_some() {
                vec!["host_execute", "db_credentials", "host_filesystem"]
            } else {
                Vec::new()
            };
            json!({
                "event_type": "hello",
                "version": 1,
                "firmware": {
                    "version": env!("CARGO_PKG_VERSION"),
                    "build": "mock-pico-dev",
                },
                "protocols": {
                    "websocket": WEBSOCKET_PROTOCOL_VERSION,
                    "transfer": transfer_protocol::TRANSFER_PROTOCOL_VERSION,
                    "filesystem": 1,
                },
                "host_agent": {
                    "present": state.agent.present,
                    "version": state.agent.version,
                    "hostname": state.agent.hostname,
                },
                "keyboard": {
                    "layouts": ["mac_de-DE"],
                    "features": ["hid_keyboard", "script_bytecode", "macos_assistant"],
                },
                "features": [
                    "usb_identity",
                    "usb_control",
                    "file_transfer",
                    "filesystem_browser",
                    "transfer_download",
                ],
                "privileged_operations": privileged,
            })
        }

        async fn register_websocket(&self, sender: mpsc::Sender<Outbound>) -> u64 {
            let generation = self.next_websocket.fetch_add(1, Ordering::Relaxed);
            let previous = {
                let mut state = self.state.lock().expect("device state lock");
                state.websocket.replace((generation, sender))
            };
            if let Some((_, previous)) = previous {
                let _ = previous
                    .send(Outbound::Close {
                        code: transfer_protocol::WEBSOCKET_CLOSE_SESSION_REPLACED,
                        reason: "newer browser session connected".to_string(),
                    })
                    .await;
            }
            generation
        }

        fn clear_websocket(&self, generation: u64) {
            let mut state = self.state.lock().expect("device state lock");
            if state
                .websocket
                .as_ref()
                .is_some_and(|(current, _)| *current == generation)
            {
                state.websocket = None;
            }
        }

        fn websocket_is_current(&self, generation: u64) -> bool {
            self.state
                .lock()
                .expect("device state lock")
                .websocket
                .as_ref()
                .is_some_and(|(current, _)| *current == generation)
        }

        fn websocket_sender(&self) -> Option<mpsc::Sender<Outbound>> {
            self.state
                .lock()
                .expect("device state lock")
                .websocket
                .as_ref()
                .map(|(_, sender)| sender.clone())
        }

        async fn send_websocket(&self, message: Outbound) -> bool {
            let Some(sender) = self.websocket_sender() else {
                return false;
            };
            sender.send(message).await.is_ok()
        }

        async fn publish_hello(&self) {
            let _ = self
                .send_websocket(Outbound::Text(self.hello().to_string()))
                .await;
        }

        fn agent_present(&self) -> bool {
            self.state.lock().expect("device state lock").agent.present
        }

        fn clear_agent(&self) {
            self.state.lock().expect("device state lock").agent = AgentInfo::default();
        }

        async fn request_agent_status(&self) -> Result<()> {
            self.serial
                .send(TlvFrame {
                    tag: TAG_REQUEST_AGENT_STATUS,
                    payload: Vec::new(),
                })
                .await
                .context("request host-agent status")
        }
    }

    pub async fn run() -> Result<()> {
        let args = Args::parse();
        let pty = super::pty::open()?;
        let slave_fd = pty.slave.as_raw_fd();
        let mut host_agent = if args.no_host_agent {
            None
        } else {
            let executable = args.host_agent.unwrap_or_else(default_host_agent_path);
            Some(spawn_host_agent(
                &executable,
                slave_fd,
                args.agent_cwd.as_deref(),
                args.quiet_agent,
            )?)
        };

        println!("mock-pico PTY: {}", pty.slave_path);
        if host_agent.is_some() {
            drop(pty.slave);
        }

        let (serial_reader, serial_writer) = tokio::io::split(pty.master);
        let (serial_tx, serial_rx) = mpsc::channel(64);
        let shared = Shared::new(serial_tx.clone());
        tokio::spawn(serial_write_loop(serial_writer, serial_rx));
        tokio::spawn(serial_read_loop(serial_reader, shared.clone()));

        if host_agent.is_some() {
            serial_tx
                .send(TlvFrame {
                    tag: TAG_REQUEST_AGENT_STATUS,
                    payload: b"handshake".to_vec(),
                })
                .await?;
        }

        let listener = TcpListener::bind(&args.listen)
            .await
            .with_context(|| format!("bind mock WebSocket server at {}", args.listen))?;
        println!("mock-pico WebSocket: ws://{}/ws", args.listen);
        println!("start the UI with: cd apps/frontend && trunk serve --config Trunk.mock.toml");

        let server = accept_loop(listener, shared);
        tokio::select! {
            result = server => result?,
            result = tokio::signal::ctrl_c() => result.context("wait for Ctrl-C")?,
        }

        if let Some(child) = host_agent.as_mut() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        Ok(())
    }

    fn default_host_agent_path() -> PathBuf {
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| workspace_root().join("target"));
        target.join("debug").join("host-agent")
    }

    fn workspace_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("mock-pico is nested under tools/")
            .to_path_buf()
    }

    fn spawn_host_agent(
        executable: &Path,
        slave_fd: i32,
        cwd: Option<&Path>,
        quiet: bool,
    ) -> Result<Child> {
        ensure!(
            executable.is_file(),
            "host-agent binary not found at {}; run `cargo build -p host-agent --features test-port-fd`",
            executable.display()
        );
        let mut command = Command::new(executable);
        command
            .arg("--port-fd")
            .arg(slave_fd.to_string())
            .kill_on_drop(true);
        if let Some(cwd) = cwd {
            command.arg("--cwd").arg(cwd);
        }
        if !quiet {
            command.arg("--debug");
        }
        command
            .spawn()
            .with_context(|| format!("launch host agent from {}", executable.display()))
    }

    async fn accept_loop(listener: TcpListener, shared: Shared) -> Result<()> {
        loop {
            let (stream, peer) = listener.accept().await.context("accept WebSocket client")?;
            let shared = shared.clone();
            tokio::spawn(async move {
                if let Err(error) = handle_websocket(stream, shared).await {
                    eprintln!("mock-pico WebSocket {peer} closed: {error:#}");
                }
            });
        }
    }

    async fn handle_websocket(stream: TcpStream, shared: Shared) -> Result<()> {
        let (mut reader, mut writer) = websocket::upgrade(stream).await?;
        let (outbound_tx, mut outbound_rx) = mpsc::channel(64);
        let generation = shared.register_websocket(outbound_tx.clone()).await;
        let writer_task = tokio::spawn(async move {
            while let Some(message) = outbound_rx.recv().await {
                let closing = matches!(message, Outbound::Close { .. });
                websocket::write_message(&mut writer, &message).await?;
                if closing {
                    break;
                }
            }
            Ok::<_, anyhow::Error>(())
        });
        outbound_tx
            .send(Outbound::Text(shared.hello().to_string()))
            .await?;
        shared.request_agent_status().await?;

        let result = loop {
            if !shared.websocket_is_current(generation) {
                break Ok(());
            }
            match websocket::read_message(&mut reader).await {
                Ok(Inbound::Text(message)) => {
                    let response = handle_rpc(&shared, &message).await;
                    if outbound_tx.send(Outbound::Text(response)).await.is_err() {
                        break Ok(());
                    }
                }
                Ok(Inbound::Binary(message)) => {
                    if let Err(error) = handle_browser_binary(&shared, message).await {
                        eprintln!("mock-pico rejected browser binary message: {error:#}");
                    }
                }
                Ok(Inbound::Ping(payload)) => {
                    if outbound_tx.send(Outbound::Pong(payload)).await.is_err() {
                        break Ok(());
                    }
                }
                Ok(Inbound::Close) => break Ok(()),
                Err(error) => break Err(error),
            }
        };
        shared.clear_websocket(generation);
        drop(outbound_tx);
        let _ = writer_task.await;
        result
    }

    async fn handle_rpc(shared: &Shared, message: &str) -> String {
        let Some(rest) = message.strip_prefix("RPC ") else {
            return json!({"error": "correlated RPC command required"}).to_string();
        };
        let Some((request_id, command)) = rest.split_once(' ') else {
            return json!({"error": "malformed RPC command"}).to_string();
        };
        let Ok(request_id) = request_id.parse::<u64>() else {
            return json!({"error": "invalid RPC request ID"}).to_string();
        };
        let payload = match execute_command(shared, command.trim()).await {
            Ok(payload) => payload,
            Err(error) => json!({"error": error.to_string()}),
        };
        json!({
            "event_type": "command/response",
            "version": WEBSOCKET_PROTOCOL_VERSION,
            "request_id": request_id,
            "payload": payload,
        })
        .to_string()
    }

    async fn execute_command(shared: &Shared, command: &str) -> Result<Value> {
        if command.eq_ignore_ascii_case("HELLO") {
            return Ok(shared.hello());
        }
        if command.eq_ignore_ascii_case("STATUS") {
            let state = shared.state.lock().expect("device state lock");
            return Ok(json!({
                "usb_enabled": state.usb_enabled,
                "usb_ready": state.usb_ready,
                "host_os": state.agent.host_os.as_deref().unwrap_or("unknown"),
            }));
        }
        if command.eq_ignore_ascii_case("CONFIG_GET") {
            let state = shared.state.lock().expect("device state lock");
            return Ok(json!({
                "usb_manufacturer": state.manufacturer,
                "usb_product": state.product,
            }));
        }
        if let Some(query) = command.strip_prefix("CONFIG_SET ") {
            let parameters = query_parameters(query)?;
            let manufacturer = parameters
                .iter()
                .find_map(|(key, value)| (key == "manufacturer").then_some(value))
                .context("missing manufacturer")?;
            let product = parameters
                .iter()
                .find_map(|(key, value)| (key == "product").then_some(value))
                .context("missing product")?;
            ensure!(manufacturer.len() <= 32, "bad manufacturer");
            ensure!(product.len() <= 48, "bad product");
            let mut state = shared.state.lock().expect("device state lock");
            ensure!(!state.usb_enabled, "USB already enabled");
            state.manufacturer = manufacturer.clone();
            state.product = product.clone();
            return Ok(json!({"ok": true}));
        }
        if command.eq_ignore_ascii_case("USB_REGISTER") || command.starts_with("USB_REGISTER ") {
            let mut state = shared.state.lock().expect("device state lock");
            state.usb_enabled = true;
            state.usb_ready = true;
            return Ok(json!({"ok": true}));
        }
        if command.eq_ignore_ascii_case("USB_UNREGISTER") {
            let mut state = shared.state.lock().expect("device state lock");
            state.usb_enabled = false;
            state.usb_ready = false;
            return Ok(json!({"ok": true}));
        }
        if let Some(bytecode) = command.strip_prefix("SCRIPT_RUN_HEX ") {
            ensure!(
                !bytecode.is_empty()
                    && bytecode.len() <= 8192
                    && bytecode.len().is_multiple_of(2)
                    && bytecode.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "bad bytecode"
            );
            return Ok(json!({"ok": true, "queued": true}));
        }
        if let Some(query) = command.strip_prefix("FS_LIST ") {
            ensure!(shared.agent_present(), "host agent unavailable");
            let request = encode_filesystem_request(query)?;
            shared
                .serial
                .send(TlvFrame {
                    tag: TAG_FS_LIST_REQUEST,
                    payload: request,
                })
                .await
                .context("queue filesystem request")?;
            return Ok(json!({"ok": true, "queued": true}));
        }
        if let Some(query) = command.strip_prefix("FS_LIST_CANCEL ") {
            let parameters = query_parameters(query)?;
            let request_id = parse_parameter::<u64>(&parameters, "request_id")?;
            shared
                .serial
                .send(TlvFrame {
                    tag: TAG_FS_LIST_CANCEL,
                    payload: request_id.to_le_bytes().to_vec(),
                })
                .await
                .context("queue filesystem cancellation")?;
            return Ok(json!({"ok": true, "queued": true}));
        }
        if command.eq_ignore_ascii_case("TRANSFER_MODE_GET") {
            return Ok(json!({
                "mode": "relay",
                "ws_clients": usize::from(shared.websocket_sender().is_some()),
            }));
        }
        bail!("unknown command")
    }

    async fn handle_browser_binary(shared: &Shared, message: Vec<u8>) -> Result<()> {
        let Some((&kind, payload)) = message.split_first() else {
            bail!("empty WebSocket binary message");
        };
        ensure!(
            kind == transfer_protocol::WS_BINARY_KIND_SESSION,
            "unsupported browser binary kind {kind}"
        );
        ensure!(shared.agent_present(), "host agent unavailable");
        ensure!(
            payload.len() <= transfer_protocol::MAX_SECURE_SESSION_FRAME,
            "secure session frame is too large"
        );
        shared
            .serial
            .send(TlvFrame {
                tag: transfer_protocol::TAG_TRANSFER_SESSION_TO_HOST,
                payload: payload.to_vec(),
            })
            .await
            .context("relay secure session message")
    }

    fn encode_filesystem_request(query: &str) -> Result<Vec<u8>> {
        let parameters = query_parameters(query)?;
        let request_id = parse_parameter::<u64>(&parameters, "request_id")?;
        let cursor = parse_parameter::<u32>(&parameters, "cursor")?;
        let limit = parse_parameter::<u16>(&parameters, "limit")?;
        let flags = parse_parameter::<u8>(&parameters, "flags")?;
        let path = parameters
            .iter()
            .find_map(|(key, value)| (key == "path").then_some(value))
            .context("missing path")?;
        ensure!(path.len() <= 512, "filesystem path is too long");
        let mut payload = Vec::with_capacity(19 + path.len());
        payload.extend_from_slice(&1u16.to_le_bytes());
        payload.extend_from_slice(&request_id.to_le_bytes());
        payload.extend_from_slice(&cursor.to_le_bytes());
        payload.extend_from_slice(&limit.to_le_bytes());
        payload.push(flags);
        payload.extend_from_slice(&(path.len() as u16).to_le_bytes());
        payload.extend_from_slice(path.as_bytes());
        ensure!(
            payload.len() <= transfer_protocol::TLV_MAX_PAYLOAD,
            "filesystem request exceeds TLV limit"
        );
        Ok(payload)
    }

    fn query_parameters(query: &str) -> Result<Vec<(String, String)>> {
        query
            .split('&')
            .map(|pair| {
                let (key, value) = pair.split_once('=').context("malformed query parameter")?;
                Ok((key.to_string(), percent_decode(value)?))
            })
            .collect()
    }

    fn parse_parameter<T: std::str::FromStr>(
        parameters: &[(String, String)],
        name: &str,
    ) -> Result<T>
    where
        T::Err: std::error::Error + Send + Sync + 'static,
    {
        parameters
            .iter()
            .find_map(|(key, value)| (key == name).then_some(value))
            .with_context(|| format!("missing {name}"))?
            .parse()
            .with_context(|| format!("invalid {name}"))
    }

    fn percent_decode(value: &str) -> Result<String> {
        let mut output = Vec::with_capacity(value.len());
        let bytes = value.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'%' {
                ensure!(index + 2 < bytes.len(), "truncated percent escape");
                let high = hex_value(bytes[index + 1]).context("invalid percent escape")?;
                let low = hex_value(bytes[index + 2]).context("invalid percent escape")?;
                output.push((high << 4) | low);
                index += 3;
            } else {
                output.push(bytes[index]);
                index += 1;
            }
        }
        String::from_utf8(output).context("query value is not UTF-8")
    }

    fn hex_value(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }

    async fn serial_write_loop(
        mut writer: WriteHalf<SerialStream>,
        mut frames: mpsc::Receiver<TlvFrame>,
    ) {
        while let Some(frame) = frames.recv().await {
            if let Err(error) = write_tlv(&mut writer, &frame).await {
                eprintln!("mock-pico serial write failed: {error:#}");
                break;
            }
        }
    }

    async fn serial_read_loop(mut reader: ReadHalf<SerialStream>, shared: Shared) {
        loop {
            match read_tlv(&mut reader).await {
                Ok(frame) => {
                    if let Err(error) = handle_agent_frame(&shared, frame).await {
                        eprintln!("mock-pico rejected host-agent frame: {error:#}");
                    }
                }
                Err(error) => {
                    eprintln!("mock-pico serial connection closed: {error:#}");
                    shared.clear_agent();
                    shared.publish_hello().await;
                    break;
                }
            }
        }
    }

    async fn read_tlv(reader: &mut ReadHalf<SerialStream>) -> Result<TlvFrame> {
        let mut header = [0u8; TLV_HEADER_LEN];
        reader
            .read_exact(&mut header)
            .await
            .context("read TLV header")?;
        let payload_len = u32::from_le_bytes(header[1..5].try_into()?) as usize;
        ensure!(
            payload_len <= transfer_protocol::TLV_MAX_PAYLOAD,
            "host-agent TLV payload exceeds limit"
        );
        let mut payload = vec![0u8; payload_len];
        reader
            .read_exact(&mut payload)
            .await
            .context("read TLV payload")?;
        Ok(TlvFrame {
            tag: header[0],
            payload,
        })
    }

    async fn write_tlv(writer: &mut WriteHalf<SerialStream>, frame: &TlvFrame) -> Result<()> {
        ensure!(
            frame.payload.len() <= transfer_protocol::TLV_MAX_PAYLOAD,
            "mock TLV payload exceeds limit"
        );
        let mut header = [0u8; TLV_HEADER_LEN];
        header[0] = frame.tag;
        header[1..].copy_from_slice(&(frame.payload.len() as u32).to_le_bytes());
        writer.write_all(&header).await?;
        writer.write_all(&frame.payload).await?;
        writer.flush().await?;
        Ok(())
    }

    async fn handle_agent_frame(shared: &Shared, frame: TlvFrame) -> Result<()> {
        match frame.tag {
            TAG_REQUEST_AGENT_STATUS => {
                {
                    let mut state = shared.state.lock().expect("device state lock");
                    state.agent.present = true;
                }
                let response = if frame.payload == b"handshake" {
                    b"handshake-ok".as_slice()
                } else {
                    b"probe-ok".as_slice()
                };
                shared
                    .serial
                    .send(TlvFrame {
                        tag: TAG_DEBUG_MSG,
                        payload: response.to_vec(),
                    })
                    .await?;
                shared.publish_hello().await;
            }
            TAG_AGENT_STATUS => {
                let agent = decode_agent_status(&frame.payload);
                println!(
                    "mock-pico host agent: version={} hostname={}",
                    agent.0.as_deref().unwrap_or("legacy"),
                    agent.1.as_deref().unwrap_or("unknown")
                );
                {
                    let mut state = shared.state.lock().expect("device state lock");
                    state.agent.present = true;
                    state.agent.version = agent.0;
                    state.agent.hostname = agent.1;
                }
                shared.publish_hello().await;
            }
            TAG_HOST_OS => {
                let host_os = std::str::from_utf8(&frame.payload)
                    .context("host OS is not UTF-8")?
                    .to_string();
                {
                    let mut state = shared.state.lock().expect("device state lock");
                    state.agent.present = true;
                    state.agent.host_os = Some(host_os);
                }
                shared.publish_hello().await;
            }
            TAG_FS_LIST_PAGE => {
                let mut binary = Vec::with_capacity(1 + frame.payload.len());
                binary.push(WS_BINARY_KIND_FILESYSTEM);
                binary.extend_from_slice(&frame.payload);
                ensure!(
                    shared.send_websocket(Outbound::Binary(binary)).await,
                    "browser relay unavailable"
                );
            }
            transfer_protocol::TAG_TRANSFER_SESSION_TO_BROWSER => {
                let mut binary = Vec::with_capacity(1 + frame.payload.len());
                binary.push(transfer_protocol::WS_BINARY_KIND_SESSION);
                binary.extend_from_slice(&frame.payload);
                ensure!(
                    shared.send_websocket(Outbound::Binary(binary)).await,
                    "browser relay unavailable"
                );
            }
            tag @ (super::relay::TAG_FILE_OPEN
            | super::relay::TAG_FILE_CHUNK
            | super::relay::TAG_FILE_CLOSE
            | super::relay::TAG_FILE_ABORT) => {
                handle_transfer_frame(shared, tag, &frame.payload).await?;
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_transfer_frame(shared: &Shared, tag: u8, payload: &[u8]) -> Result<()> {
        let event = shared
            .relay
            .lock()
            .expect("relay lock")
            .handle(tag, payload)?;
        let Some(event) = event else {
            return Ok(());
        };
        match event {
            RelayEvent::Open {
                transfer_id,
                binary,
                ack,
                progress,
            } => {
                if !shared.send_websocket(Outbound::Binary(binary)).await {
                    abort_unavailable(shared, transfer_id).await?;
                    return Ok(());
                }
                shared.send_websocket(Outbound::Text(progress)).await;
                send_feedback(shared, TAG_FILE_ACK, ack).await?;
            }
            RelayEvent::Chunk {
                transfer_id,
                binary,
                ack,
                progress,
            } => {
                if let Some(binary) = binary
                    && !shared.send_websocket(Outbound::Binary(binary)).await
                {
                    abort_unavailable(shared, transfer_id).await?;
                    return Ok(());
                }
                if let Some(progress) = progress {
                    shared.send_websocket(Outbound::Text(progress)).await;
                }
                send_feedback(shared, TAG_FILE_ACK, ack).await?;
            }
            RelayEvent::Close {
                transfer_id,
                binary,
                result,
                finished,
            } => {
                if !shared.send_websocket(Outbound::Binary(binary)).await {
                    abort_unavailable(shared, transfer_id).await?;
                    return Ok(());
                }
                send_feedback(shared, TAG_FILE_RESULT, result).await?;
                shared.send_websocket(Outbound::Text(finished)).await;
            }
            RelayEvent::Abort {
                transfer_id,
                result,
                event,
            } => {
                send_feedback(shared, TAG_FILE_RESULT, result).await?;
                ensure!(
                    shared.send_websocket(Outbound::Text(event)).await,
                    "browser relay unavailable for aborted transfer {transfer_id}"
                );
            }
        }
        Ok(())
    }

    async fn abort_unavailable(shared: &Shared, transfer_id: u64) -> Result<()> {
        shared.relay.lock().expect("relay lock").cancel(transfer_id);
        send_feedback(
            shared,
            TAG_FILE_ABORT,
            encode_unavailable_abort(transfer_id),
        )
        .await
    }

    async fn send_feedback(shared: &Shared, tag: u8, payload: Vec<u8>) -> Result<()> {
        shared
            .serial
            .send(TlvFrame { tag, payload })
            .await
            .context("send transfer feedback")
    }

    fn decode_agent_status(payload: &[u8]) -> (Option<String>, Option<String>) {
        if let Some(encoded) = payload.strip_prefix(AGENT_STATUS_MAGIC) {
            let mut fields = encoded.splitn(2, |byte| *byte == 0);
            let version = fields.next().and_then(nonempty_utf8);
            let hostname = fields.next().and_then(nonempty_utf8);
            (version, hostname)
        } else {
            (None, nonempty_utf8(payload))
        }
    }

    fn nonempty_utf8(value: &[u8]) -> Option<String> {
        std::str::from_utf8(value)
            .ok()
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn filesystem_request_matches_wire_format() {
            let payload = encode_filesystem_request(
                "request_id=7&cursor=4&limit=64&flags=1&path=%2Ftmp%2Fa%20b",
            )
            .unwrap();
            assert_eq!(&payload[0..2], &1u16.to_le_bytes());
            assert_eq!(&payload[2..10], &7u64.to_le_bytes());
            assert_eq!(&payload[10..14], &4u32.to_le_bytes());
            assert_eq!(&payload[14..16], &64u16.to_le_bytes());
            assert_eq!(payload[16], 1);
            assert_eq!(&payload[17..19], &8u16.to_le_bytes());
            assert_eq!(&payload[19..], b"/tmp/a b");
        }

        #[test]
        fn decodes_current_agent_identity() {
            let (version, hostname) = decode_agent_status(b"PICOAGENT\0v1.2.3\0workstation");
            assert_eq!(version.as_deref(), Some("v1.2.3"));
            assert_eq!(hostname.as_deref(), Some("workstation"));
        }

        #[tokio::test]
        async fn filesystem_rpc_is_correlated_and_relayed() {
            let (serial, mut receiver) = mpsc::channel(1);
            let shared = Shared::new(serial);
            shared.state.lock().unwrap().agent = AgentInfo {
                present: true,
                version: Some("0.1.0".to_string()),
                ..AgentInfo::default()
            };
            let response = handle_rpc(
                &shared,
                "RPC 9 FS_LIST request_id=7&cursor=0&limit=64&flags=0&path=%2Ftmp",
            )
            .await;
            let response: Value = serde_json::from_str(&response).unwrap();
            assert_eq!(response["request_id"], 9);
            assert_eq!(response["version"], WEBSOCKET_PROTOCOL_VERSION);
            assert_eq!(response["payload"]["ok"], true);
            let frame = receiver.recv().await.unwrap();
            assert_eq!(frame.tag, TAG_FS_LIST_REQUEST);
            assert_eq!(&frame.payload[2..10], &7u64.to_le_bytes());
            assert_eq!(&frame.payload[19..], b"/tmp");
        }
    }
}

#[cfg(unix)]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    unix::run().await
}

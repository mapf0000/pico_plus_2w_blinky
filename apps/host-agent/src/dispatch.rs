use crate::config::Config;
use crate::tlv::{self, Frame};
use crate::transport::Event;
use anyhow::{Result, bail};
use bytes::Bytes;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant, MissedTickBehavior, interval, sleep};
use tracing::{debug, info, warn};

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
const TAG_DB_CREDENTIALS_REQUEST: u8 = 11;
const TAG_DB_CREDENTIALS_RESPONSE: u8 = 12;
const HANDSHAKE_PAYLOAD: &[u8] = b"handshake";
const HANDSHAKE_OK_MSG: &str = "handshake-ok";
const PROBE_OK_MSG: &str = "probe-ok";
const KEEPALIVE_INTERVAL_SECS: u64 = 10;
const KEEPALIVE_TIMEOUT_SECS: u64 = KEEPALIVE_INTERVAL_SECS + 2;
const MAX_EXEC_OUTPUT: usize = 8 * 1024;
const EXEC_CHUNK_DELAY_MS: u64 = 20;
const MAX_PROMPT_LABEL_LEN: usize = 80;
static DB_CREDENTIALS_PROMPT_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

pub async fn run(
    mut inbound: mpsc::Receiver<Event>,
    outbound: mpsc::Sender<Frame>,
    config: &Config,
) -> Result<()> {
    let mut outbound = outbound;
    let mut debug_sink = DebugSink::new(config.debug_log.as_deref())?;
    let mut handshake_ok = false;
    let mut last_activity_at = Instant::now();
    let enable_keepalive = should_send_handshake(config);
    let mut keepalive = interval(Duration::from_secs(KEEPALIVE_INTERVAL_SECS));
    keepalive.set_missed_tick_behavior(MissedTickBehavior::Delay);
    if enable_keepalive {
        let _ = keepalive.tick().await;
    }

    if should_send_handshake(config) {
        let frame = Frame::new(
            TAG_REQUEST_AGENT_STATUS,
            Bytes::from_static(HANDSHAKE_PAYLOAD),
        );
        if outbound.send(frame).await.is_ok() {
            info!("handshake request sent");
        } else {
            warn!("handshake request failed");
        }
    }

    loop {
        tokio::select! {
            event = inbound.recv() => {
                let Some(event) = event else {
                    break;
                };
                match event {
                    Event::Raw(bytes) => {
                        last_activity_at = Instant::now();
                        if config.raw {
                            info!(len = bytes.len(), raw = %format_hex(&bytes), "raw data");
                        }
                    }
                    Event::Frame(frame) => {
                        last_activity_at = Instant::now();
                        handle_frame(frame, &mut outbound, &mut debug_sink, &mut handshake_ok).await?;
                    }
                }
            }
            _ = keepalive.tick(), if enable_keepalive => {
                let now = Instant::now();
                if now.duration_since(last_activity_at) > Duration::from_secs(KEEPALIVE_TIMEOUT_SECS) {
                    warn!("keepalive timeout; disconnecting");
                    break;
                }
                let frame = Frame::new(TAG_REQUEST_AGENT_STATUS, Bytes::new());
                if outbound.send(frame).await.is_err() {
                    warn!("keepalive send failed");
                    break;
                }
                if outbound.is_closed() {
                    warn!("serial writer closed");
                    break;
                }
            }
        }
    }

    Ok(())
}

async fn handle_frame(
    frame: Frame,
    outbound: &mut mpsc::Sender<Frame>,
    debug_sink: &mut DebugSink,
    handshake_ok: &mut bool,
) -> Result<()> {
    match frame.tag {
        TAG_REQUEST_AGENT_STATUS => {
            let handshake = frame.payload.as_ref() == HANDSHAKE_PAYLOAD;
            if handshake {
                info!("handshake request received");
            }
            let hostname = hostname::get().unwrap_or_else(|_| "unknown".into());
            let payload = Bytes::from(hostname.to_string_lossy().into_owned());
            let response = Frame::new(TAG_AGENT_STATUS, payload);
            outbound.send(response).await?;
            info!("sent agent status");
            if handshake {
                info!("handshake response sent");
                if !*handshake_ok {
                    info!("handshake ok");
                    *handshake_ok = true;
                }
            }
        }
        TAG_DEBUG_MSG => {
            let message = String::from_utf8_lossy(&frame.payload);
            if message == PROBE_OK_MSG {
                debug!(message = %message, "device debug");
                return Ok(());
            }
            info!(message = %message, "device debug");
            debug_sink.write_line(&message)?;
            if message == HANDSHAKE_OK_MSG && !*handshake_ok {
                info!("handshake ok");
                *handshake_ok = true;
            }
        }
        TAG_EXECUTE => {
            handle_execute(frame, outbound).await?;
        }
        TAG_DB_CREDENTIALS_REQUEST => {
            let outbound = outbound.clone();
            tokio::spawn(async move {
                handle_db_credentials_request_in_background(frame, outbound).await;
            });
        }
        TAG_WS_CONNECT | TAG_WS_DATA | TAG_WS_DISCONNECT | TAG_WS_DATA_RECV
        | TAG_EXECUTE_RESULT | TAG_MIC_PCM_DATA => {
            debug!(
                tag = frame.tag,
                len = frame.payload.len(),
                "unhandled frame"
            );
        }
        _ => {
            debug!(tag = frame.tag, len = frame.payload.len(), "unknown frame");
        }
    }

    Ok(())
}

fn should_send_handshake(config: &Config) -> bool {
    #[cfg(any(test, feature = "test-port-fd"))]
    {
        if config.port_fd.is_some() {
            return false;
        }
    }
    true
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

async fn handle_db_credentials_request(frame: Frame, outbound: &mpsc::Sender<Frame>) -> Result<()> {
    let label = prompt_label_from_payload(&frame.payload);
    let credentials = match prompt_db_credentials(label.as_deref()).await {
        Ok(credentials) => credentials,
        Err(err) => {
            warn!(error = %err, "db credential prompt failed");
            outbound
                .send(Frame::new(TAG_DB_CREDENTIALS_RESPONSE, Bytes::new()))
                .await?;
            return Ok(());
        }
    };

    let payload = match credentials_payload(&credentials) {
        Ok(payload) => payload,
        Err(err) => {
            warn!(error = %err, "invalid db credential payload");
            Bytes::new()
        }
    };
    outbound
        .send(Frame::new(TAG_DB_CREDENTIALS_RESPONSE, payload))
        .await?;
    Ok(())
}

async fn handle_db_credentials_request_in_background(frame: Frame, outbound: mpsc::Sender<Frame>) {
    let Ok(_guard) = DbPromptInFlightGuard::try_acquire() else {
        warn!("db credentials prompt already in flight; replying with empty credentials");
        let _ = outbound
            .send(Frame::new(TAG_DB_CREDENTIALS_RESPONSE, Bytes::new()))
            .await;
        return;
    };

    if let Err(err) = handle_db_credentials_request(frame, &outbound).await {
        warn!(error = %err, "failed to handle db credential request");
        let _ = outbound
            .send(Frame::new(TAG_DB_CREDENTIALS_RESPONSE, Bytes::new()))
            .await;
    }
}

struct DbPromptInFlightGuard;

impl DbPromptInFlightGuard {
    fn try_acquire() -> Result<Self> {
        let acquired = DB_CREDENTIALS_PROMPT_IN_FLIGHT
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        if !acquired {
            bail!("prompt already in flight");
        }
        Ok(Self)
    }
}

impl Drop for DbPromptInFlightGuard {
    fn drop(&mut self) {
        DB_CREDENTIALS_PROMPT_IN_FLIGHT.store(false, Ordering::Release);
    }
}

async fn send_execute_result(
    outbound: &mut mpsc::Sender<Frame>,
    mut output: Vec<u8>,
) -> Result<()> {
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
            .send(Frame::new(
                TAG_EXECUTE_RESULT,
                Bytes::copy_from_slice(chunk),
            ))
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

struct Credentials {
    username: String,
    password: String,
}

fn credentials_payload(credentials: &Credentials) -> Result<Bytes> {
    if credentials.username.contains('\0') || credentials.password.contains('\0') {
        bail!("credentials contain null bytes");
    }

    let mut payload =
        Vec::with_capacity(credentials.username.len() + 1 + credentials.password.len());
    payload.extend_from_slice(credentials.username.as_bytes());
    payload.push(0);
    payload.extend_from_slice(credentials.password.as_bytes());
    Ok(Bytes::from(payload))
}

fn prompt_label_from_payload(payload: &Bytes) -> Option<String> {
    let label = std::str::from_utf8(payload.as_ref()).ok()?.trim();
    if label.is_empty() {
        return None;
    }
    let mut label = label.to_string();
    if label.len() > MAX_PROMPT_LABEL_LEN {
        label.truncate(MAX_PROMPT_LABEL_LEN);
    }
    Some(label)
}

fn credentials_from_env() -> Option<Credentials> {
    let username = std::env::var("HOST_AGENT_DB_USER").ok()?;
    let password = std::env::var("HOST_AGENT_DB_PASSWORD").ok()?;
    Some(Credentials { username, password })
}

#[cfg(target_os = "macos")]
async fn prompt_db_credentials(label: Option<&str>) -> Result<Credentials> {
    if let Some(credentials) = credentials_from_env() {
        return Ok(credentials);
    }

    let context = label.unwrap_or("Database credentials");
    run_osascript_credentials_prompt(context).await
}

#[cfg(not(target_os = "macos"))]
async fn prompt_db_credentials(_label: Option<&str>) -> Result<Credentials> {
    if let Some(credentials) = credentials_from_env() {
        return Ok(credentials);
    }
    bail!("db credential prompt requires macOS GUI or env vars")
}

#[cfg(target_os = "macos")]
async fn run_osascript_credentials_prompt(context: &str) -> Result<Credentials> {
    // Best effort: present a single, native-looking credentials dialog.
    match run_jxa_credentials_prompt("Host Agent", context).await {
        Ok(credentials) => Ok(credentials),
        Err(err) if is_prompt_canceled(&err) => Err(err),
        Err(err) => {
            warn!(
                error = %err,
                "jxa credentials prompt failed; falling back to separate dialogs"
            );
            let username = run_osascript_prompt(&format!("{context}: username"), false).await?;
            let password = run_osascript_prompt(&format!("{context}: password"), true).await?;
            Ok(Credentials { username, password })
        }
    }
}

#[cfg(target_os = "macos")]
async fn run_osascript_prompt(message: &str, hidden: bool) -> Result<String> {
    // Prefer a Cocoa `NSAlert` + (secure) text field via JavaScript for Automation (JXA) so the
    // prompt looks like a typical macOS password dialog.
    match run_jxa_prompt("Host Agent", message, hidden).await {
        Ok(response) => Ok(response),
        Err(err) if is_prompt_canceled(&err) => Err(err),
        Err(err) => {
            warn!(error = %err, "jxa prompt failed; falling back to AppleScript dialog");
            run_applescript_prompt(message, hidden).await
        }
    }
}

#[cfg(target_os = "macos")]
fn escape_applescript_string(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .trim()
        .to_string()
}

#[cfg(target_os = "macos")]
async fn run_jxa_credentials_prompt(title: &str, context: &str) -> Result<Credentials> {
    const SCRIPT: &str = r#"
ObjC.import('AppKit');
ObjC.import('Foundation');
ObjC.import('stdlib');

function writeStdout(text) {
  const data = $(text).dataUsingEncoding($.NSUTF8StringEncoding);
  $.NSFileHandle.fileHandleWithStandardOutput.writeData(data);
}

function run(argv) {
  const currentApp = Application.currentApplication();
  currentApp.includeStandardAdditions = true;
  currentApp.activate();

  const title = argv[0] || 'Host Agent';
  const context = argv[1] || 'Database credentials';
  const okLabel = argv[2] || 'OK';

  const app = $.NSApplication.sharedApplication;
  app.setActivationPolicy($.NSApplicationActivationPolicyRegular);
  app.unhide(null);
  app.activateIgnoringOtherApps(true);
  try {
    $.NSRunningApplication.currentApplication.activateWithOptions(
      $.NSApplicationActivateIgnoringOtherApps | $.NSApplicationActivateAllWindows
    );
  } catch (e) {}

  const alert = $.NSAlert.alloc.init;
  alert.messageText = context;
  alert.informativeText = 'Enter a username and password to allow this.';
  alert.alertStyle = $.NSAlertStyleInformational;

  try {
    alert.icon = $.NSImage.imageNamed($.NSImageNameLockLockedTemplate);
  } catch (e) {}

  try {
    alert.window.title = title;
  } catch (e) {}

  alert.addButtonWithTitle(okLabel);
  alert.addButtonWithTitle('Cancel');

  const width = 320;
  const height = 72;
  const fieldHeight = 28;
  const margin = 4;
  const spacing = 8;

  const view = $.NSView.alloc.initWithFrame($.NSMakeRect(0, 0, width, height));
  const username = $.NSTextField.alloc.initWithFrame(
    $.NSMakeRect(0, margin + fieldHeight + spacing, width, fieldHeight)
  );
  const password = $.NSSecureTextField.alloc.initWithFrame(
    $.NSMakeRect(0, margin, width, fieldHeight)
  );

  username.placeholderString = 'Username';
  password.placeholderString = 'Password';

  try {
    username.setBezelStyle($.NSTextFieldRoundedBezel);
    password.setBezelStyle($.NSTextFieldRoundedBezel);
  } catch (e) {}

  username.nextKeyView = password;
  password.nextKeyView = username;

  view.addSubview(username);
  view.addSubview(password);
  alert.accessoryView = view;

  try {
    alert.window.makeFirstResponder(username);
  } catch (e) {}

  try {
    alert.window.setHidesOnDeactivate(false);
    alert.window.setLevel($.NSModalPanelWindowLevel);
    alert.window.center;
    alert.window.makeKeyAndOrderFront(null);
  } catch (e) {}

  const response = alert.runModal;
  if (response === $.NSAlertFirstButtonReturn) {
    const u = ObjC.unwrap(username.stringValue);
    const p = ObjC.unwrap(password.stringValue);
    writeStdout(u + '\u0000' + p);
    $.exit(0);
  }
  $.exit(1);
}
"#;

    let output = Command::new("osascript")
        .arg("-l")
        .arg("JavaScript")
        .arg("-e")
        .arg(SCRIPT)
        .arg(title)
        .arg(context)
        .arg("OK")
        .stdin(Stdio::null())
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        if stderr.is_empty() || was_user_canceled(stderr) {
            bail!("prompt canceled");
        }
        bail!("osascript (jxa) failed: {stderr}");
    }

    let mut stdout = output.stdout;
    while matches!(stdout.last(), Some(b'\n' | b'\r')) {
        stdout.pop();
    }
    let Some(separator_pos) = stdout.iter().position(|byte| *byte == 0) else {
        bail!("invalid prompt response");
    };
    let username = String::from_utf8_lossy(&stdout[..separator_pos]).to_string();
    let password = String::from_utf8_lossy(&stdout[separator_pos + 1..]).to_string();
    Ok(Credentials { username, password })
}

#[cfg(target_os = "macos")]
async fn run_jxa_prompt(title: &str, message: &str, hidden: bool) -> Result<String> {
    const SCRIPT: &str = r#"
ObjC.import('AppKit');
ObjC.import('Foundation');
ObjC.import('stdlib');

function writeStdout(text) {
  const data = $(text).dataUsingEncoding($.NSUTF8StringEncoding);
  $.NSFileHandle.fileHandleWithStandardOutput.writeData(data);
}

function run(argv) {
  const currentApp = Application.currentApplication();
  currentApp.includeStandardAdditions = true;
  currentApp.activate();

  const title = argv[0] || 'Host Agent';
  const message = argv[1] || '';
  const placeholder = argv[2] || '';
  const hidden = (argv[3] || '') === '1';

  const app = $.NSApplication.sharedApplication;
  app.setActivationPolicy($.NSApplicationActivationPolicyRegular);
  app.unhide(null);
  app.activateIgnoringOtherApps(true);
  try {
    $.NSRunningApplication.currentApplication.activateWithOptions(
      $.NSApplicationActivateIgnoringOtherApps | $.NSApplicationActivateAllWindows
    );
  } catch (e) {}

  const alert = $.NSAlert.alloc.init;
  alert.messageText = title;
  alert.informativeText = message;
  alert.alertStyle = $.NSAlertStyleInformational;
  alert.addButtonWithTitle('OK');
  alert.addButtonWithTitle('Cancel');

  const frame = $.NSMakeRect(0, 0, 320, 24);
  const field = hidden
    ? $.NSSecureTextField.alloc.initWithFrame(frame)
    : $.NSTextField.alloc.initWithFrame(frame);
  field.placeholderString = placeholder;
  alert.accessoryView = field;
  try {
    alert.window.makeFirstResponder(field);
  } catch (e) {}

  try {
    alert.window.setHidesOnDeactivate(false);
    alert.window.setLevel($.NSModalPanelWindowLevel);
    alert.window.center;
    alert.window.makeKeyAndOrderFront(null);
  } catch (e) {}

  // In JXA, zero-argument ObjC selectors are invoked by property access (not by calling a JS function).
  const response = alert.runModal;
  if (response === $.NSAlertFirstButtonReturn) {
    writeStdout(ObjC.unwrap(field.stringValue));
    $.exit(0);
  }
  $.exit(1);
}
"#;

    let placeholder = if hidden { "Password" } else { "Username" };
    let hidden_flag = if hidden { "1" } else { "0" };
    let output = Command::new("osascript")
        .arg("-l")
        .arg("JavaScript")
        .arg("-e")
        .arg(SCRIPT)
        .arg(title)
        .arg(message)
        .arg(placeholder)
        .arg(hidden_flag)
        .stdin(Stdio::null())
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        if stderr.is_empty() || was_user_canceled(stderr) {
            bail!("prompt canceled");
        }
        bail!("osascript (jxa) failed: {stderr}");
    }

    let response = String::from_utf8_lossy(&output.stdout);
    Ok(response.trim_end().to_string())
}

#[cfg(target_os = "macos")]
async fn run_applescript_prompt(message: &str, hidden: bool) -> Result<String> {
    let message: String = escape_applescript_string(message);
    let hidden_clause = if hidden { " with hidden answer" } else { "" };
    let dialog = format!(
        "text returned of (display dialog \"{message}\" with title \"Host Agent\" default answer \"\"{hidden_clause} buttons {{\"Cancel\", \"OK\"}} default button \"OK\" cancel button \"Cancel\" with icon note)"
    );
    let dialog_with_activate = format!("tell application \"System Events\" to activate\n{dialog}");

    let output = Command::new("osascript")
        .arg("-e")
        .arg(dialog_with_activate)
        .stdin(Stdio::null())
        .output()
        .await?;

    if output.status.success() {
        let response = String::from_utf8_lossy(&output.stdout);
        return Ok(response.trim_end().to_string());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    if stderr.is_empty() || was_user_canceled(stderr) {
        bail!("prompt canceled");
    }

    if was_automation_denied(stderr) {
        let output = Command::new("osascript")
            .arg("-e")
            .arg(dialog)
            .stdin(Stdio::null())
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stderr = stderr.trim();
            if stderr.is_empty() || was_user_canceled(stderr) {
                bail!("prompt canceled");
            }
            bail!("osascript failed: {stderr}");
        }

        let response = String::from_utf8_lossy(&output.stdout);
        return Ok(response.trim_end().to_string());
    }

    bail!("osascript failed: {stderr}");
}

#[cfg(target_os = "macos")]
fn was_user_canceled(stderr: &str) -> bool {
    // AppleScript/JXA cancellation typically surfaces as error -128.
    stderr.contains("(-128)") || stderr.to_lowercase().contains("user canceled")
}

#[cfg(target_os = "macos")]
fn is_prompt_canceled(err: &anyhow::Error) -> bool {
    err.to_string().contains("prompt canceled")
}

#[cfg(target_os = "macos")]
fn was_automation_denied(stderr: &str) -> bool {
    // AppleScript can fail with -1743 when it isn't allowed to send Apple Events (e.g. to System Events).
    let lower = stderr.to_lowercase();
    lower.contains("(-1743)")
        || ((lower.contains("not authorised") || lower.contains("not authorized"))
            && lower.contains("apple events"))
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
                Some(OpenOptions::new().create(true).append(true).open(path)?)
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

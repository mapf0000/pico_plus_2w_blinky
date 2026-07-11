use core::cell::RefCell;
use core::fmt::Write as _;

use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::Instant;
use heapless::String;
use portable_atomic::{AtomicBool, AtomicU64, Ordering};

use crate::http::util::escape_json_str;

pub const HELLO_VERSION: u16 = 1;
pub const WEBSOCKET_PROTOCOL_VERSION: u16 = 1;
pub const TRANSFER_PROTOCOL_VERSION: u16 = 1;
pub const FILESYSTEM_PROTOCOL_VERSION: u16 = 1;
pub const HELLO_TEXT_MAX: usize = 768;

const AGENT_STATUS_MAGIC: &[u8] = b"PICOAGENT\0";
const HOST_AGENT_STALE_AFTER_MS: u64 = 25_000;
const HOST_AGENT_VERSION_MAX: usize = 32;
const HOSTNAME_MAX: usize = 96;

struct HostAgentState {
    version: String<HOST_AGENT_VERSION_MAX>,
    hostname: String<HOSTNAME_MAX>,
}

impl HostAgentState {
    const fn new() -> Self {
        Self {
            version: String::new(),
            hostname: String::new(),
        }
    }
}

#[derive(Clone)]
pub struct HostAgentSnapshot {
    pub present: bool,
    pub version: String<HOST_AGENT_VERSION_MAX>,
    pub hostname: String<HOSTNAME_MAX>,
}

static HOST_AGENT_STATE: Mutex<CriticalSectionRawMutex, RefCell<HostAgentState>> =
    Mutex::new(RefCell::new(HostAgentState::new()));
static HOST_AGENT_SEEN: AtomicBool = AtomicBool::new(false);
static HOST_AGENT_LAST_SEEN_MS: AtomicU64 = AtomicU64::new(0);

pub fn clear_host_agent() {
    HOST_AGENT_SEEN.store(false, Ordering::Release);
    HOST_AGENT_LAST_SEEN_MS.store(0, Ordering::Release);
    HOST_AGENT_STATE.lock(|cell| {
        let mut state = cell.borrow_mut();
        state.version.clear();
        state.hostname.clear();
    });
}

pub fn mark_host_agent_seen() {
    HOST_AGENT_LAST_SEEN_MS.store(Instant::now().as_millis(), Ordering::Release);
    HOST_AGENT_SEEN.store(true, Ordering::Release);
}

pub fn record_host_agent_status(payload: &[u8]) {
    mark_host_agent_seen();
    HOST_AGENT_STATE.lock(|cell| {
        let mut state = cell.borrow_mut();
        state.version.clear();
        state.hostname.clear();

        if let Some(encoded) = payload.strip_prefix(AGENT_STATUS_MAGIC) {
            let mut fields = encoded.splitn(2, |byte| *byte == 0);
            if let Some(version) = fields
                .next()
                .and_then(|value| core::str::from_utf8(value).ok())
            {
                store_safe_token(&mut state.version, version);
            }
            if let Some(hostname) = fields
                .next()
                .and_then(|value| core::str::from_utf8(value).ok())
            {
                store_safe_token(&mut state.hostname, hostname);
            }
            return;
        }

        // Older agents reported only the hostname. Keeping this fallback makes
        // their presence visible while the missing version signals an update.
        if let Ok(hostname) = core::str::from_utf8(payload) {
            store_safe_token(&mut state.hostname, hostname);
        }
    });
}

fn store_safe_token<const N: usize>(target: &mut String<N>, value: &str) {
    for character in value.chars() {
        let safe =
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_' | '+' | ':');
        if target.push(if safe { character } else { '_' }).is_err() {
            break;
        }
    }
}

pub fn host_agent_snapshot() -> HostAgentSnapshot {
    let last_seen = HOST_AGENT_LAST_SEEN_MS.load(Ordering::Acquire);
    let present = HOST_AGENT_SEEN.load(Ordering::Acquire)
        && Instant::now().as_millis().saturating_sub(last_seen) <= HOST_AGENT_STALE_AFTER_MS;
    HOST_AGENT_STATE.lock(|cell| {
        let state = cell.borrow();
        HostAgentSnapshot {
            present,
            version: state.version.clone(),
            hostname: state.hostname.clone(),
        }
    })
}

pub fn hello_json() -> String<HELLO_TEXT_MAX> {
    let host = host_agent_snapshot();
    let version = if host.version.is_empty() {
        let mut value: String<68> = String::new();
        let _ = value.push_str("null");
        value
    } else {
        let escaped = escape_json_str(host.version.as_str());
        let mut value: String<68> = String::new();
        let _ = write!(value, "\"{}\"", escaped.as_str());
        value
    };
    let hostname = if host.hostname.is_empty() {
        let mut value: String<196> = String::new();
        let _ = value.push_str("null");
        value
    } else {
        let escaped = escape_json_str(host.hostname.as_str());
        let mut value: String<196> = String::new();
        let _ = write!(value, "\"{}\"", escaped.as_str());
        value
    };
    let privileged = if host.present && !host.version.is_empty() {
        "[\"host_execute\",\"db_credentials\",\"host_filesystem\"]"
    } else {
        "[]"
    };

    let mut hello = String::new();
    let _ = write!(
        hello,
        concat!(
            "{{\"event_type\":\"hello\",\"version\":{},",
            "\"firmware\":{{\"version\":\"{}\",\"build\":\"{}\"}},",
            "\"protocols\":{{\"websocket\":{},\"transfer\":{},\"filesystem\":{}}},",
            "\"host_agent\":{{\"present\":{},\"version\":{},\"hostname\":{}}},",
            "\"keyboard\":{{\"layouts\":[\"mac_de-DE\"],",
            "\"features\":[\"hid_keyboard\",\"script_bytecode\",\"macos_assistant\"]}},",
            "\"features\":[\"usb_identity\",\"usb_control\",\"file_transfer\",",
            "\"filesystem_browser\",\"transfer_download\"],",
            "\"privileged_operations\":{} }}"
        ),
        HELLO_VERSION,
        env!("CARGO_PKG_VERSION"),
        env!("PICO_FIRMWARE_BUILD"),
        WEBSOCKET_PROTOCOL_VERSION,
        TRANSFER_PROTOCOL_VERSION,
        FILESYSTEM_PROTOCOL_VERSION,
        host.present,
        version.as_str(),
        hostname.as_str(),
        privileged,
    );
    hello
}

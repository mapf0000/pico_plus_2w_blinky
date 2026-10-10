use core::cell::RefCell;

use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::Instant;
use heapless::String;
use portable_atomic::{AtomicBool, AtomicU64, Ordering};

pub const FILESYSTEM_PROTOCOL_VERSION: u16 = 1;

const AGENT_STATUS_MAGIC: &[u8] = b"PICOAGENT\0";
const HOST_AGENT_STALE_AFTER_MS: u64 = 25_000;
const HOST_AGENT_VERSION_MAX: usize = 32;
const HOSTNAME_MAX: usize = 96;
const HOST_OS_MAX: usize = 24;

struct HostAgentState {
    version: String<HOST_AGENT_VERSION_MAX>,
    hostname: String<HOSTNAME_MAX>,
    host_os: String<HOST_OS_MAX>,
}

impl HostAgentState {
    const fn new() -> Self {
        Self {
            version: String::new(),
            hostname: String::new(),
            host_os: String::new(),
        }
    }
}

#[derive(Clone)]
pub struct HostAgentSnapshot {
    pub version: String<HOST_AGENT_VERSION_MAX>,
    pub hostname: String<HOSTNAME_MAX>,
    pub host_os: String<HOST_OS_MAX>,
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
        state.host_os.clear();
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
        state.host_os.clear();

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

pub fn record_host_os(payload: &[u8]) {
    mark_host_agent_seen();
    HOST_AGENT_STATE.lock(|cell| {
        let mut state = cell.borrow_mut();
        state.host_os.clear();
        if let Ok(host_os) = core::str::from_utf8(payload) {
            store_safe_token(&mut state.host_os, host_os);
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

pub fn host_agent_present() -> bool {
    let last_seen = HOST_AGENT_LAST_SEEN_MS.load(Ordering::Acquire);
    HOST_AGENT_SEEN.load(Ordering::Acquire)
        && Instant::now().as_millis().saturating_sub(last_seen) <= HOST_AGENT_STALE_AFTER_MS
}

pub fn host_agent_snapshot() -> HostAgentSnapshot {
    HOST_AGENT_STATE.lock(|cell| {
        let state = cell.borrow();
        HostAgentSnapshot {
            version: state.version.clone(),
            hostname: state.hostname.clone(),
            host_os: state.host_os.clone(),
        }
    })
}

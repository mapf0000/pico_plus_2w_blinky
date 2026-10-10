//! Native companion state and bounded transport seam, independent of egui and BLE APIs.
//!
//! Client requests are internal messages. BLE uses the shared bounded fragment
//! protocol, while keyboard effects retain the production script envelope/KBD1.

mod client;
pub mod keyboard;
pub mod mock;

pub use client::Client;
pub use keyboard::layouts;
use std::collections::VecDeque;
use std::time::Duration;

pub const MAX_DEVICES: usize = 16;
pub const MAX_DIAGNOSTICS: usize = 128;
pub const MAX_METADATA_BYTES: usize = 96;
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
pub const CANCEL_TIMEOUT: Duration = Duration::from_secs(2);
pub const STATUS_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    PermissionDenied,
    Busy,
    UsbUnavailable,
    Incompatible,
    Timeout,
    LinkLost,
    InvalidData,
    QueueFull,
    NotReady,
    InvalidText,
    UnsupportedLayout,
    IdExhausted,
    AdapterUnavailable,
    BluetoothUnavailable,
    ReadOnly,
}

impl Failure {
    pub fn message(self) -> &'static str {
        match self {
            Self::PermissionDenied => "Bluetooth permission denied",
            Self::Busy => "Another controller owns control",
            Self::UsbUnavailable => "USB keyboard is unavailable",
            Self::Incompatible => "Device protocol is incompatible",
            Self::Timeout => "Request timed out; disconnected to avoid replay",
            Self::LinkLost => "Connection lost; pending work discarded",
            Self::InvalidData => "Invalid device response",
            Self::QueueFull => "Command queue is full; try again",
            Self::NotReady => "Connect and acquire control before sending",
            Self::InvalidText => "Text is empty, unsupported, or exceeds keyboard limits",
            Self::UnsupportedLayout => "Layout is not supported by this device",
            Self::AdapterUnavailable => "No Bluetooth adapter available",
            Self::BluetoothUnavailable => {
                "Bluetooth unavailable; enable the adapter and check permissions"
            }
            Self::ReadOnly => "This BLE service supports status only",
            Self::IdExhausted => "Session identifiers exhausted; restart the companion",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub id: String,
    pub name: String,
}

impl Device {
    pub fn valid(&self) -> bool {
        valid_metadata(&self.id) && valid_metadata(&self.name)
    }
}

fn valid_metadata(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_METADATA_BYTES && !value.chars().any(char::is_control)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub usb_ready: bool,
    pub usb_enabled: bool,
    pub uptime_secs: Option<u64>,
    pub host_agent_present: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub script_version: u8,
    pub read_only: bool,
    pub firmware: String,
    pub layouts: Vec<String>,
    pub status: Status,
    /// Extra budget for BLE fragmentation; mock transports need none.
    pub upload_timeout: Duration,
}

impl Capabilities {
    pub fn valid(&self) -> bool {
        valid_metadata(&self.firmware)
            && (self.read_only || !self.layouts.is_empty())
            && self.layouts.len() <= 16
            && self.layouts.iter().all(|layout| valid_metadata(layout))
    }

    pub fn compatible(&self) -> bool {
        self.read_only
            || (self.script_version == script_protocol::VERSION
                && self
                    .layouts
                    .iter()
                    .any(|layout| layouts().contains(&layout.as_str())))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Connection {
    #[default]
    Disconnected,
    Scanning,
    Connecting,
    Connected,
    Failed(Failure),
}

impl Connection {
    pub fn label(self) -> &'static str {
        match self {
            Self::Disconnected => "Disconnected",
            Self::Scanning => "Scanning",
            Self::Connecting => "Connecting",
            Self::Connected => "Connected",
            Self::Failed(error) => error.message(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Completed,
    Cancelled,
    Rejected,
    UsbUnavailable,
    Disconnected,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Job {
    #[default]
    Idle,
    Sending,
    Running,
    Cancelling,
    Finished(Outcome),
}

impl Job {
    pub fn active(self) -> bool {
        matches!(self, Self::Sending | Self::Running | Self::Cancelling)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub revision: u64,
    pub epoch: u64,
    pub connection: Connection,
    pub devices: Vec<Device>,
    pub selected: Option<Device>,
    pub capabilities: Option<Capabilities>,
    /// Granted by this connection after transport authentication.
    pub control_acquired: bool,
    pub pending: bool,
    pub job: Job,
    pub last_error: Option<Failure>,
    pub last_rtt: Option<Duration>,
    pub diagnostics: VecDeque<&'static str>,
    pub discarded_diagnostics: u64,
}

impl Snapshot {
    pub fn compatible(&self) -> bool {
        self.capabilities
            .as_ref()
            .is_some_and(Capabilities::compatible)
    }

    pub fn can_send(&self) -> bool {
        self.connection == Connection::Connected
            && self.compatible()
            && self.control_acquired
            && !self.pending
            && !self.job.active()
            && self
                .capabilities
                .as_ref()
                .is_some_and(|caps| !caps.read_only && caps.status.usb_ready)
    }
}

#[derive(Clone, Debug)]
pub enum Action {
    Scan,
    Connect(String),
    Acquire,
    Release,
    SetUsbEnabled(bool),
    SendText {
        text: String,
        layout: String,
        delay_ms: u32,
    },
    Cancel,
    Disconnect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tag {
    pub epoch: u64,
    pub request: u64,
}

#[derive(Clone, Debug)]
pub enum Operation {
    Scan,
    Connect(Device),
    Acquire,
    Release,
    SetUsbEnabled(bool),
    Status,
    Run(Vec<u8>),
    Cancel(Vec<u8>),
}

#[derive(Clone, Debug)]
pub struct Request {
    pub tag: Tag,
    pub operation: Operation,
}

#[derive(Clone, Debug)]
pub enum Reply {
    Devices(Vec<Device>),
    Connected(Capabilities),
    Acquired,
    Released,
    Status(Status),
    Accepted(script_protocol::EffectId),
    Finished {
        id: script_protocol::EffectId,
        outcome: Outcome,
    },
    Error(Failure),
}

#[derive(Clone, Debug)]
pub enum Event {
    Reply { tag: Tag, reply: Reply },
    Lost { epoch: u64 },
}

/// Non-blocking boundary owned by the backend, never called from UI rendering.
/// Native adapters can enqueue I/O to an async actor and drain its bounded events.
pub trait Transport {
    fn submit(&mut self, request: Request, now: Duration) -> Result<(), Failure>;
    fn poll(&mut self, now: Duration) -> Option<Event>;
    fn close(&mut self);
}

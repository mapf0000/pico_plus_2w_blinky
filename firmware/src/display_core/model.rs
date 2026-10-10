//! Page models accept snapshots and emit intents; they never access device services.

use core::fmt::Write;
use firmware_exec::jobs::{JobHandle, Phase};
use heapless::String;

use super::input::{Button, ButtonEvents, ButtonMask, Navigation, Routing, Selection};

pub const TEXT_CAPACITY: usize = 96;
pub type Text = String<TEXT_CAPACITY>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageId {
    Payloads,
    Transfer,
    Agent,
    System,
    Logs,
}

impl PageId {
    pub const ALL: [Self; 4] = [Self::Payloads, Self::Agent, Self::System, Self::Logs];

    pub fn title(self) -> &'static str {
        match self {
            Self::Payloads => "Payloads",
            Self::Transfer => "Transfer",
            Self::Agent => "Host Agent",
            Self::System => "System / Pico Plus 2",
            Self::Logs => "Logs",
        }
    }

    pub fn menu_label(self) -> &'static str {
        if self == Self::System {
            "System"
        } else {
            self.title()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresetAction {
    Keyboard,
    CdcInstall,
    CdcArm,
}

pub struct Preset {
    pub name: &'static str,
    pub layout: &'static str,
    pub launches_agent: bool,
    pub available: bool,
    pub program: &'static [u8],
    pub action: PresetAction,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Severity {
    #[default]
    Normal,
    Success,
    Warning,
}

#[derive(Default)]
pub struct Status {
    pub text: Text,
    pub severity: Severity,
}

impl Status {
    pub fn set(&mut self, text: &str, severity: Severity) {
        self.text = bounded_text(format_args!("{text}"));
        self.severity = severity;
    }
}

/// Formatting overflow leaves a valid bounded prefix; rendering adds viewport ellipsis.
pub fn bounded_text(args: core::fmt::Arguments<'_>) -> Text {
    let mut text = Text::new();
    // Write fragments character by character so one oversized fragment keeps its prefix.
    struct Writer<'a>(&'a mut Text);
    impl core::fmt::Write for Writer<'_> {
        fn write_str(&mut self, value: &str) -> core::fmt::Result {
            for character in value.chars() {
                self.0.push(character).map_err(|_| core::fmt::Error)?;
            }
            Ok(())
        }
    }
    let _ = Writer(&mut text).write_fmt(args);
    text
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeyboardSnapshot {
    #[default]
    Unavailable,
    Ready,
    Busy {
        local: bool,
        phase: Phase,
        cancelled: bool,
    },
}

impl KeyboardSnapshot {
    pub fn reserved(self) -> bool {
        matches!(self, Self::Busy { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionStatus {
    Completed,
    Cancelled,
    Rejected,
    UsbUnavailable,
}

pub struct Completion {
    pub handle: JobHandle,
    pub status: CompletionStatus,
    pub completed_at_ms: u64,
}

pub enum Submission {
    Accepted(JobHandle),
    Busy,
    UsbUnavailable,
    Invalid,
}

#[derive(Default)]
pub struct PayloadModel {
    pub selected: Selection,
    pub status: Status,
    pending: Option<JobHandle>,
    launching_agent: bool,
    agent_deadline_ms: Option<u64>,
    waiting_for_agent_clear: bool,
}

impl PayloadModel {
    pub fn installation_status(&mut self, text: &str, warning: bool, waiting: bool, now_ms: u64) {
        self.waiting_for_agent_clear = false;
        self.status.set(
            text,
            if warning {
                Severity::Warning
            } else {
                Severity::Normal
            },
        );
        self.agent_deadline_ms = waiting.then_some(now_ms.saturating_add(120_000));
    }
    pub fn submitted(&mut self, result: Submission, launches_agent: bool) {
        self.waiting_for_agent_clear = false;
        match result {
            Submission::Accepted(handle) => {
                self.pending = Some(handle);
                self.launching_agent = launches_agent;
                self.status
                    .set("Keyboard sequence queued", Severity::Normal);
            }
            Submission::Busy => self
                .status
                .set("Keyboard busy; use Y to stop", Severity::Warning),
            Submission::UsbUnavailable => self
                .status
                .set("USB keyboard unavailable", Severity::Warning),
            Submission::Invalid => self
                .status
                .set("Invalid keyboard preset", Severity::Warning),
        }
    }

    fn activate(&mut self, presets: &[Preset], agent_present: bool) -> Option<Intent> {
        if self.pending.is_some() {
            self.status
                .set("Keyboard busy; use Y to stop", Severity::Warning);
            return None;
        }
        if self.agent_deadline_ms.is_some() {
            self.status.set(
                "Installation pending; wait for completion",
                Severity::Warning,
            );
            return None;
        }
        self.waiting_for_agent_clear = false;
        let preset = presets.get(self.selected.0)?;
        if !preset.available {
            self.status
                .set("Agent binary missing from USB image", Severity::Warning);
            None
        } else if preset.launches_agent && agent_present {
            if preset.action != PresetAction::Keyboard {
                self.waiting_for_agent_clear = true;
                self.status.set(
                    "Agent recently seen; wait 25s after stop",
                    Severity::Warning,
                );
            } else {
                self.status
                    .set("Agent already connected", Severity::Success);
            }
            None
        } else {
            Some(Intent::RunPreset(self.selected.0))
        }
    }

    fn update(&mut self, now_ms: u64, agent_present: bool, completion: Option<Completion>) -> bool {
        let mut changed = false;
        if let Some(result) = completion.filter(|result| self.pending == Some(result.handle)) {
            self.pending = None;
            let (text, severity) = match result.status {
                CompletionStatus::Completed if self.launching_agent => {
                    self.agent_deadline_ms = Some(result.completed_at_ms.saturating_add(15_000));
                    ("Keys sent; waiting for agent handshake", Severity::Normal)
                }
                CompletionStatus::Completed => ("Keyboard sequence completed", Severity::Success),
                CompletionStatus::Cancelled => {
                    ("Cancelled; sequence will not resume", Severity::Warning)
                }
                CompletionStatus::Rejected => ("Keyboard sequence rejected", Severity::Warning),
                CompletionStatus::UsbUnavailable => {
                    ("USB unavailable; sequence stopped", Severity::Warning)
                }
            };
            self.status.set(text, severity);
            changed = true;
        }
        if let Some(deadline) = self.agent_deadline_ms {
            if agent_present {
                self.status.set("Agent connected", Severity::Success);
                self.agent_deadline_ms = None;
                changed = true;
            } else if now_ms >= deadline {
                self.status
                    .set("Keys sent; agent not detected", Severity::Warning);
                self.agent_deadline_ms = None;
                changed = true;
            }
        }
        if self.waiting_for_agent_clear && !agent_present {
            self.waiting_for_agent_clear = false;
            self.status
                .set("Agent not detected; press X to retry", Severity::Normal);
            changed = true;
        }
        changed
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TransferState {
    #[default]
    Idle,
    Open,
    Progress,
    Finished,
    Failed,
    Aborted,
}

impl TransferState {
    pub fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Open => "open",
            Self::Progress => "progress",
            Self::Finished => "finished",
            Self::Failed => "failed",
            Self::Aborted => "aborted",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TransferSnapshot {
    pub usb_ready: bool,
    pub browser_connected: bool,
    pub simulation: bool,
    pub state: TransferState,
    pub id: u64,
    pub total_bytes: u64,
    pub received_bytes: u64,
    pub total_chunks: u32,
    pub finished_chunks: u32,
}

#[derive(Default)]
pub struct TransferModel {
    pub selected: Selection,
    pub status: Status,
    pub snapshot: TransferSnapshot,
    last_update_ms: u64,
}

impl TransferModel {
    pub(crate) fn update(&mut self, snapshot: TransferSnapshot, now_ms: u64) -> bool {
        if snapshot == self.snapshot {
            return false;
        }
        // Only byte/chunk progress is coalesced. Connection, identity and terminal
        // transitions bypass the rate limit so they cannot be hidden by it.
        let progress_only = snapshot.state == TransferState::Progress
            && TransferSnapshot {
                received_bytes: self.snapshot.received_bytes,
                finished_chunks: self.snapshot.finished_chunks,
                ..snapshot
            } == self.snapshot;
        if progress_only && now_ms.saturating_sub(self.last_update_ms) < 100 {
            return false;
        }
        self.snapshot = snapshot;
        self.last_update_ms = now_ms;
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentAction {
    Status,
    Whoami,
    Hostname,
    OpenBrowser,
    Credentials,
}

impl AgentAction {
    pub const ALL: [Self; 5] = [
        Self::Status,
        Self::Whoami,
        Self::Hostname,
        Self::OpenBrowser,
        Self::Credentials,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Status => "Agent status",
            Self::Whoami => "Run whoami",
            Self::Hostname => "Run hostname",
            Self::OpenBrowser => "Open google.com",
            Self::Credentials => "DB credentials",
        }
    }
}

#[derive(Default)]
pub struct AgentModel {
    pub selected: Selection,
    pub status: Status,
}

impl AgentModel {
    pub fn submitted(&mut self, action: AgentAction, usb_ready: bool, queued: bool) {
        let (prefix, severity) = if !usb_ready {
            ("Waiting for USB", Severity::Warning)
        } else if queued {
            ("Sent", Severity::Success)
        } else {
            ("Queue busy", Severity::Warning)
        };
        self.status.text = bounded_text(format_args!("{prefix}: {}", action.name()));
        self.status.severity = severity;
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Intent {
    RunPreset(usize),
    Agent(AgentAction),
    RequireEncryptedRelay,
}

pub struct Snapshot {
    pub now_ms: u64,
    pub keyboard: KeyboardSnapshot,
    pub agent_present: bool,
    pub transfer: TransferSnapshot,
}

#[derive(Default)]
pub struct SystemSnapshot {
    pub uptime_secs: u64,
    pub cpu_mhz: u32,
    pub temperature_c: Option<f32>,
    pub sram_static_bytes: usize,
    pub sram_total_bytes: usize,
    pub psram_status: &'static str,
    pub flash_total_bytes: usize,
    pub flash_free_bytes: usize,
}

pub struct Controller {
    pub navigation: Navigation,
    pub payloads: PayloadModel,
    pub transfer: TransferModel,
    pub agent: AgentModel,
    pub keyboard: KeyboardSnapshot,
    agent_present: bool,
}

impl Default for Controller {
    fn default() -> Self {
        Self {
            navigation: Navigation::new(
                PageId::ALL
                    .iter()
                    .position(|page| *page == PageId::System)
                    .unwrap_or_default(),
            ),
            payloads: PayloadModel::default(),
            transfer: TransferModel::default(),
            agent: AgentModel::default(),
            keyboard: KeyboardSnapshot::Unavailable,
            agent_present: false,
        }
    }
}

impl Controller {
    pub fn page(&self) -> PageId {
        PageId::ALL
            .get(self.navigation.selected.0)
            .copied()
            .unwrap_or(PageId::System)
    }

    pub fn route(&mut self, buttons: ButtonEvents, job_reserved: bool) -> Routing {
        let gesture = self.navigation.apply(&buttons, PageId::ALL.len());
        self.navigation.route(
            buttons,
            gesture,
            self.page() == PageId::Payloads,
            job_reserved,
        )
    }

    pub fn handle_input(&mut self, buttons: ButtonMask, presets: &[Preset]) -> Option<Intent> {
        let activate = buttons.contains(Button::X);
        match self.page() {
            PageId::Payloads => {
                self.payloads.selected.apply(buttons, presets.len());
                if activate {
                    self.payloads.activate(presets, self.agent_present)
                } else {
                    None
                }
            }
            PageId::Agent => {
                self.agent.selected.apply(buttons, AgentAction::ALL.len());
                if activate {
                    AgentAction::ALL
                        .get(self.agent.selected.0)
                        .copied()
                        .map(Intent::Agent)
                } else {
                    None
                }
            }
            PageId::Transfer => {
                self.transfer.selected.apply(buttons, 2);
                if !activate {
                    None
                } else if self.transfer.selected.0 == 0 {
                    self.transfer
                        .status
                        .set("Pair and start in Web UI", Severity::Warning);
                    None
                } else {
                    self.transfer
                        .status
                        .set("Encrypted relay is required", Severity::Success);
                    Some(Intent::RequireEncryptedRelay)
                }
            }
            PageId::System | PageId::Logs => None,
        }
    }

    /// Updates hidden lifecycle state too, but requests drawing only for visible changes.
    pub fn update(&mut self, snapshot: Snapshot, completion: Option<Completion>) -> bool {
        let keyboard_changed = self.keyboard != snapshot.keyboard;
        self.keyboard = snapshot.keyboard;
        self.agent_present = snapshot.agent_present;
        let payload_changed =
            self.payloads
                .update(snapshot.now_ms, snapshot.agent_present, completion);
        let transfer_changed = self.transfer.update(snapshot.transfer, snapshot.now_ms);
        match self.page() {
            PageId::Payloads => payload_changed || keyboard_changed,
            PageId::Transfer => transfer_changed,
            _ => false,
        }
    }

    pub fn view<'a>(
        &'a self,
        presets: &'a [Preset],
        ssid: &'a str,
        system: &'a SystemSnapshot,
        logs: &'a [Text],
    ) -> PageView<'a> {
        match self.page() {
            PageId::Payloads => PageView::Payloads {
                model: &self.payloads,
                presets,
                keyboard: self.keyboard,
            },
            PageId::Transfer => PageView::Transfer(&self.transfer),
            PageId::Agent => PageView::Agent(&self.agent),
            PageId::System => PageView::System {
                ssid,
                metrics: system,
            },
            PageId::Logs => PageView::Logs(logs),
        }
    }
}

/// Identity and page data travel together; mismatched render requests are impossible.
pub enum PageView<'a> {
    Payloads {
        model: &'a PayloadModel,
        presets: &'a [Preset],
        keyboard: KeyboardSnapshot,
    },
    Transfer(&'a TransferModel),
    Agent(&'a AgentModel),
    System {
        ssid: &'a str,
        metrics: &'a SystemSnapshot,
    },
    Logs(&'a [Text]),
}

impl PageView<'_> {
    pub fn id(&self) -> PageId {
        match self {
            Self::Payloads { .. } => PageId::Payloads,
            Self::Transfer(_) => PageId::Transfer,
            Self::Agent(_) => PageId::Agent,
            Self::System { .. } => PageId::System,
            Self::Logs(_) => PageId::Logs,
        }
    }
}

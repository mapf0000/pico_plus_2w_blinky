use companion_core::{Connection, Device, Job, Outcome, Snapshot, keyboard};

pub fn layout_name(id: &str) -> &str {
    match id {
        "win_en-US" => "Windows · English (US)",
        "win_en-GB" => "Windows · English (UK)",
        "win_pt-BR" => "Windows · Portuguese (Brazil)",
        "win_de-DE" => "Windows · German",
        "mac_en-GB" => "Mac · English (UK)",
        "mac_pt-BR" => "Mac · Portuguese (Brazil)",
        "mac_de-DE" => "Mac · German",
        _ => id,
    }
}

pub fn selected_device<'a>(s: &'a Snapshot, selected: Option<&str>) -> Option<&'a Device> {
    selected.and_then(|id| s.devices.iter().find(|device| device.id == id))
}

pub fn connect_blocker(
    s: &Snapshot,
    device_id: Option<&str>,
    desktop_busy: bool,
) -> Option<&'static str> {
    if s.connection == Connection::Connected {
        Some("Already connected. Disconnect before choosing another Pico.")
    } else if matches!(s.connection, Connection::Scanning | Connection::Connecting) {
        Some("Wait for discovery or connection to finish, or choose Disconnect to cancel.")
    } else if desktop_busy {
        Some("Finish the profile or local dialog operation first.")
    } else if s.pending {
        Some("Waiting for the current command to finish.")
    } else if selected_device(s, device_id).is_none() {
        Some("Scan again to find this Pico before connecting.")
    } else {
        None
    }
}

pub fn acquire_blocker(s: &Snapshot, has_profile: bool) -> Option<&'static str> {
    if s.connection != Connection::Connected {
        Some("Connect to a Pico first.")
    } else if !s.compatible() || s.capabilities.as_ref().is_some_and(|c| c.read_only) {
        Some("This firmware does not support companion control.")
    } else if !has_profile {
        Some("Disconnect, then import a private profile in Devices.")
    } else if s.control_acquired {
        Some("Control is already acquired.")
    } else if s.pending {
        Some("Waiting for the current command to finish.")
    } else {
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadinessAction {
    Devices,
    Acquire,
    EnableUsb,
}
impl ReadinessAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Devices => "Open Devices",
            Self::Acquire => "Acquire control",
            Self::EnableUsb => "Enable USB",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Readiness {
    pub message: &'static str,
    pub action: Option<ReadinessAction>,
    pub ready: bool,
}

/// Device readiness is independent of the editor. It never acquires or executes.
pub fn usb_readiness(s: &Snapshot, has_profile: bool) -> Readiness {
    let blocked = |message, action| Readiness {
        message,
        action,
        ready: false,
    };
    if s.connection != Connection::Connected {
        return blocked(
            "Connect to a Pico in Devices before using its USB keyboard.",
            Some(ReadinessAction::Devices),
        );
    }
    if !s.compatible() || s.capabilities.as_ref().is_some_and(|c| c.read_only) {
        return blocked(
            "This firmware does not support companion keyboard control.",
            Some(ReadinessAction::Devices),
        );
    }
    if !s.control_acquired {
        if !has_profile {
            return blocked(
                "Connected for status only. Disconnect and import a profile in Devices to use control.",
                Some(ReadinessAction::Devices),
            );
        }
        return blocked(
            "Connected, but control is released. Acquire control to use the USB keyboard.",
            Some(ReadinessAction::Acquire),
        );
    }
    if s.job.active() {
        return blocked(
            "A keyboard effect is active. Wait for it to finish or choose Stop.",
            None,
        );
    }
    if s.pending {
        return blocked("Waiting for the current command to finish.", None);
    }
    let Some(caps) = &s.capabilities else {
        return blocked("Waiting for device capabilities.", None);
    };
    if !caps.status.usb_enabled {
        return blocked(
            "Control acquired. Enable USB to use the keyboard on the attached computer.",
            Some(ReadinessAction::EnableUsb),
        );
    }
    if !caps.status.usb_ready {
        return blocked(
            "USB is enabled. Waiting for the USB computer to recognize the keyboard.",
            None,
        );
    }
    Readiness {
        message: "Ready to type on the computer connected to Pico USB.",
        action: None,
        ready: true,
    }
}

/// Interpret only known exception names for presentation; never use traceback
/// text for permissions or execution. Arbitrary Python errors retain their details.
pub fn script_error_advice(error: &str) -> (&'static str, &'static str) {
    let exception = error
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map(|line| {
            line.trim()
                .split_once(':')
                .map_or(line.trim(), |(name, _)| name)
        });
    match exception {
        Some("UsbUnavailable") => (
            "USB keyboard was unavailable",
            "Check control and USB readiness below, then start the script again when ready.",
        ),
        Some("DeviceDisconnected") => (
            "The Pico connection was lost",
            "Reconnect in Devices, acquire control, then start again. Effects are never replayed automatically.",
        ),
        Some("HostAgentUnavailable") => (
            "The host agent was unavailable",
            "Enable USB and start the host agent on the USB computer, then start the script again.",
        ),
        Some("EffectCancelled") => (
            "The keyboard effect was cancelled",
            "Start the script again only when you want another run.",
        ),
        Some("EffectRejected") => (
            "The keyboard effect was rejected",
            "Review the layout and effect limits in the script before starting again.",
        ),
        Some("MemoryError") => (
            "Python reached its allocation limit",
            "Reduce the script's allocations, then start again.",
        ),
        Some("SyntaxError" | "IndentationError") => (
            "Python source needs a correction",
            "Open technical details to find the source line, edit it, then start again.",
        ),
        _ if error.starts_with("Python step exceeded")
            || error.starts_with("Python startup exceeded") =>
        {
            (
                "Python exceeded its time limit",
                "Yield effects regularly; avoid long computations between yields.",
            )
        }
        _ => (
            "The script stopped with an error",
            "Review technical details and correct the script or its prerequisites before starting again.",
        ),
    }
}

pub fn script_start_allowed(
    s: &Snapshot,
    run: u64,
    active: bool,
    demo: bool,
    source_valid: bool,
) -> bool {
    if !source_valid || (s.job.active() && s.job_owner != Some(run)) {
        return false;
    }
    // Only the known built-in demo has declared USB prerequisites. Never inspect
    // arbitrary Python source to guess effects or block offline event waiters.
    if !demo {
        return true;
    }
    let usb = s.connection == Connection::Connected
        && s.compatible()
        && s.control_acquired
        && s.capabilities
            .as_ref()
            .is_some_and(|c| !c.read_only && c.status.usb_enabled && c.status.usb_ready);
    usb && ((!s.pending && !s.job.active()) || active)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Workspace {
    #[default]
    Overview,
    Devices,
    Keyboard,
    Scripts,
    Files,
    Transfers,
    Settings,
    Help,
    Activity,
}

impl Workspace {
    pub const ALL: [Self; 9] = [
        Self::Overview,
        Self::Devices,
        Self::Keyboard,
        Self::Scripts,
        Self::Files,
        Self::Transfers,
        Self::Settings,
        Self::Help,
        Self::Activity,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Devices => "Devices",
            Self::Keyboard => "Keyboard",
            Self::Scripts => "Scripts",
            Self::Files => "Files",
            Self::Transfers => "Transfers",
            Self::Settings => "Settings",
            Self::Help => "Help",
            Self::Activity => "Activity",
        }
    }
}

/// Reasons follow authoritative admission, including user operations but never
/// background refresh. The UI must not invent an authenticated-session flag.
pub fn typing_blocker(s: &Snapshot, script_active: bool, layout: &str) -> Option<&'static str> {
    if s.connection != Connection::Connected {
        Some("Connect to a Pico in Devices. You can compose text offline.")
    } else if !s.compatible() {
        Some("This firmware is incompatible with keyboard control.")
    } else if !s.control_acquired {
        Some("Acquire control using the connection toolbar.")
    } else if script_active {
        Some("Stop the Python script before sending manual text.")
    } else if s.job.active() {
        Some("Wait for the current keyboard effect, or choose Stop.")
    } else if s.pending {
        Some("Waiting for the current command to finish.")
    } else if s.capabilities.as_ref().is_some_and(|c| c.read_only) {
        Some("This device exposes a read-only service.")
    } else if !s.capabilities.as_ref().is_some_and(|c| c.status.usb_ready) {
        Some("Enable USB and wait for the USB computer to recognize the keyboard.")
    } else if !s
        .capabilities
        .as_ref()
        .is_some_and(|c| c.layouts.iter().any(|l| l == layout))
    {
        Some("Choose an input layout supported by this device.")
    } else {
        None
    }
}

pub fn job_label(job: Job) -> &'static str {
    match job {
        Job::Idle => "No keyboard effect submitted",
        Job::Queued => "Waiting to submit",
        Job::Sending => "Submitting keyboard effect",
        Job::Running => "Typing",
        Job::Cancelling => "Stopping keyboard effect",
        Job::Finished(Outcome::Completed) => "Keyboard effect completed",
        Job::Finished(Outcome::Cancelled) => "Keyboard effect cancelled",
        Job::Finished(Outcome::Rejected) => "Keyboard effect rejected",
        Job::Finished(Outcome::UsbUnavailable) => "Keyboard effect ended: USB unavailable",
        Job::Finished(Outcome::Disconnected) => "Keyboard effect ended on disconnect; no replay",
    }
}

/// Cache only the bounded validation summary, never another executable job.
#[derive(Default)]
pub struct TextValidation {
    input: Option<(String, String, u32)>,
    pub summary: ResultSummary,
}
pub type ResultSummary = Option<Result<(usize, u64), companion_core::Failure>>;

impl TextValidation {
    pub fn update(&mut self, text: &str, layout: &str, delay: u32) {
        if self
            .input
            .as_ref()
            .is_some_and(|(t, l, d)| t == text && l == layout && *d == delay)
        {
            return;
        }
        self.summary = Some(
            keyboard::text_effect(text.into(), layout, delay)
                .map(|effect| (effect.bytecode.len(), effect.estimated_duration_ms)),
        );
        self.input = Some((text.into(), layout.into(), delay));
    }
}

/// Stopping Python also cancels its owned HID effect through the supervisor.
/// An unrelated/manual effect still needs its own incarnation-scoped Cancel.
pub fn cancel_manual(s: &Snapshot, script_active: bool, run: u64) -> bool {
    s.job.active() && s.job != Job::Cancelling && (!script_active || s.job_owner != Some(run))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use companion_core::{
        Action, Client,
        mock::{LATENCY, MockTransport, Scenario},
    };

    pub fn ready() -> Snapshot {
        let mut client = Client::new(MockTransport::new(Scenario::Normal));
        client.dispatch(Action::Scan, std::time::Duration::ZERO);
        client.tick(LATENCY);
        client.dispatch(Action::Connect("mock-pico-1".into()), LATENCY);
        client.tick(LATENCY * 2);
        client.dispatch(Action::Acquire, LATENCY * 2);
        client.tick(LATENCY * 3);
        client.snapshot().clone()
    }

    #[test]
    fn connection_requires_a_current_explicit_target_for_one_or_multiple_devices() {
        let mut s = ready();
        s.connection = Connection::Disconnected;
        s.control_acquired = false;
        s.pending = false;
        assert!(!s.devices.is_empty());
        assert!(connect_blocker(&s, None, false).is_some());
        let id = s.devices[0].id.clone();
        assert!(connect_blocker(&s, Some(&id), false).is_none());
        s.devices.push(Device {
            id: "other".into(),
            name: "Another Pico".into(),
        });
        assert!(connect_blocker(&s, None, false).is_some());
        assert_eq!(
            selected_device(&s, Some("other")).unwrap().name,
            "Another Pico"
        );
        assert!(connect_blocker(&s, Some("missing"), false).is_some());
        assert!(connect_blocker(&s, Some(&id), true).is_some());
        s.refreshing = true;
        assert!(connect_blocker(&s, Some(&id), false).is_none());
        s.pending = true;
        assert!(connect_blocker(&s, Some(&id), false).is_some());
    }

    #[test]
    fn readiness_distinguishes_connect_control_usb_and_idle_user_work() {
        let mut s = ready();
        assert!(usb_readiness(&s, true).ready);
        s.refreshing = true;
        assert!(usb_readiness(&s, true).ready);
        s.control_acquired = false;
        assert_eq!(
            usb_readiness(&s, true).action,
            Some(ReadinessAction::Acquire)
        );
        assert_eq!(
            usb_readiness(&s, false).action,
            Some(ReadinessAction::Devices)
        );
        assert!(acquire_blocker(&s, false).is_some());
        assert!(acquire_blocker(&s, true).is_none());
        s.pending = true;
        assert!(acquire_blocker(&s, true).is_some());
        s.pending = false;
        s.control_acquired = true;
        s.capabilities.as_mut().unwrap().status.usb_enabled = false;
        assert_eq!(
            usb_readiness(&s, true).action,
            Some(ReadinessAction::EnableUsb)
        );
        s.capabilities.as_mut().unwrap().status.usb_enabled = true;
        s.capabilities.as_mut().unwrap().status.usb_ready = false;
        assert!(!usb_readiness(&s, true).ready);
        assert!(usb_readiness(&s, true).action.is_none());
        s.connection = Connection::Disconnected;
        assert_eq!(
            usb_readiness(&s, true).action,
            Some(ReadinessAction::Devices)
        );
    }

    #[test]
    fn only_the_declared_demo_requires_usb_before_start_and_shortcut() {
        let mut s = Snapshot::default();
        assert!(!script_start_allowed(&s, 1, false, true, true));
        assert!(script_start_allowed(&s, 1, false, false, true));
        assert!(!script_start_allowed(&s, 1, false, false, false));
        s = ready();
        assert!(script_start_allowed(&s, 1, false, true, true));
        s.control_acquired = false;
        assert!(!script_start_allowed(&s, 1, false, true, true));
        assert!(script_start_allowed(&s, 1, false, false, true));
        s.control_acquired = true;
        s.job = Job::Running;
        s.job_owner = Some(1);
        s.pending = true;
        assert!(script_start_allowed(&s, 1, true, true, true));
        assert!(!script_start_allowed(&s, 2, true, false, true));
        s.capabilities.as_mut().unwrap().status.usb_ready = false;
        assert!(!script_start_allowed(&s, 1, true, true, true));
    }

    #[test]
    fn script_faults_get_recovery_copy_without_using_arbitrary_messages_as_state() {
        let usb = "runtime: Traceback (most recent call last):\n  File \"<script>\", line 4\nUsbUnavailable: USB HID is unavailable or control is not acquired\n";
        assert_eq!(script_error_advice(usb).0, "USB keyboard was unavailable");
        assert_eq!(
            script_error_advice("runtime: Traceback\nDeviceDisconnected: lost").0,
            "The Pico connection was lost"
        );
        assert_eq!(
            script_error_advice("runtime: Traceback\nValueError: UsbUnavailable: some user text").0,
            "The script stopped with an error"
        );
        assert_eq!(
            script_error_advice("").0,
            "The script stopped with an error"
        );
        assert_eq!(
            script_error_advice("runtime: Traceback\nMemoryError\n").0,
            "Python reached its allocation limit"
        );
    }

    #[test]
    fn readiness_matches_admission_and_ignores_background_refresh() {
        let ready = ready();
        assert!(ready.can_send());
        for change in 0..9 {
            let mut s = ready.clone();
            match change {
                0 => s.connection = Connection::Disconnected,
                1 => s.control_acquired = false,
                2 => s.pending = true,
                3 => s.job = Job::Running,
                4 => s.capabilities.as_mut().unwrap().status.usb_ready = false,
                5 => s.capabilities.as_mut().unwrap().read_only = true,
                6 => s.capabilities.as_mut().unwrap().script_version = 0,
                7 => s.refreshing = true,
                _ => {}
            }
            assert_eq!(
                typing_blocker(&s, false, "win_en-US").is_none(),
                s.can_send()
            );
        }
        assert!(typing_blocker(&ready, true, "win_en-US").is_some());
        assert!(typing_blocker(&ready, false, "unsupported").is_some());
    }

    #[test]
    fn stop_preserves_script_ownership_and_cancels_unrelated_effects() {
        let mut s = ready();
        s.job = Job::Running;
        s.job_owner = Some(7);
        assert!(!cancel_manual(&s, true, 7));
        assert!(cancel_manual(&s, true, 8));
        assert!(cancel_manual(&s, false, 7));
        s.job = Job::Cancelling;
        assert!(!cancel_manual(&s, false, 7));
        s.job = Job::Finished(Outcome::Completed);
        assert!(!cancel_manual(&s, true, 8));
    }

    #[test]
    fn editing_invalidates_validation_without_changing_the_job() {
        let mut v = TextValidation::default();
        v.update("Hello", "win_en-US", 0);
        let before = v.summary;
        assert!(before.unwrap().is_ok());
        v.update("Hello", "win_en-US", 0);
        assert_eq!(v.summary, before);
        v.update("Hello", "unsupported", 0);
        assert!(v.summary.unwrap().is_err());
        v.update("Hello", "win_en-US", keyboard::MAX_INITIAL_DELAY_MS);
        assert!(v.summary.unwrap().unwrap().1 > before.unwrap().unwrap().1);
        v.update("", "win_en-US", 0);
        assert!(v.summary.unwrap().is_err());
    }
}

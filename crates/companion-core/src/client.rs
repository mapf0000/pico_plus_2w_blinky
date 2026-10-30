use crate::{
    Action, CANCEL_TIMEOUT, CONNECT_TIMEOUT, Connection, Device, Diagnostic, DiagnosticContext,
    Event, Failure, IdentityUpdate, Job, MAX_DEVICES, MAX_DIAGNOSTICS, OVERVIEW_INTERVAL,
    Operation, Outcome, REQUEST_TIMEOUT, Reply, Request, STATUS_INTERVAL, Snapshot, Tag, Transport,
    keyboard,
};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Download,
    Scan,
    Connect,
    Acquire,
    Release,
    Status,
    Overview,
    IdentityRead(bool), // true = automatic background read
    IdentitySave,
    Usb,
    Filesystem { cursor: u32, hidden: bool },
    Effect(script_protocol::EffectId),
}

impl Kind {
    fn background(self) -> bool {
        matches!(
            self,
            Self::Status | Self::Overview | Self::IdentityRead(true)
        )
    }
}

struct Pending {
    tag: Tag,
    kind: Kind,
    started: Duration,
    deadline: Duration,
}

pub struct Client<T> {
    transport: T,
    snapshot: Snapshot,
    epoch: u64,
    next_request: u64,
    pending: Option<Pending>,
    queued: Option<Action>,
    reconnect: Option<Device>,
    next_status: Duration,
    next_overview: Duration,
    now: Duration,
    context: DiagnosticContext,
    next_diagnostic: u64,
}

impl<T: Transport> Client<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            snapshot: Snapshot::default(),
            epoch: 0,
            next_request: 0,
            pending: None,
            queued: None,
            reconnect: None,
            next_status: Duration::ZERO,
            next_overview: Duration::ZERO,
            now: Duration::ZERO,
            context: DiagnosticContext::Connection,
            next_diagnostic: 0,
        }
    }

    pub fn into_transport(self) -> T {
        self.transport
    }

    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    /// Change local credentials only with no live connection or owned job.
    /// Invalidate queued UI intents and responses before replacing the connector.
    pub fn replace_transport(&mut self, replacement: T, now: Duration) -> Result<T, Failure> {
        if !matches!(
            self.snapshot.connection,
            Connection::Disconnected | Connection::Failed(_) | Connection::Scanning
        ) || self.snapshot.job.active()
        {
            return Err(Failure::NotReady);
        }
        self.now = now;
        self.disconnect(None);
        if !self.new_epoch() {
            return Err(Failure::IdExhausted);
        }
        let old = std::mem::replace(&mut self.transport, replacement);
        self.snapshot.selected = None;
        self.snapshot.devices.clear();
        self.note("Connection configuration changed; connect again");
        Ok(old)
    }

    /// Reject UI commands queued for a connection that has since been replaced.
    /// In particular, stale Send/Cancel must never affect a reconnected session.
    pub fn dispatch_for_epoch(&mut self, epoch: u64, action: Action, now: Duration) {
        if epoch == self.epoch {
            self.dispatch(action, now);
        }
    }

    pub fn dispatch(&mut self, action: Action, now: Duration) {
        self.now = now;
        self.context = match &action {
            Action::Scan => DiagnosticContext::Discovery,
            Action::Acquire | Action::Release => DiagnosticContext::Control,
            Action::SetUsbEnabled(_) | Action::ReadIdentity | Action::SaveIdentity(_) => {
                DiagnosticContext::Usb
            }
            Action::SendText { .. } | Action::SendEffect { .. } | Action::Cancel => {
                DiagnosticContext::Keyboard
            }
            _ => DiagnosticContext::Connection,
        };
        match action {
            Action::ClearDiagnostics => {
                self.snapshot.diagnostics.clear();
                self.snapshot.discarded_diagnostics = 0;
                self.changed();
            }
            Action::Disconnect => self.disconnect(None),
            Action::Reconnect => {
                if !matches!(
                    self.snapshot.connection,
                    Connection::Disconnected | Connection::Failed(_)
                ) {
                    self.reject(Failure::NotReady);
                    return;
                }
                let Some(device) = self.snapshot.selected.clone() else {
                    self.reject(Failure::NotReady);
                    return;
                };
                self.dispatch(Action::Scan, now);
                if self.snapshot.connection == Connection::Scanning {
                    self.reconnect = Some(device);
                }
            }
            Action::Cancel => self.cancel(now),
            Action::SelectPath(path) => {
                if crate::filesystem::valid_path(&path) {
                    self.snapshot.filesystem.selected = Some(path);
                    self.changed();
                } else {
                    self.reject(Failure::InvalidData);
                }
            }
            Action::Download(crate::download::Command::Cancel) => {
                if !self.snapshot.control_acquired
                    || self.snapshot.job.active()
                    || self
                        .pending
                        .as_ref()
                        .is_some_and(|p| !p.kind.background() && p.kind != Kind::Download)
                {
                    self.reject(Failure::Busy);
                    return;
                }
                self.pending = None;
                self.snapshot.pending = false;
                self.queued = None;
                self.issue(
                    Operation::Download(crate::download::Command::Cancel),
                    Kind::Download,
                    now,
                    crate::download::COMMAND_TIMEOUT,
                );
            }
            Action::CancelDirectory => self.cancel_directory(now),
            action @ Action::Browse { .. }
                if self
                    .pending
                    .as_ref()
                    .is_some_and(|p| matches!(p.kind, Kind::Filesystem { .. }))
                    && self.queued.is_none() =>
            {
                if let Action::Browse { path, .. } = &action
                    && !crate::filesystem::valid_path(path)
                {
                    self.reject(Failure::InvalidData);
                    return;
                }
                self.cancel_directory(now);
                self.queued = Some(action);
            }
            action
                if self.pending.as_ref().is_some_and(|p| p.kind.background())
                    && self.queued.is_none() =>
            {
                self.queue_behind_refresh(action)
            }
            _ if self.pending.is_some() => self.reject(Failure::QueueFull),
            Action::Download(command) => {
                if !command.valid() {
                    self.reject(Failure::InvalidData);
                    return;
                }
                if self.snapshot.job.active()
                    || !self.snapshot.control_acquired
                    || !self
                        .snapshot
                        .capabilities
                        .as_ref()
                        .is_some_and(|c| c.downloads_supported && c.status.host_agent_present)
                {
                    self.reject(Failure::NotReady);
                    return;
                }
                self.issue(
                    Operation::Download(command),
                    Kind::Download,
                    now,
                    crate::download::COMMAND_TIMEOUT,
                );
            }
            Action::Scan => {
                self.disconnect(None);
                self.snapshot.devices.clear();
                if !self.new_epoch() {
                    return;
                }
                self.snapshot.connection = Connection::Scanning;
                self.issue(Operation::Scan, Kind::Scan, now, crate::SCAN_TIMEOUT);
            }
            Action::Connect(id) => {
                let Some(device) = self
                    .snapshot
                    .devices
                    .iter()
                    .find(|device| device.id == id)
                    .cloned()
                else {
                    self.reject(Failure::InvalidData);
                    return;
                };
                self.disconnect(None);
                if !self.new_epoch() {
                    return;
                }
                self.snapshot.selected = Some(device.clone());
                self.snapshot.connection = Connection::Connecting;
                self.issue(
                    Operation::Connect(device),
                    Kind::Connect,
                    now,
                    CONNECT_TIMEOUT,
                );
            }
            Action::Acquire
            | Action::Release
            | Action::SendText { .. }
            | Action::SendEffect { .. }
                if self
                    .snapshot
                    .capabilities
                    .as_ref()
                    .is_some_and(|caps| caps.read_only) =>
            {
                self.reject(Failure::ReadOnly);
            }
            Action::Acquire if self.connected_compatible() && !self.snapshot.control_acquired => {
                self.issue(
                    Operation::Acquire,
                    Kind::Acquire,
                    now,
                    if self
                        .snapshot
                        .capabilities
                        .as_ref()
                        .is_some_and(|c| c.upload_timeout != Duration::ZERO)
                    {
                        Duration::from_secs(10)
                    } else {
                        REQUEST_TIMEOUT
                    },
                );
            }
            Action::Release if self.snapshot.control_acquired && !self.snapshot.job.active() => {
                self.issue(Operation::Release, Kind::Release, now, REQUEST_TIMEOUT);
            }
            Action::SetUsbEnabled(enabled)
                if self.snapshot.control_acquired && !self.snapshot.job.active() =>
            {
                self.issue(
                    Operation::SetUsbEnabled(enabled),
                    Kind::Usb,
                    now,
                    REQUEST_TIMEOUT,
                );
            }
            Action::SendText {
                text,
                layout,
                delay_ms,
            } => {
                if !self.snapshot.can_send() {
                    self.reject(
                        if self.connected_compatible() && self.snapshot.control_acquired {
                            Failure::UsbUnavailable
                        } else {
                            Failure::NotReady
                        },
                    );
                    return;
                }
                if !self
                    .snapshot
                    .capabilities
                    .as_ref()
                    .is_some_and(|caps| caps.layouts.contains(&layout))
                {
                    self.reject(Failure::UnsupportedLayout);
                    return;
                }
                let effect = match keyboard::text_effect(text, &layout, delay_ms) {
                    Ok(effect) => effect,
                    Err(error) => {
                        self.reject(error);
                        return;
                    }
                };
                self.send_effect(effect, 0, now);
            }
            Action::SendEffect { bytecode, owner } => {
                if owner == 0 || !self.snapshot.can_send() {
                    self.reject(
                        if self.snapshot.connection != Connection::Connected
                            || !self.snapshot.control_acquired
                        {
                            Failure::LinkLost
                        } else if !self
                            .snapshot
                            .capabilities
                            .as_ref()
                            .is_some_and(|c| c.status.usb_ready)
                        {
                            Failure::UsbUnavailable
                        } else {
                            Failure::Busy
                        },
                    );
                    return;
                }
                match keyboard::Effect::validated(&bytecode) {
                    Ok(effect) => self.send_effect(effect, owner, now),
                    Err(error) => self.reject(error),
                }
            }

            Action::Browse {
                path,
                cursor,
                hidden,
            } => {
                if !crate::filesystem::valid_path(&path) {
                    self.reject(Failure::InvalidData);
                    return;
                }
                if !self.snapshot.can_browse() {
                    self.reject(Failure::AgentUnavailable);
                    return;
                }
                if cursor != 0
                    && (cursor != self.snapshot.filesystem.cursor
                        || path != self.snapshot.filesystem.directory
                        || hidden != self.snapshot.filesystem.hidden
                        || !self.snapshot.filesystem.more)
                {
                    self.reject(Failure::InvalidData);
                    return;
                }
                self.snapshot.filesystem.loading = true;
                self.snapshot.filesystem.error = None;
                self.issue(
                    Operation::ListDirectory {
                        path,
                        cursor,
                        hidden,
                    },
                    Kind::Filesystem { cursor, hidden },
                    now,
                    Duration::from_secs(35),
                );
            }
            Action::ReadIdentity if self.snapshot.can_read_identity() => {
                self.issue(
                    Operation::ReadIdentity,
                    Kind::IdentityRead(false),
                    now,
                    REQUEST_TIMEOUT,
                );
            }
            Action::SaveIdentity(identity) => {
                if !self.snapshot.can_save_identity() {
                    self.reject(if self.snapshot.can_read_identity() {
                        if self.snapshot.identity_update
                            == IdentityUpdate::Failed(Failure::Persistence)
                        {
                            Failure::Persistence
                        } else {
                            Failure::UsbActive
                        }
                    } else {
                        Failure::NotReady
                    });
                    return;
                }
                self.issue(
                    Operation::SaveIdentity(identity),
                    Kind::IdentitySave,
                    now,
                    REQUEST_TIMEOUT,
                );
            }
            _ => self.reject(Failure::NotReady),
        }
    }

    fn send_effect(&mut self, effect: keyboard::Effect, owner: u64, now: Duration) {
        let Some(tag) = self.tag() else {
            return;
        };
        let id = script_protocol::EffectId {
            request_id: tag.request,
            process_id: tag.epoch,
            effect_id: tag.request,
        };
        let frame = match keyboard::envelope(id, &effect.bytecode) {
            Ok(frame) => frame,
            Err(error) => {
                self.reject(error);
                return;
            }
        };
        self.snapshot.job = Job::Sending;
        self.snapshot.job_owner = Some(owner);
        self.start(
            Request {
                tag,
                operation: Operation::Run(frame),
            },
            Kind::Effect(id),
            now,
            Duration::from_millis(effect.estimated_duration_ms)
                + REQUEST_TIMEOUT
                + self
                    .snapshot
                    .capabilities
                    .as_ref()
                    .map_or(Duration::ZERO, |caps| caps.upload_timeout),
        );
    }

    fn queue_behind_refresh(&mut self, mut action: Action) {
        // Exactly one intent, bounded before retention. Revalidate device state
        // when the read finishes; its result may have changed USB availability.
        match &mut action {
            Action::Download(command) if command.valid() => {}
            Action::Browse { path, .. } if crate::filesystem::valid_path(path) => {}
            Action::Acquire
            | Action::Release
            | Action::SetUsbEnabled(_)
            | Action::ReadIdentity
            | Action::SaveIdentity(_) => {}
            Action::SendEffect { bytecode, owner } => {
                if *owner == 0 || keyboard::Effect::validated(bytecode).is_err() {
                    self.reject(Failure::InvalidData);
                    return;
                }
                bytecode.shrink_to_fit();
                self.snapshot.job = Job::Queued;
                self.snapshot.job_owner = Some(*owner);
            }
            Action::SendText {
                text,
                layout,
                delay_ms,
            } => {
                if layout.len() > crate::MAX_METADATA_BYTES {
                    self.reject(Failure::UnsupportedLayout);
                    return;
                }
                if text.len() > keyboard::MAX_TEXT_BYTES {
                    self.reject(Failure::InvalidText);
                    return;
                }
                if let Err(error) = keyboard::text_effect(text.clone(), layout, *delay_ms) {
                    self.reject(error);
                    return;
                }
                text.shrink_to_fit();
                layout.shrink_to_fit();
                self.snapshot.job = Job::Queued;
                self.snapshot.job_owner = Some(0);
            }
            _ => {
                self.reject(Failure::QueueFull);
                return;
            }
        }
        self.queued = Some(action);
        self.snapshot.pending = true;
        self.note("Command waiting for the current status read");
    }

    fn connected_compatible(&self) -> bool {
        self.snapshot.connection == Connection::Connected && self.snapshot.compatible()
    }

    fn new_epoch(&mut self) -> bool {
        match self.epoch.checked_add(1) {
            Some(epoch) => {
                self.epoch = epoch;
                self.snapshot.epoch = epoch;
                true
            }
            None => {
                self.disconnect(Some(Failure::IdExhausted));
                false
            }
        }
    }

    fn tag(&mut self) -> Option<Tag> {
        match self.next_request.checked_add(1) {
            Some(request) => {
                self.next_request = request;
                Some(Tag {
                    epoch: self.epoch,
                    request,
                })
            }
            None => {
                self.disconnect(Some(Failure::IdExhausted));
                None
            }
        }
    }

    fn issue(&mut self, operation: Operation, kind: Kind, now: Duration, timeout: Duration) {
        if let Some(tag) = self.tag() {
            self.start(Request { tag, operation }, kind, now, timeout);
        }
    }

    fn start(&mut self, request: Request, kind: Kind, now: Duration, timeout: Duration) {
        let background = kind.background();
        if !background {
            self.snapshot.last_error = None;
        }
        self.pending = Some(Pending {
            tag: request.tag,
            kind,
            started: now,
            deadline: now + timeout,
        });
        self.snapshot.pending = !background;
        self.snapshot.refreshing = background;
        self.changed();
        if let Err(error) = self.transport.submit(request, now) {
            self.disconnect(Some(error));
        }
    }

    fn cancel_directory(&mut self, now: Duration) {
        if matches!(self.queued, Some(Action::Browse { .. })) {
            self.queued = None;
            self.snapshot.pending = false;
            self.note("Queued directory request cancelled");
            return;
        }
        let Some(pending) = self.pending.as_mut() else {
            return;
        };
        if !matches!(pending.kind, Kind::Filesystem { .. }) {
            return;
        }
        pending.deadline = now + crate::REQUEST_TIMEOUT;
        let request = Request {
            tag: pending.tag,
            operation: Operation::CancelDirectory,
        };
        if let Err(error) = self.transport.submit(request, now) {
            self.disconnect(Some(error));
        }
    }
    fn cancel(&mut self, now: Duration) {
        if matches!(
            self.queued,
            Some(Action::SendText { .. } | Action::SendEffect { .. })
        ) {
            self.queued = None;
            self.snapshot.pending = false;
            self.snapshot.job = Job::Finished(Outcome::Cancelled);
            self.note("Queued effect cancelled before submission");
            return;
        }
        let Some(pending) = self.pending.as_mut() else {
            self.reject(Failure::NotReady);
            return;
        };
        let Kind::Effect(id) = pending.kind else {
            self.reject(Failure::NotReady);
            return;
        };
        if self.snapshot.job == Job::Cancelling {
            return;
        }
        let frame = match keyboard::cancel_envelope(id) {
            Ok(frame) => frame,
            Err(error) => {
                self.disconnect(Some(error));
                return;
            }
        };
        let request = Request {
            tag: pending.tag,
            operation: Operation::Cancel(frame),
        };
        pending.deadline = now + CANCEL_TIMEOUT;
        self.snapshot.job = Job::Cancelling;
        self.note("Cancellation requested for the current effect");
        if let Err(error) = self.transport.submit(request, now) {
            self.disconnect(Some(error));
        }
    }

    pub fn tick(&mut self, now: Duration) {
        self.now = now;
        // Bound work even if a faulty adapter floods stale events.
        for _ in 0..32 {
            let Some(event) = self.transport.poll(now) else {
                break;
            };
            self.receive(event, now);
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| now >= pending.deadline)
        {
            self.disconnect(Some(Failure::Timeout));
            return;
        }
        if self.snapshot.can_read_identity()
            && self.pending.is_none()
            && self
                .snapshot
                .capabilities
                .as_ref()
                .is_some_and(|c| c.identity.is_none())
        {
            self.context = DiagnosticContext::Usb;
            self.issue(
                Operation::ReadIdentity,
                Kind::IdentityRead(true),
                now,
                REQUEST_TIMEOUT,
            );
        } else if self.connected_compatible()
            && self.pending.is_none()
            && self.snapshot.control_acquired
            && now >= self.next_overview
            && self
                .snapshot
                .capabilities
                .as_ref()
                .is_some_and(|c| c.overview_supported)
        {
            self.next_overview = now + OVERVIEW_INTERVAL;
            self.context = DiagnosticContext::Status;
            self.issue(
                Operation::Overview,
                Kind::Overview,
                now,
                Duration::from_secs(8),
            );
        } else if self.connected_compatible() && self.pending.is_none() && now >= self.next_status {
            self.next_status = now + STATUS_INTERVAL;
            self.context = DiagnosticContext::Status;
            self.issue(Operation::Status, Kind::Status, now, REQUEST_TIMEOUT);
        }
    }

    fn receive(&mut self, event: Event, now: Duration) {
        self.now = now;
        let (tag, reply) = match event {
            Event::Downloads { epoch, view } => {
                if epoch == self.epoch
                    && self.snapshot.connection == Connection::Connected
                    && self.snapshot.control_acquired
                {
                    self.snapshot.downloads = view;
                    self.changed();
                }
                return;
            }
            Event::Button { epoch, button } => {
                if epoch == self.epoch && self.snapshot.control_acquired && button < 4 {
                    if self.snapshot.buttons.len() == 32 {
                        self.snapshot.buttons.pop_front();
                        self.snapshot.dropped_buttons =
                            self.snapshot.dropped_buttons.saturating_add(1);
                    }
                    self.snapshot.buttons.push_back(button);
                    self.snapshot.buttons_received =
                        self.snapshot.buttons_received.saturating_add(1);
                    self.changed();
                }
                return;
            }
            Event::Lost { epoch } => {
                if epoch == self.epoch
                    && matches!(
                        self.snapshot.connection,
                        Connection::Connected | Connection::Connecting
                    )
                {
                    self.disconnect(Some(Failure::LinkLost));
                }
                return;
            }
            Event::Reply { tag, reply } => (tag, reply),
        };
        let Some(pending) = self.pending.as_ref() else {
            return;
        };
        if pending.tag != tag {
            return;
        }
        let kind = pending.kind;
        let deadline = pending.deadline;
        self.context = match kind {
            Kind::Download => DiagnosticContext::Status,
            Kind::Scan => DiagnosticContext::Discovery,
            Kind::Connect => DiagnosticContext::Connection,
            Kind::Acquire | Kind::Release => DiagnosticContext::Control,
            Kind::Status | Kind::Overview => DiagnosticContext::Status,
            Kind::Usb => DiagnosticContext::Usb,
            Kind::IdentityRead(_) | Kind::IdentitySave => DiagnosticContext::Usb,
            Kind::Effect(_) => DiagnosticContext::Keyboard,
            Kind::Filesystem { .. } => DiagnosticContext::Status,
        };
        if now >= deadline {
            self.disconnect(Some(Failure::Timeout));
            return;
        }
        match (kind, reply) {
            (Kind::Download, Reply::DownloadAccepted) => {}
            (Kind::Filesystem { cursor, hidden }, Reply::Directory(bytes)) => {
                let path = self.snapshot.filesystem.directory.clone();
                if let Err(error) =
                    self.snapshot
                        .filesystem
                        .apply(&bytes, tag.request, cursor, &path, hidden)
                {
                    self.disconnect(Some(error));
                    return;
                }
            }
            (Kind::Filesystem { .. }, Reply::DirectoryCancelled) => {
                self.snapshot.filesystem.loading = false;
                self.snapshot.filesystem.error = Some("Directory request cancelled".into());
            }
            (Kind::Scan, Reply::Devices(devices)) => {
                if devices.len() > MAX_DEVICES
                    || devices.iter().any(|device| !device.valid())
                    || devices
                        .iter()
                        .enumerate()
                        .any(|(i, device)| devices[..i].iter().any(|other| other.id == device.id))
                {
                    self.disconnect(Some(Failure::InvalidData));
                    return;
                }
                self.snapshot.devices = devices;
                self.snapshot.connection = Connection::Disconnected;
                self.note(if self.snapshot.devices.is_empty() {
                    "Scan finished: no devices found"
                } else {
                    "Scan finished: select a device to connect"
                });
            }
            (Kind::Connect, Reply::Connected(caps)) => {
                if !caps.valid() {
                    self.disconnect(Some(Failure::InvalidData));
                    return;
                }
                let compatible = caps.compatible();
                if caps.identity.is_some() {
                    self.snapshot.identity_update = IdentityUpdate::Loaded;
                    self.snapshot.identity_revision =
                        self.snapshot.identity_revision.saturating_add(1);
                }
                self.snapshot.capabilities = Some(caps);
                self.snapshot.connection = Connection::Connected;
                self.snapshot.last_status_update = Some(now);
                self.next_status = now + STATUS_INTERVAL;
                self.note(
                    if self
                        .snapshot
                        .capabilities
                        .as_ref()
                        .is_some_and(|caps| caps.read_only)
                    {
                        "Connected to read-only BLE status service"
                    } else {
                        "Connected; control has not been acquired"
                    },
                );
                if !compatible {
                    self.reject(Failure::Incompatible);
                }
            }
            (Kind::Acquire, Reply::Acquired) => {
                self.snapshot.control_acquired = true;
                self.next_overview = now;
                self.note("Control acquired");
            }
            (Kind::Release, Reply::Released) => {
                self.snapshot.control_acquired = false;
                self.snapshot.filesystem = crate::filesystem::Browser::default();
                self.snapshot.buttons.clear();
                self.note("Control released");
            }
            (Kind::Status | Kind::Usb, Reply::Status(status)) => {
                if let Some(caps) = self.snapshot.capabilities.as_mut() {
                    caps.status = status;
                }
                self.snapshot.last_status_update = Some(now);
            }
            (Kind::Overview, Reply::Overview(overview)) => {
                let value = overview.value();
                if value.script_version != script_protocol::VERSION
                    || usize::from(value.max_bytecode) < script_protocol::MAX_BYTECODE
                {
                    self.disconnect(Some(Failure::Incompatible));
                    return;
                }
                if let Some(caps) = self.snapshot.capabilities.as_mut() {
                    caps.firmware = value.firmware.into();
                    caps.script_version = value.script_version;
                    caps.overview = Some(overview);
                }
                self.snapshot.last_overview_update = Some(now);
            }
            (Kind::IdentityRead(_) | Kind::IdentitySave, Reply::Identity { identity, error }) => {
                if let Some(caps) = self.snapshot.capabilities.as_mut() {
                    caps.identity = Some(identity);
                }
                self.snapshot.identity_revision = self.snapshot.identity_revision.saturating_add(1);
                self.snapshot.identity_update = if let Some(error) = error {
                    self.reject(error);
                    IdentityUpdate::Failed(error)
                } else if kind == Kind::IdentitySave {
                    self.note("USB identity persisted and verified; applies on next USB start");
                    IdentityUpdate::Saved
                } else {
                    self.note("USB identity read from flash");
                    IdentityUpdate::Loaded
                };
            }
            (Kind::Effect(expected), Reply::Accepted(id)) if id == expected => {
                if self.snapshot.job != Job::Cancelling {
                    self.snapshot.job = Job::Running;
                }
                self.note("Effect admitted by the device");
                return;
            }
            (Kind::Effect(expected), Reply::Finished { id, outcome }) if id == expected => {
                self.snapshot.job = Job::Finished(outcome);
                self.record(
                    match outcome {
                        Outcome::Completed => "Effect completed",
                        Outcome::Cancelled => "Effect cancelled",
                        Outcome::Rejected => "Effect rejected",
                        Outcome::UsbUnavailable => "USB keyboard is unavailable",
                        Outcome::Disconnected => "Effect ended on disconnect",
                    },
                    match outcome {
                        Outcome::Rejected => Some(Failure::Rejected),
                        Outcome::UsbUnavailable => Some(Failure::UsbUnavailable),
                        _ => None,
                    },
                );
            }
            (_, Reply::Error(error)) => {
                if matches!(
                    kind,
                    Kind::Scan | Kind::Connect | Kind::Status | Kind::Overview
                ) || !matches!(
                    error,
                    Failure::Busy
                        | Failure::UsbUnavailable
                        | Failure::NotReady
                        | Failure::ReadOnly
                        | Failure::AgentUnavailable
                ) {
                    self.disconnect(Some(error));
                    return;
                }
                if matches!(kind, Kind::Effect(_)) {
                    self.snapshot.job = Job::Finished(Outcome::Rejected);
                }
                if matches!(kind, Kind::Filesystem { .. }) {
                    self.snapshot.filesystem.loading = false;
                    self.snapshot.filesystem.error = Some(error.message().into());
                }
                self.reject(error);
            }
            _ => {
                self.disconnect(Some(Failure::InvalidData));
                return;
            }
        }
        if let Some(pending) = self.pending.take() {
            self.snapshot.last_rtt = Some(now.saturating_sub(pending.started));
        }
        self.snapshot.pending = false;
        self.snapshot.refreshing = false;
        self.changed();
        if kind == Kind::Scan
            && let Some(device) = self.reconnect.take()
        {
            if self.snapshot.devices.iter().any(|d| d.id == device.id) {
                self.dispatch(Action::Connect(device.id), now);
            } else {
                self.reject(Failure::DeviceNotFound);
            }
            return;
        }
        if let Some(action) = self.queued.take() {
            if self.snapshot.job == Job::Queued {
                self.snapshot.job = Job::Idle;
            }
            self.dispatch(action, now);
        }
    }

    fn disconnect(&mut self, error: Option<Failure>) {
        self.reconnect = None;
        self.transport.close();
        self.pending = None;
        self.queued = None;
        self.snapshot.pending = false;
        self.snapshot.refreshing = false;
        self.snapshot.last_status_update = None;
        self.snapshot.last_overview_update = None;
        self.snapshot.identity_update = IdentityUpdate::Unknown;
        let downloading = self.snapshot.downloads.active();
        for item in &mut self.snapshot.downloads.items {
            if matches!(
                item.state,
                crate::download::State::Queued
                    | crate::download::State::Requested
                    | crate::download::State::Receiving
                    | crate::download::State::Verifying
            ) {
                item.state = crate::download::State::Failed;
                item.error = Some("Connection ended; download was not retried".into());
            }
        }
        self.snapshot.downloads.session = crate::download::Session::Idle;
        if downloading {
            self.snapshot.downloads.error =
                Some("Connection ended; download was not retried".into());
        }
        self.snapshot.control_acquired = false;
        self.snapshot.capabilities = None;
        self.snapshot.filesystem = crate::filesystem::Browser::default();
        self.snapshot.buttons.clear();
        self.snapshot.last_rtt = None;
        if self.snapshot.job.active() {
            self.snapshot.job = Job::Finished(Outcome::Disconnected);
        }
        self.snapshot.connection = error.map_or(Connection::Disconnected, Connection::Failed);
        self.snapshot.last_error = error;
        self.record(
            error.map_or(
                "Disconnected; no pending command will be replayed",
                Failure::message,
            ),
            error,
        );
    }

    fn reject(&mut self, error: Failure) {
        self.snapshot.last_error = Some(error);
        self.record(error.message(), Some(error));
    }

    fn note(&mut self, message: &'static str) {
        self.record(message, None);
    }

    fn record(&mut self, message: &'static str, error: Option<Failure>) {
        if self.snapshot.diagnostics.len() == MAX_DIAGNOSTICS {
            self.snapshot.diagnostics.pop_front();
            self.snapshot.discarded_diagnostics =
                self.snapshot.discarded_diagnostics.saturating_add(1);
        }
        self.next_diagnostic = self.next_diagnostic.saturating_add(1);
        self.snapshot.diagnostics.push_back(Diagnostic {
            sequence: self.next_diagnostic,
            at: self.now,
            epoch: self.epoch,
            context: self.context,
            error,
            message,
        });
        self.changed();
    }

    fn changed(&mut self) {
        self.snapshot.revision = self.snapshot.revision.saturating_add(1);
    }
}

#[cfg(test)]
mod tests;

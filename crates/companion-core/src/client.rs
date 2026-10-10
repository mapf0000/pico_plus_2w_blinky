use crate::{
    Action, CANCEL_TIMEOUT, Connection, Event, Failure, Job, MAX_DEVICES, MAX_DIAGNOSTICS,
    Operation, Outcome, REQUEST_TIMEOUT, Reply, Request, STATUS_INTERVAL, Snapshot, Tag, Transport,
    keyboard,
};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Scan,
    Connect,
    Acquire,
    Release,
    Status,
    Effect(script_protocol::EffectId),
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
    next_status: Duration,
}

impl<T: Transport> Client<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            snapshot: Snapshot::default(),
            epoch: 0,
            next_request: 0,
            pending: None,
            next_status: Duration::ZERO,
        }
    }

    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    /// Reject UI commands queued for a connection that has since been replaced.
    /// In particular, stale Send/Cancel must never affect a reconnected session.
    pub fn dispatch_for_epoch(&mut self, epoch: u64, action: Action, now: Duration) {
        if epoch == self.epoch {
            self.dispatch(action, now);
        }
    }

    pub fn dispatch(&mut self, action: Action, now: Duration) {
        match action {
            Action::Disconnect => self.disconnect(None),
            Action::Cancel => self.cancel(now),
            _ if self.pending.is_some() => self.reject(Failure::QueueFull),
            Action::Scan => {
                self.disconnect(None);
                self.snapshot.devices.clear();
                if !self.new_epoch() {
                    return;
                }
                self.snapshot.connection = Connection::Scanning;
                self.issue(Operation::Scan, Kind::Scan, now, REQUEST_TIMEOUT);
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
                    REQUEST_TIMEOUT,
                );
            }
            Action::Acquire if self.connected_compatible() && !self.snapshot.control_acquired => {
                self.issue(Operation::Acquire, Kind::Acquire, now, REQUEST_TIMEOUT);
            }
            Action::Release if self.snapshot.control_acquired && !self.snapshot.job.active() => {
                self.issue(Operation::Release, Kind::Release, now, REQUEST_TIMEOUT);
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
                self.start(
                    Request {
                        tag,
                        operation: Operation::Run(frame),
                    },
                    Kind::Effect(id),
                    now,
                    Duration::from_millis(effect.estimated_duration_ms) + REQUEST_TIMEOUT,
                );
            }
            _ => self.reject(Failure::NotReady),
        }
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
        self.snapshot.last_error = None;
        self.pending = Some(Pending {
            tag: request.tag,
            kind,
            started: now,
            deadline: now + timeout,
        });
        self.snapshot.pending = true;
        self.changed();
        if let Err(error) = self.transport.submit(request, now) {
            self.disconnect(Some(error));
        }
    }

    fn cancel(&mut self, now: Duration) {
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
        if self.connected_compatible() && self.pending.is_none() && now >= self.next_status {
            self.next_status = now + STATUS_INTERVAL;
            self.issue(Operation::Status, Kind::Status, now, REQUEST_TIMEOUT);
        }
    }

    fn receive(&mut self, event: Event, now: Duration) {
        let (tag, reply) = match event {
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
        match (kind, reply) {
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
                self.snapshot.capabilities = Some(caps);
                self.snapshot.connection = Connection::Connected;
                self.next_status = now + STATUS_INTERVAL;
                self.note("Connected; control has not been acquired");
                if !compatible {
                    self.reject(Failure::Incompatible);
                }
            }
            (Kind::Acquire, Reply::Acquired) => {
                self.snapshot.control_acquired = true;
                self.note("Mock control acquired; pairing is simulated");
            }
            (Kind::Release, Reply::Released) => {
                self.snapshot.control_acquired = false;
                self.note("Control released");
            }
            (Kind::Status, Reply::Status(status)) => {
                if let Some(caps) = self.snapshot.capabilities.as_mut() {
                    caps.status = status;
                }
            }
            (Kind::Effect(expected), Reply::Accepted(id)) if id == expected => {
                if self.snapshot.job != Job::Cancelling {
                    self.snapshot.job = Job::Running;
                }
                self.note("Effect admitted by the mock executor");
                return;
            }
            (Kind::Effect(expected), Reply::Finished { id, outcome }) if id == expected => {
                self.snapshot.job = Job::Finished(outcome);
                self.note(match outcome {
                    Outcome::Completed => "Effect completed (simulated)",
                    Outcome::Cancelled => "Effect cancelled (simulated)",
                    Outcome::Rejected => "Effect rejected",
                    Outcome::UsbUnavailable => "USB keyboard is unavailable",
                    Outcome::Disconnected => "Effect ended on disconnect",
                });
            }
            (_, Reply::Error(error)) => {
                if matches!(kind, Kind::Scan | Kind::Connect) {
                    self.disconnect(Some(error));
                    return;
                }
                if matches!(kind, Kind::Effect(_)) {
                    self.snapshot.job = Job::Finished(Outcome::Rejected);
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
        self.changed();
    }

    fn disconnect(&mut self, error: Option<Failure>) {
        self.transport.close();
        self.pending = None;
        self.snapshot.pending = false;
        self.snapshot.control_acquired = false;
        self.snapshot.capabilities = None;
        self.snapshot.last_rtt = None;
        if self.snapshot.job.active() {
            self.snapshot.job = Job::Finished(Outcome::Disconnected);
        }
        self.snapshot.connection = error.map_or(Connection::Disconnected, Connection::Failed);
        self.snapshot.last_error = error;
        self.note(error.map_or(
            "Disconnected; no pending command will be replayed",
            Failure::message,
        ));
    }

    fn reject(&mut self, error: Failure) {
        self.snapshot.last_error = Some(error);
        self.note(error.message());
    }

    fn note(&mut self, message: &'static str) {
        if self.snapshot.diagnostics.len() == MAX_DIAGNOSTICS {
            self.snapshot.diagnostics.pop_front();
            self.snapshot.discarded_diagnostics =
                self.snapshot.discarded_diagnostics.saturating_add(1);
        }
        self.snapshot.diagnostics.push_back(message);
        self.changed();
    }

    fn changed(&mut self) {
        self.snapshot.revision = self.snapshot.revision.saturating_add(1);
    }
}

#[cfg(test)]
mod tests;

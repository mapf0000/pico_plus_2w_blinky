//! Deterministic in-memory transport. Never creates USB reports or accesses a radio.

use crate::{
    Capabilities, Device, Event, Failure, Operation, Outcome, Reply, Request, Status, Tag,
    Transport, layouts,
};
use std::{collections::VecDeque, time::Duration};

pub const LATENCY: Duration = Duration::from_millis(80);
const MAX_EVENTS: usize = 16;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scenario {
    #[default]
    Normal,
    Empty,
    PermissionDenied,
    Busy,
    UsbUnavailable,
    Incompatible,
    Timeout,
    LinkLoss,
}

impl Scenario {
    pub const ALL: [Self; 8] = [
        Self::Normal,
        Self::Empty,
        Self::PermissionDenied,
        Self::Busy,
        Self::UsbUnavailable,
        Self::Incompatible,
        Self::Timeout,
        Self::LinkLoss,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Empty => "empty",
            Self::PermissionDenied => "permission-denied",
            Self::Busy => "busy",
            Self::UsbUnavailable => "usb-unavailable",
            Self::Incompatible => "incompatible",
            Self::Timeout => "timeout",
            Self::LinkLoss => "link-loss",
        }
    }
}

impl std::str::FromStr for Scenario {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|scenario| scenario.name() == value)
            .ok_or_else(|| format!("Unknown mock scenario: {value}"))
    }
}

struct Scheduled {
    due: Duration,
    event: Event,
}
struct Active {
    tag: Tag,
    id: script_protocol::EffectId,
}

pub struct MockTransport {
    scenario: Scenario,
    events: VecDeque<Scheduled>,
    connected: bool,
    acquired: bool,
    active: Option<Active>,
}

impl MockTransport {
    pub fn new(scenario: Scenario) -> Self {
        Self {
            scenario,
            events: VecDeque::new(),
            connected: false,
            acquired: false,
            active: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn events_for_test_clear(&mut self) {
        self.events.clear();
    }

    fn status(&self) -> Status {
        Status {
            usb_enabled: true,
            uptime_secs: None,
            usb_ready: self.scenario != Scenario::UsbUnavailable,
            host_agent_present: true,
        }
    }

    fn schedule(&mut self, due: Duration, event: Event) -> Result<(), Failure> {
        if self.events.len() == MAX_EVENTS {
            return Err(Failure::QueueFull);
        }
        let index = self
            .events
            .iter()
            .position(|event| event.due > due)
            .unwrap_or(self.events.len());
        self.events.insert(index, Scheduled { due, event });
        Ok(())
    }

    fn reply(&mut self, tag: Tag, now: Duration, reply: Reply) -> Result<(), Failure> {
        self.schedule(now + LATENCY, Event::Reply { tag, reply })
    }

    fn run(&mut self, tag: Tag, frame: &[u8], now: Duration) -> Result<(), Failure> {
        let script_protocol::Message::Run { id, bytecode } =
            script_protocol::decode(frame).map_err(|_| Failure::InvalidData)?
        else {
            return Err(Failure::InvalidData);
        };
        if id.request_id != tag.request || id.process_id != tag.epoch || id.effect_id != tag.request
        {
            return Err(Failure::InvalidData);
        }
        if !self.connected || !self.acquired {
            return self.reply(tag, now, Reply::Error(Failure::NotReady));
        }
        if self.active.is_some() {
            return self.reply(tag, now, Reply::Error(Failure::Busy));
        }
        let valid = firmware_exec::validate_bytecode(bytecode).is_ok();
        if !valid || !self.status().usb_ready {
            let outcome = if valid {
                Outcome::UsbUnavailable
            } else {
                Outcome::Rejected
            };
            return self.reply(tag, now, Reply::Finished { id, outcome });
        }
        let duration_ms = duration_ms(bytecode)?;
        self.active = Some(Active { tag, id });
        self.reply(tag, now, Reply::Accepted(id))?;
        if self.scenario == Scenario::LinkLoss {
            self.schedule(now + LATENCY * 2, Event::Lost { epoch: tag.epoch })?;
        } else {
            self.schedule(
                now + LATENCY + Duration::from_millis(duration_ms),
                Event::Reply {
                    tag,
                    reply: Reply::Finished {
                        id,
                        outcome: Outcome::Completed,
                    },
                },
            )?;
        }
        Ok(())
    }
}

/// Read actual validated KBD1 delays/taps so cancellation tests have real time
/// to run. This estimates execution timing; it does not emulate USB scheduling.
fn duration_ms(bytecode: &[u8]) -> Result<u64, Failure> {
    use keyboard_core::bytecode::{OP_DELAY, OP_END, OP_TAP, Reader};
    let mut reader = Reader::new(bytecode).map_err(|_| Failure::InvalidData)?;
    let mut duration = 0;
    loop {
        match reader.read_u8().map_err(|_| Failure::InvalidData)? {
            OP_DELAY => {
                duration += u64::from(reader.read_varu32().map_err(|_| Failure::InvalidData)?)
            }
            OP_TAP => {
                reader.read_u8().map_err(|_| Failure::InvalidData)?;
                reader.read_u8().map_err(|_| Failure::InvalidData)?;
                duration += 20;
            }
            OP_END => return Ok(duration),
            _ => return Err(Failure::InvalidData),
        }
    }
}

impl Transport for MockTransport {
    fn submit(&mut self, request: Request, now: Duration) -> Result<(), Failure> {
        let tag = request.tag;
        match request.operation {
            Operation::Scan => {
                let reply = match self.scenario {
                    Scenario::PermissionDenied => Reply::Error(Failure::PermissionDenied),
                    Scenario::Empty => Reply::Devices(Vec::new()),
                    _ => Reply::Devices(vec![Device {
                        id: "mock-pico-1".into(),
                        name: "Pico Plus 2 W (simulated)".into(),
                    }]),
                };
                self.reply(tag, now, reply)
            }
            Operation::Connect(device) => {
                if device.id != "mock-pico-1" {
                    return Err(Failure::InvalidData);
                }
                if self.scenario == Scenario::Timeout {
                    return Ok(());
                }
                self.connected = true;
                self.reply(
                    tag,
                    now,
                    Reply::Connected(Capabilities {
                        read_only: false,
                        script_version: if self.scenario == Scenario::Incompatible {
                            script_protocol::VERSION + 1
                        } else {
                            script_protocol::VERSION
                        },
                        firmware: "mock / no radio or USB".into(),
                        layouts: layouts().iter().map(|layout| (*layout).into()).collect(),
                        status: self.status(),
                    }),
                )
            }
            Operation::Acquire if self.connected => {
                if self.scenario == Scenario::Busy {
                    return self.reply(tag, now, Reply::Error(Failure::Busy));
                }
                self.acquired = true;
                self.reply(tag, now, Reply::Acquired)
            }
            Operation::Release if self.connected && self.active.is_none() => {
                self.acquired = false;
                self.reply(tag, now, Reply::Released)
            }
            Operation::Status if self.connected => {
                self.reply(tag, now, Reply::Status(self.status()))
            }
            Operation::Run(frame) => self.run(tag, &frame, now),
            Operation::Cancel(frame) => {
                let script_protocol::Message::Cancel { id } =
                    script_protocol::decode(&frame).map_err(|_| Failure::InvalidData)?
                else {
                    return Err(Failure::InvalidData);
                };
                if !self
                    .active
                    .as_ref()
                    .is_some_and(|active| active.id == id && active.tag == tag)
                {
                    return Err(Failure::NotReady);
                }
                self.events.retain(|event| !matches!(&event.event, Event::Reply { tag: other, reply: Reply::Finished { .. } } if *other == tag));
                self.reply(
                    tag,
                    now,
                    Reply::Finished {
                        id,
                        outcome: Outcome::Cancelled,
                    },
                )
            }
            _ => self.reply(tag, now, Reply::Error(Failure::NotReady)),
        }
    }

    fn poll(&mut self, now: Duration) -> Option<Event> {
        if !self.events.front().is_some_and(|event| event.due <= now) {
            return None;
        }
        let event = self.events.pop_front()?.event;
        if matches!(&event, Event::Reply { tag, reply: Reply::Finished { .. } } if self.active.as_ref().is_some_and(|active| active.tag == *tag))
        {
            self.active = None;
        }
        Some(event)
    }

    fn close(&mut self) {
        self.events.clear();
        self.active = None;
        self.connected = false;
        self.acquired = false;
    }
}

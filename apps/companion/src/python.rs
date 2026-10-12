//! Lazy supervised interpreter process and local effect scheduler. No VM runs here.
use companion_core::{Action, Client, Connection, Job, Outcome, Snapshot, Transport};
use python_runtime::{
    Effect, StepResult,
    ipc::{self, Command, Input, Response},
};
use serde_json::{Value, json};
use std::{collections::VecDeque, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStdin, ChildStdout},
    sync::{mpsc, watch},
};

pub const DEMO: &str = "layout(\"win_en-US\")\n\ndef main():\n    yield require_usb()\n    yield sleep(5000)\n    yield text(\"Hello from Python!\", 20)\n";
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum State {
    #[default]
    Idle,
    Starting,
    Computing,
    Sleeping,
    WaitingEvent,
    WaitingDevice,
    Completed,
    Stopped,
    Faulted,
}
impl State {
    pub fn active(self) -> bool {
        matches!(
            self,
            Self::Starting
                | Self::Computing
                | Self::Sleeping
                | Self::WaitingEvent
                | Self::WaitingDevice
        )
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Starting => "Starting Python",
            Self::Computing => "Running Python",
            Self::Sleeping => "Sleeping",
            Self::WaitingEvent => "Waiting for an event",
            Self::WaitingDevice => "Waiting for keyboard completion",
            Self::Completed => "Completed",
            Self::Stopped => "Stopped",
            Self::Faulted => "Faulted",
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct ScriptSnapshot {
    pub revision: u64,
    pub run: u64,
    pub state: State,
    pub error: Option<String>,
    pub layout: Option<String>,
    pub heap_peak: usize,
    pub completed_effects: u64,
}
struct Work {
    run: u64,
    step: u64,
    input: Input,
}
enum EngineEvent {
    Response(Response),
    Fault { run: u64, message: &'static str },
}
struct Engine {
    commands: mpsc::Sender<Work>,
    reset: watch::Sender<u64>,
    events: mpsc::Receiver<EngineEvent>,
    task: tokio::task::JoinHandle<()>,
}
struct Process {
    child: Child,
    input: ChildStdin,
    output: ChildStdout,
}
impl Process {
    fn spawn() -> std::io::Result<Self> {
        let mut command = tokio::process::Command::new(std::env::current_exe()?);
        command
            .arg("--python-worker")
            .env_clear()
            .current_dir(std::env::temp_dir())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command.spawn()?;
        let input = child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("Missing Python stdin"))?;
        let output = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("Missing Python stdout"))?;
        Ok(Self {
            child,
            input,
            output,
        })
    }
    async fn exchange(&mut self, work: Work) -> Result<Response, &'static str> {
        let run = work.run;
        let step = work.step;
        let bytes = ipc::encode(&Command {
            version: ipc::VERSION,
            run,
            step,
            input: work.input,
        })
        .map_err(|_| "Python command exceeds IPC limits")?;
        let result = async {
            self.input.write_u32_le(bytes.len() as u32).await?;
            self.input.write_all(&bytes).await?;
            self.input.flush().await?;
            let n = self.output.read_u32_le().await? as usize;
            if n == 0 || n > ipc::FRAME_MAX {
                return Err(std::io::Error::other("Invalid Python IPC length"));
            }
            let mut bytes = vec![0; n];
            self.output.read_exact(&mut bytes).await?;
            serde_json::from_slice::<Response>(&bytes).map_err(std::io::Error::other)
        }
        .await
        .map_err(|_| "Python process ended unexpectedly (memory limit, crash or invalid IPC)")?;
        if result.version != ipc::VERSION
            || result.run != run
            || result.step != step
            || result.heap_peak > ipc::HEAP_LIMIT
            || !result.result.valid()
        {
            return Err("Invalid Python response correlation");
        }
        Ok(result)
    }
    async fn stop(&mut self) {
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(2), self.child.wait()).await;
    }
}
impl Engine {
    fn new() -> Self {
        let (commands, mut rx) = mpsc::channel::<Work>(2);
        let (reset, mut reset_rx) = watch::channel(0u64);
        let (tx, events) = mpsc::channel(4);
        let task = tokio::spawn(async move {
            let mut process: Option<Process> = None;
            loop {
                let work = tokio::select! { biased;
                    changed=reset_rx.changed()=>{
                        if let Some(mut p)=process.take(){p.stop().await;}
                        if changed.is_err(){break;}
                        reset_rx.borrow_and_update(); continue;
                    }
                    work=rx.recv()=>match work{Some(w)=>w,None=>break},
                };
                if work.run != *reset_rx.borrow() {
                    continue;
                }
                let run = work.run;
                let initial = matches!(work.input, Input::Start { .. });
                if initial {
                    if let Some(mut p) = process.take() {
                        p.stop().await;
                    }
                    match Process::spawn() {
                        Ok(p) => process = Some(p),
                        Err(_) => {
                            let _ = tx.try_send(EngineEvent::Fault {
                                run,
                                message: "Could not start Python child process",
                            });
                            continue;
                        }
                    }
                }
                let Some(p) = process.as_mut() else {
                    continue;
                };
                let deadline = if initial {
                    Duration::from_secs(30)
                } else {
                    Duration::from_millis(500)
                };
                let result = tokio::select! { biased;
                    changed=reset_rx.changed()=>{
                        if let Some(mut p)=process.take(){p.stop().await;}
                        if changed.is_err(){break;}
                        reset_rx.borrow_and_update();continue;
                    }
                    result=tokio::time::timeout(deadline,p.exchange(work))=>result.unwrap_or(Err(if initial {"Python startup exceeded 30 seconds"} else {"Python step exceeded 500 ms"})),
                };
                let terminal = matches!(
                    &result,
                    Ok(Response {
                        result: StepResult::Completed {} | StepResult::Error { .. },
                        ..
                    })
                ) || result.is_err();
                if terminal && let Some(mut p) = process.take() {
                    p.stop().await;
                }
                let event = match result {
                    Ok(r) => EngineEvent::Response(r),
                    Err(message) => EngineEvent::Fault { run, message },
                };
                if tx.try_send(event).is_err() {
                    break;
                }
            }
            if let Some(mut p) = process {
                p.stop().await;
            }
        });
        Self {
            commands,
            reset,
            events,
            task,
        }
    }
    async fn shutdown(self) {
        drop(self.commands);
        drop(self.reset);
        let _ = self.task.await;
    }
}
enum Stage {
    Idle,
    Computing {
        step: u64,
    },
    Timer {
        at: Duration,
    },
    Event {
        kinds: Vec<String>,
        at: Option<Duration>,
    },
    Device {
        epoch: u64,
        submitted: bool,
        bytecode: Option<Vec<u8>>,
        at: Duration,
    },
}
#[derive(Clone, Copy)]
struct Observed {
    epoch: u64,
    connected: bool,
    controlled: bool,
    usb: bool,
    agent: bool,
    buttons: u64,
}
impl Observed {
    fn new(s: &Snapshot) -> Self {
        let connected = s.connection == Connection::Connected;
        let controlled = connected && s.control_acquired;
        let status = s.capabilities.as_ref().map(|c| c.status);
        Self {
            epoch: s.epoch,
            connected,
            controlled,
            usb: controlled && status.is_some_and(|s| s.usb_ready),
            agent: controlled && status.is_some_and(|s| s.host_agent_present),
            buttons: s.buttons_received,
        }
    }
}
pub struct Supervisor {
    engine: Engine,
    pub snapshot: ScriptSnapshot,
    stage: Stage,
    observed: Option<Observed>,
    buttons: VecDeque<Value>,
    step: u64,
}
impl Supervisor {
    pub fn new() -> Self {
        Self {
            engine: Engine::new(),
            snapshot: ScriptSnapshot::default(),
            stage: Stage::Idle,
            observed: None,
            buttons: VecDeque::new(),
            step: 0,
        }
    }
    fn changed(&mut self) {
        self.snapshot.revision = self.snapshot.revision.saturating_add(1);
    }
    fn fault(&mut self, message: String) {
        self.engine.reset.send_replace(0);
        self.stage = Stage::Idle;
        self.buttons.clear();
        self.snapshot.state = State::Faulted;
        self.snapshot.error = Some(message);
        self.changed();
    }
    pub fn stop<T: Transport>(&mut self, client: &mut Client<T>, now: Duration) {
        if client.snapshot().job_owner == Some(self.snapshot.run) && client.snapshot().job.active()
        {
            client.dispatch(Action::Cancel, now);
        }
        self.engine.reset.send_replace(0);
        self.stage = Stage::Idle;
        self.buttons.clear();
        self.snapshot.state = State::Stopped;
        self.snapshot.error = None;
        self.changed();
    }
    pub fn start<T: Transport>(&mut self, source: String, client: &mut Client<T>, now: Duration) {
        self.stop(client, now);
        if let Some(error) = python_runtime::source_limit_error(&source) {
            self.fault(error);
            return;
        }
        let Some(run) = self.snapshot.run.checked_add(1) else {
            self.fault("Python run counter exhausted".into());
            return;
        };
        self.snapshot = ScriptSnapshot {
            revision: self.snapshot.revision + 1,
            run,
            state: State::Starting,
            ..Default::default()
        };
        self.buttons.clear();
        self.observed = Some(Observed::new(client.snapshot()));
        self.step = 0;
        self.engine.reset.send_replace(run);
        self.send(Input::Start { source });
    }
    fn send(&mut self, input: Input) {
        let Some(step) = self.step.checked_add(1) else {
            self.fault("Python step counter exhausted".into());
            return;
        };
        self.step = step;
        if self
            .engine
            .commands
            .try_send(Work {
                run: self.snapshot.run,
                step,
                input,
            })
            .is_err()
        {
            self.fault("Python command queue is full".into());
            return;
        }
        self.stage = Stage::Computing { step };
        if step != 1 {
            self.snapshot.state = State::Computing;
        }
        self.changed();
    }
    fn resume(&mut self, value: Value) {
        self.send(Input::Resume { value });
    }
    fn raise(&mut self, code: &str, message: &str) {
        self.send(Input::Raise {
            code: code.into(),
            message: message.into(),
        });
    }
    fn event(&mut self, kind: &str, value: Value) {
        if matches!(&self.stage,Stage::Event{kinds,..} if kinds.iter().any(|k|k==kind)) {
            self.resume(value);
        } else if kind == "button" {
            if self.buttons.len() == 32 {
                self.buttons.pop_front();
            }
            self.buttons.push_back(value);
        }
    }
    fn effect(&mut self, effect: Effect, now: Duration, observed: Observed) {
        match effect {
            Effect::Keyboard { bytecode } => {
                if companion_core::keyboard::Effect::validated(&bytecode).is_err() {
                    self.fault("Invalid Python keyboard effect".into());
                    return;
                }
                self.stage = Stage::Device {
                    epoch: observed.epoch,
                    submitted: false,
                    bytecode: Some(bytecode),
                    at: now + Duration::from_secs(360),
                };
                self.snapshot.state = State::WaitingDevice;
                self.changed();
            }
            Effect::Sleep { milliseconds } => {
                self.stage = Stage::Timer {
                    at: now + Duration::from_millis(u64::from(milliseconds)),
                };
                self.snapshot.state = State::Sleeping;
                self.changed();
            }
            Effect::WaitEvent { kinds, timeout_ms } => {
                if kinds.is_empty() || kinds.len() > 8 || kinds.iter().any(|k| k.len() > 4096) {
                    self.fault("Invalid Python event wait".into());
                    return;
                }
                if kinds.iter().any(|k| k == "button")
                    && let Some(button) = self.buttons.pop_front()
                {
                    self.resume(button);
                    return;
                }
                self.stage = Stage::Event {
                    kinds,
                    at: timeout_ms.map(|ms| now + Duration::from_millis(u64::from(ms))),
                };
                self.snapshot.state = State::WaitingEvent;
                self.changed();
            }
            Effect::RequireUsb {} => {
                if observed.usb {
                    self.resume(Value::Null);
                } else {
                    self.raise(
                        "usb_unavailable",
                        "USB HID is unavailable or control is not acquired",
                    );
                }
            }
            Effect::RequireHostAgent {} => {
                if observed.agent {
                    self.resume(Value::Null);
                } else {
                    self.raise("host_agent_unavailable", "Host agent is unavailable");
                }
            }
        }
    }
    pub fn tick<T: Transport>(&mut self, client: &mut Client<T>, now: Duration) {
        let current = Observed::new(client.snapshot());
        while let Ok(event) = self.engine.events.try_recv() {
            match event {
                EngineEvent::Fault { run, message }
                    if run == self.snapshot.run && self.snapshot.state.active() =>
                {
                    self.fault(message.into())
                }
                EngineEvent::Response(response)
                    if response.run == self.snapshot.run
                        && matches!(self.stage,Stage::Computing{step} if step==response.step) =>
                {
                    self.snapshot.heap_peak = response.heap_peak;
                    match response.result {
                        StepResult::Completed {} => {
                            self.stage = Stage::Idle;
                            self.snapshot.state = State::Completed;
                            self.changed();
                        }
                        StepResult::Error { phase, message } => {
                            self.fault(format!("{phase}: {message}"))
                        }
                        StepResult::Effect { layout, effect } => {
                            if let Some(layout) = layout {
                                if layout.len() > 96
                                    || !companion_core::layouts().contains(&layout.as_str())
                                {
                                    self.fault("Unsupported Python keyboard layout".into());
                                    continue;
                                }
                                self.snapshot.layout = Some(layout);
                            }
                            self.effect(effect, now, current);
                        }
                    }
                }
                _ => {}
            }
        }
        if !self.snapshot.state.active() {
            return;
        }
        if let Some(previous) = self.observed {
            if previous.epoch != current.epoch || !current.controlled {
                self.buttons.clear();
            }
            if previous.connected != current.connected {
                self.event(
                    "connection",
                    json!({"kind":"connection","connected":current.connected}),
                );
            }
            if previous.usb != current.usb {
                self.event("usb", json!({"kind":"usb","ready":current.usb}));
            }
            if previous.agent != current.agent {
                self.event(
                    "host_agent",
                    json!({"kind":"host_agent","ready":current.agent}),
                );
            }
            if current.controlled && previous.epoch == current.epoch {
                let added = current.buttons.saturating_sub(previous.buttons).min(32) as usize;
                let buttons: Vec<u8> = client
                    .snapshot()
                    .buttons
                    .iter()
                    .rev()
                    .take(added)
                    .copied()
                    .collect();
                for button in buttons.into_iter().rev() {
                    self.event("button",json!({"kind":"button","button":(["A","B","X","Y"][button as usize]),"edge":"released"}));
                }
            }
        }
        self.observed = Some(current);
        match &mut self.stage {
            Stage::Timer { at } if now >= *at => self.resume(json!({"kind":"timer"})),
            Stage::Event { at: Some(at), .. } if now >= *at => {
                self.resume(json!({"kind":"timer","timeout":true}))
            }
            Stage::Device {
                epoch,
                submitted,
                bytecode,
                at,
            } => {
                if current.epoch != *epoch || !current.controlled {
                    self.raise(
                        "device_disconnected",
                        "Device disconnected or control released; keyboard output was not retried",
                    );
                } else if !current.usb {
                    if *submitted {
                        client.dispatch(Action::Cancel, now);
                    }
                    self.raise("usb_unavailable", "USB HID is unavailable");
                } else if now >= *at {
                    if *submitted {
                        client.dispatch(Action::Cancel, now);
                    }
                    self.raise(
                        "effect_cancelled",
                        "Keyboard effect completion deadline expired",
                    );
                } else if !*submitted {
                    if client.snapshot().can_send() {
                        let bytes = bytecode
                            .take()
                            .expect("unsubmitted device effect owns its bounded bytecode");
                        client.dispatch(
                            Action::SendEffect {
                                bytecode: bytes,
                                owner: self.snapshot.run,
                            },
                            now,
                        );
                        if client.snapshot().job_owner == Some(self.snapshot.run)
                            && client.snapshot().job.active()
                        {
                            *submitted = true;
                        } else {
                            self.raise(
                                "effect_rejected",
                                "Keyboard effect was rejected before submission",
                            );
                        }
                    }
                } else if client.snapshot().job_owner == Some(self.snapshot.run)
                    && let Job::Finished(outcome) = client.snapshot().job
                {
                    self.snapshot.completed_effects =
                        self.snapshot.completed_effects.saturating_add(1);
                    match outcome {
                        Outcome::Completed => self.resume(Value::Null),
                        Outcome::Cancelled => {
                            self.raise("effect_cancelled", "Keyboard effect was cancelled")
                        }
                        Outcome::UsbUnavailable => {
                            self.raise("usb_unavailable", "USB HID is unavailable")
                        }
                        Outcome::Disconnected => self.raise(
                            "device_disconnected",
                            "Device disconnected; keyboard output was not retried",
                        ),
                        Outcome::Rejected => {
                            self.raise("effect_rejected", "Firmware rejected the keyboard effect")
                        }
                    }
                }
            }
            _ => {}
        }
    }
    pub async fn shutdown<T: Transport>(mut self, client: &mut Client<T>, now: Duration) {
        self.stop(client, now);
        self.engine.shutdown().await;
    }
}

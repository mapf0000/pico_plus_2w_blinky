use gloo_timers::callback::Timeout;
use serde::{Deserialize, Serialize};
use std::{cell::RefCell, collections::VecDeque, rc::Rc};
use wasm_bindgen::{JsCast, closure::Closure};
use web_sys::{ErrorEvent, MessageEvent, Worker, WorkerOptions, WorkerType};

use crate::api;

const WORKER_PROTOCOL_VERSION: u8 = 1;
const STEP_TIMEOUT_MS: u32 = 500;
const START_TIMEOUT_MS: u32 = 30_000;
const DEVICE_EFFECT_TIMEOUT_MS: u32 = 360_000;

thread_local! {
    static ACTIVE: RefCell<Option<Supervisor>> = const { RefCell::new(None) };
    static NEXT_PROCESS_SEQUENCE: std::cell::Cell<u64> = const { std::cell::Cell::new(1) };
    static USB_READY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static HOST_AGENT_READY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProcessState {
    Stopped,
    Starting,
    Running,
    Waiting,
    Faulted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProcessSnapshot {
    pub state: ProcessState,
    pub message: String,
}

type StatusCallback = Rc<dyn Fn(ProcessSnapshot)>;

struct Supervisor {
    worker: Worker,
    process_id: String,
    process_id_number: u64,
    step_id: u64,
    outstanding: Option<script_protocol::EffectId>,
    deadline: Option<Timeout>,
    device_deadline: Option<Timeout>,
    local_wait: Option<Timeout>,
    wait_kinds: Option<Vec<String>>,
    event_queue: VecDeque<serde_json::Value>,
    status: StatusCallback,
    _onmessage: Closure<dyn FnMut(MessageEvent)>,
    _onerror: Closure<dyn FnMut(ErrorEvent)>,
}

#[derive(Debug, Deserialize)]
struct WorkerResponse {
    version: u8,
    #[serde(rename = "type")]
    message_type: String,
    #[serde(default)]
    process_id: Option<String>,
    #[serde(default)]
    step_id: Option<u64>,
    #[serde(default)]
    effect: Option<Effect>,
    #[serde(default)]
    phase: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Effect {
    Keyboard {
        bytecode: Vec<u8>,
    },
    Sleep {
        milliseconds: u32,
    },
    WaitEvent {
        kinds: Vec<String>,
        timeout_ms: Option<u32>,
    },
    RequireUsb,
    RequireHostAgent,
}

#[derive(Serialize)]
struct WorkerCommand<'a, T: Serialize> {
    version: u8,
    #[serde(rename = "type")]
    message_type: &'a str,
    process_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    step_id: Option<u64>,
    #[serde(flatten)]
    body: T,
}

#[derive(Serialize)]
struct StartBody<'a> {
    source: &'a str,
}

#[derive(Serialize)]
struct ResumeBody {
    value: serde_json::Value,
}

#[derive(Serialize)]
struct RaiseBody<'a> {
    error_code: &'a str,
    message: &'a str,
}

#[derive(Deserialize)]
struct DeviceResult {
    event_type: String,
    version: u8,
    request_id: String,
    process_id: String,
    effect_id: String,
    status: String,
}

#[derive(Deserialize)]
struct DeviceEvent {
    event_type: String,
    version: u8,
    event: serde_json::Value,
}

pub(crate) fn start(
    source: String,
    status: impl Fn(ProcessSnapshot) + 'static,
) -> Result<(), String> {
    stop();
    // Keep IDs stable within a process but unlikely to collide with a stale
    // device result or cancellation after a page reload. Date.now() is exact
    // at millisecond precision in a JavaScript number; reserving 12 sequence
    // bits keeps the combined value below JavaScript's 53-bit integer limit.
    let process_id_number = NEXT_PROCESS_SEQUENCE.with(|next| {
        let sequence = next.get() & 0x0fff;
        next.set(sequence.wrapping_add(1));
        ((js_sys::Date::now() as u64) << 12) | sequence
    });
    let process_id = format!("{process_id_number:016x}");
    let status: StatusCallback = Rc::new(status);
    let options = WorkerOptions::new();
    options.set_type(WorkerType::Module);
    let worker = Worker::new_with_options("/ui/python-worker.js", &options)
        .map_err(|_| "could not create the RustPython Worker".to_owned())?;

    let onmessage = Closure::wrap(Box::new(move |event: MessageEvent| {
        let Ok(text) = js_sys::JSON::stringify(&event.data()) else {
            fault("Worker returned a non-serializable message");
            return;
        };
        let Some(text) = text.as_string() else {
            fault("Worker returned an invalid message");
            return;
        };
        match serde_json::from_str::<WorkerResponse>(&text) {
            Ok(response) => handle_worker_response(response),
            Err(_) => fault("Worker returned malformed JSON"),
        }
    }) as Box<dyn FnMut(_)>);
    let onerror = Closure::wrap(Box::new(move |_event: ErrorEvent| {
        fault("RustPython Worker crashed");
    }) as Box<dyn FnMut(_)>);
    worker.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
    worker.set_onerror(Some(onerror.as_ref().unchecked_ref()));

    ACTIVE.with(|active| {
        active.replace(Some(Supervisor {
            worker,
            process_id: process_id.clone(),
            process_id_number,
            step_id: 0,
            outstanding: None,
            deadline: None,
            device_deadline: None,
            local_wait: None,
            wait_kinds: None,
            event_queue: VecDeque::with_capacity(32),
            status: status.clone(),
            _onmessage: onmessage,
            _onerror: onerror,
        }));
    });
    (status)(ProcessSnapshot {
        state: ProcessState::Starting,
        message: "Loading compressed RustPython runtime…".into(),
    });
    post_command(
        WorkerCommand {
            version: WORKER_PROTOCOL_VERSION,
            message_type: "start",
            process_id: &process_id,
            step_id: None,
            body: StartBody { source: &source },
        },
        START_TIMEOUT_MS,
    )
}

pub(crate) fn stop() {
    ACTIVE.with(|active| {
        if let Some(supervisor) = active.borrow_mut().take() {
            if let Some(id) = supervisor.outstanding {
                let _ = api::cancel_script_effect(id);
            }
            supervisor.worker.terminate();
            (supervisor.status)(ProcessSnapshot {
                state: ProcessState::Stopped,
                message: "Stopped".into(),
            });
        }
    });
}

pub(crate) fn connection_changed(connected: bool) {
    if connected {
        resume_waiting_event(
            "connection",
            serde_json::json!({"kind":"connection","connected":true}),
        );
        return;
    }
    ACTIVE.with(|active| {
        let mut slot = active.borrow_mut();
        let Some(supervisor) = slot.as_mut() else {
            return;
        };
        if supervisor.outstanding.take().is_some() {
            supervisor.device_deadline.take();
            send_raise(
                supervisor,
                "device_disconnected",
                "device disconnected; effect outcome is unknown and was not retried",
            );
        } else {
            drop(slot);
            resume_waiting_event(
                "connection",
                serde_json::json!({"kind":"connection","connected":false}),
            );
        }
    });
}

pub(crate) fn capabilities_changed(usb_ready: bool, host_agent_ready: bool) {
    let usb_changed = USB_READY.with(|value| value.replace(usb_ready)) != usb_ready;
    let host_changed =
        HOST_AGENT_READY.with(|value| value.replace(host_agent_ready)) != host_agent_ready;
    if usb_changed {
        let interrupted = !usb_ready
            && ACTIVE.with(|active| {
                let mut slot = active.borrow_mut();
                let Some(supervisor) = slot.as_mut() else {
                    return false;
                };
                if supervisor.outstanding.take().is_some() {
                    supervisor.device_deadline.take();
                    send_raise(
                        supervisor,
                        "usb_unavailable",
                        "USB disconnected; effect outcome is unknown and was not retried",
                    );
                    true
                } else {
                    false
                }
            });
        if !interrupted {
            resume_waiting_event("usb", serde_json::json!({"kind":"usb","ready":usb_ready}));
        }
    }
    if host_changed {
        resume_waiting_event(
            "host_agent",
            serde_json::json!({"kind":"host_agent","ready":host_agent_ready}),
        );
    }
}

pub(crate) fn handle_device_event(text: &str) {
    if let Ok(event) = serde_json::from_str::<DeviceEvent>(text)
        && event.event_type == "script/event"
        && event.version == 1
        && let Some(kind) = event
            .event
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    {
        resume_waiting_event(&kind, event.event);
        return;
    }
    let Ok(result) = serde_json::from_str::<DeviceResult>(text) else {
        return;
    };
    if result.event_type != "script/effect_result" || result.version != 1 {
        return;
    }
    ACTIVE.with(|active| {
        let mut slot = active.borrow_mut();
        let Some(supervisor) = slot.as_mut() else {
            return;
        };
        let Some(expected) = supervisor.outstanding else {
            return;
        };
        if result.request_id != format!("{:016x}", expected.request_id)
            || result.process_id != format!("{:016x}", expected.process_id)
            || result.effect_id != format!("{:016x}", expected.effect_id)
        {
            return;
        }
        supervisor.outstanding = None;
        supervisor.device_deadline.take();
        match result.status.as_str() {
            "completed" => send_resume(supervisor, serde_json::Value::Null),
            "cancelled" => send_raise(
                supervisor,
                "effect_cancelled",
                "device effect was cancelled",
            ),
            "usb_unavailable" => {
                send_raise(supervisor, "usb_unavailable", "USB HID is unavailable")
            }
            _ => send_raise(
                supervisor,
                "effect_rejected",
                "device rejected the keyboard effect",
            ),
        }
    });
}

fn handle_worker_response(response: WorkerResponse) {
    if response.version != WORKER_PROTOCOL_VERSION {
        fault("Worker protocol version mismatch");
        return;
    }
    ACTIVE.with(|active| {
        let mut slot = active.borrow_mut();
        let Some(supervisor) = slot.as_mut() else {
            return;
        };
        if response.process_id.as_deref() != Some(supervisor.process_id.as_str()) {
            return;
        }
        supervisor.deadline.take();
        match response.message_type.as_str() {
            "effect" => {
                let (Some(step_id), Some(effect)) = (response.step_id, response.effect) else {
                    drop(slot);
                    fault("Worker effect response is incomplete");
                    return;
                };
                if step_id != supervisor.step_id + 1 {
                    drop(slot);
                    fault("Worker effect step is out of order");
                    return;
                }
                supervisor.step_id = step_id;
                dispatch_effect(supervisor, effect);
            }
            "stopped" => {
                let status = supervisor.status.clone();
                let reason = response.reason.unwrap_or_else(|| "completed".into());
                supervisor.worker.terminate();
                slot.take();
                (status)(ProcessSnapshot {
                    state: ProcessState::Stopped,
                    message: reason,
                });
            }
            "python_error" | "internal_error" => {
                let status = supervisor.status.clone();
                let message = match response.phase {
                    Some(phase) => format!("{phase}: {}", response.message.unwrap_or_default()),
                    None => response.message.unwrap_or_else(|| "Worker failed".into()),
                };
                supervisor.worker.terminate();
                slot.take();
                (status)(ProcessSnapshot {
                    state: ProcessState::Faulted,
                    message,
                });
            }
            _ => {
                drop(slot);
                fault("Worker returned an unknown message type");
            }
        }
    });
}

fn dispatch_effect(supervisor: &mut Supervisor, effect: Effect) {
    match effect {
        Effect::Sleep { milliseconds } => {
            (supervisor.status)(ProcessSnapshot {
                state: ProcessState::Waiting,
                message: format!("Sleeping for {milliseconds} ms"),
            });
            let process_id = supervisor.process_id.clone();
            let step_id = supervisor.step_id;
            supervisor.local_wait = Some(Timeout::new(milliseconds, move || {
                resume_if_current(&process_id, step_id, serde_json::json!({"kind":"timer"}));
            }));
        }
        Effect::WaitEvent { kinds, timeout_ms } => {
            (supervisor.status)(ProcessSnapshot {
                state: ProcessState::Waiting,
                message: format!("Waiting for {}", kinds.join(", ")),
            });
            supervisor.wait_kinds = Some(kinds);
            if let Some(index) = supervisor.event_queue.iter().position(|event| {
                event
                    .get("kind")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|kind| {
                        supervisor
                            .wait_kinds
                            .as_ref()
                            .is_some_and(|kinds| kinds.iter().any(|candidate| candidate == kind))
                    })
            }) && let Some(event) = supervisor.event_queue.remove(index)
            {
                send_resume(supervisor, event);
                return;
            }
            if let Some(milliseconds) = timeout_ms {
                let process_id = supervisor.process_id.clone();
                let step_id = supervisor.step_id;
                supervisor.local_wait = Some(Timeout::new(milliseconds, move || {
                    resume_if_current(
                        &process_id,
                        step_id,
                        serde_json::json!({"kind":"timer","timeout":true}),
                    );
                }));
            }
        }
        Effect::RequireUsb => {
            if USB_READY.with(std::cell::Cell::get) {
                send_resume(supervisor, serde_json::Value::Null);
            } else {
                send_raise(supervisor, "usb_unavailable", "USB HID is unavailable");
            }
        }
        Effect::RequireHostAgent => {
            if HOST_AGENT_READY.with(std::cell::Cell::get) {
                send_resume(supervisor, serde_json::Value::Null);
            } else {
                send_raise(
                    supervisor,
                    "host_agent_unavailable",
                    "host-agent is unavailable",
                );
            }
        }
        Effect::Keyboard { bytecode } => match keyboard_core::bytecode::validate(&bytecode) {
            Ok(()) => {
                let id = script_protocol::EffectId {
                    request_id: supervisor.step_id,
                    process_id: supervisor.process_id_number,
                    effect_id: supervisor.step_id,
                };
                match api::send_script_effect(id, &bytecode) {
                    Ok(()) => {
                        supervisor.outstanding = Some(id);
                        let process_id = supervisor.process_id.clone();
                        let effect_id = id.effect_id;
                        supervisor.device_deadline =
                            Some(Timeout::new(DEVICE_EFFECT_TIMEOUT_MS, move || {
                                device_effect_timeout(&process_id, effect_id)
                            }));
                        (supervisor.status)(ProcessSnapshot {
                            state: ProcessState::Running,
                            message: "Keyboard effect is running".into(),
                        });
                    }
                    Err(_) => send_raise(
                        supervisor,
                        "device_disconnected",
                        "device is disconnected; effect was not sent",
                    ),
                }
            }
            Err(error) => send_raise(
                supervisor,
                "effect_rejected",
                &format!("Worker returned invalid keyboard bytecode: {error:?}"),
            ),
        },
    }
}

fn post_command<T: Serialize>(
    command: WorkerCommand<'_, T>,
    timeout_ms: u32,
) -> Result<(), String> {
    let json = serde_json::to_string(&command)
        .map_err(|_| "could not encode Worker command".to_owned())?;
    let value =
        js_sys::JSON::parse(&json).map_err(|_| "could not convert Worker command".to_owned())?;
    ACTIVE.with(|active| {
        let mut slot = active.borrow_mut();
        let supervisor = slot
            .as_mut()
            .ok_or_else(|| "process is not running".to_owned())?;
        supervisor
            .worker
            .post_message(&value)
            .map_err(|_| "could not send command to Worker".to_owned())?;
        let process_id = supervisor.process_id.clone();
        supervisor.deadline = Some(Timeout::new(timeout_ms, move || {
            timeout_process(&process_id);
        }));
        Ok(())
    })
}

fn send_resume(supervisor: &mut Supervisor, value: serde_json::Value) {
    supervisor.local_wait.take();
    supervisor.wait_kinds.take();
    let process_id = supervisor.process_id.clone();
    let command = WorkerCommand {
        version: WORKER_PROTOCOL_VERSION,
        message_type: "resume",
        process_id: &process_id,
        step_id: Some(supervisor.step_id),
        body: ResumeBody { value },
    };
    send_direct(supervisor, &command);
}

fn send_raise(supervisor: &mut Supervisor, code: &str, message: &str) {
    supervisor.local_wait.take();
    supervisor.wait_kinds.take();
    let process_id = supervisor.process_id.clone();
    let command = WorkerCommand {
        version: WORKER_PROTOCOL_VERSION,
        message_type: "raise",
        process_id: &process_id,
        step_id: Some(supervisor.step_id),
        body: RaiseBody {
            error_code: code,
            message,
        },
    };
    send_direct(supervisor, &command);
}

fn send_direct<T: Serialize>(supervisor: &mut Supervisor, command: &WorkerCommand<'_, T>) {
    let result = serde_json::to_string(command)
        .ok()
        .and_then(|json| js_sys::JSON::parse(&json).ok())
        .and_then(|value| supervisor.worker.post_message(&value).ok());
    if result.is_none() {
        let process_id = supervisor.process_id.clone();
        wasm_bindgen_futures::spawn_local(async move {
            fault_if_current(&process_id, "could not resume RustPython")
        });
        return;
    }
    let process_id = supervisor.process_id.clone();
    supervisor.deadline = Some(Timeout::new(STEP_TIMEOUT_MS, move || {
        timeout_process(&process_id);
    }));
}

fn resume_if_current(process_id: &str, step_id: u64, value: serde_json::Value) {
    ACTIVE.with(|active| {
        let mut slot = active.borrow_mut();
        let Some(supervisor) = slot.as_mut() else {
            return;
        };
        if supervisor.process_id == process_id && supervisor.step_id == step_id {
            send_resume(supervisor, value);
        }
    });
}

fn resume_waiting_event(kind: &str, value: serde_json::Value) {
    ACTIVE.with(|active| {
        let mut slot = active.borrow_mut();
        let Some(supervisor) = slot.as_mut() else {
            return;
        };
        let accepts = supervisor
            .wait_kinds
            .as_ref()
            .is_some_and(|kinds| kinds.iter().any(|candidate| candidate == kind));
        if accepts {
            send_resume(supervisor, value);
        } else if kind == "button" {
            if supervisor.event_queue.len() == 32 {
                supervisor.event_queue.pop_front();
            }
            supervisor.event_queue.push_back(value);
        }
    });
}

fn timeout_process(process_id: &str) {
    fault_if_current(process_id, "Python step exceeded its execution deadline");
}

fn device_effect_timeout(process_id: &str, effect_id: u64) {
    ACTIVE.with(|active| {
        let mut slot = active.borrow_mut();
        let Some(supervisor) = slot.as_mut() else {
            return;
        };
        if supervisor.process_id != process_id
            || supervisor
                .outstanding
                .is_none_or(|id| id.effect_id != effect_id)
        {
            return;
        }
        if let Some(id) = supervisor.outstanding.take() {
            let _ = api::cancel_script_effect(id);
        }
        supervisor.device_deadline.take();
        send_raise(
            supervisor,
            "effect_cancelled",
            "device effect exceeded its completion deadline and was cancelled",
        );
    });
}

fn fault(message: &str) {
    let process_id = ACTIVE.with(|active| {
        active
            .borrow()
            .as_ref()
            .map(|supervisor| supervisor.process_id.clone())
    });
    if let Some(process_id) = process_id {
        fault_if_current(&process_id, message);
    }
}

fn fault_if_current(process_id: &str, message: &str) {
    ACTIVE.with(|active| {
        let mut slot = active.borrow_mut();
        if slot
            .as_ref()
            .is_none_or(|item| item.process_id != process_id)
        {
            return;
        }
        if let Some(supervisor) = slot.take() {
            supervisor.worker.terminate();
            (supervisor.status)(ProcessSnapshot {
                state: ProcessState::Faulted,
                message: message.to_owned(),
            });
        }
    });
}

use rustpython_vm::{
    AsObject, Interpreter, PyObjectRef, VirtualMachine, compiler::Mode, py_serde, scope::Scope,
};
use serde::{Deserialize, Serialize, de::IntoDeserializer};
use std::collections::HashMap;
use wasm_bindgen::prelude::*;

const MAX_SOURCE_BYTES: usize = 32 * 1024;
const MAX_SOURCE_LINES: usize = 1_024;
const MAX_SOURCE_NESTING: usize = 128;
const MAX_DIAGNOSTIC_BYTES: usize = 8 * 1024;
const MAX_TEXT_CHARS: usize = 1_024;
const MAX_EVENT_KINDS: usize = 8;
const MAX_STRING_BYTES: usize = 4 * 1024;
const MAX_DEVICE_DELAY_MS: u32 = 5_000;
const MAX_LOCAL_TIMER_MS: u32 = i32::MAX as u32;

const PRELUDE: &str = r#"
class PicoError(Exception):
    pass

class DeviceDisconnected(PicoError):
    pass

class UsbUnavailable(PicoError):
    pass

class HostAgentUnavailable(PicoError):
    pass

class EffectRejected(PicoError):
    pass

class EffectCancelled(PicoError):
    pass

class _PicoEffect:
    __slots__ = ("payload",)

    def __init__(self, payload):
        self.payload = payload

_layout_id = None

def _string(value, name):
    if not isinstance(value, str):
        raise TypeError(name + " must be a string")
    return value

def _milliseconds(value, name, maximum):
    if not isinstance(value, int) or isinstance(value, bool):
        raise TypeError(name + " must be an integer")
    if value < 0 or value > maximum:
        raise ValueError(name + " is outside the supported range")
    return value

def layout(layout_id):
    global _layout_id
    layout_id = _string(layout_id, "layout id")
    if _layout_id is not None:
        raise PicoError("layout() may only be called once")
    _layout_id = layout_id

def tap(key):
    return _PicoEffect({"kind": "tap", "key": _string(key, "key")})

def modtap(chord):
    return _PicoEffect({"kind": "modtap", "chord": _string(chord, "chord")})

def text(value, delay_ms=10):
    return _PicoEffect({
        "kind": "text",
        "value": _string(value, "text"),
        "delay_ms": _milliseconds(delay_ms, "delay_ms", 5000),
    })

def sleep(milliseconds):
    return _PicoEffect({
        "kind": "sleep",
        "milliseconds": _milliseconds(milliseconds, "milliseconds", 2147483647),
    })

def wait_event(*kinds, timeout_ms=None):
    if not kinds:
        raise ValueError("wait_event() needs at least one event kind")
    checked = [_string(kind, "event kind") for kind in kinds]
    if timeout_ms is not None:
        timeout_ms = _milliseconds(timeout_ms, "timeout_ms", 2147483647)
    return _PicoEffect({
        "kind": "wait_event",
        "kinds": checked,
        "timeout_ms": timeout_ms,
    })

def require_usb():
    return _PicoEffect({"kind": "require_usb"})

def require_host_agent():
    return _PicoEffect({"kind": "require_host_agent"})
"#;

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum RawEffect {
    Tap {
        key: String,
    },
    Modtap {
        chord: String,
    },
    Text {
        value: String,
        delay_ms: u32,
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

#[derive(Debug, Serialize)]
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
#[serde(tag = "status", rename_all = "snake_case")]
enum StepResult {
    Effect {
        #[serde(skip_serializing_if = "Option::is_none")]
        layout: Option<String>,
        effect: Effect,
    },
    Completed,
    Error {
        phase: &'static str,
        message: String,
    },
}

fn json_result(result: StepResult) -> String {
    serde_json::to_string(&result).unwrap_or_else(|_| {
        r#"{"status":"error","phase":"internal","message":"response encoding failed"}"#.to_owned()
    })
}

fn bounded_message(mut message: String) -> String {
    if message.len() <= MAX_DIAGNOSTIC_BYTES {
        return message;
    }
    let mut end = MAX_DIAGNOSTIC_BYTES;
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    message.truncate(end);
    message.push_str("\n[diagnostic truncated]");
    message
}

fn render_exception(
    vm: &VirtualMachine,
    error: &rustpython_vm::builtins::PyBaseExceptionRef,
) -> String {
    let mut rendered = String::new();
    if vm.write_exception(&mut rendered, error).is_err() || rendered.is_empty() {
        rendered = "Python execution failed".to_owned();
    }
    bounded_message(rendered)
}

fn source_limit_error(source: &str) -> Option<String> {
    if source.len() > MAX_SOURCE_BYTES {
        return Some(format!("source exceeds the {MAX_SOURCE_BYTES}-byte limit"));
    }
    if source.lines().count() > MAX_SOURCE_LINES {
        return Some(format!("source exceeds the {MAX_SOURCE_LINES}-line limit"));
    }

    let mut nesting = 0usize;
    let mut maximum = 0usize;
    let mut quote = None;
    let mut escaped = false;
    let mut comment = false;
    for ch in source.chars() {
        if comment {
            if ch == '\n' {
                comment = false;
            }
            continue;
        }
        if let Some(delimiter) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == delimiter {
                quote = None;
            }
            continue;
        }
        match ch {
            '#' => comment = true,
            '\'' | '"' => quote = Some(ch),
            '(' | '[' | '{' => {
                nesting += 1;
                maximum = maximum.max(nesting);
            }
            ')' | ']' | '}' => nesting = nesting.saturating_sub(1),
            _ => {}
        }
    }
    (maximum > MAX_SOURCE_NESTING)
        .then(|| format!("source nesting exceeds the {MAX_SOURCE_NESTING}-level limit"))
}

fn validate_effect(effect: &RawEffect) -> Result<(), String> {
    let bounded_string = |value: &str, name: &str| {
        if value.len() > MAX_STRING_BYTES {
            Err(format!("{name} exceeds the {MAX_STRING_BYTES}-byte limit"))
        } else {
            Ok(())
        }
    };
    match effect {
        RawEffect::Tap { key } => bounded_string(key, "key"),
        RawEffect::Modtap { chord } => bounded_string(chord, "chord"),
        RawEffect::Text { value, delay_ms } => {
            bounded_string(value, "text")?;
            if value.chars().count() > MAX_TEXT_CHARS {
                return Err(format!("text exceeds the {MAX_TEXT_CHARS}-character limit"));
            }
            if *delay_ms > MAX_DEVICE_DELAY_MS {
                return Err(format!(
                    "delay_ms exceeds the {MAX_DEVICE_DELAY_MS}-millisecond limit"
                ));
            }
            Ok(())
        }
        RawEffect::Sleep { milliseconds } => {
            if *milliseconds > MAX_LOCAL_TIMER_MS {
                Err(format!(
                    "sleep exceeds the {MAX_LOCAL_TIMER_MS}-millisecond browser timer limit"
                ))
            } else {
                Ok(())
            }
        }
        RawEffect::WaitEvent { kinds, timeout_ms } => {
            if kinds.is_empty() || kinds.len() > MAX_EVENT_KINDS {
                return Err(format!(
                    "wait_event needs between 1 and {MAX_EVENT_KINDS} event kinds"
                ));
            }
            for kind in kinds {
                bounded_string(kind, "event kind")?;
            }
            if timeout_ms.is_some_and(|milliseconds| milliseconds > MAX_LOCAL_TIMER_MS) {
                return Err(format!(
                    "timeout_ms exceeds the {MAX_LOCAL_TIMER_MS}-millisecond browser timer limit"
                ));
            }
            Ok(())
        }
        RawEffect::RequireUsb | RawEffect::RequireHostAgent => Ok(()),
    }
}

fn remove_unsafe_builtins(vm: &VirtualMachine) {
    for name in [
        "open",
        "__import__",
        "eval",
        "exec",
        "compile",
        "input",
        "print",
    ] {
        let _ = vm.builtins.dict().del_item(name, vm);
    }
}

fn parse_effect(
    vm: &VirtualMachine,
    yielded: PyObjectRef,
    effect_class: &PyObjectRef,
) -> Result<RawEffect, String> {
    if !yielded
        .is_instance(effect_class, vm)
        .map_err(|error| render_exception(vm, &error))?
    {
        return Err("main() yielded a value that is not a Pico effect".to_owned());
    }
    let payload = vm
        .get_attribute_opt(yielded, "payload")
        .map_err(|error| render_exception(vm, &error))?
        .ok_or_else(|| "effect payload is missing".to_owned())?;
    let value = serde_json::to_value(py_serde::PyObjectSerializer::new(vm, &payload))
        .map_err(|error| bounded_message(format!("invalid effect payload: {error}")))?;
    let effect: RawEffect = serde_json::from_value(value)
        .map_err(|error| bounded_message(format!("invalid effect payload: {error}")))?;
    validate_effect(&effect)?;
    Ok(effect)
}

fn prepare_effect(layout: &str, effect: RawEffect) -> Result<Effect, String> {
    use core::str::FromStr as _;
    use keyboard_core::{FlatProgram, Mods, OpOwned, ProgramOwned};

    let mut flat = FlatProgram::new();
    match effect {
        RawEffect::Tap { key } => {
            let usage = keyboard_core::parse_key(&key)
                .ok_or_else(|| bounded_message(format!("unknown key {key}")))?;
            flat.push_tap(usage, Mods::empty())
                .map_err(|_| "keyboard effect has too many operations".to_owned())?;
        }
        RawEffect::Modtap { chord } => {
            let (mods, usage) = keyboard_core::parse_modtap(&chord)
                .map_err(|_| bounded_message(format!("invalid modifier chord {chord}")))?;
            flat.push_tap(usage, mods)
                .map_err(|_| "keyboard effect has too many operations".to_owned())?;
        }
        RawEffect::Text { value, delay_ms } => {
            let layout = keyboard_core::LayoutId::from_str(layout)
                .map_err(|_| bounded_message(format!("unsupported keyboard layout {layout}")))?;
            let mut program = ProgramOwned::new();
            program.ops.push(OpOwned::Layout(layout));
            program.ops.push(OpOwned::Text {
                s: value,
                delay_ms: delay_ms as u16,
            });
            flat =
                keyboard_core::lower_to_flat(&program).map_err(|error| error.message.to_owned())?;
        }
        RawEffect::Sleep { milliseconds } => return Ok(Effect::Sleep { milliseconds }),
        RawEffect::WaitEvent { kinds, timeout_ms } => {
            return Ok(Effect::WaitEvent { kinds, timeout_ms });
        }
        RawEffect::RequireUsb => return Ok(Effect::RequireUsb),
        RawEffect::RequireHostAgent => return Ok(Effect::RequireHostAgent),
    }
    let bytecode = keyboard_core::bytecode::encode(&flat)
        .map_err(|_| "keyboard effect could not be encoded".to_owned())?;
    keyboard_core::bytecode::validate(&bytecode)
        .map_err(|error| bounded_message(format!("invalid keyboard effect: {error:?}")))?;
    Ok(Effect::Keyboard { bytecode })
}

fn step_result(
    vm: &VirtualMachine,
    result: rustpython_vm::PyResult,
    effect_class: &PyObjectRef,
    layout: Option<String>,
    keyboard_layout: &str,
) -> StepResult {
    match result {
        Ok(yielded) => match parse_effect(vm, yielded, effect_class) {
            Ok(effect) => match prepare_effect(keyboard_layout, effect) {
                Ok(effect) => StepResult::Effect { layout, effect },
                Err(message) => StepResult::Error {
                    phase: "effect",
                    message: bounded_message(message),
                },
            },
            Err(message) => StepResult::Error {
                phase: "effect",
                message: bounded_message(message),
            },
        },
        Err(error) if error.fast_isinstance(vm.ctx.exceptions.stop_iteration) => {
            StepResult::Completed
        }
        Err(error) => StepResult::Error {
            phase: "runtime",
            message: render_exception(vm, &error),
        },
    }
}

/// A single cooperative Python generator running inside the real RustPython VM.
///
/// The object is intended to live in a dedicated browser Worker. The browser
/// supervisor must terminate that Worker when a synchronous method exceeds its
/// deadline; RustPython cannot cooperatively interrupt a tight loop on wasm.
#[wasm_bindgen]
pub struct PythonProcess {
    interpreter: Interpreter,
    scope: Scope,
    effect_class: PyObjectRef,
    error_classes: HashMap<&'static str, PyObjectRef>,
    generator: Option<PyObjectRef>,
    layout: Option<String>,
}

#[wasm_bindgen]
impl PythonProcess {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Result<PythonProcess, JsValue> {
        let interpreter = Interpreter::without_stdlib(Default::default());
        let initialized = interpreter.enter(|vm| {
            let scope = vm.new_scope_with_builtins();
            let code = vm
                .compile(PRELUDE, Mode::Exec, "<pico-prelude>".to_owned())
                .map_err(|error| vm.new_syntax_error(&error, Some(PRELUDE)))?;
            vm.run_code_obj(code, scope.clone())?;
            let effect_class = scope
                .globals
                .get_item_opt("_PicoEffect", vm)?
                .ok_or_else(|| vm.new_runtime_error("internal effect class is missing"))?;
            let mut error_classes = HashMap::new();
            for name in [
                "DeviceDisconnected",
                "UsbUnavailable",
                "HostAgentUnavailable",
                "EffectRejected",
                "EffectCancelled",
            ] {
                let class = scope
                    .globals
                    .get_item_opt(name, vm)?
                    .ok_or_else(|| vm.new_runtime_error("internal exception class is missing"))?;
                error_classes.insert(name, class);
            }
            remove_unsafe_builtins(vm);
            Ok::<_, rustpython_vm::builtins::PyBaseExceptionRef>((
                scope,
                effect_class,
                error_classes,
            ))
        });
        let (scope, effect_class, error_classes) =
            initialized.map_err(|error| JsValue::from_str(&format!("{error:?}")))?;
        Ok(Self {
            interpreter,
            scope,
            effect_class,
            error_classes,
            generator: None,
            layout: None,
        })
    }

    /// Compile the user module, call `main()`, and run it to the first yield.
    pub fn start(&mut self, source: &str) -> String {
        if self.generator.is_some() {
            return json_result(StepResult::Error {
                phase: "protocol",
                message: "this process has already been started".to_owned(),
            });
        }
        if let Some(message) = source_limit_error(source) {
            return json_result(StepResult::Error {
                phase: "limits",
                message,
            });
        }

        let scope = self.scope.clone();
        let effect_class = self.effect_class.clone();
        let outcome = self.interpreter.enter(|vm| {
            let code = match vm.compile(source, Mode::Exec, "<script>".to_owned()) {
                Ok(code) => code,
                Err(error) => {
                    return StepResult::Error {
                        phase: "syntax",
                        message: render_exception(vm, &vm.new_syntax_error(&error, Some(source))),
                    };
                }
            };
            if let Err(error) = vm.run_code_obj(code, scope.clone()) {
                return StepResult::Error {
                    phase: "startup",
                    message: render_exception(vm, &error),
                };
            }
            let layout = match scope.globals.get_item_opt("_layout_id", vm) {
                Ok(Some(value)) if !value.is(&vm.ctx.none) => {
                    match value.try_to_value::<String>(vm) {
                        Ok(value) => value,
                        Err(error) => {
                            return StepResult::Error {
                                phase: "startup",
                                message: render_exception(vm, &error),
                            };
                        }
                    }
                }
                Ok(_) => {
                    return StepResult::Error {
                        phase: "startup",
                        message: "script must call layout() exactly once".to_owned(),
                    };
                }
                Err(error) => {
                    return StepResult::Error {
                        phase: "startup",
                        message: render_exception(vm, &error),
                    };
                }
            };
            if layout.len() > MAX_STRING_BYTES {
                return StepResult::Error {
                    phase: "limits",
                    message: "layout id is too long".to_owned(),
                };
            }
            let main = match scope.globals.get_item_opt("main", vm) {
                Ok(Some(main)) => main,
                Ok(None) => {
                    return StepResult::Error {
                        phase: "startup",
                        message: "script must define main()".to_owned(),
                    };
                }
                Err(error) => {
                    return StepResult::Error {
                        phase: "startup",
                        message: render_exception(vm, &error),
                    };
                }
            };
            let generator = match main.call((), vm) {
                Ok(generator) => generator,
                Err(error) => {
                    return StepResult::Error {
                        phase: "startup",
                        message: render_exception(vm, &error),
                    };
                }
            };
            self.generator = Some(generator.clone());
            self.layout = Some(layout.clone());
            step_result(
                vm,
                vm.call_method(&generator, "send", (vm.ctx.none(),)),
                &effect_class,
                Some(layout.clone()),
                &layout,
            )
        });
        json_result(outcome)
    }

    /// Resume the generator with one JSON value after the current effect finishes.
    pub fn resume(&mut self, value_json: &str) -> String {
        let Some(generator) = self.generator.clone() else {
            return json_result(StepResult::Error {
                phase: "protocol",
                message: "process is not running".to_owned(),
            });
        };
        if value_json.len() > MAX_STRING_BYTES {
            return json_result(StepResult::Error {
                phase: "limits",
                message: "resume value is too large".to_owned(),
            });
        }
        let value: serde_json::Value = match serde_json::from_str(value_json) {
            Ok(value) => value,
            Err(error) => {
                return json_result(StepResult::Error {
                    phase: "protocol",
                    message: bounded_message(format!("invalid resume JSON: {error}")),
                });
            }
        };
        let effect_class = self.effect_class.clone();
        let keyboard_layout = self.layout.clone().unwrap_or_default();
        let outcome = self.interpreter.enter(|vm| {
            let value = match py_serde::deserialize(vm, value.into_deserializer()) {
                Ok(value) => value,
                Err(error) => {
                    return StepResult::Error {
                        phase: "protocol",
                        message: bounded_message(format!("unsupported resume value: {error}")),
                    };
                }
            };
            step_result(
                vm,
                vm.call_method(&generator, "send", (value,)),
                &effect_class,
                None,
                &keyboard_layout,
            )
        });
        json_result(outcome)
    }

    /// Inject a named, catchable Pico exception into the suspended generator.
    pub fn raise(&mut self, error_code: &str, message: &str) -> String {
        let Some(generator) = self.generator.clone() else {
            return json_result(StepResult::Error {
                phase: "protocol",
                message: "process is not running".to_owned(),
            });
        };
        let class_name = match error_code {
            "device_disconnected" => "DeviceDisconnected",
            "usb_unavailable" => "UsbUnavailable",
            "host_agent_unavailable" => "HostAgentUnavailable",
            "effect_rejected" => "EffectRejected",
            "effect_cancelled" => "EffectCancelled",
            _ => {
                return json_result(StepResult::Error {
                    phase: "protocol",
                    message: "unknown Python exception code".to_owned(),
                });
            }
        };
        if message.len() > MAX_STRING_BYTES {
            return json_result(StepResult::Error {
                phase: "limits",
                message: "exception message is too large".to_owned(),
            });
        }
        let effect_class = self.effect_class.clone();
        let keyboard_layout = self.layout.clone().unwrap_or_default();
        let Some(class) = self.error_classes.get(class_name).cloned() else {
            return json_result(StepResult::Error {
                phase: "internal",
                message: "Python exception class is missing".to_owned(),
            });
        };
        let outcome = self.interpreter.enter(|vm| {
            let exception = match class.call((message.to_owned(),), vm) {
                Ok(exception) => exception,
                Err(error) => {
                    return StepResult::Error {
                        phase: "internal",
                        message: render_exception(vm, &error),
                    };
                }
            };
            step_result(
                vm,
                vm.call_method(&generator, "throw", (exception,)),
                &effect_class,
                None,
                &keyboard_layout,
            )
        });
        json_result(outcome)
    }
}

fn py_error(message: impl core::fmt::Debug) -> JsValue {
    JsValue::from_str(&format!("{message:?}"))
}

/// Initial integration gate: prove that RustPython generators can be resumed
/// and have an exception injected while preserving their frame state.
#[wasm_bindgen]
pub fn generator_smoke() -> Result<String, JsValue> {
    let interpreter = Interpreter::without_stdlib(Default::default());
    interpreter.enter(|vm| {
        let scope = vm.new_scope_with_builtins();
        let source = r#"
def main():
    received = yield 1
    try:
        yield 2
    except RuntimeError:
        yield 3
    return received
"#;
        let code = vm
            .compile(source, Mode::Exec, "<generator-smoke>".to_owned())
            .map_err(|err| py_error(vm.new_syntax_error(&err, Some(source))))?;
        vm.run_code_obj(code, scope.clone()).map_err(py_error)?;

        let main = scope
            .globals
            .get_item_opt("main", vm)
            .map_err(py_error)?
            .ok_or_else(|| JsValue::from_str("main() was not defined"))?;
        let generator = main.call((), vm).map_err(py_error)?;

        let first: i32 = vm
            .call_method(&generator, "send", (vm.ctx.none(),))
            .map_err(py_error)?
            .try_to_value(vm)
            .map_err(py_error)?;
        let second: i32 = vm
            .call_method(&generator, "send", (vm.ctx.new_int(7),))
            .map_err(py_error)?
            .try_to_value(vm)
            .map_err(py_error)?;
        let injected = vm.new_runtime_error("injected smoke-test failure");
        let third: i32 = vm
            .call_method(&generator, "throw", (injected,))
            .map_err(py_error)?
            .try_to_value(vm)
            .map_err(py_error)?;

        let completed = match vm.call_method(&generator, "send", (vm.ctx.none(),)) {
            Ok(_) => false,
            Err(err) => err.fast_isinstance(vm.ctx.exceptions.stop_iteration),
        };
        Ok(format!("{first},{second},{third},{completed}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(value: String) -> serde_json::Value {
        serde_json::from_str(&value).expect("worker response must be JSON")
    }

    fn bytecode(response: &serde_json::Value) -> Vec<u8> {
        serde_json::from_value(response["effect"]["bytecode"].clone())
            .expect("keyboard effect bytecode")
    }

    #[test]
    fn process_preserves_state_across_effects() {
        let mut process = PythonProcess::new().expect("prelude should initialize");
        let first = response(process.start(
            r#"
layout("mac_de-DE")

def main():
    value = yield tap("F15")
    yield text(value["message"], 25)
"#,
        ));
        assert_eq!(first["status"], "effect");
        assert_eq!(first["layout"], "mac_de-DE");
        assert_eq!(first["effect"]["kind"], "keyboard");
        assert!(keyboard_core::bytecode::validate(&bytecode(&first)).is_ok());

        let second = response(process.resume(r#"{"message":"still alive"}"#));
        assert_eq!(second["status"], "effect");
        assert_eq!(second["effect"]["kind"], "keyboard");
        assert!(keyboard_core::bytecode::validate(&bytecode(&second)).is_ok());
        assert_eq!(response(process.resume("null"))["status"], "completed");
    }

    #[test]
    fn device_failure_is_catchable_in_python() {
        let mut process = PythonProcess::new().expect("prelude should initialize");
        let first = response(process.start(
            r#"
layout("win_en-GB")

def main():
    try:
        yield tap("ENTER")
    except DeviceDisconnected as error:
        yield text(str(error), 0)
"#,
        ));
        assert_eq!(first["effect"]["kind"], "keyboard");

        let caught = response(process.raise("device_disconnected", "Pico went away"));
        assert_eq!(caught["status"], "effect");
        assert_eq!(caught["effect"]["kind"], "keyboard");
        assert!(keyboard_core::bytecode::validate(&bytecode(&caught)).is_ok());
    }

    #[test]
    fn invalid_yield_and_missing_layout_are_rejected() {
        let mut process = PythonProcess::new().expect("prelude should initialize");
        let missing = response(process.start("def main():\n    yield tap(\"A\")\n"));
        assert_eq!(missing["status"], "error");
        assert_eq!(missing["phase"], "startup");

        let mut process = PythonProcess::new().expect("prelude should initialize");
        let invalid =
            response(process.start("layout(\"win_en-GB\")\ndef main():\n    yield 123\n"));
        assert_eq!(invalid["status"], "error");
        assert_eq!(invalid["phase"], "effect");
    }

    #[test]
    fn source_limits_are_checked_before_parsing() {
        let mut process = PythonProcess::new().expect("prelude should initialize");
        let source = "x\n".repeat(MAX_SOURCE_LINES + 1);
        let result = response(process.start(&source));
        assert_eq!(result["status"], "error");
        assert_eq!(result["phase"], "limits");
    }

    #[test]
    fn diagnostic_truncation_preserves_utf8_boundaries() {
        let message = "é".repeat(MAX_DIAGNOSTIC_BYTES);
        let bounded = bounded_message(message);
        assert!(bounded.is_char_boundary(bounded.len()));
        assert!(bounded.starts_with('é'));
        assert!(bounded.ends_with("[diagnostic truncated]"));
    }

    #[test]
    fn generator_retains_loop_state_for_one_hundred_steps() {
        let mut process = PythonProcess::new().expect("prelude should initialize");
        let first = response(process.start(
            r#"
layout("win_en-GB")

def main():
    for index in range(100):
        resumed = yield sleep(0)
        if resumed != index:
            raise RuntimeError("generator state was lost")
"#,
        ));
        assert_eq!(first["status"], "effect");
        for index in 0..100 {
            let result = response(process.resume(&index.to_string()));
            if index == 99 {
                assert_eq!(result["status"], "completed");
            } else {
                assert_eq!(result["status"], "effect");
                assert_eq!(result["effect"]["kind"], "sleep");
            }
        }
    }

    #[test]
    fn every_supervisor_exception_is_catchable() {
        for (code, class_name) in [
            ("device_disconnected", "DeviceDisconnected"),
            ("usb_unavailable", "UsbUnavailable"),
            ("host_agent_unavailable", "HostAgentUnavailable"),
            ("effect_rejected", "EffectRejected"),
            ("effect_cancelled", "EffectCancelled"),
        ] {
            let mut process = PythonProcess::new().expect("prelude should initialize");
            let source = format!(
                r#"
layout("win_en-GB")

def main():
    try:
        yield sleep(0)
    except {class_name}:
        yield sleep(1)
"#
            );
            assert_eq!(response(process.start(&source))["status"], "effect");
            let caught = response(process.raise(code, "injected by test"));
            assert_eq!(caught["status"], "effect", "{class_name} was not caught");
            assert_eq!(caught["effect"]["kind"], "sleep");
        }
    }

    #[test]
    fn imports_are_unavailable() {
        let mut process = PythonProcess::new().expect("prelude should initialize");
        let result = response(process.start(
            r#"
layout("win_en-GB")
import os

def main():
    yield sleep(0)
"#,
        ));
        assert_eq!(result["status"], "error");
        assert_eq!(result["phase"], "startup");
    }
}

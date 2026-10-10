# Python scripting reference

The Web UI runs one RustPython generator in a dedicated browser Worker. Keyboard effects use the same bounded KBD1 execution service as firmware-resident Rust presets. Python runs in the browser, not on the Pico.

This is the current API and lifecycle reference. Component ownership is in [ARCHITECTURE.md](ARCHITECTURE.md#rustpython-process-and-effect-path), device effect envelopes are in [PROTOCOL.md](PROTOCOL.md#python-keyboard-effect-envelope-binary-kind-8), and security assumptions are in [THREAT_MODEL.md](THREAT_MODEL.md).

## Writing a process

Call `layout()` exactly once at module scope and define `main()` as a generator. Effect constructors return tokens; only yielding a token requests an action. Ordinary Python functions, local state, loops, and exceptions are available. An invalid yielded value stops the process.

```python
layout("mac_de-DE")

def main():
    while True:
        event = yield wait_event("button", "connection", timeout_ms=1_000)
        if event["kind"] == "button" and event["button"] == "A":
            try:
                yield text("Button A", 10)
            except (DeviceDisconnected, UsbUnavailable):
                pass
```

Match the layout to the source PC's active keyboard input layout. The Worker enables `win_en-US`, `win_en-GB`, `win_pt-BR`, `win_de-DE`, `mac_en-GB`, `mac_pt-BR`, and `mac_de-DE`; the device advertises its supported layouts in `HELLO`.

## Effects

Names are injected into the Python scope; no imports are needed.

| Function | Behavior |
| --- | --- |
| `layout(id)` | Select the layout once before `main()` is called; do not yield it. |
| `tap(key)` | Yield one keyboard key tap, for example `tap("ENTER")`. |
| `modtap(chord)` | Yield a modifier/key chord, for example `modtap("CTRL+C")`. |
| `text(value, delay_ms=10)` | Yield layout-aware typing with the specified delay between characters. Unsupported characters are rejected. |
| `sleep(milliseconds)` | Yield a local browser timer; resumes with `{"kind": "timer"}`. |
| `wait_event(*kinds, timeout_ms=None)` | Yield a wait for a matching event. A timeout resumes with `{"kind": "timer", "timeout": true}`. |
| `require_usb()` | Yield a check of the current USB HID capability; resumes with `None` or throws `UsbUnavailable`. |
| `require_host_agent()` | Yield a check of agent availability; resumes with `None` or throws `HostAgentUnavailable`. It does not establish host trust. |

Keyboard completion resumes the generator with `None`. Values are delivered through `generator.send()` and device errors through `generator.throw()`; at most one effect is outstanding.

Supported events:

| Kind | Fields beyond `kind` | Source |
| --- | --- | --- |
| `timer` | `timeout: true` for a timed-out event wait | Browser timers |
| `connection` | `connected: bool` | WebSocket lifecycle |
| `usb` | `ready: bool` | HID capability changes |
| `host_agent` | `ready: bool` | Agent capability changes |
| `button` | `button: "A" \| "B" \| "X" \| "Y"`, `edge: "released"` | Debounced display input |

Capability changes notify an active matching wait; they are not queued for later replay. Only button events are buffered when no wait matches, up to 32, dropping the oldest when full. Events lost during device disconnection are not replayed. Display controls consumed by Payloads, menu gestures, or device-wide Stop are not also published to Python. Device button delivery is best effort under queue pressure.

## Exceptions and cancellation

All injected device exceptions derive from `PicoError`:

| Exception | Trigger |
| --- | --- |
| `DeviceDisconnected` | A keyboard effect cannot be sent, or the WebSocket disconnects during an outstanding effect. |
| `UsbUnavailable` | USB HID is unavailable or disconnects during an outstanding effect. |
| `HostAgentUnavailable` | `require_host_agent()` finds no available agent. |
| `EffectRejected` | Invalid keyboard bytecode or a rejected firmware effect. |
| `EffectCancelled` | Firmware reports cancellation, or the device effect's completion deadline expires. |

After connection loss, an effect's outcome may be unknown. The browser never retries keyboard output automatically, because that could duplicate keystrokes. An uncaught exception faults the process and displays a bounded diagnostic.

The Web UI's Stop action best-effort cancels the current device effect and immediately calls `Worker.terminate()`; it does not resume Python to run cleanup or catch a Stop exception. A non-yielding step also terminates the Worker on timeout. Firmware performs its own key-release cleanup independently of Python. Physical Y Stop cancels the active HID job from any display page, including a firmware preset.

## Lifetime and capabilities

- One process exists at a time. Starting a replacement stops the previous process.
- WebSocket disconnect leaves the Worker and Python state alive while the tab stays open. A process can catch device exceptions and wait for reconnection.
- Closing or reloading the tab terminates the Worker. State is not persisted to the Pico and is not restored after a reboot.
- A fault or timeout does not automatically restart the process. Edit source, then Stop/Start to use the new source; editing does not modify the running generator.
- Imports, stdlib/host/JS bridges, filesystem, network, DOM, host commands, and credentials are unavailable to Python. The VM uses `Interpreter::without_stdlib`; `open`, `__import__`, `eval`, `exec`, `compile`, `input`, and `print` are removed.
- Ordinary Python computation is synchronous inside the Worker. It must yield periodically to stay within the step deadline; there is no Python threading or background-task API.

The Worker isolates computation from the Yew main thread and receives no browser or host capability handles. Removed builtins alone are not a security sandbox; keep the Worker and host/JS bridge restrictions intact.

## Enforced limits

| Resource | Current limit | Enforcement |
| --- | ---: | --- |
| Source | 32 KiB, 1,024 lines | Worker before compilation |
| Source nesting pre-scan | 128 | Worker before compilation |
| Processes / outstanding effects | 1 / 1 | Frontend supervisor |
| Buffered button events | 32 | Frontend supervisor |
| Initialization, compilation, and first yield | 30 seconds combined | Frontend Worker deadline |
| Subsequent resume/raise step | 500 ms | Frontend Worker deadline |
| Device effect completion | 360 seconds | Frontend; best-effort cancellation then exception |
| WASM linear memory maximum | 128 MiB | Root target linker flag `--max-memory=134217728` |
| Text effect | 1,024 Unicode scalars and 4 KiB UTF-8 | Worker |
| Layout/key/chord/event-kind strings | 4 KiB each | Worker |
| Event kinds per wait | 1–8 | Worker |
| One KBD1 effect | 4,096 bytes, 10,000 decoded operations | Shared keyboard/executor bounds |
| One device delay / total declared effect delay | 5 seconds / 5 minutes | Keyboard core and firmware executor |
| One local timer | 2,147,483,647 ms | Worker |
| Rendered diagnostic | 8 KiB before a truncation suffix | Worker |

The WASM memory limit bounds linear memory, not total browser memory or all JavaScript allocations. The source nesting check is a pre-scan rather than a general execution-depth limit. Startup's first yield has the initialization deadline; subsequent steps have the shorter deadline.

## Implementation and build

| Responsibility | Source |
| --- | --- |
| Restricted VM, Python API, validation, keyboard lowering, send/throw | `apps/python-worker/src/lib.rs` |
| Worker version-1 start/resume/raise and effect/error/stopped messages | `apps/python-worker/worker.js` |
| Worker ownership, deadlines, event waits, device result correlation | `apps/frontend/src/python.rs` |
| Correlated device envelope and KBD1 constants | `crates/script-protocol`, `crates/bytecode-constants` |
| Layout/key/chord lowering and strict execution | `crates/keyboard-core`, `crates/firmware-exec`, `firmware/src/usb/hid.rs` |
| Local preset generation and shared job ownership | `crates/build-support/src/presets.rs`, `crates/firmware-exec/src/jobs.rs` |
| Worker compilation, wasm-bindgen, compression, asset embedding | `crates/build-support/src/frontend.rs` |

Build-support separately compiles the Worker for `wasm32-unknown-unknown`, runs wasm-bindgen, and embeds its driver, glue, and WASM alongside the Trunk-built frontend. The frontend creates a module Worker at `/ui/python-worker.js` only on Start; its runtime lives at `/ui/python-runtime.js` and `/ui/python-runtime.wasm`. Firmware serves compressed assets with the appropriate content types and `Content-Encoding: gzip`.

RustPython is pinned to 0.6.0. The Ruff family is pinned to 0.16.5 because newer permitted AST APIs break that compiler. Native tests optimize only `rustpython-vm` to avoid codec-bootstrap stack-guard failures while retaining debug assertions and the default test stack. Keep these dependency/profile constraints documented beside their Cargo settings.

Worker compressed-WASM build gates default to a 3,500,000-byte warning and a 4,750,000-byte maximum, configurable with `PICO_PYTHON_WASM_WARN_BYTES` and `PICO_PYTHON_WASM_MAX_BYTES`. These are artifact-size gates, distinct from runtime memory limits. Current memory layout and dated release measurements belong in [HARDWARE.md](HARDWARE.md#flash-and-ram-layout).

## Validation

Use the RustPython/keyboard validation matrix in [AGENTS.md](../AGENTS.md#rustpython-layout-or-bytecode-changes), including native Worker tests, the layout matrix, and compilation for the affected real targets. Browser Worker tests use `run_in_browser` and need a compatible WebDriver; a wasm compile alone does not exercise browser execution.

VM tests cover generator state, send/throw, exception handling, invalid yields, source bounds, diagnostics, codec initialization, and denied imports. They do not by themselves prove supervisor deadlines, reconnection, lazy loading, physical HID cancellation, or key release. Connected-browser and hardware coverage gaps are listed in [DEVICE_TESTING.md](DEVICE_TESTING.md#connected-browser-and-security-validation).

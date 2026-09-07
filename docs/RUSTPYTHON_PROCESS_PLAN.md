# RustPython browser process: greenfield one-session implementation plan

## Status and objective

This document defines one bounded implementation session for replacing the existing user-facing DSL scripting system with a single long-lived RustPython process in the Web frontend.

Implementation status (2026-08-17): the greenfield cutover is implemented. The former DSL editor, parser/linker/WASM adapter, built-in scripts, payload generator/page, and `SCRIPT_RUN_HEX` path have been removed. RustPython 0.5.0 is built as a dedicated Worker; its 12,011,052-byte post-bindgen WASM is stored as deterministic gzip (3,739,823 bytes in the validated release build) and served with `Content-Encoding: gzip`. The linked firmware uses 5,320,832 of the 8,380,416-byte `FLASH` region, leaving 3,059,584 bytes (2.92 MiB). Its statically allocated striped SRAM ends at byte 360,928 of 524,288, leaving 163,360 bytes before runtime stacks; the two direct 4 KiB banks remain separate.

Native RustPython process tests, wasm target compilation, all-layout keyboard tests, strict protocol/executor tests, frontend release bundling, Clippy, and the linked RP2350 release build pass. The 3.5 MB compressed-Worker warning is expected and remains below the 4.5 MB hard gate. Browser/WebDriver execution and physical HID/disconnect testing require their external environments and were not run as part of this implementation pass.

The process uses the actual RustPython VM in a dedicated Web Worker. It may contain ordinary Python functions, state, exceptions, and cooperative loops. It remains alive when the Pico WebSocket disconnects, provided the browser tab remains open. Device-dependent operations fail with catchable Python exceptions while their capability is unavailable.

This is a browser process, not a device-resident process:

- Closing or reloading the tab terminates it.
- A Pico reboot does not restore it.
- A WebSocket disconnect does not terminate it.
- The process is not retried automatically after a crash or timeout.
- Only one process may exist at a time.

RustPython is the only scripting interface in the completed tree. KBD1 remains an internal firmware action format, but users no longer edit DSL, load DSL built-ins, or call the legacy script RPC.

The phrase “continue when the browser disconnects” is interpreted here as “continue when the frontend loses its WebSocket connection to the Pico while the tab and Worker remain alive.” A browser Worker cannot survive the browser tab itself closing.

## One-session result

At the end of the session, the following script should run through the real RustPython VM:

```python
layout("mac_de-DE")

def main():
    while True:
        try:
            yield tap("F15")
        except (DeviceDisconnected, UsbUnavailable):
            pass

        event = yield wait_event(
            "timer",
            "connection",
            "usb",
            "host_agent",
            "button",
            timeout_ms=1_000,
        )

        if event["kind"] == "button" and event["button"] == "A":
            yield text("Button A", 10)
```

The process is cooperative: every external effect is yielded to the browser supervisor. A loop such as `while True: pass` never yields, so the main thread terminates the Worker when the per-step deadline expires.

## Greenfield cutover policy

The session may temporarily leave the DSL path in place while the RustPython spike and size gates are being proven. Once the RustPython acceptance tests pass, the same session removes the old user-facing path rather than shipping two scripting systems.

The final implementation must:

- Remove the DSL/Python mode switch; the scripting page is Python-only.
- Remove `SCRIPT_RUN_HEX` and replace it with the tracked binary effect protocol.
- Remove built-in DSL scripts and the on-device payload library/page.
- Remove DSL-specific frontend state, components, compilation, and documentation.
- Extract reusable layout, key parsing, text lowering, KBD1 encoding, and validation into a language-neutral crate.
- Remove the DSL parser, preprocessor, linker, WASM adapter, examples, and payload generator after their reusable pieces have moved.
- Keep the low-level KBD1 executor because it remains the bounded action format used by RustPython effects.

If the RustPython spike fails its API, memory, or flash gate, stop before deleting the DSL implementation and report the failure. There is no requirement to retain a permanent compatibility fallback after RustPython succeeds.

## Explicit non-goals

The session does not implement:

- Persistence after tab close, browser exit, Pico reboot, or power loss.
- Multiple Python processes, Python threads, or subprocesses.
- `asyncio`, arbitrary awaitables, or background Python tasks.
- Python imports, frozen stdlib, filesystem, network, DOM, Fetch, or JavaScript access.
- Host command execution, filesystem operations, or credentials from Python.
- Automatic process restart or restoration of Python state.
- A general firmware state-machine VM.
- Migration or source compatibility for existing DSL scripts.
- Production browser-matrix, fuzzing, CSP, or flash-compression work beyond the build/size gates below.

## Runtime model

RustPython executes one generator returned by `main()`:

```text
Python generator
    │ yields one bounded effect
    ▼
Worker response
    │
    ▼
Yew process supervisor
    ├─ local timer/event handling
    ├─ capability validation
    └─ correlated firmware HID request
    │
    ├─ success/value ──→ generator.send(value)
    └─ failure ────────→ generator.throw(PicoException)
```

Only one effect may be outstanding. This provides backpressure and returns the Worker to its event loop between Python steps.

The process lifetime is unlimited. Resource limits apply to each generator step, yielded value, HID effect, queued event, and Worker memory.

### Python API

The Worker injects these names directly into the execution scope; no import is required:

```python
layout(id)
tap(key)
modtap(chord)
text(value, delay_ms=10)
sleep(milliseconds)
wait_event(*kinds, timeout_ms=None)
require_usb()
require_host_agent()
```

Each function returns an opaque effect token. The script must yield it:

```python
yield tap("ENTER")
yield sleep(500)
event = yield wait_event("button", "connection")
```

Calling an effect constructor does not itself perform I/O. Returning or yielding any value that is not a registered effect token is a runtime error.

### Catchable exceptions

The execution scope includes:

```text
PicoError
DeviceDisconnected
UsbUnavailable
HostAgentUnavailable
EffectRejected
EffectCancelled
```

Semantics:

- All device effects throw `DeviceDisconnected` when the Pico WebSocket is unavailable.
- HID effects and `require_usb()` throw `UsbUnavailable` when USB HID is unavailable.
- `require_host_agent()` and future host-dependent effects throw `HostAgentUnavailable` when the agent is absent or stale.
- A disconnect during an in-flight HID effect throws `DeviceDisconnected` with an “outcome unknown; not retried” message. The device effect may finish because the disconnected browser cannot reliably deliver cancellation.
- Firmware rejection throws `EffectRejected`.
- User Stop or language/process replacement throws `EffectCancelled` if the generator can be resumed safely; the Worker is then terminated.
- An uncaught exception stops the process and displays a bounded traceback.

No effect is retried automatically because retrying keyboard output could duplicate keystrokes.

### Initial events

The one-session event set is:

- `timer`: local `sleep` or `wait_event` timeout.
- `connection`: Pico WebSocket connected/disconnected.
- `usb`: HID ready/unavailable.
- `host_agent`: present/unavailable.
- `button`: debounced Pico Display A/B/X/Y press/release while connected.

Connection, USB, and host-agent events use latest-value semantics. Button events use a bounded FIFO and are best effort. Events occurring on the device while the WebSocket is disconnected are not replayed.

## Dependency and capability boundary

Create a separate workspace package:

```text
apps/python-worker/
  Cargo.toml
  src/lib.rs
  src/runtime.rs
  src/effects.rs
  src/protocol.rs
```

Start with an exact release pin:

```toml
rustpython-vm = {
    version = "=0.5.0",
    default-features = false,
    features = ["compiler"]
}
```

Measure `gc` enabled and disabled during the initial spike. Do not enable:

- `wasmbind`
- `host_env`
- `stdio`
- `importlib`
- `encodings`
- `threading`
- `freeze-stdlib`

Use `Interpreter::without_stdlib`. Do not use RustPython’s general browser demo or expose JS values to Python.

Remove or replace these builtins in the process scope:

- `open`
- `__import__`
- `eval`
- `exec`
- `compile`
- `input`
- `print`

The Worker, rather than filtered Python builtins, is the capability boundary. Python receives no WebSocket, DOM, network, filesystem, IndexedDB, or host-agent handle.

## Worker protocol

Add a small shared crate or shared serde module for a versioned protocol:

```text
Start {
    version,
    process_id,
    source,
}

Started {
    process_id,
    layout,
}

Effect {
    process_id,
    step_id,
    effect,
}

Resume {
    process_id,
    step_id,
    value,
}

Raise {
    process_id,
    step_id,
    error_code,
    message,
}

Stopped | PythonError | InternalError
```

Required validation:

- Protocol version must match exactly.
- Process and step IDs must match the current generation.
- Unknown variants and trailing/oversized fields are rejected.
- Worker source, results, and diagnostics are bounded before conversion.
- Stale responses from a terminated Worker are ignored.
- Python source and text effect contents are never written to the browser console, firmware logs, or device status events.

The Worker runs synchronously only while starting or resuming the generator. The main thread starts a deadline before every `Start`, `Resume`, or `Raise`. On expiry it calls `Worker.terminate()` and marks the process faulted.

## Language-neutral keyboard core and KBD1 validation

Create `crates/keyboard-core` by extracting the reusable pieces from `crates/dsl/dsl-core`:

- Keyboard layout identifiers and character mappings.
- Key and modifier/chord parsing.
- Flat tap/delay operations and normalization.
- A bounded program builder.
- KBD1 encoding, decoding, constants, and strict validation.

The crate remains `no_std` by default, with allocation enabled only where encoding needs it. Its primary API is:

```rust
let mut program = KeyboardProgramBuilder::new();
program.layout("mac_de-DE")?;
program.tap("ENTER")?;
let bytecode = program.finish()?;
```

The builder must reuse the existing layout, key/chord parsing, text lowering, delay, normalization, and bytecode encoding. It must enforce layout ordering, operation count, per-delay limit, unsupported characters, and final bytecode size.

The Worker uses the builder to convert yielded keyboard effects to KBD1. The main frontend treats returned bytes as untrusted and validates them again before transport.

After all consumers use `keyboard-core`, remove:

- `apps/frontend/src/dsl.rs`
- `crates/dsl/dsl-core`
- `crates/dsl/dsl-wasm`
- `crates/dsl/examples`
- `crates/builtin-scripts`
- `crates/build-support/src/payloads.rs` and its generated payload integration

Retain the firmware executor, moving or renaming it only if that can be done mechanically without risking the session. It executes KBD1 and is not itself a DSL implementation.

Before accepting tracked Python effects, harden firmware execution:

- Validate the complete KBD1 before the first HID report.
- Reject nonzero flags, malformed/noncanonical varints, unknown opcodes, bad modifiers/usages, missing or misplaced `END`, trailing bytes, bad CRC, excessive operations, and excessive delays.
- Align the firmware operation limit with `MAX_TOTAL_FLAT_OPS`.
- Execute only after a successful validation pass.
- Make every tap/modifier/delay suspension cancellable.
- Always attempt an all-zero keyboard report on cancellation and before the first action after USB reconnect.

Each Python HID effect may use the full 4,096-byte KBD1 limit because the new binary effect protocol replaces the text/hex transport.

## Correlated HID effects

Remove `SCRIPT_RUN_HEX`. Add one versioned browser-to-firmware binary message kind for tracked process control:

```text
kind:          u8
version:       u8 = 1
operation:     u8 = RUN_EFFECT | CANCEL_PROCESS
request_id:    u64 little-endian
process_id:    u64 little-endian
effect_id:     u64 little-endian
bytecode_len:  u16 little-endian
bytecode:      0..4096 bytes for RUN_EFFECT
```

The initial queue/rejection response remains correlated through the existing request envelope. Firmware later emits:

```json
{
  "event_type": "script/effect_result",
  "version": 1,
  "process_id": "0011223344556677",
  "effect_id": "0000000000000004",
  "status": "completed"
}
```

Statuses are `completed`, `rejected`, `cancelled`, or `usb_unavailable`. JSON event IDs are fixed-width hexadecimal strings to avoid JavaScript integer precision issues, even though the binary request uses integers.

Increase or relocate the WebSocket frame scratch space to accept the 4,125-byte maximum envelope. The existing 8 KiB PSRAM receive allocation is sufficient; measure the static SRAM delta from any stack-buffer change.

Update `HidCommand` with an origin, process/effect IDs, and frontend connection generation. The HID owner sends completion only for the matching generation.

When the WebSocket generation closes:

- Cancel its active tracked effect through a high-priority signal.
- Remove its pending tracked effects.
- Release all HID keys.
- Do not stop the browser Worker.
- The frontend resumes the generator with `DeviceDisconnected`.

There is no legacy script command or payload queue in the completed tree. All browser-originated keyboard scripting goes through the tracked binary effect path.

## Frontend supervisor

Add `apps/frontend/src/python_process.rs` with one owner for:

- Worker generation and process ID.
- Current step ID and one pending effect.
- Per-step timeout.
- Bounded event queue.
- Connection/USB/host capability snapshot.
- Correlated firmware effect completion.
- Start, Stop, disconnect, reconnect, and teardown behavior.

Rules:

- Start sets process state before initializing RustPython, preventing double starts.
- Only one process and one effect are allowed.
- WebSocket close updates capabilities and enqueues a connection event; it does not terminate the Worker.
- Reconnect refreshes `HELLO` before device effects become available again.
- Editing or replacing the source does not mutate a running process; the user must Stop and Start.
- Stop terminates local timers, cancels a connected firmware effect, terminates the Worker, clears events, and invalidates all IDs.
- Tab teardown terminates the Worker with its owning browser context. Explicit Stop best-effort cancels an active tracked effect; unload is not treated as a reliable control-message opportunity.

Update the WebSocket event router in `apps/frontend/src/api.rs` for button and effect-result events. Unknown process IDs are ignored.

## Firmware button events

The display task already debounces A/B/X/Y at 50 ms. Publish copies of press/release edges to the current WebSocket generation without changing display behavior.

Use a versioned bounded event:

```json
{
  "event_type": "device/button",
  "version": 1,
  "button": "A",
  "edge": "pressed"
}
```

Use nonblocking insertion. Dropping a button event under queue pressure is allowed for the one-session MVP; it must not stall the display task.

## UI changes

Update the scripting card with:

- A single Python editor; remove the language selector and DSL source state.
- Python starter example using a cooperative generator.
- Start process and Stop process controls.
- State: loading, running, waiting, executing, disconnected, faulted, stopped.
- Current wait/effect and bounded diagnostic.
- Clear note: “The process survives device connection loss while this tab remains open. Closing or reloading the tab stops it.”
- Clear note that every external action must be yielded.

Remove the built-in DSL library. If examples are useful, keep a small static list of Python examples in the frontend without adding a second compiler or payload format.

Remove the on-device payload page from the display registry. It may be replaced with a small Python-process status page only if the status view fits the session after the runtime is working; otherwise remove the old page without replacement.

## Trunk, embedding, and routes

Use Trunk’s Rust worker asset support from `apps/frontend/index.html`; do not link RustPython into the main Yew WASM.

Extend the existing isolated release build in `crates/build-support` to:

- Watch and fingerprint `apps/python-worker` and `Cargo.lock`.
- Discover exactly one worker loader, wasm-bindgen glue module, and Worker WASM.
- Fail on missing or ambiguous artifacts.
- Generate stable byte statics and route paths.
- Report main WASM, Python WASM, Python JavaScript, and aggregate frontend sizes separately.
- Add `PICO_PYTHON_WASM_WARN_BYTES` and `PICO_PYTHON_WASM_MAX_BYTES`.

Extend firmware frontend routes with correct JavaScript and `application/wasm` content types. Python assets must be loaded only when Start is pressed; initial application and scripting-page rendering must not fetch them.

The existing three-second HTTP write timeout must be measured for the Python WASM response and increased only if the embedded access-point test demonstrates it is necessary.

## Initial resource limits and gates

| Resource | One-session limit |
|---|---:|
| Python source | 32 KiB |
| Source lines | 1,024 |
| Parser nesting pre-scan | 128 |
| Worker process count | 1 |
| Outstanding effects | 1 |
| Worker event queue | 32 |
| One generator step | 500 ms initially |
| Worker initialization | 30 seconds |
| Worker WASM memory maximum | 64 MiB target, 128 MiB absolute stop |
| One text effect | 1,024 Unicode scalars |
| One KBD1 effect | 4,096 bytes |
| One device delay | 5 seconds |
| Total delay in one queued KBD1 keyboard effect | 5 minutes |
| One browser timer | 2,147,483,647 ms (about 24.9 days) |
| Bounded traceback/diagnostic | 8 KiB |

Build gates:

- Main frontend WASM remains below its existing threshold.
- Warn when Python worker assets exceed 3.5 MiB stored bytes.
- Stop when they exceed 4.5 MiB stored bytes unless compression is deliberately added and remeasured.
- Final firmware retains at least 1.5 MiB flash headroom.
- Firmware static SRAM regression remains within the measured headroom; report the exact ELF delta.

The session begins with the minimal RustPython build and generator `send`/`throw` spike. If the public RustPython 0.5.0 API cannot support the cooperative generator driver, or the artifact fails the flash gate, stop and record measurements rather than silently replacing RustPython with a custom language VM.

## Implementation order

Execute in this order so the largest uncertainties fail early:

1. **RustPython spike**
   - Add the Worker crate and exact dependency pin.
   - Compile and run a generator through `next`, `send`, and `throw`.
   - Prove that an injected exception is catchable in Python.
   - Measure optimized Worker JS/WASM and linear-memory declaration.

2. **Worker protocol and restricted API**
   - Add versioned messages, effects, exception types, source/error limits, and per-step timeout recovery.
   - Prove `while True: pass` is terminated and a subsequent process can start.

3. **Language-neutral keyboard core and KBD1 safety**
   - Extract the bounded builder, layout lowering, encoder, and strict complete-program validator into `keyboard-core`.
   - Make firmware execution validate before HID output.

4. **Tracked HID execution**
   - Add the correlated binary run/result/cancel protocol and remove the text/hex script command.
   - Add cancellable HID phases and neutral-report cleanup.

5. **Frontend supervisor and event routing**
   - Implement one-process lifecycle, local timers, capability exceptions, connection continuity, and button events.

6. **Worker asset embedding**
   - Extend Trunk/build-support discovery, generated statics, firmware routes, size gates, and lazy loading.

7. **Greenfield cutover, minimal UI, and documentation**
   - Add Python source state, Start/Stop, status/diagnostics, starter script, and lifecycle notes.
   - Remove the DSL editor/compiler, built-in scripts, payload display page, payload generator, legacy RPC, and obsolete workspace members.
   - Remove DSL documentation and update architecture/protocol documentation to describe the Python-only path.

8. **Validation and handoff**
   - Run the targeted matrix below.
   - Inspect `git diff` and final ELF/frontend sizes.
   - Document any browser or hardware validation that was unavailable.

## Tests required in the session

### RustPython and Worker tests

- Generator yields, resumes with a value, and catches each injected Pico exception.
- Python locals and loop state survive multiple steps and a WebSocket disconnect/reconnect.
- Syntax and runtime errors retain line/column information.
- Invalid yielded values stop with a bounded diagnostic.
- Stale process/step IDs are ignored.
- Step timeout terminates the Worker; the next process starts successfully.
- Imports and browser/JS/filesystem/network access are unavailable.

### KBD1 and firmware tests

- Bad CRC, flags, opcode, varint, end position, trailing bytes, delay, or operation count produces zero HID reports.
- Tracked completion carries the exact process/effect IDs.
- Cancel at modifier-down, key-down, delay, and USB-write boundaries ends with a neutral report.
- WebSocket generation loss injects `DeviceDisconnected`, never retries the unknown effect, and ignores a later stale result from that generation.
- USB unavailable returns a terminal tracked result and does not queue replay.

### Frontend tests

- Python assets are not fetched during initial application or scripting-page rendering.
- Start is single-flight and Stop clears all pending state.
- Disconnect does not terminate the Worker.
- HID effects while disconnected throw `DeviceDisconnected`.
- HID effects without USB throw `UsbUnavailable`.
- `require_host_agent()` without an agent throws `HostAgentUnavailable`.
- Reconnect refreshes capabilities and permits later effects.
- Button/timer/connection events resolve only matching waits.
- No failed or timed-out effect is retried.

### Build validation

```sh
cargo fmt --all -- --check
cargo test -p keyboard-core --features "std layout_win_en_gb layout_win_pt_br layout_win_de_de layout_mac_en_gb layout_mac_pt_br layout_mac_de_de"
cargo test -p firmware-exec
cargo test -p build-support
cargo clippy -p build-support --all-targets -- -D warnings
cargo test -p python-worker --target wasm32-unknown-unknown --no-run
cargo test -p frontend --target wasm32-unknown-unknown --no-run
cd apps/frontend && trunk build --release
cargo build -p pico_rust --release --target thumbv8m.main-none-eabihf
```

Run browser tests when the configured browser/WebDriver environment is available. Do not flash hardware during the routine implementation session. HID hardware testing must use a separately invoked safe capture target and must not be added to the default `scripts/device-test` path.

## Acceptance criteria

The one-session implementation is complete only when:

1. An actual RustPython 0.5.0 VM runs in a separate lazy Worker.
2. One Python generator retains loop/local state across at least 100 effect steps.
3. A Pico WebSocket disconnect leaves the Worker process alive.
4. Unavailable device, USB, and host-agent capabilities become catchable Python exceptions.
5. Reconnection permits new effects without restarting Python.
6. A non-yielding Python loop is hard-terminated without freezing the Yew UI.
7. Tracked HID effects have completion and cancellation semantics and always release keys.
8. The DSL editor/compiler, built-in payloads, payload display page, text/hex script RPC, and obsolete DSL workspace members are absent from the final tree.
9. Worker assets and final firmware pass the flash/SRAM gates.
10. The targeted build and test commands pass, or unavailable browser/hardware checks are explicitly recorded.

## Cutover and failure handling

Do not delete the working DSL implementation before the initial RustPython generator and artifact-size gates pass. Once those gates pass, complete the greenfield cutover in the same change; do not leave a hidden compatibility route or second scripting UI.

If the RustPython spike fails, leave the existing functionality intact and hand off the measurements and blocker. If a later integration step fails after cutover work begins, fix forward within the session or restore only the session's incomplete cutover edits without discarding unrelated work. Strict KBD1 validation and HID cancellation improvements remain valuable regardless of the scripting frontend.

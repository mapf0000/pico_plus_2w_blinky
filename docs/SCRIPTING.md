# Native Python scripting

The companion runs one RustPython generator in a supervised child of its own
executable. Python runs on the control computer; the Pico executes only bounded
KBD1. There is no browser frontend, system Python dependency or interpreter on the
board. In **Scripts**, edit source and use Start/Restart, Stop or
Ctrl/Cmd+Enter. Edits affect the next run. The demo waits five seconds so you can
focus a disposable editor on the USB computer. Select that computer's actual
input layout in the script; the text-demo selector does not change Python layout.

The unchanged built-in typing demo requires a connected compatible Pico,
acquired control and a ready USB keyboard before Start or Ctrl/Cmd+Enter is
available. Its readiness card offers **Acquire control**, **Enable USB** or
**Open Devices** as appropriate; these actions do not start the script. General
scripts remain runnable while disconnected so they can wait for events. The UI
does not inspect arbitrary Python source to guess its effects. Runtime effect
checks remain authoritative, including if readiness changes after Start.

An uncaught error shows a short recovery message; expand **Technical details**
for the bounded Python traceback. Layout and interpreter heap statistics are
under **Run details**. Recovery hints are presentation only and never
grant permission, retry, acquire control or replay effects automatically.

```python
layout("win_en-US")

def main():
    yield require_usb()
    yield sleep(5000)
    yield text("Hello from Python!", 20)
```

Call `layout()` once at module scope and define `main()` as a generator. Ordinary
Python functions, state, loops and exceptions work. Effect constructors return
tokens; only yielding a token requests an action. Invalid yields fault the run.
Layouts: win_en-US, win_en-GB, win_pt-BR, win_de-DE, mac_en-GB, mac_pt-BR, mac_de-DE.

| API | Result |
| --- | --- |
| `layout(id)` | Select input layout once; do not yield it |
| `tap(key)` | One key tap, e.g. ENTER or F15; resumes with None |
| `modtap(chord)` | Modifier/key chord, e.g. CTRL+C; resumes with None |
| `text(value, delay_ms=10)` | Layout-aware text; delay between characters; resumes with None |
| `sleep(ms)` | Local timer; resumes with `{"kind":"timer"}` |
| `wait_event(*kinds, timeout_ms=None)` | Matching event, or `{"kind":"timer","timeout":true}` |
| `require_usb()` | None if authenticated acquired USB HID is ready; otherwise UsbUnavailable |
| `require_host_agent()` | None if the current acquired connection reports agent presence; otherwise HostAgentUnavailable |

Keyboard completion uses generator.send(); device failures use generator.throw().
Agent availability does not establish trust. Sleep/event waits run in the native
supervisor, so the interpreter is idle until resumed. Events:

| Kind | Fields |
| --- | --- |
| `connection` | `connected: bool` for BLE lifecycle |
| `usb` | `ready: bool` for acquired USB HID capability |
| `host_agent` | `ready: bool` for acquired agent availability |
| `button` | `button: A/B/X/Y`, `edge: released` |
| `timer` | `timeout: true` for an expired event wait |

Capability transitions only notify a currently matching wait. Buttons buffer up
to 32 during the run, dropping oldest; events from before Start, disconnected
incarnations or released control are discarded. Menu/Payloads/Stop gestures stay
local. Delivery is best effort under queue pressure.

```python
layout("win_en-US")

def main():
    while True:
        event = yield wait_event("button", "usb", timeout_ms=1000)
        if event["kind"] == "button" and event["button"] == "A":
            try:
                yield require_usb()
                yield text("Button A", 10)
            except (DeviceDisconnected, UsbUnavailable, EffectCancelled):
                pass
```

All device exceptions derive from PicoError: DeviceDisconnected, UsbUnavailable,
HostAgentUnavailable, EffectRejected and EffectCancelled. Connection/control loss
throws into an outstanding keyboard effect, with no automatic retry. A script
may catch it and retain state while waiting for an explicit reconnect/acquisition.
An uncaught exception faults the run and shows a bounded diagnostic. No script
is restarted automatically. Do not assume output completed if its result was lost.

Stop terminates the child without executing Python cleanup, and best-effort
cancels only this run's queued/active effect. Restart first stops the old run.
App exit kills/reaps the child and disconnects. Firmware performs key-release
cleanup independently. Physical Y Stop cancels the shared HID job; Python sees
EffectCancelled if still awaiting that job. It does not kill the interpreter.
Manual text submission is disabled while a script runs. Filesystem browsing and
connection/USB controls remain independent; one bounded device request/effect
runs at a time.

| Resource | Bound / enforcement |
| --- | --- |
| Source | 32 KiB, 1,024 lines, 128-level nesting pre-scan before compilation |
| Child / effect | One child and one yielded effect |
| Initialization, compile, first yield | 30 seconds, parent hard kill |
| Later send/throw step | 500 ms, parent hard kill |
| Device effect | 360-second supervisor ceiling plus the existing client execution/upload deadline |
| Rust heap in child | 128 MiB accounted live allocations; allocation fails before exceeding it |
| Button buffer | 32, drop oldest |
| Text | 1,024 Unicode scalars / 4 KiB UTF-8 |
| KBD1 | 4,096 bytes, strict firmware-exec validation before submission |
| Device delay / total declared delay | 5 seconds / 5 minutes |
| Local timer | 2,147,483,647 ms |
| Diagnostic | 8 KiB plus truncation suffix, retained only for current run |
| IPC frame | 256 KiB maximum; four-byte LE length, version/run/step checks |

The heap limit includes VM/compiler/IPC Rust allocations; it is not a total RSS,
address-space or OS memory limit. Native stacks, mapped code and allocator overhead
are additional. Ordinary small runs measured about 2.8 MiB accounted heap and
20 MiB child RSS on Apple Silicon macOS; dated evidence is in DEVICE_TESTING.md.
Source/errors/effects are not written to logs or temporary files by the app.

The child receives only bounded source/effects/results over pipes, with a cleared
environment and no profile or Bluetooth handles. RustPython defaults, host_env,
stdio, threading and native stdlib are disabled. Initialization fails closed if
host module definitions or FileIO become available through feature unification.
Imports, open, eval/exec/compile, input and print are removed. This is a restricted
language interface and crash/resource containment, **not an OS sandbox against
native VM vulnerabilities**. RustPython/native dependencies are trusted. See
THREAT_MODEL.md before changing capabilities or running adversarial native code.

RustPython stays pinned to 0.6.0, Ruff to 0.16.5. The portable VM/prelude/tests are
in crates/python-runtime. Private IPC is specified there; apps/companion/src/python.rs
owns process lifetime and local scheduling, python_worker.rs owns the child entry,
python_heap.rs owns its allocator budget, and companion-core validates/scopes
keyboard effects through unchanged script-v1/KBD1. No firmware update is required.

```sh
cargo test -p python-runtime -p companion-core -p pico-companion
cargo clippy -p pico-companion -p companion-core -p python-runtime --all-targets -- -D warnings
cargo run -p pico-companion -- --python-self-test
# Exclusive real BLE connection: empty KBD1 effects; no typing or persistent changes.
cargo run -p pico-companion -- --profile /private/path/pico.json --python-ble-self-test
```

The actual-process test exercises loops/deadlines, memory pressure, Stop, send/throw,
mock HID completion/cancellation, button waits and reconnect. Mock coverage does
not replace physical text/chord/key-release, button/Stop, platform or soak checks.

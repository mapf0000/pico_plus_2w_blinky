# Pico Companion

Native Rust/egui feasibility app alongside the WLAN UI. Choose an explicit
transport from the repository root:

```sh
cargo run -p pico-companion -- --ble
cargo run -p pico-companion -- --mock
```

BLE mode discovers the custom Pico service and reads its build label and live
status once per second. It has no control writes, pairing/bonding, keyboard
execution, Python or file transfers. Reconnect is explicit. The mock keeps the
keyboard lifecycle demonstration available without hardware.

Build the opt-in Pico service (normal builds keep BLE disabled):

```sh
cargo build -p pico_rust --release --target thumbv8m.main-none-eabihf --features ble
```

The Pico advertises `Pico BLE` with a static random address regenerated on each
boot. The AP, HTTP and WebSocket tasks remain enabled. To get USB logger output
at boot during a board test, add `usb_autostart` to the firmware feature list.
Flash only as an explicitly requested device test.

macOS builds embed `Info.plist` with the Bluetooth usage declaration described
by [Apple](https://developer.apple.com/documentation/bundleresources/information-property-list/nsbluetoothalwaysusagedescription).
Allow the OS Bluetooth prompt and enable Bluetooth before scanning. Permission
may be attributed to the launching terminal/application; signing and packaged
app distribution remain later work. Linux also needs BlueZ, D-Bus and libdbus
build/runtime libraries. Windows uses WinRT. Linux/Windows are not yet validated.

Headless hardware check, against a running BLE firmware:

```sh
cargo run -p pico-companion -- --ble --self-test
```

This scans, connects, validates device information, checks three periodic live
status reads with advancing uptime, disconnects and reconnects. It fails after
30 seconds at most (plus bounded cleanup), with no command writes. On a fresh
macOS permission prompt, grant access and retry if the first scan times out.
Connection rediscovers the device each time because macOS invalidates its
peripheral handle after disconnect; device addresses may also change on reboot.

In mock mode, the app automatically scans the simulated device list. Select the device,
choose **Connect**, then **Acquire mock control**. Select a target layout and
choose **Send mock effect**. Increase the initial delay to 5,000 ms to exercise
**Cancel effect** while the effect is active. **Release control** and
**Disconnect** clear ownership; reconnecting does not reacquire control or replay
text.

In mock mode, all device identity, USB readiness, host-agent presence, pairing/control and
execution results shown in the window are simulated. The mock decodes real
`script-protocol` frames and validates real KBD1 bytecode through `firmware-exec`.
It estimates timing from the bytecode; it does not emulate USB scheduling, BLE
fragmentation, pairing, or radio behavior. Native Python and file transfer are
not implemented.

## Failure scenarios

Select a scenario at launch:

```sh
cargo run -p pico-companion -- --mock --mock-scenario busy
```

| Scenario | Behavior |
| --- | --- |
| `normal` | Discovery, connect, acquire, completion/cancel, release and reconnect |
| `empty` | Scan returns no devices |
| `permission-denied` | Scan reports denied Bluetooth permission |
| `busy` | Control acquisition is refused; status remains available |
| `usb-unavailable` | Connect/control work but keyboard submission is disabled |
| `incompatible` | Device script version is incompatible; control is disabled |
| `timeout` | Connection never replies; fails after five seconds |
| `link-loss` | Connection drops after effect admission; ownership and pending work clear |

These simulate failure handling; they are not measurements of an OS Bluetooth
stack. Reconnection is explicit in this milestone. Automatic backoff/reconnect
will be implemented with the hardware transport.

Run the normal mock lifecycle without opening a window:

```sh
cargo run -p pico-companion -- --mock --self-test
```

This uses deterministic virtual time, not wall-clock sleeps. It covers discovery,
connection, control, a completed KBD1 effect, cancellation, release, and
reconnection without reacquisition. Tests additionally cover timeouts, stale
responses/queued commands, wrong IDs, hostile metadata, bounds and failure cases.

## Build and validate

```sh
cargo fmt --all -- --check
cargo test -p ble-protocol -p companion-core -p pico-companion
cargo clippy -p pico-companion -p companion-core -p ble-protocol --all-targets -- -D warnings
cargo build -p pico-companion --release
```

The app uses eframe's Glow renderer, default fonts and native accessibility,
with other eframe defaults disabled. Linux enables both X11 and Wayland support
and needs the platform development/runtime libraries described in the
[eframe setup guide](https://github.com/emilk/egui/tree/main/crates/eframe).
macOS is the first build/run target; Windows/Linux need their own compile and
device acceptance checks. A normal companion build does not run Trunk, compile
firmware, or build the browser Python Worker.

Initial validation on 2026-10-10, Apple Silicon macOS:

- `cargo fmt --all -- --check`: passed.
- `cargo test -p ble-protocol -p companion-core -p pico-companion`: 15 tests passed.
- `cargo clippy -p pico-companion -p companion-core -p ble-protocol --all-targets -- -D warnings`:
  passed.
- `cargo build -p pico-companion --release`: passed; stripped arm64 binary
  7,226,448 bytes (6.89 MiB), without the screenshot feature.
- `target/release/pico-companion --mock --self-test`: passed.
- `cargo test -p keyboard-core --features "std layout_win_en_gb layout_win_pt_br layout_win_de_de layout_mac_en_gb layout_mac_pt_br layout_mac_de_de"`:
  four tests passed.
- `cargo test -p firmware-exec -p script-protocol`: 11 tests passed.
- The screenshot command below launched the native window successfully and its
  output was visually inspected. Interactive OS input automation was not run.

No existing locked package version was removed or upgraded; new GUI dependencies
and their optional/platform dependency graphs account for lockfile additions.
Hardware BLE, pairing, coexistence, real HID output, Windows/Linux execution,
idle RAM/CPU, and startup timing have not been measured. The Pico was not flashed.

Read-only BLE validation on 2026-10-10:

- 20 native/protocol tests, targeted Clippy, formatting, mock smoke and native
  release build passed. Stripped release: 7,825,264 bytes (7.46 MiB).
- Normal/`ble`/`ble usb_autostart` firmware release links passed, with 30 firmware
  host tests and targeted Clippy passing. The user authorized the board flash.
- Real BLE discovery, validated info, three live status reads and disconnect/
  reconnect passed in both development and release runs; the native window
  connected as confirmed by the user. Latest measured read RTTs were 96/97 ms,
  including backend polling. These are not a latency benchmark.
- No keyboard writes or pairing were attempted. WLAN AP startup was logged;
  the user deferred HTTP/WebSocket coexistence testing. Linux/Windows and
  extended radio/reconnect/permission tests remain open.

Exact commands, results and limits are in
[device validation](../../docs/DEVICE_TESTING.md#read-only-ble-spike).

Development-only screenshot capture:

```sh
EFRAME_SCREENSHOT_TO=/tmp/pico-companion.png cargo run -p pico-companion --features screenshot -- --mock
```

This opens the real native window, captures it, then exits. The normal release
does not enable screenshot capture. Do not commit generated screenshots/logs.

## Ownership

- `src/app.rs` owns rendering and inputs. Bluetooth/device work never runs in
  egui callbacks.
- `src/backend.rs` owns a dedicated Tokio runtime, a 16-command queue, a separate
  four-command cancellation/disconnect queue, and a coalesced latest-snapshot
  channel. Shutdown has priority and joins the runtime thread.
- `crates/companion-core` owns connection incarnations, correlation, deadlines,
  control gating, the bounded 128-entry diagnostic ring, text lowering, and the
  transport seam. UI commands from old connection incarnations are discarded.
- `src/ble.rs` owns the native BLE worker: four queued requests, 16 replies,
  coalesced urgent connection resets, four-second I/O deadlines, and 750 ms
  per-operation cleanup deadlines. Reset/shutdown interrupts pending I/O;
  requests queued for an old connection generation are discarded. Discovered
  application state is capped at 16 devices. Platform caches belong to btleplug.
- `crates/ble-protocol` owns strict, fixed-size, `no_std` information/status codecs.
- `MockTransport` supplies bounded scheduled responses. No arbitrary typed text
  or bytecode contents are written to diagnostics.

The read-only service is the radio gate in
[the implementation plan](../../NATIVE_COMPANION_BLE_PLAN.md). Firmware
command/session extraction, a bounded BLE command codec, physically confirmed
authenticated pairing, and real keyboard effects follow it. Read-only connection
success does not prove authenticated control or WLAN coexistence under load.

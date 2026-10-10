# Pico Companion

Native Rust/egui feasibility app. This first milestone uses an explicit in-memory
mock transport. It does **not** scan Bluetooth, pair with the Pico, send USB
keyboard reports, or require the Pico to leave BOOTSEL. The existing WLAN UI and
firmware are unchanged.

From the repository root:

```sh
cargo run -p pico-companion -- --mock
```

The app automatically scans the simulated device list. Select the device,
choose **Connect**, then **Acquire mock control**. Select a target layout and
choose **Send mock effect**. Increase the initial delay to 5,000 ms to exercise
**Cancel effect** while the effect is active. **Release control** and
**Disconnect** clear ownership; reconnecting does not reacquire control or replay
text.

All device identity, USB readiness, host-agent presence, pairing/control and
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
cargo test -p companion-core -p pico-companion
cargo clippy -p pico-companion -p companion-core --all-targets -- -D warnings
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
- `cargo test -p companion-core -p pico-companion`: 15 tests passed.
- `cargo clippy -p pico-companion -p companion-core --all-targets -- -D warnings`:
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

Development-only screenshot capture:

```sh
EFRAME_SCREENSHOT_TO=/tmp/pico-companion.png cargo run -p pico-companion --features screenshot -- --mock
```

This opens the real native window, captures it, then exits. The normal release
does not enable screenshot capture. Do not commit generated screenshots/logs.

## Ownership and next milestone

- `src/app.rs` owns rendering and inputs. Bluetooth/device work never runs in
  egui callbacks.
- `src/backend.rs` owns a dedicated Tokio runtime, a 16-command queue, a separate
  four-command cancellation/disconnect queue, and a coalesced latest-snapshot
  channel. Shutdown has priority and joins the runtime thread.
- `crates/companion-core` owns connection incarnations, correlation, deadlines,
  control gating, the bounded 128-entry diagnostic ring, text lowering, and the
  transport seam. UI commands from old connection incarnations are discarded.
- `MockTransport` supplies bounded scheduled responses. No arbitrary typed text
  or bytecode contents are written to diagnostics.

The next step is the read-only board radio spike in
[the implementation plan](../../NATIVE_COMPANION_BLE_PLAN.md): native BLE service
discovery plus an opt-in Pico BLE service while WLAN remains active. Firmware
command/session extraction, the shared BLE fragment codec, authenticated
pairing, and real keyboard effects follow that radio gate. The mock demonstrates
the app/client lifecycle only; full BLE feasibility is still unproven.

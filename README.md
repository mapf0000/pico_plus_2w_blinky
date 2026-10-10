# pico_rust — Bluetooth companion and Pico firmware

A native Rust/egui companion controls the Pimoroni Pico Plus 2 W over Bluetooth
Low Energy. The Pico executes keyboard effects on its USB-connected computer.
The companion PC and USB target can be different computers.

WLAN, HTTP/WebSocket, Yew/WASM and the browser Python Worker have been removed.
Firmware builds require no browser tooling. Bluetooth is enabled by default;
`--features ble` remains a compatibility alias.

```sh
cargo run -p pico-companion
# deterministic development mode, no hardware access
cargo run -p pico-companion -- --mock
cargo run -p pico-companion -- --mock --self-test
```

Provision a unique device key before deployment (see
[provisioning](apps/companion/README.md#trusted-provisioning)), then run:

```sh
cargo run -p pico-companion -- --profile /private/path/pico.json
```

Scan, select the Pico, connect, then choose **Authenticate and acquire control**.
The app authenticates on connect using the profile and encrypts commands/results.
No Pico button press, display, OS pairing or Bluetooth bond is required, including
reconnect. Firmware without a provisioned key exposes public status but refuses
control. BLE v3 deliberately rejects older apps/firmware. Physical Y Stop remains.
The prototype stores the profile in a private file; OS credential-store import
and authenticated remote key rotation are still planned.

Select the input layout used by the USB target. Set an initial delay, send text,
and focus a disposable editor on that target. Cancel stops the current effect;
Pico **Y** provides device-wide Stop. Disconnect cancels this companion's job.
Pending effects are never replayed after reconnect. USB enable/disable controls
require acquired control and an idle keyboard executor.

The companion currently supports status, control acquisition/release, USB
on/off and bounded text effects. Python, filesystem browsing, credentials and
Bluetooth file transfer are not implemented. The host-agent USB protocol and
local display presets remain. Browser-dependent bulk requests fail closed:
chunks cannot be acknowledged without a receiver.

## Build and validation

Use current stable Rust and install the embedded target:

```sh
rustup target add thumbv8m.main-none-eabihf
cargo build -p pico-companion --release
cargo build -p pico_rust --release --target thumbv8m.main-none-eabihf
```

Install `picotool` only for flashing (`brew install picotool` on macOS).
An explicitly requested deployment can use `scripts/fw-deploy-with-agent` to
package the local agent, flash, and stream CDC logs, or:

```sh
cargo run -p pico_rust --release --target thumbv8m.main-none-eabihf
```

Never use bare workspace `cargo test`: the default member is embedded firmware.

```sh
cargo fmt --all -- --check
cargo test -p companion-core -p pico-companion -p ble-protocol -p ble-session
cargo clippy -p pico-companion -p companion-core -p ble-protocol -p ble-session --all-targets -- -D warnings
cargo test -p pico_rust --lib --no-default-features
cargo clippy -p pico_rust --lib --tests --no-default-features -- -D warnings
cargo test -p firmware-exec -p build-support -p script-protocol
cargo test -p host-agent -p transfer-crypto -p transfer-protocol
```

`cargo run -p pico-companion -- --ble --self-test` checks real discovery,
information, live status and reconnect without sending keyboard effects.
Add `--profile /private/path/pico.json` to test fresh authenticated sessions,
acquire/release and encrypted status without HID effects or Pico interaction.

## Repository and documentation

- `apps/companion`: native egui app, bounded Tokio backend and btleplug adapter.
- `crates/companion-core`: client state, correlation, deadlines and keyboard lowering.
- `crates/ble-protocol`: shared `no_std` BLE values and fragmentation.
- `crates/ble-session`: allocation-free Noise sessions and persistent provisioning.
- `firmware`: RP2350B Embassy firmware, Bluetooth, display, USB HID/CDC.
- `apps/host-agent`: portable serial agent and internal CDC bootstrap artifact.
- `crates/keyboard-core`, `firmware-exec`, `script-protocol`: shared keyboard machinery.
- `crates/build-support`: linker setup, typed presets and internal agent image.

See [the milestone plan](NATIVE_COMPANION_BLE_PLAN.md),
[companion commands](apps/companion/README.md), [architecture](docs/ARCHITECTURE.md),
[protocol](docs/PROTOCOL.md), [hardware](docs/HARDWARE.md),
[device checks](docs/DEVICE_TESTING.md), [security boundary](docs/THREAT_MODEL.md),
and [contributor guidance](AGENTS.md).

The board retains 16 MiB flash, 8 MiB PSRAM and 520 KiB SRAM. PSRAM is initialized
for diagnostics; Bluetooth uses fixed SRAM buffers. The internal 4 MiB agent
image and two 4 KiB persistent USB-identity slots retain their flash addresses.
A separate 8 KiB control-key region precedes the agent image.

## Hardware keyboard payloads

The display's **Payloads** page works without a browser or a running host agent.
Use A/B to select, X to run, and A+X to enter or leave the sidebar. The keyboard
test types `Hello from Pico!` into the focused application; focus a text editor
before using it.

USB CDC installation is available on native Apple Silicon macOS. The device
exposes logger CDC, control CDC, and a HID keyboard; it no longer exposes a USB
mass-storage drive. Package the agent and provision a unique, short USB serial
identity when building firmware:

```sh
scripts/build-host-agent
PICO_USB_SERIAL=P1234567 cargo build -p pico_rust --release --target thumbv8m.main-none-eabihf
```

Replace the example serial with a per-board value: 2–8 ASCII alphanumeric
characters starting with `P`. Without this setting, CDC installation entries are
disabled and USB keeps its existing location-based naming. The full composite
retains logger first and control second.

Select **macOS: Install via CDC (DE/US)** for the active input layout. It opens
Terminal and types a 96-character receiver command; the installer and executable
arrive over the control CDC port. The installer verifies size and SHA-256,
atomically installs `~/pico-agent/HOSTAGNT`, closes the serial handles, and launches
the agent with the selected port. **macOS: Arm manual CDC install** instead arms
the receiver for 30 seconds without sending keyboard reports; enter this in a
fresh zsh Terminal:

```zsh
(p=(/dev/cu.usbmodemP*3);(($#p==1))&&exec 3<>$p&&stty raw -echo<&3&&echo B1>&3&&exec /bin/sh<&3)
```

These are alternative workflows. To switch from manual arm to the automatic
preset, press Y to cancel the pending arm, then select **Install via CDC**.
Pressing X during a pending installation does not start another one.

The DE mapping targets a generic Pico keyboard identified as ANSI by macOS,
with the German input source selected. Keyboard hardware classification and
input language are separate settings. ISO classification has not been validated
for this preset.

The 96-character count excludes Terminal launch and Return; shifted characters
also require additional HID reports. Stop an existing agent before
installation and allow up to 25 seconds for cached agent presence to expire;
the CDC install footer reports recent agent activity and changes to
**Agent not detected; press X to retry** when that cached presence expires.
Zero/multiple matching Picos are refused. Y cancels the device
transfer. Use Terminal Control-C if its receiver remains waiting. After the
installer has arrived, its watchdog bounds stalled downloads to 120 seconds.
Display status distinguish transfer verification from an agent
handshake. Terminal may print `>` continuation prompts and `dd` job messages;
these are expected shell output during a successful install. Intel macOS is
unsupported by the current artifact. An optional
`PICO_CDC_INSTALL_DIR` in the Terminal environment selects another installation
directory. See [CDC identity and flash use](docs/HARDWARE.md#cdc-installer-identity-and-flash-use),
the [bootstrap protocol](docs/PROTOCOL.md#explicitly-armed-cdc-bootstrap-v1), and
[validation coverage](docs/DEVICE_TESTING.md#cdc-installation-checks).

The preset list scrolls to keep the selection visible and reserves space for
layout details and status. Status messages wrap across two rows; long labels use
ellipsis with the sidebar open. Menu gestures consume both A and X releases,
so closing the sidebar cannot also run the selected preset. Completion and the
agent-handshake timeout continue while other pages are visible.

Y stops the active keyboard job from any display page, including companion effects.
Jobs never preempt each other: a second Run returns busy. Cancellation is terminal,
with bounded key-release cleanup and no automatic restart after USB reconnect.
Payloads/menu controls and the Stop press are consumed locally rather than also
being delivered to a Python button-event handler. With no active job, Y retains
its LED-color behavior outside Payloads.

## Host agent packaging
- One-command local flow:
  - `scripts/fw-deploy-with-agent`
- Build the macOS host agent:
  - `cargo build -p host-agent --release --target aarch64-apple-darwin`
  - Or run `scripts/build-host-agent` (copies into the artifacts folder).
- Place binaries under `apps/host-agent/artifacts/<target>/`:
  - macOS: `apps/host-agent/artifacts/aarch64-apple-darwin/host-agent`
  - Windows (optional): `apps/host-agent/artifacts/x86_64-pc-windows-msvc/host-agent.exe`
  - Linux (optional): `apps/host-agent/artifacts/x86_64-unknown-linux-gnu/host-agent`
- Firmware builds embed a read-only 4 MiB FAT16 image at `OUT_DIR/host-agent.img`. Packaged binaries share this capacity; the build fails if their combined size plus FAT metadata exceeds it.
- The FAT image is an internal artifact container for CDC downloads; it is not exposed to the host as a USB drive. Its existing flash reservation and `.msc_image` section remain unchanged.
- `PICO_MSC_LABEL` only changes the internal FAT label; it does not enable a USB drive.
- Safe device checks without Wi-Fi or HID/file activity:
  - `scripts/device-test` runs raw protocol injection, restricted production host-agent transport tests, against installed firmware.
  - `scripts/device-test --flash` builds, verify-flashes, executes, and tests a BOOTSEL device.
  - See `docs/DEVICE_TESTING.md` for safeguards and exact coverage.
- Host agent credential prompt (macOS):
  - Responds to `TAG_DB_CREDENTIALS_REQUEST` with a native dialog (masked password).
  - For headless runs/tests, set `HOST_AGENT_DB_USER` and `HOST_AGENT_DB_PASSWORD`.


## License

MIT OR Apache-2.0.

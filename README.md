# pico_rust — Firmware + Web UI

This workspace contains three cooperating components:
- RP235x firmware for Raspberry Pi Pico 2 / 2 W (Cortex‑M33)
- A Yew Web UI compiled to WebAssembly and embedded into the firmware HTTP server
- A host-side serial agent for commands, filesystem browsing, credentials, and file transfer

The firmware build is wired so a single `cargo run -p pico_rust --release` builds the firmware and Web UI, flashes the board, and opens a serial log. Use `scripts/fw-deploy-with-agent` to package the local host agent as well. Contributors and automated agents should also read [AGENTS.md](AGENTS.md) for architecture constraints, cross-target workflows, and the validation matrix.

## Documentation

- [System architecture](docs/ARCHITECTURE.md): components, startup, lifecycle, data flow, backpressure, and RustPython execution.
- [Protocol reference](docs/PROTOCOL.md): USB TLV, WebSocket RPC/HELLO, transfer/filesystem layouts, versions, errors, and compatibility.
- [Threat model](docs/THREAT_MODEL.md): hostile source PC, trusted Pico/Wi-Fi clients, encryption guarantees, accepted limitations, and receiver hardening priorities.
- [Hardware and recovery](docs/HARDWARE.md): supported board, pin and memory maps, USB/Wi-Fi configuration, flashing, BOOTSEL, and smoke tests.
- [Python scripting](docs/SCRIPTING.md): current API, examples, process lifecycle, exceptions, limits, and Worker implementation.
- [USB device testing](docs/DEVICE_TESTING.md): safe hardware checks, commands, and connected-browser coverage gaps.
- [Throughput experiments](docs/THROUGHPUT_EXPERIMENTS.md): measured USB results, rejected optimizations, and remaining experiments.

## Overview
- Primary hardware target: Pimoroni Pico Plus 2 W (RP2350B) with 16 MiB QSPI flash, 8 MiB PSRAM, and 520 KiB SRAM.
- Default target is the host triple; firmware builds use `thumbv8m.main-none-eabihf`.
- The frontend crate (`apps/frontend/`) always targets `wasm32-unknown-unknown` and is built by Trunk from `firmware/build.rs`.
- A custom runner (`scripts/pico-run`) uses `picotool` to flash the ELF and then tails the USB CDC log.
- The built frontend files are embedded at compile time and served by the firmware under `/` and `/ui/*`.

### Memory configuration
- `firmware/memory.x` reserves 16 MiB of flash, splitting the final 8 KiB into a persistent configuration area.
- `firmware/src/device_config.rs` mirrors that layout via `FLASH_CAPACITY` (`16 * 1024 * 1024` bytes) and persists two 4 KiB slots.
- The `psram` feature is enabled by default so HTTP/WebSocket TCP windows and the singleton transfer-batch slot allocate from the 8 MiB external RAM exposed by Embassy.

## Prerequisites
- Rust targets
  - `rustup target add thumbv8m.main-none-eabihf`
  - `rustup target add wasm32-unknown-unknown`
- Trunk for building the Web UI
  - `cargo install trunk`
- wasm-bindgen CLI for the separately built RustPython Worker
  - `cargo install wasm-bindgen-cli --version 0.2.129 --locked`
  - The CLI version must match the workspace's `wasm-bindgen` dependency.
- Picotool for flashing
  - macOS: `brew install picotool` (or build from source: https://github.com/raspberrypi/picotool)

## Workspace Layout
- Firmware crate: `firmware/` (package: `pico_rust`)
  - Entry point: `firmware/src/main.rs`
  - HTTP server + embedded UI: `firmware/src/http/` (includes generated `frontend_static.rs`)
  - Linker script: `firmware/memory.x`
  - Build script: `firmware/build.rs` (delegates to `crates/build-support`)
- Frontend crate: `apps/frontend/`
  - Trunk config: `apps/frontend/Trunk.toml`
  - Forces `wasm32-unknown-unknown` via `apps/frontend/.cargo/config.toml`
- RustPython Worker: `apps/python-worker/`
- Language-neutral keyboard lowering: `crates/keyboard-core/`
- Strict firmware KBD1 executor: `crates/firmware-exec/`
- Build helper crate: `crates/build-support/`
  - `crates/build-support/src/lib.rs` orchestrates linker script setup and the Trunk pipeline
- Custom runner: `scripts/pico-run`

## Build: One‑shot
- Flash + run with logs, while also packaging the local host-agent into the USB drive image:
  - From repo root: `scripts/fw-deploy-with-agent`
- Flash + run with logs:
  - From repo root: `cargo run -p pico_rust --release --target thumbv8m.main-none-eabihf`
  - Or: `cd firmware && cargo run --release`
- Notes:
  - `scripts/fw-deploy-with-agent` first builds `host-agent` for the local machine's host target and copies it into `apps/host-agent/artifacts/<target>/`, then runs the normal firmware deploy command.
  - Set `HOST_AGENT_TARGET=<triple>` to override the detected host target if needed.
  - The runner uses `picotool load -u -x` and waits for a USB serial device (120s default). Use `--timeout=<secs>` or `--no-wait` after the ELF to change behavior.
  - If logs don’t appear immediately, the device may not have brought up USB CDC yet. The runner prints hints; you can also access the device over Wi‑Fi at `http://192.168.4.1/` and use the UI to trigger features.

## Build: Separate pieces
- Firmware only:
  - `cargo build -p pico_rust --release --target thumbv8m.main-none-eabihf`
  - Or: `cd firmware && cargo build --release`
- Frontend only (dev server):
  - `cd apps/frontend && trunk serve` (proxies API calls per `Trunk.toml`)
  - Without hardware, run `scripts/mock-pico`, then use
    `cd apps/frontend && trunk serve --config Trunk.mock.toml --open` in another terminal.
    The mock bridges the UI to a real host-agent process over a PTY;
    `scripts/mock-pico --no-host-agent` simulates firmware with no agent present.
- Frontend only (release build):
  - `cd apps/frontend && trunk build --release` (outputs to `apps/frontend/dist/`)

## Hardware keyboard payloads

The display's **Payloads** page works without a browser or a running host agent.
Use A/B to select, X to run, and A+X to enter or leave the sidebar. macOS agent
launch and debug presets are provided for German and US input layouts; select the
layout matching the host's active input source. The launchers open Terminal, copy
the packaged agent from the USB drive to `~/pico-agent/HOSTAGNT`, and start it.
The display distinguishes completed keyboard input from a detected agent handshake
and reports a timeout after 15 seconds without an agent. Missing packaged binaries
disable the launchers. The keyboard test types `Hello from Pico!` into the focused
application; focus a text editor before using it.

USB CDC installation is also available on native Apple Silicon macOS, without
mounting the USB drive or downloading anything. Package the agent and provision
a unique, short USB serial identity when building firmware:

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

The count excludes Terminal launch and Return. Stop an existing agent before
installation and allow up to 25 seconds for cached agent presence to expire;
the CDC install footer reports recent agent activity and changes to
**Agent not detected; press X to retry** when that cached presence expires.
Zero/multiple matching Picos are refused. Y cancels the device
transfer. Use Terminal Control-C if its receiver remains waiting. After the
installer has arrived, its watchdog bounds stalled downloads to 120 seconds.
Display and Web UI status distinguish transfer verification from an agent
handshake. Terminal may print `>` continuation prompts and `dd` job messages;
these are expected shell output during a successful install. Intel macOS is
unsupported by the current artifact. An optional
`PICO_CDC_INSTALL_DIR` in the Terminal environment selects another installation
directory. Details and remaining platform checks are in
[MACOS_CDC_BOOTSTRAP_PLAN.md](MACOS_CDC_BOOTSTRAP_PLAN.md).

The preset list scrolls to keep the selection visible and reserves space for
layout details and status. Status messages wrap across two rows; long labels use
ellipsis with the sidebar open. Menu gestures consume both A and X releases,
so closing the sidebar cannot also run the selected preset. Completion and the
agent-handshake timeout continue while other pages are visible.

Y stops the active keyboard job from any display page, including browser effects.
Jobs never preempt each other: a second Run returns busy. Cancellation is terminal,
with bounded key-release cleanup and no automatic restart after USB reconnect.
Payloads/menu controls and the Stop press are consumed locally rather than also
being delivered to a Python button-event handler. With no active job, Y retains
its LED-color behavior outside Payloads.

The Transfer page refreshes connection and transfer status automatically. Progress
is coalesced to 10 Hz; connection changes and terminal states update immediately.

## Host agent USB mass storage image
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
- The device exposes a USB drive (label `PICO_AGENT` by default) with:
  - `/MAC/HOSTAGNT`
  - `/WIN/HOSTAGNT.EXE` (if provided)
  - `/LINUX/HOSTAGNT` (if provided)
- Set `PICO_MSC_LABEL` to override the volume label (11 ASCII chars max).
- Safe device checks without Wi-Fi or HID/file activity:
  - `scripts/device-test` runs raw protocol injection, restricted production host-agent transport tests, and read-only MSC checks against installed firmware.
  - `scripts/device-test --flash` builds, verify-flashes, executes, and tests a BOOTSEL device.
  - See `docs/DEVICE_TESTING.md` for safeguards and exact coverage.
- Host agent credential prompt (macOS):
  - Responds to `TAG_DB_CREDENTIALS_REQUEST` with a native dialog (masked password).
  - For headless runs/tests, set `HOST_AGENT_DB_USER` and `HOST_AGENT_DB_PASSWORD`.

## Secure file transfer

- The USB-connected source PC is considered hostile; the Pico and receiving Wi-Fi clients are trusted. Encryption protects file contents from passive USB observers, but cannot hide source files or agent keys from that PC or make its files trustworthy. See the [threat model](docs/THREAT_MODEL.md).
- Start the host agent and open the Web UI. The browser automatically negotiates an encrypted file-transfer session.
- After the UI reports an encrypted session, select or enter a host file path and queue it.
- The host streams the file through bounded plaintext buffers, encrypts each record before USB transfer, and waits for an authenticated browser hash receipt.
- The Pico display reports transfer progress but intentionally cannot start a transfer without an active encrypted session. Plaintext transfer commands and legacy simulation/drop mode are disabled.
- `--send-file <path>` records a default candidate but does not bypass browser session negotiation.
- Unattended negotiation does not authenticate the browser: every client that can access the Pico Web UI can establish a session and request host files.
- Security guarantees and accepted limitations are in the [threat model](docs/THREAT_MODEL.md); session/data flow is in [architecture](docs/ARCHITECTURE.md#file-transfer-path-and-backpressure), and wire formats are in [protocol](docs/PROTOCOL.md#secure-file-transfer-protocol-v2).

## Source filesystem browser
- With the browser WebSocket connected and the host-agent running, the Web UI can browse the source computer's filesystem through the Pico.
- The browser starts in the host user's home directory and supports root/home/up navigation, breadcrumbs, hidden files, metadata, and paginated listings.
- Select a regular file to populate the manual transfer path or queue it directly.

## Testing
- Format the workspace: `cargo fmt --all -- --check`
- Host-agent unit and platform tests: `cargo test -p host-agent`
  - On macOS this includes the pseudo-terminal end-to-end suite; those tests are skipped on other platforms.
- Keyboard lowering tests with every layout enabled:
  - `cargo test -p keyboard-core --features "std layout_win_en_gb layout_win_pt_br layout_win_de_de layout_mac_en_gb layout_mac_pt_br layout_mac_de_de"`
- RustPython generator/process tests: `cargo test -p python-worker`
- Compile frontend wasm tests: `cargo test -p frontend --target wasm32-unknown-unknown --no-run`
  - The integration suite is configured to run in a browser and needs `wasm-bindgen-test-runner` plus a compatible browser/WebDriver setup for execution.
- Check the embedded target: `cargo check -p pico_rust --release --target thumbv8m.main-none-eabihf`

Avoid bare `cargo test` at the workspace root: the default member is the embedded firmware and its build script also prepares frontend and generated assets. See [AGENTS.md](AGENTS.md#validation-matrix) for change-specific validation.

## Cargo/Target Configuration
- Root config: `.cargo/config.toml`
  - No default target; host builds/tests use the host triple.
  - Embedded‑only flags are scoped under `[target.thumbv8m.main-none-eabihf]`:
    - `-C link-arg=--nmagic`, `-Tlink.x`, `-Tdefmt.x`
    - `-C target-cpu=cortex-m33`
- Custom runner: `runner = "./scripts/pico-run"`
- DEFMT log level: `[env] DEFMT_LOG = "debug"`
- Firmware config: `firmware/.cargo/config.toml`
  - `[build].target = "thumbv8m.main-none-eabihf"` keeps firmware builds on the MCU target.
- Frontend config: `apps/frontend/.cargo/config.toml`
  - Forces `wasm32-unknown-unknown` so UI builds are isolated from the firmware target.

## Build Script Behavior
- `firmware/build.rs` calls into the `build-support` crate to:
  1) Copy `firmware/memory.x` to `OUT_DIR` and add it to the linker search path.
  2) Fingerprint `apps/frontend/` and `apps/python-worker/` sources.
  3) If needed, invoke `trunk build --release` with a separate `CARGO_TARGET_DIR` inside `OUT_DIR`.
     - The Trunk subprocess environment is scrubbed so embedded `RUSTFLAGS` do not leak into the wasm build.
     - The build forces `CARGO_BUILD_TARGET=wasm32-unknown-unknown`.
     - Release assets are written to an isolated directory inside `OUT_DIR`; firmware builds never reuse the development output in `apps/frontend/dist/`.
  4) Build the real RustPython Worker, run wasm-bindgen, gzip the Worker WASM deterministically, and copy the Worker driver/glue into `OUT_DIR`.
  5) Normalize asset paths, gzip all HTML/CSS/JavaScript/WASM assets deterministically, and generate `frontend_static.rs` containing compressed `include_bytes!` declarations. HTTP responses retain their media types and include `Content-Encoding: gzip`; decompression happens in the browser.
  6) Apply frontend and compressed-Worker size gates.

### Environment knobs (optional)
- `PICO_WASM_WARN_BYTES` (default: `1200000`)
  - Emits a Cargo warning if `app.wasm` exceeds this many bytes.
- `PICO_WASM_MAX_BYTES` (unset by default)
  - Fails the build if `app.wasm` exceeds this many bytes.
- `PICO_PYTHON_WASM_WARN_BYTES` (default: `3500000`)
  - Warns when the stored gzip-compressed RustPython Worker exceeds this size.
- `PICO_PYTHON_WASM_MAX_BYTES` (default: `4750000`)
  - Fails the firmware build when the stored compressed Worker exceeds this size.

## Firmware Features
- Default features include `firmware` which pulls in Embassy RP, defmt, panic‑probe, CYW43 Wi‑Fi, DHCP, and the HTTP server.
- RP235x silicon selection is set to `rp235xb` (Pico 2 / 2 W shipping silicon). If you have A‑silicon, switch the `embassy-rp` feature to `rp235xa` in `firmware/Cargo.toml`.

## Troubleshooting
- “mach‑o section specifier requires a segment and section”
  - This happens if embedded crates compile for the host target. Run `cargo run -p pico_rust --release --target thumbv8m.main-none-eabihf` or build from `firmware/` where the target is set.
- Wasm build errors mentioning `--nmagic` or `-Tlink.x`
  - Those flags are for the MCU linker only. The Trunk subprocess environment is scrubbed to avoid leaking them; ensure you are building via `cargo run` (which runs `firmware/build.rs`) or run `trunk build` inside `apps/frontend/`.
- Trunk not installed / firmware frontend bundle missing
  - Install Trunk (`cargo install trunk`). A firmware build can reuse its own current release bundle from `OUT_DIR`, but it will not use `apps/frontend/dist/` because that directory may contain unoptimized `trunk serve` output.
- Picotool cannot find the device
  - Enter BOOTSEL mode (hold BOOTSEL while plugging in), or ensure the board is connected via USB. The runner uses `picotool load -u` to auto‑discover.

## Git Hygiene
- `target/` is ignored globally.
- `apps/frontend/dist/` is ignored local Trunk output for development or standalone release builds. Firmware builds use their isolated release output under Cargo's `OUT_DIR`.

## License
Licensed under either of
- Apache License, Version 2.0, or
- MIT license
at your option.

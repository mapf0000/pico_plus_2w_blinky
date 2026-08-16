# pico_rust — Firmware + Web UI

This workspace contains three cooperating components:
- RP235x firmware for Raspberry Pi Pico 2 / 2 W (Cortex‑M33)
- A Yew Web UI compiled to WebAssembly and embedded into the firmware HTTP server
- A host-side serial agent for commands, filesystem browsing, credentials, and file transfer

The firmware build is wired so a single `cargo run -p pico_rust --release` builds the firmware and Web UI, flashes the board, and opens a serial log. Use `scripts/fw-deploy-with-agent` to package the local host agent as well. Contributors and automated agents should also read [AGENTS.md](AGENTS.md) for architecture constraints, cross-target workflows, and the validation matrix.

## Documentation

- [System architecture](docs/ARCHITECTURE.md): components, startup, lifecycle, data flow, backpressure, and DSL execution.
- [Protocol reference](docs/PROTOCOL.md): USB TLV, WebSocket RPC/HELLO, transfer/filesystem layouts, versions, errors, and compatibility.
- [Hardware and recovery](docs/HARDWARE.md): supported board, pin and memory maps, USB/Wi-Fi configuration, flashing, BOOTSEL, and smoke tests.
- [Keyboard DSL](crates/dsl/README.md): language syntax, layouts, limits, and bytecode workflow.

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
- DSL crates: `crates/dsl/`
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
- Firmware builds embed a read-only 8 MiB FAT16 image at `OUT_DIR/host-agent.img`.
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

- Start the host agent and open the Web UI. The browser automatically negotiates an encrypted file-transfer session.
- After the UI reports an encrypted session, select or enter a host file path and queue it.
- The host streams the file through bounded plaintext buffers, encrypts each record before USB transfer, and waits for an authenticated browser hash receipt.
- The Pico display reports transfer progress but intentionally cannot start a transfer without an active encrypted session. Plaintext transfer commands and legacy simulation/drop mode are disabled.
- `--send-file <path>` records a default candidate but does not bypass browser session negotiation.
- Unattended negotiation does not authenticate the browser: every client that can access the Pico Web UI can establish a session and request host files.
- Security scope, trust assumptions, metadata leakage, and follow-up work are documented in [Secure file-transfer design](docs/SECURE_FILE_TRANSFER.md).

## Source filesystem browser
- With the browser WebSocket connected and the host-agent running, the Web UI can browse the source computer's filesystem through the Pico.
- The browser starts in the host user's home directory and supports root/home/up navigation, breadcrumbs, hidden files, metadata, and paginated listings.
- Select a regular file to populate the manual transfer path or queue it directly.

## Testing
- Format the workspace: `cargo fmt --all -- --check`
- Host-agent unit and platform tests: `cargo test -p host-agent`
  - On macOS this includes the pseudo-terminal end-to-end suite; those tests are skipped on other platforms.
- DSL tests with every keyboard layout enabled:
  - `cargo test -p dsl-core --features "std layout_win_en_gb layout_win_pt_br layout_win_de_de layout_mac_en_gb layout_mac_pt_br layout_mac_de_de"`
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
  2) Fingerprint the `apps/frontend/` sources (excluding `dist`, `target`, `.git`, `node_modules`) plus `crates/dsl/`.
  3) If needed, invoke `trunk build --release` with a separate `CARGO_TARGET_DIR` inside `OUT_DIR`.
     - The Trunk subprocess environment is scrubbed so embedded `RUSTFLAGS` do not leak into the wasm build.
     - The build forces `CARGO_BUILD_TARGET=wasm32-unknown-unknown`.
     - Release assets are written to an isolated directory inside `OUT_DIR`; firmware builds never reuse the development output in `apps/frontend/dist/`.
  4) Copy and normalize built assets into `OUT_DIR` and generate `frontend_static.rs` containing `include_*` declarations.
  5) Optional size checks controlled by environment variables (see below).

### Environment knobs (optional)
- `PICO_WASM_WARN_BYTES` (default: `1200000`)
  - Emits a Cargo warning if `app.wasm` exceeds this many bytes.
- `PICO_WASM_MAX_BYTES` (unset by default)
  - Fails the build if `app.wasm` exceeds this many bytes.

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

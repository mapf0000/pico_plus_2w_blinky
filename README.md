# pico_rust — Firmware + Web UI

This workspace builds two artifacts together:
- RP235x firmware for Raspberry Pi Pico 2 / 2 W (Cortex‑M33)
- A Yew Web UI compiled to WebAssembly and embedded into the firmware HTTP server

The build is wired so a single `cargo run --release` builds both parts, flashes the board, and opens a serial log.

## Overview
- Primary hardware target: Pimoroni Pico Plus 2 W (RP2350B) with 16 MiB QSPI flash, 8 MiB PSRAM, and 520 KiB SRAM.
- Default Cargo target is `thumbv8m.main-none-eabihf` for firmware.
- The frontend crate (`frontend/`) always targets `wasm32-unknown-unknown` and is built by Trunk from `build.rs`.
- A custom runner (`scripts/pico-run`) uses `picotool` to flash the ELF and then tails the USB CDC log.
- The built frontend files are embedded at compile time and served by the firmware under `/` and `/ui/*`.

### Memory configuration
- `memory.x` reserves 16 MiB of flash, splitting the final 8 KiB into a persistent configuration area.
- `src/device_config.rs` mirrors that layout via `FLASH_CAPACITY` (`16 * 1024 * 1024` bytes) and persists two 4 KiB slots.
- The PSRAM worker pool feature (`psram`) is enabled by default so HTTP buffers allocate from the 8 MiB external RAM exposed by the Embassy git HAL.

## Prerequisites
- Rust targets
  - `rustup target add thumbv8m.main-none-eabihf`
  - `rustup target add wasm32-unknown-unknown`
- Trunk for building the Web UI
  - `cargo install trunk`
- Picotool for flashing
  - macOS: `brew install picotool` (or build from source: https://github.com/raspberrypi/picotool)

## Workspace Layout
- Firmware crate (this package)
  - Entry point: `src/main.rs`
  - HTTP server + embedded UI: `src/http.rs` (includes generated `frontend_static.rs`)
  - Linker script: `memory.x`
  - Build script: `build.rs` (delegates to `build-support`)
- Frontend crate: `frontend/`
  - Trunk config: `frontend/Trunk.toml`
  - Forces `wasm32-unknown-unknown` via `frontend/.cargo/config.toml`
- Build helper crate: `build-support/`
  - `build-support/src/lib.rs` orchestrates linker script setup and the Trunk pipeline
- Custom runner: `scripts/pico-run`

## Host Agent (macOS)
- Build (use your host target, e.g. `aarch64-apple-darwin`):
  - `cargo build -p agentd --target aarch64-apple-darwin`
- Run (VID/PID auto-discovery):
  - `./target/aarch64-apple-darwin/debug/agentd vid=cafe pid=403f`
- Run (manual port):
  - `./target/aarch64-apple-darwin/debug/agentd --port /dev/cu.usbmodem*`
- Notes:
  - The agent speaks TLV over a dedicated USB CDC interface and responds to `RequestAgentStatus`.
  - Command execution can be triggered via the device WebSocket with `HOST_EXEC <cmd>`.
  - Mic data (if enabled) is appended to `mic.pcm` in the agent working directory.

## Build: One‑shot
- Flash + run with logs:
  - `cargo run --release`
- Notes:
  - The runner uses `picotool load -u -x` and waits for a USB serial device (120s default). Use `--timeout=<secs>` or `--no-wait` after the ELF to change behavior.
  - If logs don’t appear immediately, the device may not have brought up USB CDC yet. The runner prints hints; you can also access the device over Wi‑Fi at `http://192.168.4.1/` and use the UI to trigger features.

## Build: Separate pieces
- Firmware only:
  - `cargo build --release` (or `cargo run --release` to flash)
- Frontend only (dev server):
  - `cd frontend && trunk serve` (proxies API calls per `Trunk.toml`)
- Frontend only (release build):
  - `cd frontend && trunk build --release` (outputs to `frontend/dist/`)

## Testing
- Host-only unit tests (skip embedded dependencies):
  - `cargo test --no-default-features --features "" --target aarch64-apple-darwin`
    - Disables the default `firmware` feature so crates that expect a Cortex-M target are not pulled in when running on macOS/ARM.
- Frontend wasm tests:
  - `cargo test -p frontend --target wasm32-unknown-unknown --no-run`
  - Then execute the produced `.wasm` with `wasm-bindgen-test-runner target/wasm32-unknown-unknown/debug/deps/<name>.wasm` (install via `cargo install wasm-bindgen-test`).

## Cargo/Target Configuration
- Root config: `.cargo/config.toml`
  - `[build].target = "thumbv8m.main-none-eabihf"` makes embedded the default.
  - Embedded‑only flags are scoped under `[target.thumbv8m.main-none-eabihf]`:
    - `-C link-arg=--nmagic`, `-Tlink.x`, `-Tdefmt.x`
    - `-C target-cpu=cortex-m33`
  - Custom runner: `runner = "./scripts/pico-run"`
  - DEFMT log level: `[env] DEFMT_LOG = "debug"`
- Frontend config: `frontend/.cargo/config.toml`
  - Forces `wasm32-unknown-unknown` so UI builds are isolated from the firmware target.

## Build Script Behavior
- `build.rs` calls into the `build-support` crate to:
  1) Copy `memory.x` to `OUT_DIR` and add it to the linker search path.
  2) Fingerprint the `frontend/` sources (excluding `dist`, `target`, `.git`, `node_modules`).
  3) If needed, invoke `trunk build --release` with a separate `CARGO_TARGET_DIR` inside `OUT_DIR`.
     - The Trunk subprocess environment is scrubbed so embedded `RUSTFLAGS` do not leak into the wasm build.
     - The build forces `CARGO_BUILD_TARGET=wasm32-unknown-unknown`.
  4) Copy and normalize built assets into `OUT_DIR` and generate `frontend_static.rs` containing `include_*` declarations.
  5) Optional size checks controlled by environment variables (see below).

### Environment knobs (optional)
- `PICO_WASM_WARN_BYTES` (default: `1200000`)
  - Emits a Cargo warning if `app.wasm` exceeds this many bytes.
- `PICO_WASM_MAX_BYTES` (unset by default)
  - Fails the build if `app.wasm` exceeds this many bytes.

## Firmware Features
- Default features include `firmware` which pulls in Embassy RP, defmt, panic‑probe, CYW43 Wi‑Fi, DHCP, and the HTTP server.
- RP235x silicon selection is set to `rp235xb` (Pico 2 / 2 W shipping silicon). If you have A‑silicon, switch the `embassy-rp` feature to `rp235xa` in `Cargo.toml`.

## Troubleshooting
- “mach‑o section specifier requires a segment and section”
  - This happens if embedded crates compile for the host (macOS) target. The workspace default target fixes this. Alternatively, run `cargo run --release --target thumbv8m.main-none-eabihf` when the default is not set.
- Wasm build errors mentioning `--nmagic` or `-Tlink.x`
  - Those flags are for the MCU linker only. The Trunk subprocess environment is scrubbed to avoid leaking them; ensure you are building via `cargo run` (which runs `build.rs`) or run `trunk build` inside `frontend/`.
- Trunk not installed / `frontend/dist` missing
  - Install Trunk (`cargo install trunk`) or pre‑build the frontend (`trunk build --release`). The firmware build will fail if `dist/` is missing and Trunk is unavailable.
- Picotool cannot find the device
  - Enter BOOTSEL mode (hold BOOTSEL while plugging in), or ensure the board is connected via USB. The runner uses `picotool load -u` to auto‑discover.

## Git Hygiene
- `target/` is ignored globally.
- `frontend/dist/` is ignored (release UI output from Trunk).

## License
Licensed under either of
- Apache License, Version 2.0, or
- MIT license
at your option.

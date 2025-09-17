# pico_rust – Pico W USB‑on‑demand AP

This firmware brings up the Raspberry Pi Pico 2W as a WPA2 Access Point (CYW43 via PIO‑SPI) with a tiny HTTP server and a tiny in‑device web UI. It only enumerates as a USB device when it receives a specific HTTP POST request (or when triggered from the web UI).

HTTP endpoints
- `GET /` or `/ui`: Serves the compiled WebAssembly frontend (Yew) if built (falls back to a stub if skipped).
- `GET /status`: Returns `{ usb_enabled, usb_ready, host_os }`.
- `POST /usb/register[?assistant=1&os=mac|windows]`: Start USB; optionally force macOS Assistant and set host OS.
- `POST /kb/script` with body: Executes a simple line-based keyboard DSL.
  

Scripting DSL
- Commands (case-insensitive):
  - `tap KEY`
  - `modtap MOD+MOD+KEY` (MOD: LCTRL, LSHIFT, LALT, LGUI, RCTRL, RSHIFT, RALT, RGUI; aliases: CTRL, SHIFT, ALT, GUI/CMD/WIN, OPTION/CONTROL)
  - `delay MS` (0..5000)
  - `text STRING [DELAY]` (optional per-char delay)
- Lines starting with `#` or empty lines are ignored.
- Payload limited to 512 bytes; max 256 non-empty lines.

Wi‑Fi
- AP SSID: `PicoEndpoint`
- AP passphrase: `pico12345`
- Static IP: `192.168.4.1/24`

Quick test
1. Build and flash as usual.
2. Connect a phone/laptop to the `PicoEndpoint` Wi‑Fi using password `pico12345`.
3. Open http://192.168.4.1/ in a browser and use the UI to Start USB (Assistant or No Assistant). Alternatively:
   - `curl -X POST "http://192.168.4.1/usb/register?assistant=1&os=mac"`
4. Plug into a host via USB; the device will now enumerate (composite CDC + HID).

Frontend
- Built automatically during firmware compilation via `build.rs` using Trunk.
- Requirements (host): `cargo install trunk` and `rustup target add wasm32-unknown-unknown`.
- Access on device: `GET /` (alias: `/ui`); assets at `/ui/app.js` and `/ui/app.wasm`.
- Skip during build: set `PICO_SKIP_FRONTEND=1` env var.
  - Speed options:
    - `PICO_FRONTEND_MODE=dist`: reuse `frontend/dist` and skip invoking Trunk.
    - `PICO_FRONTEND_MODE=trunk`: force a Trunk build (default for release when available).
    - `PICO_REQUIRE_FRONTEND=1`: fail build if embedding fails.
- Develop separately: in `frontend/` run `trunk serve --open` for a hot‑reload dev server; `trunk build --release` to produce `frontend/dist/`.
- Build warning: build.rs emits a warning if the generated WASM exceeds a threshold (default 1.2 MiB). Override via `PICO_WASM_WARN_BYTES=...` if needed.

Dev aliases and scripts
- Cargo aliases (see `.cargo/config.toml`):
  - `cargo check-host` (Apple Silicon): host check without embedded features
  - `cargo check-host-intel` (Intel macOS)
  - `cargo check-host-linux` (Linux)
  - `cargo clippy-host` (warnings as errors)
  - `cargo fmt-check` (format check)
  - `cargo run-flash` (same as `cargo run --release`): builds firmware (and the frontend in release), flashes via picotool, then waits for a USB CDC device and streams logs.
    - Default wait: 120s. Change via `PICO_WAIT_USB_TIMEOUT=<secs>` or runner flag `--timeout=<secs>` (after `--`).
    - Skip waiting: `cargo run --release -- --no-wait` (just flashes and exits).
    - Trigger USB over Wi‑Fi (http://192.168.4.1) to start logs. Use `PICO_SKIP_FRONTEND=1` to skip embedding the frontend during fast iterations.
- Frontend dev: `cd frontend && trunk serve --open`

CI
- GitHub Actions workflow `.github/workflows/ci.yml` runs:
  - Host checks: `fmt`, `clippy`, and `check` with `PICO_SKIP_FRONTEND=1`.
  - Embed check: installs Trunk, builds the frontend, then runs `cargo check --features firmware --target thumbv8m.main-none-eabihf` which executes `build.rs` and embeds assets.
- Size guard:
  - `PICO_WASM_WARN_BYTES` warns when the WASM exceeds this many bytes (default 1.2 MiB).
  - `PICO_WASM_MAX_BYTES` (CI step) fails the embed-check if exceeded.

Environment variables
- `PICO_SKIP_FRONTEND=1`: skip Trunk and embed a stub; useful for host checks and fast iteration.
- `PICO_WASM_WARN_BYTES=<bytes>`: adjust the size warning threshold.
- `PICO_WASM_MAX_BYTES=<bytes>`: hard fail when embedding if the WASM exceeds this (used in CI).

**Host Tests**
- Why: most parsing and DSL logic is platform-agnostic; you can run unit tests on macOS without hardware.
- Alias (Apple Silicon): run `cargo test-host`.
- Alias (Intel Macs): run `cargo test-host-intel`.
- Manual command (Apple Silicon): `cargo test --lib --no-default-features --target aarch64-apple-darwin`.
- Manual command (Intel): `cargo test --lib --no-default-features --target x86_64-apple-darwin`.
- Note: `.cargo/config.toml` sets an embedded default target; passing `--target` and `--no-default-features` avoids pulling in RP-only crates.

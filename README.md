# pico_rust – Pico W USB‑on‑demand AP

This firmware brings up the Raspberry Pi Pico 2W as a WPA2 Access Point (CYW43 via PIO‑SPI) with a tiny HTTP server and a tiny in‑device web UI. It only enumerates as a USB device when it receives a specific HTTP POST request (or when triggered from the web UI).

HTTP endpoints
- `GET /`: Serves a minimal web UI for starting USB and sending actions.
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

**Host Tests**
- Why: most parsing and DSL logic is platform-agnostic; you can run unit tests on macOS without hardware.
- Alias (Apple Silicon): run `cargo test-host`.
- Alias (Intel Macs): run `cargo test-host-intel`.
- Manual command (Apple Silicon): `cargo test --lib --no-default-features --target aarch64-apple-darwin`.
- Manual command (Intel): `cargo test --lib --no-default-features --target x86_64-apple-darwin`.
- Note: `.cargo/config.toml` sets an embedded default target; passing `--target` and `--no-default-features` avoids pulling in RP-only crates.

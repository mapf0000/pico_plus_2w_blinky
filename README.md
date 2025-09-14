# pico_rust – Pico W USB‑on‑demand AP

This firmware brings up the Raspberry Pi Pico W as a WPA2 Access Point (CYW43 via PIO‑SPI) with a tiny HTTP server and a tiny in‑device web UI. It only enumerates as a USB device when it receives a specific HTTP POST request (or when triggered from the web UI).

HTTP endpoints
- `GET /`: Serves a minimal web UI for starting USB and sending actions.
- `GET /status`: Returns `{ usb_enabled, usb_ready, host_os }`.
- `POST /usb/register[?assistant=1&os=mac|windows]`: Start USB; optionally force macOS Assistant and set host OS.
- `POST /kb/type[?delay_ms=10]` with body: Types the provided text via HID.
- `POST /automation/open_macos_terminal`: Opens Terminal (macOS Spotlight flow).
- `POST /automation/mac_assistant`: Runs the macOS Keyboard Setup Assistant sequence.

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

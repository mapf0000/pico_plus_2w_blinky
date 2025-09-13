# pico_rust – Pico W USB‑on‑demand AP

This firmware brings up the Raspberry Pi Pico W as a WPA2 Access Point (CYW43 via PIO‑SPI) with a tiny HTTP server. It only enumerates as a USB device when it receives a specific HTTP POST request.

Endpoints
- `GET /`: Health check, returns `OK`.
- `POST /usb/register`: Triggers USB composite (CDC logger + HID keyboard) to start.

Wi‑Fi
- AP SSID: `PicoEndpoint`
- AP passphrase: `pico12345`
- Static IP: `192.168.4.1/24`

Quick test
1. Build and flash as usual.
2. Connect a phone/laptop to the `PicoEndpoint` Wi‑Fi using password `pico12345`.
3. `curl -X POST http://192.168.4.1/usb/register`
4. Plug into a host via USB; the device will now enumerate (composite CDC + HID).

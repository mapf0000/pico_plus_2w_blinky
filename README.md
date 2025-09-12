# pico_rust – Pico W temp sensor

This firmware connects the Raspberry Pi Pico W to Wi‑Fi (CYW43 via PIO-SPI), periodically samples the RP2040’s internal temperature sensor, and exposes it over HTTP.

Endpoints
- `GET /temp`: JSON with fields `c`, `f`, `uptime_ms`, `valid`.
- `GET /metrics`: Prometheus text with `pico_temperature_celsius`.

Notes
- The RP2040 internal sensor is not factory‑calibrated and is mainly suited for relative temperature changes. You can adjust the constants in `src/temp.rs` if you calibrate per device.
- Sampling runs at 1 Hz with a small EMA filter to reduce noise.

Quick test
1. Build and flash as usual (USB logger is enabled).
2. After DHCP, note the IP in logs.
3. `curl http://<device-ip>/temp`

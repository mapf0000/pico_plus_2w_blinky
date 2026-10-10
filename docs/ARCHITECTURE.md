# Architecture

The project has one remote controller transport: a native companion over BLE.
WLAN/AP, HTTP/WebSocket, browser assets and RustPython Worker have been removed.

```text
Control PC (egui) <--- authenticated BLE GATT ---> Pico Plus 2 W
                                                  | USB HID keyboard
                                                  | USB control CDC TLV
                                                  v
                                            USB-connected target + host agent
```

`apps/companion/src/app.rs` renders snapshots and enqueues actions. A bounded
Tokio backend owns `companion-core::Client`; the btleplug actor owns all radio
objects and asynchronous I/O. Connection incarnation plus request IDs scope
commands and replies. Scan/connect/status, authentication/control, USB controls,
KBD1 text execution and cancellation share one pending-request state machine.
Reset and cancel bypass the four-slot normal command queue. No device I/O runs
in egui callbacks. Reconnect is explicit; effects are never replayed.

`firmware/src/main.rs` initializes the board, USB supervisor, watchdog, display,
PSRAM diagnostics, persistent USB identity and CYW43 once. Its mandatory
`new_with_bluetooth` driver uses the board's existing RM2 PIO pins/divider/DMA.
No network stack, CLM/AP initialization, DHCP, TCP or asset-serving task starts.
`firmware/src/ble.rs` runs one TrouBLE peripheral with signalling, ATT and SMP.

Info/status/result are public bounded reads. Reading the pairing gate initiates
OS pairing. Numeric comparison appears on the Pico display; X can confirm only
after that code has been rendered. Y rejects and also preserves device-wide
Stop. Confirmation expires after 30 seconds. Commands require both an
authenticated encrypted connection and this physical confirmation. No persistent
bonding or flash-format change is introduced.

Each BLE connection owns one fixed 4,125-byte fragment receiver, a monotonic
16-bit message-token sequence and a control lease. A newer token drops a partial
upload, allowing cancel to bypass it. Partial uploads expire after five seconds.
`ble_control::Session` allocates a checked nonreused 32-bit incarnation. Its Drop
invalidates completion delivery, clears pairing and cancels only its remote
keyboard job. Local display jobs remain independent.

`usb/hid.rs` retains the shared non-preempting execution service: strict KBD1
validation, one reservation through all-zero key-release cleanup, one terminal
completion and physical Y Stop. Owners are `Local` or `Companion` with incarnation
and full effect IDs. Remote completion is synchronously published to the
connection-scoped result slot before admission is released; it never waits for
BLE I/O. The result persists until a subsequent command. The companion polls it
until a correlated terminal result, timeout or disconnect. A stalled observer
cannot retain the HID reservation forever.

USB CDC control framing, host-agent dispatcher, encrypted bulk formats and
bootstrap remain compatible. Browser bulk delivery is unavailable:
`usb/events.rs` explicitly refuses relay delivery, so firmware aborts instead
of acknowledging chunks without a receiver. A BLE status/control connection
never counts as a bulk receiver. Native bulk workflows require a separate design.

The build helper installs `memory.x`, embeds the internal four-MiB FAT16 agent
artifact and generates typed keyboard presets. It has no subprocess browser
build or interpreter assets. The internal image is not a USB mass-storage drive.
Persistent USB identity remains readable at the original two flash slots.

## Bounds

| Resource | Bound |
| --- | --- |
| BLE peripheral connections | 1 |
| L2CAP channels | 3 (signalling, ATT, SMP) |
| Shared BLE packet pool | 8 × 128 bytes, plus fixed metadata |
| GATT command write | 20 bytes maximum; 8-byte header, 12-byte payload |
| Reassembly | 1 × 4,125 bytes |
| Result value | 1 × 20 bytes; one active effect correlation |
| HID program/queued command | 4,096-byte maximum; one reservation |
| Companion radio requests/events | 4 / 16 |
| Companion device list/diagnostics | 16 / 128 |
| Persistent bond storage | none; pairing each connection |

The CYW43 vendor firmware and driver still contain combined radio support;
removing the WLAN application does not remove the hardware's required base blob.
PSRAM no longer owns HTTP/WebSocket or batch buffers. Linked measurements and
hardware evidence belong in [DEVICE_TESTING.md](DEVICE_TESTING.md).

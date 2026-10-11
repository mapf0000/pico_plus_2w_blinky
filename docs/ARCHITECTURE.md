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
Status/overview reads are background work and do not disable controls. One bounded
user intent may wait behind a read and is revalidated on completion. Queued text
can be cancelled before submission; disconnect, failure and timeout discard it.
The UI busy state covers user work only. Diagnostics retain bounded static messages
with monotonic timestamps, connection incarnation, operation context and errors;
filter/clear/autoscroll are native UI controls.
Reset and cancel bypass the four-slot normal command queue. No device I/O runs
in egui callbacks. Reconnect is explicit; effects are never replayed.

`firmware/src/main.rs` initializes the board, USB supervisor, watchdog, display,
PSRAM diagnostics, persistent USB identity and CYW43 once. Its mandatory
`new_with_bluetooth` driver uses the board's existing RM2 PIO pins/divider/DMA.
No network stack, CLM/AP initialization, DHCP, TCP or asset-serving task starts.
`firmware/src/ble.rs` runs one TrouBLE peripheral with signalling and ATT; SMP is disabled.

Info/status are public bounded diagnostics. Auth writes and sequential Result reads
carry empty-payload `Noise_NNpsk0_25519_ChaChaPoly_SHA256` handshake messages.
`ble-session` owns fixed-buffer Noise and directional AEAD record keys; the
provisioned key/device ID binds the prologue. SMP pairing is not built or required.
Connect authenticates when a profile is supplied; an encrypted Acquire grants the
lease. Commands and result/status snapshots remain encrypted for the connection.
Acquired ReadOverview reuses `capabilities.rs` heartbeat/version/OS state and
`usb/bootstrap.rs` phase/availability. A finite versioned record reports those
alongside full build and execution/USB limits. It refreshes separately at ten
seconds while idle; the public discovery values and ordinary snapshots stay small.
Info flag 32 separately negotiates native filesystem/button streaming; USB version
numbers alone do not advertise it.
An unauthenticated connection has a fixed 30-second lifetime, even under traffic.

Each BLE connection owns one fixed 4,142-byte ciphertext receiver, a 4,126-byte
plaintext scratch buffer, a 304-byte finite response slot, independent 16-bit
record sequences and a control lease. A newer token drops a partial upload,
allowing Cancel to bypass it without nonce reuse. Partial uploads expire after
five seconds. Authenticated Poll replaces any partially read response and retains
the current effect's result. No effect is retried. `ble_control::Session` allocates
a checked nonreused 32-bit incarnation. Drop invalidates completion delivery and
cancels only its remote job; local display jobs remain independent. Session keys
and caller-owned plaintext/ephemeral buffers are zeroized. Upstream Noise scratch
is not claimed to be comprehensively wiped.

`usb/hid.rs` retains the shared non-preempting execution service: strict KBD1
validation, one reservation through all-zero key-release cleanup, one terminal
completion and physical Y Stop. Owners are `Local` or `Companion` with incarnation
and full effect IDs. Remote completion is synchronously published to the
connection-scoped logical result slot before admission is released; it never waits for
BLE I/O. The result persists until a subsequent command. The companion requests encrypted snapshots
until a correlated terminal result, timeout or disconnect. A stalled observer
cannot retain the HID reservation forever.

USB CDC control framing, host-agent dispatcher, encrypted bulk formats and
bootstrap remain compatible. Browser bulk delivery is unavailable:
`usb/events.rs` explicitly refuses relay delivery, so firmware aborts instead
of acknowledging chunks without a receiver. Filesystem pages use a separate
`ble_relay_core` ownership/queue policy and `ble_relay` CDC adapter; generic binary
queues stay unavailable. One page and eight released buttons feed one 2,070-byte
encrypted ciphertext sender with four application credits. The native radio actor
subscribes, validates/decrypts and acknowledges bounded batches; it also pumps
notifications between HID polls. Finite results have separate nonce/replay state.
`companion-core::filesystem` owns the bounded native browser model. A BLE status/control connection
never counts as a bulk receiver. Native bulk workflows require a separate design.

The build helper installs `memory.x`, embeds the internal four-MiB FAT16 agent
artifact and generates typed keyboard presets. It has no subprocess browser
build or interpreter assets. The internal image is not a USB mass-storage drive.
`device_config_core` owns the portable USB identity validation, legacy schema,
CRC/sequence selection and verified inactive-slot transaction. `device_config.rs`
adapts it to the single Embassy flash driver and RAM configuration. Encrypted
acquired BLE requests read or save identity; the shared USB lifecycle mutex blocks
start/stop while saving. The USB task removes the hardware pull-up and stops
controller/interrupt sources before acknowledging shutdown or releasing session
buffers; this compensates for the pinned driver's empty disable hook.
Saving requires fully stopped USB and no reserved HID/CDC
work. Descriptors consume the committed configuration on the next USB start.
The native core owns bounded commands and save/readback results; egui owns the
draft fields and queues operations. Background reads preserve drafts, and disconnect never replays
persistent writes. The original two identity flash slots retain their addresses.
A separate two-slot 8-KiB control-key reservation precedes the unchanged image.
Firmware reads it through the shared flash driver at BLE startup. The companion
creates private factory provisioning files; no BLE/USB key export exists. OS
credential-store import and runtime key rotation are not implemented.

## Bounds

| Resource | Bound |
| --- | --- |
| BLE peripheral connections | 1 |
| L2CAP channels | 3 reserved (signalling and ATT used; SMP disabled) |
| Shared BLE packet pool | 8 × 128 bytes, plus fixed metadata |
| GATT command write | 20 bytes maximum; 8-byte header, 12-byte payload |
| Reassembly | 1 × 4,142 ciphertext bytes + 4,126-byte plaintext scratch |
| Response | 1 × 304 bytes; ordinary snapshot 48; sequential 20-byte reads; one active effect correlation |
| Stream sender / relay queue | 1 × 2,070 ciphertext bytes / 1 × 2,048-byte page + 8 buttons; 4 fragment credits |
| Native filesystem / button history | 512 entries / 32 releases; reset on disconnect |
| HID program/queued command | 4,096-byte maximum; one reservation |
| Companion radio requests/events | 4 / 16 |
| Companion device list/diagnostics | 16 / 128 |
| Companion intent waiting behind background read | 1; text at most 4,096 bytes / 1,024 characters |
| Persistent bond storage | none; fresh application-layer authentication each connection |

The CYW43 vendor firmware and driver still contain combined radio support;
removing the WLAN application does not remove the hardware's required base blob.
PSRAM no longer owns HTTP/WebSocket or batch buffers. Linked measurements and
hardware evidence belong in [DEVICE_TESTING.md](DEVICE_TESTING.md).

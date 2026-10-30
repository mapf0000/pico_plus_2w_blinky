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

`apps/companion/src/app.rs` renders snapshots and enqueues actions. Its child
`ui/shell.rs` composes task navigation, persistent connection/owned-job panels and
an optional activity drawer. `ui/state.rs` derives presentation/readiness from
core snapshots and caches bounded text validation; it owns no transport state.
`ui/theme.rs` applies neutral canvas/chrome/card tokens in light/dark themes;
`ui/components.rs` owns filled/quiet/Stop buttons, status badges and focus rings.
One footer retains Stop and Activity; the toolbar Device menu owns Release and
Disconnect. Connected discovery renders a summary rather than completed steps. Child modules `ui/devices.rs`
and `ui/scripts.rs` render explicit per-device Connect/acquire controls and script
readiness/recovery. Only the unchanged known demo declares UI USB prerequisites;
arbitrary scripts remain runnable offline. Traceback-derived recovery hints are
presentation only; bounded interpreter IPC and effect admission are unchanged. Task drafts stay in the app across
navigation; existing epoch invalidation still clears device identity on a new
connection. Stop routes through the Python supervisor for its active run and
through scoped Cancel for a manual/unrelated effect. Download cancel is separate.
No view preferences or drafts are persisted by this UI slice. A bounded
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

`desktop.rs` owns one background local operation, with one request/result slot.
Native dialog futures start on the window thread and are awaited on that worker;
profile loading, Keychain and destination filesystem checks stay off egui and the
radio/backend executors. Destination replies carry their connection epoch/state and
original draft, so late dialogs cannot replace newer work. Closing a window does
not join a blocked native permission/dialog operation or defer BLE/HID cleanup.

`profiles.rs` separates a bounded non-secret catalog from macOS Keychain records.
Import is add-only for credentials, with an exclusive OS-released catalog lock
and verified input bounds. Startup restores the last selection unless a CLI
profile overrides it. Active profile replacement goes through a bounded backend
intent, requires no live connection/owned job/active Python, advances the core
epoch and drains the old radio before scanning with the replacement. Secret
profiles never enter snapshots, diagnostics or Python. Reconnect scans for the
previous device and authenticates fresh, without reacquiring control or replay.
`scripts/build-companion` produces a local macOS bundle with Bluetooth permission
metadata; the Python helper remains the same executable in worker mode.

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

USB CDC framing, host-agent dispatcher, transfer-v2 crypto and bootstrap remain
compatible. Info flag 64 separately enables native downloads, independently of
filesystem flag 32. `usb/events.rs` maps retained internal binary kinds to one
bounded transfer record in `ble_download_core`; `ble_download` scopes it to acquired
control. Its ticket remains owned until the BLE sender receives the native final
ACK. Cancel/reset release the USB wait and cancel its relay state. A BLE-only
barrier drains cancelled bytes before another host session starts. File keys stay
in the agent and native receiver; the Pico only validates public structure/order.

`downloads.rs` and the recovered `download_crypto.rs` implement a separate host
Noise session and one bounded file receiver. The radio actor retains the owned
async disk task across normal command/refresh preemption, and withholds the final
record ACK until authentication/disk work finish. ACKs flush before new controls;
sealed host controls retain nonce order and cannot be replayed after interruption.
The actor retains one 20-byte ATT write across preemption and owns the full
two-fragment ACK until completion, before sending a newer command token. Cleanup
bounds the drain and closes the peripheral before a new connection incarnation.
The task streams plaintext into an anonymous file, hashes incrementally, verifies
manifest/close totals and digest, copies with bounded buffers to a private save
file, fsyncs and persists without overwrite before issuing the encrypted receipt.
No untrusted filename determines a local path and no saved content executes.
Four requests/eight history entries and a provisional 1-MiB file limit are bounded.
Source errors, slow/full disk, control/USB loss, disconnect and app exit terminate
work without automatic retry. See DOWNLOADS.md for crash and filesystem limits.

Filesystem pages use the independent `ble_relay_core` policy and CDC adapter.
One page, eight buttons and one transfer record feed the single 2,071-byte encrypted
sender with four fragment credits. Native notification reassembly is bounded and
has distinct nonce/replay state from finite replies. Notifications are pumped
between HID polls; general credential/text events remain unavailable.

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

## Native Python process and effect path

The companion links python-runtime (RustPython 0.6.0/Ruff 0.16.5) but initializes
no VM until Start. The Tokio backend owns a native Supervisor and a lazy process
actor. The actor executes the same binary in private --python-worker mode with
cleared environment and pipe-only bounded versioned IPC. The child never loads a
profile, initializes egui/BLE or receives radio handles. It owns a restricted VM;
the parent owns 30-second startup/500-ms subsequent step deadlines, hard kill and
reaping. Child error/exit/invalid IPC faults only its correlated run.

The source editor renders separate script snapshots. Start/Restart/Stop are
bounded intents; run IDs scope Stop/child replies and connection incarnations scope
device work. Native local scheduling implements sleep, event waits and capability
checks. Only buttons buffer (32/drop oldest); capability transitions are delivered
to current matching waits. Child steps use generator.send/throw. Disconnect does
not restart the interpreter; an outstanding effect raises without output replay.

General keyboard effects are strictly validated by companion-core, assigned fresh
script-v1 IDs and scoped to a native run. The manual text demo shares this path
and is disabled during scripting. Stop cancels only its run's queued/active HID
job; firmware key release and local-preset independence are unchanged. No new BLE
or USB format is required. App shutdown kills/reaps the child and disconnects.

RustPython host_env/stdio/threading/default features and native stdlib are off;
VM initialization rejects host modules/FileIO. The child enforces a 128-MiB live
Rust allocation budget, including compiler/IPC. This is not an RSS limit or an OS
sandbox against native VM bugs; native runtime code remains trusted. Source and
bounded errors stay in memory/UI. See SCRIPTING.md and THREAT_MODEL.md.

## Bounds

| Resource | Bound |
| --- | --- |
| Native downloads | One receiver/task; four requests/eight history entries; 1 MiB file; bounded anonymous staging and verified save copy |
| Native Python / yielded effect | One lazy child / one effect |
| Python Rust heap / private IPC | 128 MiB live allocations / 256 KiB frame; version/run/step checked |
| Python source / synchronous execution | 32 KiB, 1,024 lines / 30-second startup, 500-ms later step |
| BLE peripheral connections | 1 |
| L2CAP channels | 3 reserved (signalling and ATT used; SMP disabled) |
| Shared BLE packet pool | 8 × 128 bytes, plus fixed metadata |
| GATT command write | 20 bytes maximum; 8-byte header, 12-byte payload |
| Reassembly | 1 × 4,142 ciphertext bytes + 4,126-byte plaintext scratch |
| Response | 1 × 304 bytes; ordinary snapshot 48; sequential 20-byte reads; one active effect correlation |
| Stream sender / relay queue | 1 × 2,071 ciphertext bytes / 1 × 2,048-byte page + 8 buttons; 4 fragment credits |
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

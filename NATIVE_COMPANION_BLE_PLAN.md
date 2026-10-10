# Native companion and Bluetooth plan

Updated 2026-10-10 after the deployed Bluetooth keyboard demonstration milestone.

## Scope decision

The user requested: **“rip wlan out and implement bluetooth.”** This supersedes
the original coexistence plan. Removed WLAN AP/DHCP/TCP/HTTP/WebSocket, browser
frontend, RustPython Worker, generated asset pipeline and obsolete browser mocks.
No transport registry, dual-controller arbitration or WLAN fallback is needed.

The product is a native Rust/egui app controlling the Pico over authenticated
Bluetooth LE. The Pico remains the USB HID/CDC endpoint for the target computer.
Keep the implementation small: one BLE connection, one pending logical request/
effect, bounded fragmentation and the existing keyboard executor. Native Python,
bulk file transfer, filesystem browsing and credential workflows are separate
future features, not requirements for the current keyboard milestone.

## Milestone status

| Milestone | Status | Evidence / remaining gate |
| --- | --- | --- |
| 1. Native shell and client | Complete | egui/Glow window, bounded Tokio backend, production KBD1 lowering, deterministic mock lifecycle, correlation/cancellation tests |
| 2. Read-only BLE feasibility | Complete | actual macOS/Pico discovery, information, live status and reconnect passed; former WLAN coexistence gate withdrawn |
| 3. Bluetooth-only keyboard demonstration | Complete and hardware accepted | WLAN/browser removal, BLE v2, numeric comparison, scoped completion, USB controls and text/cancel; verified flash, BLE/USB smoke, user-confirmed pairing, exact demo output and delayed cancellation |
| 4. Hardware edge cases | Next | X/Y reject/timeout, physical Y Stop, USB enable/disable/detach, reconnect/re-pair, near-limit upload and local contention |
| 5. Reliability and packaging | Planned after 4 | reconnect/forget/re-pair, repeated cycles/soak, measured RTT/memory/CPU/startup, platform builds, app bundle/permissions |
| 6. Additional native workflows | Deferred | choose separately: host metadata/commands, authenticated bulk receiver, filesystem, supervised native scripting |

The updated native companion is open. The user confirmed acquired control after
PC/Pico pairing and then confirmed real demo text output and delayed cancellation.
These establish the first authenticated keyboard demonstration; broader fault/
platform acceptance is still pending. Evidence is in DEVICE_TESTING.md.

## Implemented architecture

```text
Control PC                        Pico Plus 2 W             USB target PC
native egui UI                    BLE GATT + pairing
    |                                  |
companion-core client <--- BLE ---> session-scoped control
    |                                  |
btleplug async actor              existing HID executor ---> keyboard input
                                       +--- USB CDC ------> host agent
```

- `apps/companion`: egui/eframe with Glow, bounded backend, btleplug radio actor.
  Bluetooth is default; `--mock` is explicit. No firmware/browser build dependency.
- `companion-core`: one pending request, connection incarnations, deadlines,
  supported layouts, keyboard lowering and deterministic mock.
- `ble-protocol`: shared `no_std` v2 codecs and fixed 4,125-byte reassembly.
- `firmware/src/ble.rs`: one TrouBLE/CYW43 peripheral, pairing and dispatch.
- `firmware/src/ble_control.rs`: checked nonreused incarnation, pairing display
  state, active effect correlation and fixed terminal result slot.
- `usb/hid.rs`: local/companion owners, strict validator, non-preemption, scoped
  cancel, key-release cleanup, independent local completion and physical Y Stop.
- `usb/events.rs`: explicitly unavailable bulk sink. A BLE control connection
  cannot cause file ACKs without a native receiver. Existing USB tags remain.
- `build-support`: linker, internal CDC agent image and typed presets only.

The host radio actor uses four queued requests and sixteen events. Priority
reset/cancel interrupts upload/result polling rather than waiting behind Run.
Old-generation requests/results are discarded and execution is never replayed.
Firmware publishes completion before releasing HID admission without awaiting
radio output. Result reads avoid a notification subscription/receipt queue.

BLE writes are at most 20 bytes, including an eight-byte header. A monotonic
nonzero connection token plus exact offsets scopes each logical message.
New tokens abandon partial upload, permitting prompt cancellation. Partial state
expires after five seconds. Strict script v1/KBD1 stays unchanged inside BLE v2.
Read-only v1 firmware is deliberately incompatible with the new companion.

Control requires authenticated encryption and physically confirmed numeric
comparison. The current six-digit code must have actually rendered before Pico X
can confirm. Y rejects and preserves Stop; confirmation expires after 30 seconds.
Just Works/fallback pairing cannot grant control. No persistent bonding or flash
layout/config-schema changes. Troubleshooting may require forgetting stale OS
bonds. Upstream TrouBLE logging is disabled to avoid passkey/bond-key diagnostics.

## Recorded validation and size

- Targeted native/protocol/executor/build-support/script suites: 44 tests passed.
- Firmware portable display/bootstrap suite: 31 tests passed.
- Host-agent: 24 unit tests plus six macOS PTY end-to-end tests passed.
- Transfer crypto/protocol: three/six tests passed; keyboard layout matrix: four.
- Device test tool: five tests and targeted Clippy passed.
- Native/firmware portable Clippy, formatting, mock self-test, native release,
  embedded release and embedded no_std codec check passed.
- Actual default firmware verify-flash/reboot passed; persistent slots excluded.
- Actual BLE discovery/info/three status reads/disconnect/reconnect passed;
  latest status RTT 104 ms. No control writes in this automated board test.
- User confirmed physical pairing/control acquisition and successful demo text
  plus cancellation during the initial delay on the real USB target.
- Actual USB suite: 11/11 protocol cases, host-agent handshake and two keepalives.
  No HID reports or source files used. Unavailable bulk open correctly aborted.

| Linked measurement | Before (WLAN + read-only BLE) | Bluetooth only |
| --- | ---: | ---: |
| Firmware allocated flash sections, including four-MiB internal image | 9,862,860 bytes | 4,857,200 bytes |
| Static striped SRAM end | 263,552 bytes | 92,508 bytes |
| Stripped native release | 7,825,264 bytes | 7,858,720 bytes |

Runtime stack/peak RAM, idle CPU and startup remain unmeasured. One connection,
three L2CAP channels and eight 128-byte BLE packets are statically allocated.
Application load ends at `0x100A1D78`, below the internal image `0x10BFE000`;
persistent slots remain at `0x10FFE000` and `0x10FFF000`. Static striped SRAM
leaves 431,780 bytes before runtime stacks. Hardware details and exact checks:
[DEVICE_TESTING.md](docs/DEVICE_TESTING.md).

## Next acceptance steps

1. Pairing/control and delayed text/cancel are already confirmed on hardware.
   Next reject once with Y and once by timeout, ensuring no control is granted;
   reconnect/re-pair and confirm old commands are not replayed.
2. Verify USB enable/disable and local-preset contention.
3. Cancel during upload; press physical Y during another effect.
   Confirm all keys release, no text resumes and a new effect can be admitted.
4. Test local-preset contention, USB disable/enable, USB detach during a job, BLE
   disconnect and app exit. Old completions/cancels cannot affect a new connection.
5. Exercise a valid near-4-KiB effect over default-size fragments, repeated
   reconnect/re-pair and a soak. Measure RTT distributions and app resources.

Linux/Windows physical BLE and permission packaging remain unverified. Add those
only after remaining macOS/Pico edge-case checks pass. Keep native Python and bulk
transfer out of this acceptance change. The next workflow should be selected from
actual usage; bulk transfer needs an authenticated receiver, bounded storage,
verification and real backpressure before USB chunks may be acknowledged.

## Run commands

```sh
cargo run -p pico-companion
cargo run -p pico-companion -- --mock --self-test
cargo run -p pico-companion -- --ble --self-test
cargo build -p pico_rust --release --target thumbv8m.main-none-eabihf
```

Builds need stable Rust and the embedded target, not Trunk/WASM/Python. Flashing
continues to require an explicit user request and available hardware. No commits
or pushes are made by agents unless requested.

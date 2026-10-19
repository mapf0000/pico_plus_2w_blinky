# Pico Companion

Native egui/eframe app using the Glow renderer, Tokio and btleplug. Bluetooth is
the default. Native builds do not build firmware or need Trunk/WASM.

```sh
cargo run -p pico-companion
cargo run -p pico-companion -- --ble --self-test
cargo run -p pico-companion -- --mock
cargo run -p pico-companion -- --mock --self-test
cargo run -p pico-companion -- --mock --mock-scenario busy
cargo build -p pico-companion --release
```

Mock scenarios: normal, empty, permission-denied, busy, usb-unavailable,
incompatible, timeout and link-loss. Mock execution never accesses radio or USB.

For hardware: load a matching profile, scan, select and connect. Connect establishes
a fresh mutually authenticated session. Choose **Authenticate and acquire control**
to permit effects. No Pico interaction, display or OS pairing is required. A missing
profile permits public diagnostics only; wrong key/device ID fails authentication.
Firmware must be BLE v3 and already provisioned. Never share a universal key.

## Trusted provisioning

Use a trusted provisioning PC and private directory before remote deployment:

```sh
mkdir -m 700 /private/path/pico-provisioning
cargo run -p pico-companion -- --create-profile /private/path/pico-provisioning/pico.json
# Pico in BOOTSEL; explicitly requested provisioning/deployment only:
picotool load -v /private/path/pico-provisioning/pico.flash.bin -t bin -o 0x10BFC000
picotool load -v target/thumbv8m.main-none-eabihf/release/pico_rust -t elf
picotool reboot
cargo run -p pico-companion -- --profile /private/path/pico-provisioning/pico.json
cargo run -p pico-companion -- --profile /private/path/pico-provisioning/pico.json --self-test
```

`--create-profile` generates a random 32-byte key and device ID, writes JSON plus
an 8-KiB `.flash.bin` with private Unix permissions, and never overwrites files.
Both files contain the key; neither belongs in Git, logs, shell arguments, or the
USB target/host agent. Import/export them only through a trusted route. Back up
the JSON securely outside `target/` before deployment; `cargo clean` deletes that
directory. The flash image can be removed after verified provisioning. The loader
rejects non-private Unix file permissions and malformed/oversize profiles without
printing their contents. Windows file ACL handling is not yet accepted.

The reserved region is separate from USB identity and absent from firmware ELF
load segments. Ordinary ELF updates preserve it; whole-chip erase or reprovisioning
can destroy/replace it. Reprovisioning changes the key, so old profiles stop working.
This prototype reads a private file per launch. OS credential-store integration,
profile selection UI and authenticated atomic rotation are still backlog items.
Losing every copy requires another trusted provisioning route; unauthenticated
BLE cannot reset or retrieve the key. Never provision through the hostile USB target.

## Operation

The app polls status once per second, displays read RTT, and offers USB enable/
disable and bounded text effects with input-layout selection and initial delay.
Text is typed on the Pico's USB target. Focus a disposable editor during the
delay. Cancel interrupts an upload or running job; Pico Y stops any keyboard job.
Disconnect clears ownership and pending work without replay.

The radio actor has four queued requests and sixteen events. Reset and cancel
use separate coalescing priority signals. One message/effect is outstanding;
connection generations suppress stale commands/results. BLE writes are at most
20 bytes and require ATT responses; completion is read from an encrypted correlated
snapshot. Authentication has a 10-second actor budget, upload has an additional 60-second
budget, ordinary requests five seconds, protected overview eight seconds, and
cancellation two seconds before client disconnect. Connect allows ten seconds
including rediscovery and authentication. Diagnostics retain 128 timestamped
entries with session/operation/error context and omit text/bytecode/keys. Filter,
Errors only, Clear and Autoscroll control the diagnostic view.

Background status polling leaves buttons enabled. One bounded command can wait
behind a status/overview read; further commands receive a queue-full error.
Cancel removes queued text before any upload; Disconnect discards all queued work.
Commands are revalidated against the completed read and never replay after timeout
or reconnect. The protected overview adds the full firmware build, agent version,
OS/hostname/heartbeat age, CDC installer stage and execution/USB limits after
control acquisition, targeting a ten-second refresh while idle. Heartbeat age is
sampled at overview time; live presence/USB status targets a one-second interval.
Displayed layouts are the companion's compiled lowering choices. USB protocol versions do not imply native workflows; Info flag 32 negotiates
filesystem/button streaming. Info flag 64 separately advertises native encrypted downloads.

Update both app and firmware for the overview extension: older v3 companions
reject its newly advertised Info flag. New companions can use older v3 firmware
without the detailed overview. USB identity editing is implemented.

`cargo run -p pico-companion --example auth_boundary` performs explicit board
acceptance for plaintext-command rejection and the unauthenticated connection
deadline under continuous public reads. Run with exactly one Pico in range and
other controllers disconnected; it takes about 35 seconds and sends no valid
encrypted command or HID effect.

Encrypted file downloads are implemented; credentials remain optional future work.
macOS is the hardware test target; Linux/Windows radio operation is unverified.
See [the plan](../../NATIVE_COMPANION_BLE_PLAN.md) and
[wire reference](../../docs/PROTOCOL.md).

## Filesystem and event streaming

Connect/acquire, enable USB and start the host agent on the USB computer. Use
**Host filesystem** Home/Root/Up, path input or breadcrumbs; double-click a folder,
select/copy a path, toggle hidden files or Load more. Errors and Cancel listing
leave the connection usable. Each page is at most 2,048 bytes; displayed entries
are capped at 512. No file content is read or downloaded by browsing. The last
eligible released Pico button is displayed; menu/Payloads/Stop gestures are excluded.

Read-only stream acceptance (requires the board and exclusive companion connection):

```sh
cargo run -p pico-companion -- --profile /private/path/pico.json --stream-self-test
# Optional real host-agent pagination/error/cancel checks, against a chosen directory:
cargo run -p pico-companion -- --profile /private/path/pico.json --stream-self-test --test-directory /target/computer/path
```

The first command sends deterministic maximum-size records, measures throughput,
reports negotiated MTU/queue high-water marks, and reconnects with a fresh session.
The optional directory check lists metadata, tests a missing child path, cancels
queued and active lists, and immediately starts fresh listings. Neither command saves flash, toggles USB, executes shell commands or sends
HID. Values remain 20 bytes at all MTUs, with four application credits and strict
validation. Hardware acceptance and pending platforms are in DEVICE_TESTING.md.


## Native Python

Use **Python scripting** Start/Restart/Stop or Ctrl/Cmd+Enter. The demo waits five
seconds before typing; set layout() to the USB computer's actual input layout
and focus a disposable editor there. The text demonstration's layout selector
is independent. Source edits affect the next run. The interpreter runs in a lazy
child with bounded IPC, 30-second startup/500-ms later-step deadlines and a 128-MiB
Rust heap budget. Stop kills the child and cancels only its own effect. No system
Python or firmware update is needed.

```sh
cargo run -p pico-companion -- --python-self-test
# Real board, empty effects and key-release cleanup; no characters typed:
cargo run -p pico-companion -- --profile /private/path/pico.json --python-ble-self-test
```

See [SCRIPTING.md](../../docs/SCRIPTING.md) for generators, events, exceptions,
limits and the native trust boundary. Host modules, FileIO and imports are denied;
the child is not an OS sandbox against native VM bugs. Source, error messages and
keyboard contents are not sent to logs or temporary files.


## Encrypted downloads

Acquire control and connect the encrypted host session in **Encrypted file downloads**.
Enter a host source path and an absolute new local filename, then Queue file.
The default-path buttons use the USB host agent's in-memory default candidate.
Files authenticate, stream to anonymous disk staging, verify size/count/SHA-256,
and save without overwriting an existing destination before sending their receipt.
One file receives, four requests/eight history entries are bounded. The provisional
limit is 1 MiB per file; disconnect/cancellation requires an explicit fresh session
and never resumes output. See [DOWNLOADS.md](../../docs/DOWNLOADS.md) for limits,
backpressure, cancellation, disk/crash behavior and headless board tests.
Update both companion and firmware for Info flag 64. New companions can connect
to older v3 firmware and report downloads unsupported; older companions reject
this new feature bit. Use the updated host agent (60-second ACK timeout/one credit).

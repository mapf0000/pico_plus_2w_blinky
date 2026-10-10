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

For hardware: scan, select, connect, then **Pair and acquire control**. Compare
PC/Pico pairing codes and press Pico X to confirm (Y rejects, 30-second timeout).
A working Pico display is required. Just Works is refused for control.
Reconnect requires fresh pairing; forget stale OS bonds if pairing fails.

The app polls status once per second, displays read RTT, and offers USB enable/
disable and bounded text effects with input-layout selection and initial delay.
Text is typed on the Pico's USB target. Focus a disposable editor during the
delay. Cancel interrupts an upload or running job; Pico Y stops any keyboard job.
Disconnect clears ownership and pending work without replay.

The radio actor has four queued requests and sixteen events. Reset and cancel
use separate coalescing priority signals. One message/effect is outstanding;
connection generations suppress stale commands/results. BLE writes are at most
20 bytes and require ATT responses; completion is read from a correlated result
slot. Pairing has a 90-second host budget, upload has an additional 60-second
budget, ordinary requests five seconds, and cancellation two seconds before
client disconnect. Diagnostics retain 128 entries and omit text/bytecode/keys.

Python, bulk transfer, filesystem browsing and credentials remain future work.
macOS is the hardware test target; Linux/Windows radio operation is unverified.
See [the plan](../../NATIVE_COMPANION_BLE_PLAN.md) and
[wire reference](../../docs/PROTOCOL.md).

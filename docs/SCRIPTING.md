# Keyboard automation

The browser Python Worker and WASM frontend were removed with WLAN. There is
currently no Python interpreter or script editor in the companion.

The native app lowers bounded text through `keyboard-core`, wraps KBD1 in the
existing version-1 `script-protocol` run/cancel envelope, and transports it over
BLE version 2. The Pico strictly validates and executes that bytecode as USB HID
reports. It contains no interpreter. Hardware payload presets use the same
non-preempting executor and continue to work independently.

Limits remain 1,024 text characters and 4,096 bytecode bytes. The wire envelope
adds 29 bytes. One effect is outstanding. Cancel targets the full effect IDs and
current connection; disconnect cancels remote work; Pico Y stops any active job.
No timeout/reconnect replays execution. Layout selection describes the USB
computer's active input layout, not the companion's input layout.

Native Python requires a separately supervised process, bounded IPC, hard
termination and a reviewed isolation boundary. Restoring the former Python API
and button/capability events is a feature-parity milestone in
[the plan](../NATIVE_COMPANION_BLE_PLAN.md#3-native-python-scripting).

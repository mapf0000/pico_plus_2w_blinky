# Mock Pico

`mock-pico` is a development-only substitute for the firmware bridge. It lets
the browser frontend and the production host-agent dispatcher run together on a
Unix host without a USB device.

The mock creates a pseudo-terminal, starts a host agent built with its existing
`test-port-fd` transport seam, and serves the firmware-compatible WebSocket at
`ws://127.0.0.1:9001/ws`. It implements capability/status/configuration RPCs,
filesystem TLV relaying, secure session negotiation, and authenticated file
transfer with firmware-style ACK/result feedback. HID scripts and USB state are
acknowledged in memory; no USB or keyboard device is created.

From the repository root, start the mock bridge and agent:

```sh
scripts/mock-pico
```

In another terminal, start the frontend with its mock proxy:

```sh
cd apps/frontend
trunk serve --config Trunk.mock.toml --open
```

Use `scripts/mock-pico --no-host-agent` to exercise the UI state where firmware
is reachable but the host agent is absent. `--agent-cwd PATH` changes the
agent's working directory, but it is not a filesystem sandbox: an empty browser
path still follows the host agent's normal home-directory behavior.

Only one browser session owns the simulated firmware connection. A newer tab
replaces the previous one with the same WebSocket close code as the firmware.
Run `cargo run -p mock-pico -- --help` for address and host-agent options.

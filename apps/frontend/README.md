# Frontend (Yew + WebAssembly)

This is a small Yew application compiled to WebAssembly.

Build options:

- Using Trunk (recommended):
  - Install: `cargo install trunk` and `rustup target add wasm32-unknown-unknown`
  - Dev server: `trunk serve --open` (runs a local server with hot reload)
  - Release build: `trunk build --release`
  - Files in `ui/` are copied to `dist/ui/`. The IndexedDB helper is imported
    from the stable `/ui/idb.js` URL used by both the development server and
    firmware.
  - Without hardware, run `scripts/mock-pico` from the repository root and use
    `trunk serve --config Trunk.mock.toml --open`. The mock runs the host agent
    through a pseudo-terminal and relays filesystem and secure-transfer traffic.
    Use `scripts/mock-pico --no-host-agent` for the connected-device/absent-agent
    UI state.

- Using cargo + wasm-bindgen directly:
  - `rustup target add wasm32-unknown-unknown`
  - `cargo build -p frontend --release --target wasm32-unknown-unknown`
  - Then run `wasm-bindgen` on the resulting `.wasm` to generate the JS glue, or use `wasm-pack`.

The firmware build invokes Trunk when frontend sources change and embeds the
generated index, JavaScript, WebAssembly, CSS, and IndexedDB helper.

## WebSocket compatibility

Port 81 `/ws` starts with a versioned JSON `hello` event. It reports the firmware
version/build, WebSocket/file-transfer/filesystem protocol versions, detected
host-agent version, supported keyboard layouts and features, and currently
available privileged operations. The same snapshot is available through the
correlated `HELLO` RPC and is refreshed by the UI while connected.

All browser RPCs have a request ID and deadline. Read requests time out after
five seconds and mutations after ten seconds. Disconnects and reconnects cancel
pending requests; late responses for expired IDs are discarded. UI pending
state is scoped to each action so an unrelated request cannot leave every
control disabled.

Firmware accepts a replacement WebSocket while the previous connection is
still retiring, but only the newest browser is the active logical session. It
closes the displaced page with application status `4001`; that page pauses its
automatic reconnect until explicitly refreshed so two tabs cannot continually
displace each other. A browser connection that remains in `CONNECTING` for
three seconds is closed and retried with bounded exponential backoff.

Set `PICO_FIRMWARE_BUILD` during a firmware build to override the build
identifier advertised by HELLO. It is normalized to a bounded ASCII token.

## Secure file transfer

File transfer uses an ephemeral unattended host session. When a compatible host
agent is present, the browser negotiates the session automatically without a
terminal code. The browser generates the session master, decrypts authenticated
per-file records, verifies the streamed SHA-256, and sends an encrypted receipt
to the host. This mode does not authenticate the browser: every client that can
access the Pico Web UI can establish a session and request host files.

The session is memory-only and is cleared on WebSocket disconnect or page
refresh. Decrypted chunks are staged in IndexedDB for download; filesystem
browsing remains outside this encryption scope. See
[`docs/SECURE_FILE_TRANSFER.md`](../../docs/SECURE_FILE_TRANSFER.md).

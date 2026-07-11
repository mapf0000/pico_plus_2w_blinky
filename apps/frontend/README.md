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

- Using cargo + wasm-bindgen directly:
  - `rustup target add wasm32-unknown-unknown`
  - `cargo build -p frontend --release --target wasm32-unknown-unknown`
  - Then run `wasm-bindgen` on the resulting `.wasm` to generate the JS glue, or use `wasm-pack`.

The firmware build invokes Trunk when frontend sources change and embeds the
generated index, JavaScript, WebAssembly, CSS, and IndexedDB helper.

## WebSocket compatibility

`/ws` starts with a versioned JSON `hello` event. It reports the firmware
version/build, WebSocket/file-transfer/filesystem protocol versions, detected
host-agent version, supported keyboard layouts and features, and currently
available privileged operations. The same snapshot is available through the
correlated `HELLO` RPC and is refreshed by the UI while connected.

All browser RPCs have a request ID and deadline. Read requests time out after
five seconds and mutations after ten seconds. Disconnects and reconnects cancel
pending requests; late responses for expired IDs are discarded. UI pending
state is scoped to each action so an unrelated request cannot leave every
control disabled.

Set `PICO_FIRMWARE_BUILD` during a firmware build to override the build
identifier advertised by HELLO. It is normalized to a bounded ASCII token.

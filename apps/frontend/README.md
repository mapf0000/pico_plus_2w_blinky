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

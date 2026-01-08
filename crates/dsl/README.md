# Pico Keyboard DSL

A tiny, portable DSL for authoring keyboard macros that compile in the browser (WASM) and execute on a Raspberry Pi Pico (or any USB HID host) as a compact bytecode stream. The Pi stays minimal and predictable: no parsing, no strings, just “tap keys” and “delay”.

---

## Why this exists

- **Web-first authoring**: Validate and compile programs entirely in the frontend via `wasm-bindgen`.
- **Device-slim execution**: The Pico executes a tiny, versioned bytecode over USB HID—no recursion, no dynamic string handling.
- **Deterministic & safe**: Fixed caps (lines, delays, ops), CRC-checked transport, and OS/layout stability (default US ANSI; optional compile-time layouts).

---

## Workspace at a glance

- **`dsl-core`** (no_std): DSL parser, line-numbered diagnostics, US ANSI text→HID lowering, flat IR, and bytecode encoder/decoder (`KBD1`, varints, CRC32).
- **`dsl-wasm`**: WASM adapter exposing `compile_to_bytecode(entry_dsl, scripts_json)` and `lint_dsl(dsl)`.
- **`firmware-exec`** (no_std): Streaming bytecode executor for the Pico using USB HID (Embassy + usbd-hid).

---

## DSL Overview (default US ANSI)

**Commands (case-insensitive):**
- `tap("KEY")` — press & release a key (legacy `tap KEY` still works).  
  Example: `tap("A")`, `tap("ENTER")`, `tap("F5")`, `tap("KP_ENTER")`
- `modtap("MOD+...+KEY")` — modifiers held while tapping a key (legacy `modtap MOD+...+KEY` still works).  
  Mods: `LCTRL`, `RCTRL`, `LSHIFT`, `RSHIFT`, `LALT`, `RALT`, `LGUI`, `RGUI`, also aliases `CTRL`, `SHIFT`, `ALT`, `CMD/WIN/META/SUPER`.  
  Example: `modtap("LCTRL+LALT+DELETE")`
- `delay(ms)` — sleep clamped to `0..=5000`. `delay(0)` is a no-op.  
  Example: `delay(150)`
- `text("STRING", per_char_delay_ms)` — types a string via US ANSI mapping; optional per-char delay (default 10ms, clamped to `<=5000`).  
  Example: `text("Hello, world!", 20)`
- `layout("ID")` — sets the active layout for subsequent `text(...)` lowering (compile-time only).  
  Example: `layout("win_en-GB")`
- `call <script_id>` — inline another script by id. Resolved at **compile/link** time in the frontend.
- `fn <name>[ (PARAM, ...) ] { ... }` — define a reusable block at the top level; bodies may include any commands, `repeat`, and `let`. Parameters must be `NAME` constants.
- `<name>([args...])` — call a previously defined function. Arguments can be string/number literals or previously defined `let` constants.

**Misc:**
- Blank lines and lines starting with `#` are ignored.
- Limits: `MAX_DSL_LINES=256`, `MAX_DSL_DELAY_MS=5000`. After lowering, a global safety cap limits total ops.
- Default layout: `win_en-US`. `tap`/`modtap` always use raw keycodes and bypass layout mapping.

**Layouts (compile-time):**
- Enable layouts via cargo features in `dsl-core` and downstream crates:
  - `layout_win_en_gb`, `layout_win_pt_br`, `layout_win_de_de`
  - `layout_mac_en_gb`, `layout_mac_pt_br`, `layout_mac_de_de`
- Layout IDs: `win_en-US`, `win_en-GB`, `win_pt-BR`, `win_de-DE`, `mac_en-GB`, `mac_pt-BR`, `mac_de-DE`
- Use `layout("ID")` to switch layouts mid-script; affects `text(...)` only.

**Key names (subset):**
- Letters `A..Z`, number row `0..9`, function keys `F1..F12`
- Navigation: `HOME`, `END`, `LEFT`, `RIGHT`, `UP`, `DOWN`, `PAGE_UP`, `PAGE_DOWN`, `INSERT`, `DELETE`
- Punctuation cluster: `MINUS`, `EQUAL`, `LEFT_BRACKET`, `RIGHT_BRACKET`, `BACKSLASH`, `SEMICOLON`, `APOSTROPHE`, `GRAVE`, `COMMA`, `DOT`, `SLASH`
- Numpad: `KP_0..KP_9`, `KP_ENTER`, `KP_PLUS`, `KP_MINUS`, `KP_ASTERISK`, `KP_SLASH`, `KP_DOT`

**Example:**
```txt
# Take a screenshot and log in
tap("F12")
delay(250)
modtap("LCTRL+LALT+DELETE")
text("myUser", 30)
tap("TAB")
text("S3cr3t!", 30)
tap("ENTER")
call post_login
```

---

Phase 1 compile-time extensions

- `let NAME = <string|number>` — file-local constants for reuse. NAME must match `[A-Z_][A-Z0-9_]*`.
  - Substitution sites:
    - `delay <NAME|number>` (NAME must be numeric)
    - `text <NAME|string> [<NAME|number>]` (NAME string for the text; optional trailing per-char delay NAME must be numeric)
  - Example: `let S = "Hello"` then `text(S, 15)`; `let D = 150` then `delay(D)`.
- `repeat(N) { ... }` — compile-time unrolling of a block (legacy `repeat N { ... }` still works). Nested repeats allowed and bounded by safety caps.
- `fn <NAME>[ (PARAMS) ] { ... }` / `<NAME>(args...)` — compile-time functions. Definitions are hoisted, cannot be nested, and calls inline the fully preprocessed body (including nested calls) with the caller's constants.

Example:

```txt
repeat(3) {
  text("Hi", 20)
  tap("ENTER")
}

fn greet(MSG) {
  text(MSG, 15)
}

greet("Hello!")
```

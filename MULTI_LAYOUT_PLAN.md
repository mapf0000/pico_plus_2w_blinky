# Multi-Layout Keyboard Support Plan (pico_rust)

## Goal
Implement multi keyboard layout support similar to USB Army Knife, while keeping the firmware HID layer layout-agnostic. Layout selection and switching happen in the DSL compiler (frontend/WASM), with the DSL syntax allowing scripts to change layouts mid-script.

## High-Level Behavior
- Default layout remains US ANSI (current behavior).
- New DSL command (e.g., `layout("win_en-GB")`) switches the active layout for subsequent `text(...)` lowering.
- `tap(...)` / `modtap(...)` remain raw key usage codes and are unaffected by layout (layout bypass).
- Layouts are compiled into the DSL compiler via cargo features (similar to `LOCALE_*` flags in USB Army Knife). If a layout is not compiled in, `layout(...)` fails with a clear compile error.

## Design Decisions (Aligning With USB Army Knife)
- **Mapping location**: layout tables live in `dsl-core`, not in firmware, matching “layout logic outside HID.”
- **Runtime switching**: use a DSL `layout` command that updates the lowering state (similar to `KEYBOARD_LAYOUT`).
- **Compile-time inclusion**: only include enabled layouts in the build to limit WASM size and binary bloat.
- **Fallback**: if no layout set, `text` uses US mapping; per-layout tables can be small overrides.
- **Typed layout IDs**: use a `LayoutId` enum (feature-gated variants + `FromStr`) to avoid typos and keep lookups zero-cost.
- **No-alloc mapping**: layout lookup stays `no_std` friendly (static tables + small override scans).

## Step-by-Step Implementation Plan

### 1) Add layout tables, IDs, and registry in `dsl-core`
- Create a new module like `dsl/dsl-core/src/layouts.rs` (or `dsl/dsl-core/src/layouts/mod.rs`).
- Keep US ANSI as the base table (`char_to_key_us` remains or becomes `layout_us`).
- Add per-layout override tables mirroring DuckScriptInterpreter’s approach:
  - Use static slices: `&[(char, (u8, u8))]` for overrides.
  - Lookup strategy: check override first, then US base.
  - Consider sorting overrides and using `binary_search_by` for faster lookup if tables grow.
  - Consider a small `macro_rules!` helper to define tables consistently.
  - Optional: add a `build.rs` importer to normalize tables from DuckScriptInterpreter and avoid drift.
- Introduce a `LayoutId` enum:
  - Variants compiled behind `#[cfg(feature = "layout_win_en_gb")]`.
  - `impl FromStr for LayoutId` for parsing `layout("win_en-GB")`.
  - `fn map_char(self, c: char) -> Option<(u8, u8)>` for lowering.
- Provide a registry for UI/diagnostics:
  - `fn available_layouts() -> &'static [&'static str]`.
  - `fn lookup_layout(id: &str) -> Option<LayoutId>`.

Files:
- `dsl/dsl-core/src/lib.rs` (wire in registry + new lower API)
- `dsl/dsl-core/src/layouts.rs` (new)

### 2) Add cargo features for layout inclusion
- In `dsl/dsl-core/Cargo.toml`, add features like:
  - `layout_win_en_gb`, `layout_win_pt_br`, `layout_win_de_de`, etc.
- Gate layout tables using `#[cfg(feature = "layout_win_en_gb")]`.
- Pass layout features through downstream crates (`frontend`, `dsl-wasm`) so UI and compiler stay in sync.

Files:
- `dsl/dsl-core/Cargo.toml`
- `dsl/dsl-core/src/layouts.rs`

### 3) Extend the DSL syntax with `layout` directive
- Add a new opcode in the AST (`Op::Layout` / `OpOwned::Layout`) to carry a `LayoutId`.
- Extend parsing in `compile_dsl_with_diag` to recognize a new command, matching the existing function-style syntax:
  - `layout("win_en-GB")` (preferred)
  - Optionally allow `layout win_en-GB` as legacy/shortcut.
- Emit a clear error if:
  - Layout string is empty.
  - Layout is unknown or not compiled in (new `DslError` variants, mapped in WASM/UI).

Files:
- `dsl/dsl-core/src/lib.rs` (parser + program enums + owned ops)

### 4) Update lowering to use the active layout
- Replace `lower_to_flat_us` with a generalized API, e.g.:
  - `lower_to_flat_with_layout(p: &ProgramOwned, default_layout: LayoutId)`
- Maintain a `current_layout` variable in the lowering pass:
  - Initialize to `default_layout` (US by default).
  - On `OpOwned::Layout`, update `current_layout`.
  - On `OpOwned::Text`, map via `current_layout.map_char(...)`.
- Preserve existing behavior for `Tap`, `Delay`, `Call`.

Files:
- `dsl/dsl-core/src/lib.rs`

### 5) Update compilation entry points (frontend + WASM)
- Add layout selection in both compile entry points:
  - `frontend/src/dsl.rs` (compile function)
  - `dsl/dsl-wasm/src/lib.rs` (`compile_to_bytecode`)
- Decide the default layout source:
  - UI selection (dropdown), stored in UI state and passed into compile.
  - Or a config default (US) if no selection.
- Ensure the DSL `layout(...)` directive overrides the default in-script.

Files:
- `frontend/src/dsl.rs`
- `dsl/dsl-wasm/src/lib.rs`

### 6) UI exposure for layout selection
- Add a layout picker in the scripting UI (e.g., in `ScriptingCard`).
- Populate options using a hardcoded list or a list exported by `dsl-core` (for WASM builds).
- Show a short hint: “`layout(...)` in script overrides this default.”

Files:
- `frontend/src/lib.rs` (UI wiring)
- `frontend/ui/style.css` (if new UI elements need styling)

### 7) Documentation updates
- Update `dsl/README.md` to describe:
  - `layout("id")` command.
  - Available layout IDs.
  - `tap`/`modtap` as layout bypass.
  - Build flags/features to enable layouts.
- Update `dsl/EXTENSION_PLAN.md` to mark layout support as implemented or in progress.

Files:
- `dsl/README.md`
- `dsl/EXTENSION_PLAN.md`

### 8) Tests and validation
- Add unit tests in `dsl-core`:
  - US mapping still works.
  - Layout overrides apply correctly.
  - Unknown/disabled layout fails with the right error.
  - Switching layouts mid-script affects only subsequent `text`.
- Update `frontend/tests/compile_run.rs` to compile a script using the new layout or layout directive.

Files:
- `dsl/dsl-core/src/lib.rs` (tests section)
- `frontend/tests/compile_run.rs`

## Initial Layout Set (Recommendation)
- Start with a small, high-value set:
  - `win_en-GB`
  - `win_pt-BR`
  - `win_de-DE`
- Port tables from DuckScriptInterpreter (USB Army Knife uses the same source), keeping only differences from US.

## Error Handling and Diagnostics
- If a `layout(...)` ID is unknown or not compiled in, emit a compile-time error:
  - Example: “Invalid layout: win_tr-TR (not compiled). Enable feature layout_win_tr_tr.”
- If a character is not representable in the current layout, keep existing `ParseKey` error.

## Out of Scope (for this pass)
- Firmware changes (USB HID remains layout-agnostic).
- Runtime layout switching on the device (not needed; layout changes are applied during compile/lower).
- New bytecode versioning or layout metadata in the bytecode header (can be added later if needed).

## Acceptance Checklist
- DSL compiles with US layout unchanged by default.
- `layout("...")` in script changes `text` lowering only.
- Layouts can be enabled/disabled via cargo features.
- UI provides a default layout selection.
- Tests cover layout selection and switching behavior.

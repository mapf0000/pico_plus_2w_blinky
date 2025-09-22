# Pico Keyboard DSL Extension Plan

## Overview and Objectives

Extend the DSL to improve expressiveness and developer experience while **preserving core guarantees**: browser-side compilation (WASM), a tiny deterministic bytecode format, and a minimal, safe runtime on the Raspberry Pi Pico.

**Objectives**
- **Improve Language Expressiveness** — Add loops and explicit key hold/release semantics.
- **Maintain Determinism & Safety** — No runtime I/O, bounded timing, strict caps.
- **Preserve Web-First Workflow** — All parsing/expansion in the browser; device stays parser-free.
- **Backward Compatibility** — Clear bytecode versioning and safe upgrade path.
- **Better Tooling & UX** — Rich diagnostics, linting, docs, and tests.

## Guiding Principles

- **Minimal firmware, maximal frontend**: Prefer compile-time expansion. Add new opcodes only when a feature cannot be expressed via TAP/DELAY lowering.
- **Deterministic execution**: Timing from explicit `delay`; no hidden waits or host introspection.
- **Safety & caps**: Max lines/ops/delay; CRC-guarded streams; fail-closed behavior.
- **Stable mapping**: US ANSI is the default for `text` lowering; firmware remains layout-agnostic.
- **Versioned bytecode**: Magic/flags guard features; unsupported features must hard-fail safely.
- **Forward-compat design**: Reserve opcodes; define behavior on unknown/unsupported features.

## Roadmap Overview

- **Phase 1 – Syntax & Ergonomics (non-breaking)**: `repeat`, `let`, richer diagnostics/lints. Output remains KBD1 (TAP/DELAY/END only).
- **Phase 2 – Bytecode & Executor (stateful input)**: Introduce `KEYDOWN`/`KEYUP` with KBD2 (or required feature flag). Firmware tracks pressed keys (6KRO), auto-release on END/error.
- **Phase 3 – Future features (deferred)**: Optional layouts and parametric scripts, compile-time only.
- **Phase 4 – Hardening**: Tests, fuzzing, docs/specs, safety audits.

---

## Phase 1: Syntax Enhancements & Tooling (Immediate)

### New Syntax (compile-time only)

- **`repeat <N> { ... }`**  
  *Compile-time unrolling* into existing ops.  
  Caps: max `N` (e.g., 100) and max expanded ops; graceful diagnostic on overflow.

- **`let <name> = <string|number>`**  
  File-local constants for reuse. Compile-time substitution into `text`, `delay`, etc.  
  Errors: undefined name, redefinition, circular reference.

- **Hold/Release (placeholders)**  
  Reserve `hold <KEY|MOD>` / `release <KEY|MOD>` syntax; in Phase 1:
  - If equivalent to a single `modtap`/`tap`, expand accordingly.
  - Otherwise emit a *warning* that true hold semantics require updated firmware (Phase 2).

### Diagnostics & Linting (WASM)

- **Richer diagnostics**: line/column spans, offending token, structured codes, suggestions.
- **Lints**:
  - Unused/Unreachable scripts
  - Useless delays (`delay 0`, adjacent delays)
  - Near-cap usage (ops, lines, total delay)
- **WASM API**: extend diagnostics payload (code, message, line, col, span). Multi-error reports.

### Deliverables

- Parser/AST updates for `repeat` and `let`.
- Compile-time expansion in linker/lowering; flat IR remains Tap/Delay only.
- Diagnostic & lint framework (with error codes).
- Tests: parsing, unrolling, constants, lints, regression (old scripts unchanged).
- Docs: updated DSL reference and examples.

### Acceptance Criteria

- Existing scripts compile to byte-identical KBD1.
- `repeat` unrolls deterministically with caps enforced.
- Constants substitute correctly; clear errors on misuse.
- Web editor shows precise spans and actionable suggestions.

---

## Phase 2: Bytecode & Executor Enhancements (Stateful Input)

### KBD2 (or KBD1 + Required Feature Flag)

**New opcodes**
- `KEYDOWN usage:u8`
- `KEYUP   usage:u8`
- (Keep `TAP usage:u8 mods:u8`, `DELAY varu32`, `END`)
- Reserve a few opcode slots for future use (e.g., `CHORD`).

**Versioning**
- Prefer **`KBD2` magic** for breaking add-ons.  
  Alternatively: `KBD1` with a **required feature flag** set in header; old firmware must fail fast.

**Encoding**
- Retain varint for `DELAY`.
- Single-byte usages for key codes and modifier bits (as today).

### Firmware Executor

- **Pressed-set state**: track up to 6 concurrent non-mod keys (Boot protocol). Track modifiers via modifier byte.
- **Dispatch**:
  - `KEYDOWN` → add to set, send report.
  - `KEYUP`   → remove from set, send report.
  - `TAP`     → compatible fast-path (press then release with short built-in cadence).
  - `DELAY`   → explicit wait.
- **Safety**:
  - Enforce 6KRO cap; overflow aborts.
  - Minimal inter-report pacing (e.g., 1 ms) if needed.
  - **Auto-release** all keys on `END` or any error/abort.
- **Compatibility**:
  - Still accept **KBD1** streams (TAP/DELAY/END).
  - Compiler toggle to “target KBD1 only” for legacy devices.

### Deliverables

- Bytecode spec: KBD2 header/flags, opcode table, invariants.
- `dsl-core` encoder/decoder update + round-trip tests.
- `firmware-exec` stateful executor + tests (modifiers, order, abort, end).
- Migration notes and compile-target option.

### Acceptance Criteria

- Multi-key sequences (e.g., hold CTRL, type C) verified end-to-end.
- Unknown/unsupported version → safe fail (no keys stuck).
- KBD1 scripts still run unchanged.

---

## Phase 3: Deferred Features (After Phase 2)

- **Layouts**: `layout <ID>` affects compile-time `text` lowering using mapping tables (US default). Bytecode stays layout-agnostic; optional header metadata for layout ID.
- **Parametric Scripts**: `call_with <id>(args…)` → compile-time substitution; types: string/number. Caps on expansion; no recursion.
- **Libraries/Includes**: Namespaces and includes with deterministic linking order.

---

## Phase 4: Hardening, Testing, and Documentation (Continuous)

### Testing

- **Unit**: parser/AST for new syntax; encoder/decoder for KBD2.
- **Property/Fuzz**: random DSL → compile → decode → flat IR round-trip; mutated bytecode → safe abort.
- **Hardware-in-loop**: timing sanity, 6KRO behavior, auto-release on abort.
- **Regression**: golden scripts compile identically (KBD1).

### Safety & Security

- Strict caps: lines, expanded ops, total delay, bytecode size, nesting depth.
- CRC integrity check mandatory; fail-closed on mismatch.
- No dynamic allocation or I/O in executor; HID keyboard only.
- Release-on-end/error invariant.

### Documentation

- DSL reference (EBNF-style), examples for `repeat`, `let`, hold/release.
- Bytecode spec (KBD1/KBD2), opcode tables, flags, invariants.
- Migration guide: targeting KBD1 vs KBD2.
- Editor UX: syntax highlighting, inline suggestions, example library.

---

## Concrete Extension Points (Codebase)

- **Parser (`dsl-core`)**
  - Extend `compile_dsl_with_diag` for `repeat` and `let`.
  - Add error kinds and span tracking (line/col).
- **IR & Lowering**
  - AST: `Repeat { n, body }`, `Let { name, value }`.
  - Owned/Flat IR unchanged in Phase 1 (expanded).
  - Phase 2: add FlatOps for `KeyDown`, `KeyUp`; keep `Tap`, `Delay`.
- **Bytecode (`dsl-core::bytecode`)**
  - KBD2 header/flags and opcodes.
  - Decoder fast-fail on unknown/required features.
- **WASM (`dsl-wasm`)**
  - Diagnostics payload: codes, spans, suggestions, multi-error.
  - Option to target KBD1 or KBD2.
- **Firmware (`firmware-exec`)**
  - Pressed-set state machine; 6KRO; auto-release; pacing.
  - Dual-path support: KBD1 and KBD2.

---

## Testing Matrix

- **Parser/Linking**: valid/invalid loops, constants, nested calls, recursion detection.
- **Lowering**: `text` mapping (US), delay coalescing, repeat expansion caps.
- **Bytecode**: KBD1/KBD2 round-trips, unknown opcode handling, CRC corruption tests.
- **Firmware**: KEYDOWN/UP sequences, modifiers, END/abort release, max-ops/size rejection.

---

## Compatibility & Versioning

- **Encoding**: KBD1 (status quo) and KBD2 (extended). Flags document required features.
- **Unknown features**: hard-fail with clear error; never attempt best-effort execution.
- **Semver**:
  - `dsl-core`: minor for additive, major for breaking bytecode/IR changes.
  - `firmware-exec`: expose supported bytecode versions/features.
- **Migration**: compiler switch to target KBD1; docs explaining when KBD2 is required.

---

## Function Support (Phase 1.5)

- **Goal**: add reusable blocks (`fn name { … }`) expanded at preprocess time so runtime bytecode stays KBD1.
- **Syntax**:
  - Definition: `fn IDENT [(PARAM, ...)] { … }` appearing at top level; body may contain existing commands, `let`, and `repeat`.
  - Invocation: `IDENT(args…)` emits the body inline after preprocessing; nested calls allowed. Arguments are literals or previously defined constants.
  - Built-in helpers: `delay(ms)`, `text("..", ms)`, `modtap("MOD+KEY")`, `tap("KEY")` lower to legacy commands for compatibility.
- **Scoping**:
  - Functions use the caller's current `let` environment (no definition-time capture). Body-local lets shadow outer names but do not leak.
  - Functions cannot be nested; definitions are hoisted before preprocessing passes and must appear at the file top level.
- **Expansion & Caps**:
  - Preprocessor performs a def pass, storing `PreBlock` snapshots per function, then an expansion pass resolving `IDENT(...)` calls.
  - Enforce recursion depth via call stack tracking; detect cycles and emit `FnRecursion` before caps overflow.
  - Validate expanded line count against `MAX_EXPANDED_LINES`; re-use existing diagnostics for near-cap warnings.
- **Diagnostics**:
  - New errors: `FnInvalidName`, `FnMissingBrace`, `FnRedefinition`, `FnUndefined`, `FnRecursion`, `FnNested`, `FnWrongArgCount`, `FnCallMissingParen`, `FnArg*`, `UnexpectedBrace`.
  - Warnings: `FnUnused` (report once per function not called from reachable entry roots).
- **Tooling**:
  - Update WASM bridge to pass through new codes; surface script/id context on diagnostics.
  - Extend tests to cover expansion, recursion detection, sourcemap mapping, and mixed repeat/function usage.

---

## Ergonomics Notes

- Reviewed current examples (`dsl/examples/`) and observed repeated inline key sequences; function support now covers the common “greet + type” pattern without duplicating `text` blocks.
- `let` identifiers remain uppercase-only for predictability; consider allowing lowercase aliases in a future pass to better match typical function naming while keeping constants visually distinct.
- Multi-line strings are still awkward for paragraph-sized text; candidate enhancements include triple-quoted literals or automatic whitespace collapse inside `text` bodies.
- Inline `fn` shortcuts (e.g., `fn NAME => tap(\"A\")`) could reduce boilerplate for one-liners; evaluate once usage data for full block form is collected.
- Trailing comments on semantic lines are still rejected; investigate relaxations to allow `tap(\"A\") # comment` without confusing the parser.

---

## Security, Safety, and Caps

- Document and enforce:
  - `MAX_DSL_LINES`, `MAX_DSL_DELAY_MS`
  - Max expanded ops, nesting depth
  - Max bytecode size and op count at executor entry
- CRC always checked; any error → abort + release all keys.
- Define max key hold duration (optional lint) to warn about OS key repeat surprises.

---

## Diagnostics & UX Improvements

- **Errors**: spans + suggestions (e.g., “unknown key ‘ENTR’; did you mean ‘ENTER’?”).
- **Warnings**: unused scripts, useless delays, near-cap usage.
- **WASM API**: structured, machine-readable error codes for i18n; multi-error returns.

---

## Open Questions

- Are true multi-key chords beyond 6KRO needed, or is Boot protocol sufficient?
- Do users prefer `repeat N { ... }` only, or also a single-line variant (`repeat 3: ... end`)?
- Should parametric scripts accept only strings/numbers, or also enums?
- What compatibility guarantees are required for already-recorded macros as KBD2 lands?

---

## Acceptance Criteria (Overall)

- Deterministic, bounded execution with strict caps and integrity checks.
- Backward compatibility: KBD1 flow unchanged for existing scripts.
- Clear, actionable diagnostics in the web editor.
- Comprehensive tests: parser → lowering → encode/decode → executor.
- Minimal, auditable firmware changes; no dynamic allocation; safe release-on-end.

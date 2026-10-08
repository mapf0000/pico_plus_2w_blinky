**Hardware display improvement plan**

Prepared on 2026-10-08 against HEAD `ed124e7babfb147857f8c27f5e3f355b43199d9f` and the current uncommitted working tree. This document proposes follow-up work; it does not implement or replace the changes already in progress.

**Objective and scope**

Improve display correctness, code ownership, extensibility, and responsiveness while preserving the new standalone Payloads workflow and the shared HID execution service. Prioritize observable behavior fixes, then make the model and rendering easier to test, then optimize measured performance bottlenecks.

The scope is `firmware/src/display/`, its hardware-independent test surface, and narrow adapters to existing services. Changes to HID execution, preset generation, or WebSocket routing are needed only where a display-facing interface requires them. Preserve unrelated working-tree changes.

**Current implementation**

| Source | Responsibility |
| --- | --- |
| [firmware/src/main.rs](firmware/src/main.rs) | Assign peripherals and spawn the display task. |
| [firmware/src/display/mod.rs](firmware/src/display/mod.rs) | Initialize hardware, sample buttons every 50 ms, route events, update LEDs, collect System metrics, and coordinate rendering. |
| [firmware/src/display/backend.rs](firmware/src/display/backend.rs) | Board-specific ST7789 initialization and blocking SPI transport with a 512-byte static buffer. |
| [firmware/src/display/input.rs](firmware/src/display/input.rs), [ui.rs](firmware/src/display/ui.rs) | Debounce GPIO samples and manage menu navigation. |
| [firmware/src/display/renderer.rs](firmware/src/display/renderer.rs) | Compute layout, plan redraws, draw the sidebar/content shell, and invalidate page state. |
| [firmware/src/display/pages.rs](firmware/src/display/pages.rs) | Statically dispatch five pages: Payloads, Transfer, Host Agent, System, and Logs. |
| `firmware/src/display/page_*.rs` | Page state, input handling, service calls, drawing, and some render caches. |

The existing `no_std` design, fixed-capacity storage, generic `DrawTarget`, static page dispatch, and change-based text updates provide a useful foundation. Keep them.

**How the uncommitted changes affect the analysis**

- `page_payloads.rs` adds local preset selection, keyboard-job status, local completion consumption, and a 15-second host-agent handshake wait.
- `display/mod.rs` now gives Y device-wide Stop behavior whenever a HID job is reserved, including cleanup. Payloads/menu/gesture/Stop controls are withheld from Python button-event publication.
- `usb/hid.rs` and `firmware-exec::jobs` already provide the execution service this display needs: one-job admission, local/browser ownership, job handles, cancellation, key-release cleanup, and separate completion routing. Reuse this service instead of introducing a display-owned executor or job queue.
- `build-support/src/presets.rs` compiles typed Rust presets into static KBD1 using `keyboard-core`. Preset launcher paths and availability come from MSC image metadata. Keep generation in build support and leave generated output untouched.
- HTTP/WebSocket and USB changes add generation-scoped browser completion and cancellation. A display refactor must preserve those boundaries and the existing wire format.
- The updated README, architecture, protocol, scripting plan, and AGENTS.md describe these behaviors. Treat them as the intended baseline.

The changes resolve the need for a shared local/browser execution service and partially address local event ownership. They do not resolve the original gesture-release bug, stale Transfer display, reset semantics, synchronous transport, or duplicated layout/drawing code.

**Findings and priority**

| Priority | Finding and evidence | Required outcome |
| --- | --- | --- |
| P0 | `ui::apply_input` clears the menu gesture block when A/B are up, without checking X. Closing the menu with A+X, releasing A, then releasing X permits a page action. With Payloads added, this can launch a preset accidentally. | Consume the complete menu gesture, including both release orders, before enabling page actions or script publication. |
| P1 | Payloads copies `body_style`, changes the selected row foreground to black, but retains its black text background. The selected label therefore has identical foreground/background colors despite the white row fill. | Set both text colors consistently with the row colors and verify the rendered pixels. |
| P1 | Transfer's `on_tick` returns false; transfer/USB/browser state is read only during rendering. No state change requests a redraw. | Visible connection and transfer status updates without button activity. |
| P1 | The coordinator ticks only the selected page. Payloads consumes `LOCAL_RESULT` and starts/checks its agent deadline in `on_tick`. Navigating away postpones result handling and can start the handshake timeout much later than keyboard completion. | Local completion and handshake timing continue while any page is visible. |
| P1 | Content clears call `reset_all`; Transfer and Host Agent resets change `status_bg` while retaining status text. | Menu/layout/page redraws preserve application state and status meaning. |
| P2 | Payloads repaints every line and preset row whenever it is dirty, adds another private `draw_line`, and ignores the available content height. The current five presets fit, with the status baseline at y=230; a sixth pushes it to y=246, beyond the 240-pixel screen. | Shared widgets, incremental updates, a bounded scrolling list, and reserved status/detail space. |
| P2 | Page updates and rendering read global services directly; System/Logs scheduling lives in the coordinator. Page ID and render-data variants can be mismatched. | Consistent typed snapshots, explicit action intents, and clear model/render ownership. |
| P2 | GPIO-dependent debounce logic and display modules are absent from the host-test library surface. | Native tests for input, routing, deadlines, redraw planning, and drawing. |
| P3 | Initialization uses at least 400 ms of explicit blocking delays. Rendering uses blocking SPI on the executor shared with USB/network tasks. Full-screen/content clears, static headers, action rows, and the menu gap include redundant work. | Remove redundant work, measure executor occupancy and Stop responsiveness, then decide whether asynchronous flushing is needed. |

The behavior findings above follow from the current source. The gesture bug was reproduced with the existing UI code during the initial review, and that code remains unchanged. SPI throughput, visual quality on the panel, and Stop latency under load have not been measured.

**Behavior and resource constraints**

- Preserve System as the initial page and all five existing pages.
- Preserve A/B selection, X activation, and A+X menu behavior. Y Stop takes precedence whenever a job is reserved; consume its release even if the terminal result has already committed. With no job, retain the documented LED behavior outside Payloads.
- Preserve the documented Python event ownership boundaries. Normal releases outside Payloads/menu/gesture/Stop continue to follow the existing policy; do not silently change other pages' script-event behavior.
- Preserve job handles, one-job admission through cleanup, local completion independence, scoped browser cancellation, and generation-owned browser results. Never retry or resume keyboard input automatically.
- Preserve the distinction between keyboard input completion and host-agent detection, including the 15-second detection deadline.
- Keep fixed-capacity state and queues. Avoid heap allocation, additional unbounded collections, or a new interpreter in firmware.
- Preserve pin assignments, panel initialization commands, protocol versions, transfer backpressure, and flash layout unless separately justified and validated.
- Keep button handling and device-wide Stop available when display initialization fails.

**Implementation sequence**

1. **Fix gesture ownership and make input routing testable.**

   Extract debounce state into a pure type that accepts raw pressed states. Keep GPIO sampling in a thin adapter. Replace the repeated per-button debounce fields with a small reusable button state type where it simplifies the code.

   Make menu gesture ownership explicit. Suppress its participating buttons until both A and X have been released, including the tick containing the final release. This state should govern both page input and Python publication.

   Extract a pure routing decision for menu/gesture controls, page controls, Y Stop, script events, and idle LED cycling. Track consumption per button instead of deriving all ownership from a single `local_controls` boolean. Give Stop precedence over activation in the same sample. The coordinator executes routed service intents; it does not duplicate HID admission logic.

   Expose these pure modules through the firmware library without importing Embassy RP peripherals or generated presets. Start the native regression suite here so subsequent refactors have coverage.

   Acceptance: closing/opening the menu cannot run a preset, submit a Host Agent action, or leak the gesture to Python in either release order; Y Stop remains global; simultaneous Stop/Run does not submit new work; menu navigation still wraps correctly.

2. **Fix selected text and make live state independent of page visibility.**

   Correct Payloads row foreground/background handling immediately, with a pixel-level regression check.

   Capture display-facing snapshots before rendering: USB readiness, active HID job, host-agent presence, transfer state, and browser connectivity. Keep snapshots bounded and acquire them once per update rather than querying services independently while drawing.

   Consume local completion in one coordinator-owned path and update the Payloads model even while hidden. Start the handshake deadline when completion is handled promptly, not when the user returns to Payloads. If precise completion timing requires it, add an internal completion timestamp; this does not require a wire-format change. Inject monotonic time into the model for deterministic tests.

   Compare the Transfer view's rendered fields against the previous snapshot. Render only the visible page. Coalesce rapid progress updates at a provisional maximum of 10 Hz, while ensuring terminal states become visible on the next update; tune that rate after measurements.

   Acceptance: transfer readiness/progress/completion changes appear without input; a local job completed on another page is processed promptly; returning to Payloads shows the actual handshake outcome or elapsed timeout; job ownership and cancellation remain unchanged.

3. **Separate page models from render caches.**

   Replace ambiguous reset behavior with explicit cache invalidation. Selection, pending handles, status, and deadlines belong to the model. Previously drawn text, selection, styles, and layout validity belong to render caches. Rendering and layout changes must not mutate the model.

   Represent status severity independently of RGB colors. The theme translates severity into foreground/background colors. Invalidate the target page's cache when its content is cleared; inactive page models continue to exist independently of their cache validity.

   Introduce a bounded cached-line helper with explicit validity. Its comparison must account for style changes as well as text changes. Normalize or truncate text before caching, and handle capacity failures deliberately. Empty text after invalidation must still clear an old row.

   Acceptance: status text and severity survive menu toggles and page switches; changing only severity repaints; shorter/empty replacements erase old text; clearing the content forces a complete page repaint without losing Payloads job state.

4. **Unify widgets, layout, and list behavior.**

   Add small shared primitives for page headers, status banners, cached text rows, and selectable list rows. Adopt them in all five pages, including Payloads, rather than adding a general UI framework.

   Replace competing `clear_w`, `content_width`, and rectangle widths with one page viewport and a consistent padding policy. Use a clipped drawing target for page content. Derive text placement from font metrics rather than repeating baseline constants such as `y - 11`.

   Reserve space for Payloads status and selected-preset detail before calculating visible list rows. Maintain a bounded scroll offset so the selected item stays visible as the catalog grows. Use explicit ellipsis or wrapping for long labels and status messages, especially with the sidebar open. Make unavailable presets visibly distinguishable while preserving their explanatory messages.

   Keep preset names/layout metadata in flash. Bound visible-row caches independently of total catalog size. Validate catalog assumptions at build time, including the nonempty catalog required by the current selection modulo operation; do not edit generated `keyboard_presets.rs`.

   Acceptance: all pages render within their viewport with the sidebar open/closed; selected labels remain readable; six or more presets do not displace status off-screen; long strings and empty lists have defined behavior; navigation keeps selection visible.

5. **Complete the ownership cleanup and reduce redraw work.**

   Move ADC/linker metrics collection out of `display/mod.rs` into a hardware adapter. Keep the task focused on sampling, collecting inputs, updating models, executing intents, and submitting redraws.

   Keep static page dispatch. Use typed per-page views or a combined render-request enum so page identity and its data cannot disagree. Separate model update, user activation, and cache invalidation; no rendering path should submit commands, consume completion signals, or start deadlines.

   Draw headers and decorations only after layout/page invalidation. Update only affected selection rows and changed text. Draw the menu/content gap only when geometry changes. Avoid clearing the content again after the first full-screen clear.

   Keep menu selection changes inside redraw invalidation so the planner remains correct even if a future menu contains multiple entries mapping to the same page.

   For Logs, consider snapshotting only the visible tail and sizing its render cache to the bounded visible rows. Preserve log-buffer synchronization, and do not keep interrupts disabled during SPI drawing.

   Acceptance: an unchanged visible model causes no draw calls; a selection change redraws the old/new rows; a single status change does not repaint the preset list/header; entering or resizing a page redraws its complete viewport.

6. **Measure scheduling and implement asynchronous transport only if needed.**

   Record maximum synchronous render duration, SPI bytes/writes, idle redraw count, missed 50 ms input ticks, and physical Y-release-to-cancellation latency. Exercise Payloads job transitions, transfer progress, a busy Logs page, and menu switching alongside USB/WebSocket traffic. Use bounded counters and summarized diagnostics; log no secrets or transferred contents.

   Separate initialization delays from runtime rendering. Replace blocking initialization waits with asynchronous waits where practical. The current mipidsi model initialization callback is synchronous, so this requires an explicit initialization design; making the wrapper function `async` alone does not help.

   If optimized drawing still exceeds the agreed scheduling budget, render into a bounded row/tile buffer and flush asynchronously using DMA. Keep device-wide Stop sampling independent of any awaited display flush. Audit DMA ownership, buffer lifetimes, cancellation, and peripheral allocation before implementing this step.

   A 320 x 240 RGB565 framebuffer needs 153,600 bytes (150 KiB); do not add one by default. Compare one/two tile buffers against available SRAM, Embassy task storage, and stack use. A synchronous DMA wrapper that waits for completion still blocks the executor.

   Acceptance: measured input/Stop latency and USB/network responsiveness meet the budget chosen from the baseline; the final linked image fits; buffer/queue/task capacities and memory placement are documented. Hardware measurements are a gate for choosing this transport change.

Steps 1–3 address correctness. Steps 4–5 build on their model/cache boundaries. Step 6 starts with measurements after redundant drawing has been removed; DMA is conditional, not an automatic dependency of the earlier work.

**Validation for implementation**

Run the smallest applicable set for each change, expanding when service, protocol, layout, or resource boundaries change.

| Change | Commands and checks |
| --- | --- |
| Every handoff | `cargo fmt --all -- --check`; inspect `git diff` and `git status`; exclude generated outputs. |
| Pure display models/input/widgets | After exposing the hardware-independent modules: `cargo test -p pico_rust --lib --no-default-features`. Account for the existing build-script/tooling requirements; keep RP-only and generated-preset imports out of this library path. |
| Any firmware implementation | `cargo check -p pico_rust --release --target thumbv8m.main-none-eabihf`. |
| HID/preset adapters or generator assumptions | `cargo test -p firmware-exec -p build-support`; preserve existing controller and preset tests. |
| Keyboard lowering/layout/bytecode changes | Run the full keyboard layout matrix and Worker validation from AGENTS.md, including real affected targets. Avoid these changes for a display-only refactor. |
| Task storage, buffers, DMA, generated embedding, linker/size changes, or broad refactor | `cargo build -p pico_rust --release --target thumbv8m.main-none-eabihf`, plus flash/static SRAM/task-storage/stack review. |
| Browser or wire behavior changes | Validate frontend wasm tests and affected endpoints per AGENTS.md; no wire change is planned. |
| Timing and panel behavior | Explicitly authorized board smoke test: both gesture-release orders, all pages/sidebar states, long text, unavailable presets, off-page completion, agent timeout, Y Stop for local/browser jobs, disconnect/reconnect, and concurrent transfer traffic. |

Use native mock/recording draw targets to test visual invariants and redraw regions. Include adversarial transitions: simultaneous edges, stale completion handles, cancellation while hidden, completion during menu navigation, and identical text with a different severity. Exercise empty and larger preset catalogs with synthetic metadata without executing any keyboard actions.

Update README and `docs/ARCHITECTURE.md` for changed UI workflow or module ownership. Update `docs/HARDWARE.md` for transport, capacity, or measured timing changes. Keep the current protocol documentation intact unless behavior actually changes across endpoints.

**Review validation already performed**

- Reviewed the tracked uncommitted diffs and the new untracked Payloads, preset-generator, and job-controller sources. There were no staged changes at review time.
- `cargo fmt --all -- --check`: passed.
- `cargo test -p firmware-exec -p build-support`: passed; 9 firmware-exec tests and 3 build-support tests, with no failures. These suites cover the controller/preset baseline, not hardware drawing or complete USB/WebSocket integration.
- No firmware build, flashing, board timing measurement, or browser execution was performed for this documentation-only change.

No clarification is required to begin the correctness phases: the current code and updated documentation establish the intended controls and execution semantics. Preserve those semantics by default; select a transport budget from measurements before committing to DMA.

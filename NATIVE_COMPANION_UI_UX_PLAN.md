# Native companion: desktop styling and usability upgrade plan

Created 2026-10-11; updated 2026-10-24. The desktop shell and polish are
implemented. This is now a remaining-work and acceptance plan, with the detailed
interaction contracts below retained as reference. macOS is the first acceptance
platform; Windows/Linux claims require their own evidence.

[NATIVE_COMPANION_BLE_PLAN.md](NATIVE_COMPANION_BLE_PLAN.md) owns feature parity,
authentication, transport limits, interpreter/resource boundaries and packaging.
This file owns presentation, navigation, interaction, accessibility and usability.
Implementation and automated checks do not establish human or device acceptance.

## Implemented baseline

| Area | Implemented | Remaining acceptance / refinement |
| --- | --- | --- |
| Shell and appearance | Native title bar, compact toolbar, neutral light/dark surfaces, task navigation, activity drawer, one fixed Stop/Activity footer | Actual themes, resize/scale, keyboard/focus and assistive-technology acceptance |
| Current-page indication | Filled selected row and left accent marker, independent keyboard-focus ring, native selected-state metadata | Confirm selection remains clear without hover/focus and after navigation from toolbar/context actions |
| Connection and readiness | Explicit radio selection and named Connect target; selected/connecting/connected states; Acquire/USB actions; connected-device summary | Unassisted one/multiple-device flows, errors, reconnect and stale-state presentation |
| Keyboard and identity | Offline composer, friendly layout names, cached lowering, scoped Stop, responsive identity editor and retained drafts | Delay presentation, draft replacement/conflicts, last-run context and real key-release acceptance |
| Scripts, Files and Transfers | Working native workspaces, known-demo readiness checks, custom offline scripts, actionable faults, bounded browsing and verified downloads | Interactive workflow/empty/failure states and hardware acceptance in the BLE plan |
| Native integration | macOS Keychain profiles, import/selection, explicit reconnect and native destination picker | Permission/dialog/relaunch acceptance and other platforms; do not re-plan these as missing implementations |
| Preferences and accessibility | Launch-local System/Light/Dark, native egui controls and explicit focus rings | Bounded safe preferences, full keyboard paths, VoiceOver, high contrast and reduced-motion support |

Software evidence is in [docs/DEVICE_TESTING.md](docs/DEVICE_TESTING.md), including
121 passing tests, pointer events through the real mock backend, painted sidebar
selection and Stop checks, compact toolbar/footer checks, token contrast and
actual Python-child checks. Operator workflows are in README.md and
docs/SCRIPTING.md.
The code is authoritative for implemented styles and available actions.

User feedback has exposed three concrete defects: device selection was not
apparent, the demo could start with control released, and sidebar selection was
not painted after the polish pass. Fixes are implemented with regression coverage;
interactive acceptance is still open. Earlier comments that navigation/resizing
looked good are not acceptance of the latest build or functional Stop. OS window
capture was unavailable; do not invent captured visual or usability results.
Detailed change history belongs in DEVICE_TESTING.md rather than this backlog.

## Remaining milestones, in order

| Milestone | Work | Exit criteria |
| --- | --- | --- |
| U1 — Accept the current shell and readiness flow | Exercise selection/connect/acquire/USB/demo/Stop without verbal guidance; verify current-page indication, hover versus focus, resize, light/dark and keyboard paths | Flows complete without accidental execution, focus loss, ambiguous selection or hidden Stop; record observed defects and actual human/device evidence separately |
| U2 — Freshness, drafts and execution feedback | Make live versus stale state and last-run/session context explicit; consolidate actionable errors; protect text/source/identity drafts from replacement and conflicts; refine delay entry without changing limits | Refresh/reconnect and rejected commands cannot misrepresent readiness or silently replace drafts; last results cannot be mistaken for current execution |
| U3 — Preferences and accessibility | Persist a bounded non-secret UI record; accept keyboard-only/VoiceOver paths, contrast/focus at scale, high contrast/reduced motion; centralize remaining selection/button color variants before extending themes | Preferences recover from corrupt/oversize files and never restore effects or authority; accepted accessibility paths and settings are documented |
| U4 — Performance and platform refinement | Measure idle CPU/RAM/repaint, startup and artifact size; test long names, lists and monitor/scale changes; refine native dialogs and remaining shortcut/menu behavior | Measured baseline and regressions explained; platform-specific behavior is supported by actual tests rather than compilation alone |
| U5 — Release acceptance | Complete task sessions, real Pico keyboard/cancel/reconnect and accepted platform checks, using the BLE-plan release dependencies | Every claimed workflow/platform has recorded evidence; remaining limitations are explicit |

U1 is the immediate validation priority. If interactive testing is unavailable,
leave it pending and continue independent U2 engineering; do not block all work
or treat a mock pass as physical HID/USB acceptance. Accessibility and keyboard
correctness apply to every milestone, not only U3. Keep new transport/security,
key rotation and download-throughput work in the BLE plan.

Non-negotiable presentation regressions:

- Current page stays marked after the pointer leaves, focus moves, background
  refresh or navigation through any entry point. Selection and focus are distinct;
  all renderers derive the page from the same local Workspace value.
- Discovery never silently selects/connects the first result. Selected, connected
  and previous devices are different; explain why the next action is unavailable.
- Status refresh does not flicker controls, swallow clicks, reset drafts or steal
  focus. Preserve the earlier button-flicker fix.
- Stop stays in a fixed location outside scrolling on every workspace. It cancels
  only owned work; downloads have separate cancellation. Remote cleanup is not
  inferred from immediate local feedback.
- The built-in demo explains its known USB prerequisites. Custom scripts may
  start offline; do not guess arbitrary Python effects or silently acquire/replay.
- Active/selected/hover/focus/disabled states must be checked as rendered states,
  not only as colors or snapshot flags. No automatic execution on navigation,
  profile changes, reconnect or preference restoration.

## 1. Design assessment and intended outcome

The original screenshots and source review identified a single scrolling page,
weak action hierarchy, repeated connection state, uniform surfaces and prominent
technical detail. The implemented baseline addresses those design problems.
Retain the contracts below for remaining work; completed shell/style work is not
an instruction to redesign it again. New visual changes should address observed
friction or an unmet contract and preserve the regression requirements above.

Contrast and legibility must be measured from actual rendered themes. The
screenshot alone does not establish contrast ratios, system scale, or font size.

The target is a calm, precise desktop control tool: someone can identify the
connected Pico, determine whether control and USB are ready, compose text, execute
it and stop it without searching through logs. Expert information remains easy
to inspect, but does not dominate ordinary operation.

### Product goals and design principles

- **Task first:** show the next useful action, not every possible command.
- **Visible state:** distinguish discovery, authenticated connection, control
  ownership, USB enabled, USB ready and job activity.
- **Recognition:** use human-readable labels, visible selection and explanatory
  state messages instead of requiring users to remember protocol terms.
- **Consistency:** share spacing, colors, components, labels and shortcut rules.
- **Progressive disclosure:** put firmware limits, protocol versions and detailed
  diagnostics behind a deliberate details action.
- **Error prevention and recovery:** validate before submission, retain drafts,
  explain blocked actions and avoid automatic effect replay.
- **Efficiency:** support keyboard, pointer and trackpad use; minimize repeated
  setup without silently granting control or sending text.
- **Inclusive operation:** readable contrast, visible focus, assistive technology
  semantics, scalable text and complete keyboard paths are release requirements.
- **Restraint:** every decoration and animation should clarify grouping, state or
  an interaction. Avoid decorative dashboards, excessive cards and large empty
  hero areas in this utility.

## 2. Scope, priorities and invariants

### Current release scope

All existing native workflows are in scope: discovery/control, overview, keyboard,
identity, scripts, filesystem, encrypted downloads, profiles and diagnostics.
Reusable themes, navigation and recovery presentation are implemented. Remaining
work is acceptance and targeted refinement, not adding placeholder workspaces.

### Dependent work

macOS profile storage and native file dialogs already exist; accept their actual
interaction paths. Windows/Linux native stores, installation/signing and real BLE
validation stay in the feature plan. An app preferences store, further native menu
integration and OS settings links remain separate follow-up work where useful.

### Constraints that the redesign must preserve

- Keep egui/eframe with Glow and the existing bounded asynchronous backend. Do
  not introduce a web renderer, CSS framework or frontend build pipeline.
- One BLE connection, one outstanding effect and the existing queue/backpressure
  rules remain. UI responsiveness is not permission to increase concurrency.
- Connect authenticates using the provisioned per-device secret; a separate
  intentional Acquire action grants control. No OS pairing or physical confirmation
  is added. A diagnostics-only connection is never labelled authenticated.
- Commands remain scoped to their connection incarnation. Reconnect does not
  replay text, save operations, USB changes or scripts.
- Stop targets the companion's owned job. Physical Pico Y Stop retains its wider
  device role; a global UI button must not claim it can stop unrelated local
  presets without such a backend capability.
- Stop/Disconnect bypass ordinary command queues. Background reads must not
  disable task controls, move focus or cause geometry to jump.
- Preserve 1,024 text characters, 4,096 KBD1 bytes and the current 0–5,000 ms
  initial delay. Keep 16 discovered devices and 128 diagnostic entries; do not
  add an unbounded UI history, notification list or draft store.
- USB identity retains printable ASCII bounds of 32 manufacturer bytes and 48
  product bytes. Saving requires supported firmware, acquired control, no job
  and confirmed USB off/not ready; firmware remains authoritative.
- Never log or include text contents, bytecode, secrets or profile contents in
  diagnostics, screenshots for support, clipboard exports or crash reports.
- The styling phase does not change BLE/USB formats, firmware memory placement,
  ownership or key provisioning. Any later state/API extension needs its own
  cross-endpoint review where applicable.

### Priority order

| Priority | Deliverable | Reason |
| --- | --- | --- |
| P0 | Accurate state hierarchy, actionable failures, persistent Stop, input validation, keyboard/focus paths | Correctness and basic operation |
| P1 | Accept/refine the implemented shell, selected state, themes, workspace layout and drawer where observed issues remain | Everyday usability and visual quality |
| P1 | Identity draft handling, resizing, scale support and screen-reader validation | Prevent data loss and inaccessible workflows |
| P2 | Safe UI preferences, remaining menu/shortcut integration and acceptance of existing native dialogs | Desktop convenience |
| Dependent | Other-platform credentials, transport/security limits and distribution | Feature-plan dependencies; current native workflow screens are implemented |

## 3. Information architecture and desktop shell

Use a single main window with an OS-managed title bar, a compact connection
toolbar, a labelled sidebar, the selected workspace and an optional bottom
activity drawer. Keep connection and owned-job controls visible on every page.

Apple describes sidebars as navigation between major app areas; this is a suitable
desktop pattern for these distinct tasks. The structure below reflects the implemented shell; remaining interaction
requirements are acceptance targets. [Apple sidebar guidance](https://developer.apple.com/design/human-interface-guidelines/sidebars)

| Area | Contents and behavior |
| --- | --- |
| Native title bar | App name; optional device display name when connected. Keep standard drag, resize, minimize/maximize and close behavior |
| Connection toolbar | Device name, connection/control label, compact USB readiness, context action such as Connect or Acquire control; Disconnect in a labelled menu when connected |
| Sidebar | Overview, Devices, Keyboard, Scripts, Files, Transfers; Settings, Help and Activity in a lower group. Persistent selected fill/accent; keyboard focus remains distinct. Icons are optional and never replace labels |
| Workspace header | Current task title, one sentence of useful context and only relevant actions |
| Workspace body | Focused content with stable alignment and appropriate scroll ownership |
| Fixed footer | Persistent Stop, compact job state, separate download cancel when active, and Activity. Stop stays in place when idle and busy, outside all scrolling |
| Activity drawer | Closed by default; footer Activity toggles the resizable drawer or opens the full workspace at short heights. Counts describe retained history; details show discarded entries |

Initial navigation is Overview with a setup/recovery card when disconnected.
Restoring the last task is U3 work after safe preferences exist. Keyboard stays usable for composing
while disconnected. Do not automatically change pages on polling or job results.
An explicit “Continue to Keyboard” action may follow successful setup.

Scripts, Files and Transfers are implemented and belong in normal navigation.
Do not add disabled placeholders for unavailable capabilities. Use the relevant
workspace's readiness/empty state to explain a missing device or host agent.

### Shell layout reference

```text
+-----------------------------------------------------------------------+
| Native title bar: Pico Companion                                       |
+-----------------------------------------------------------------------+
| Pico name   Connected   Control needed   USB ready   [Acquire] Device ▾ |
+----------------+------------------------------------------------------+
| Overview       | Keyboard                                             |
| Devices        | Types on the computer connected to Pico USB.         |
| > Keyboard     |                                                      |
| Scripts        | Layout                 Delay before typing           |
| Files          | [Windows · English US] [0 ms]                        |
| Transfers      |                                                      |
|                | [Multiline text composer]                            |
| Settings       |                                                      |
| Help           | 32 / 1,024 characters                   [Send text]   |
| Activity       | Readiness and last-run feedback                      |
+----------------+------------------------------------------------------+
| [Stop typing / script]  Job state                         [Activity]   |
+-----------------------------------------------------------------------+
```

This illustrates hierarchy, not exact measurements or readiness for every state.
The footer remains fixed during idle, running and stopping states; activity
expands above it. Historical last-run feedback must retain device/session context.
Do not add a second persistent full-width status bar or hide idle Stop.

### Desktop window sizes and resizing

All measurements are logical egui points, not physical screenshot pixels. Start
with approximately 1,120 × 760; constrain initial placement to the usable monitor
area. Keep the current 480 × 520 minimum reachable until real testing establishes
whether it needs adjustment. Do not solve clipping by arbitrarily raising it.

| Available width | Layout |
| --- | --- |
| 1,100 points and above | Current sidebar 184 points; workspace may use two columns for overview summary and details; composer stays the main area |
| 800–1,099 points | Current sidebar 184 points; one main content column; details collapse or open in a separate inspector |
| 480–799 points | Replace sidebar with a labelled task selector or compact task tabs; wrap toolbar into two rows; stack fields; keep Stop and primary action visible |

These are desktop resize modes, not a mobile interface. Prefer content fit over
hard breakpoints at large text scale. Scale-aware layouts must degrade to the
compact structure even when physical window width is large.

- Let the composer and drawer take useful extra height. Cap ordinary form line
  width around 760 points; tables/editors may use more. Avoid stretching every
  field and button across a wide display.
- Use one main vertical scroll region per workspace. The composer and activity
  drawer may scroll independently; wheel and trackpad events follow the hovered
  region and stop at its appropriate boundary.
- Reserve toolbar/action areas rather than drawing overlays over content. Scroll
  focused inputs into view and keep focus rings clear of pinned bars.
- Use a bounded, resizable drawer: roughly 160–300 points initially, clamped so
  the main task remains usable. At the smallest heights, show activity as its own
  full workspace with the global job controls retained.
- Persist non-sensitive window geometry, active task and drawer size; restore to
  a visible monitor after display changes. Test multi-monitor scale changes.
- Long device names and hostnames truncate visually with accessible full text
  and selectable details. Forms wrap labels or stack rather than clip.

## 4. Visual design system

### Direction

Use a restrained instrument-panel aesthetic: slightly cool neutral surfaces,
readable text, one blue interaction accent, precise alignment, and small outlined
icons. Color indicates interaction and semantic state; whitespace and surface
levels establish groups. Keep the main workspace visually quieter than its actions.

Use native window decorations. Start with opaque surfaces and subtle borders;
translucency, blur, gradients, glowing accents and large shadows add no necessary
information here. Do not simulate macOS traffic lights or copy one platform's
window chrome onto another.

Default appearance follows the OS; provide System, Light and Dark choices plus
an explicit high-contrast option if system high-contrast detection is incomplete.
Apply OS changes live where supported. Light and dark palettes are designed
separately, not produced through simple inversion. [Apple dark appearance guidance](https://developer.apple.com/design/human-interface-guidelines/dark-mode)

### Style tokens and contrast

[ui/theme.rs](apps/companion/src/ui/theme.rs) owns the current canvas, chrome,
card, input, text, boundary and semantic colors. Button variants are in
[ui/components.rs](apps/companion/src/ui/components.rs); sidebar selection
variants currently remain in `ui/shell.rs`. Centralize those remaining variants
before adding theme modes. Do not maintain a second hex-value palette here or
scatter new literals through workspace code.

| Role | Requirement |
| --- | --- |
| Canvas / chrome / card / input | Distinct but quiet surfaces; grouping does not require bright borders |
| Text / secondary text / success / warning / error | Meaningful labels, measured against every surface on which they appear |
| Primary / secondary / Stop | One clear task action; quiet secondary hover state; danger treatment for active Stop |
| Selected navigation | Persistent background and accent marker even when unfocused/unhovered; programmatic selected state |
| Focus | Separate visible outline; do not confuse keyboard focus with current-page selection |
| Fields and disabled states | Recognizable boundaries, readable values and visible reasons for blocked actions |

Token checks cover normal text at 4.5:1 and field boundaries at 3:1. They do not
establish contrast, legibility or behavior of every actual hover, focus, selected,
pressed and disabled widget. Render those states in both themes and at scale.
High contrast remains planned; do not claim it from the ordinary dark theme.

### Typography, spacing and geometry

| Token | Current baseline / constraint |
| --- | --- |
| Body and ordinary controls | 15 points; normal weight; preserve readable scaling |
| Supporting text | 12–13 points; retain readable contrast |
| Page title | 24 points |
| Section title | 18 points; strong |
| Code / diagnostic details | 14 points in a monospaced font |
| Spacing scale | 4, 8, 12, 16, 24, 32 points |
| Workspace padding | 24 points normal; 16 compact |
| Related field spacing | 8–12 points; 20–24 between groups |
| Button / input height | 32 points normal; 28 compact, growing with text scale |
| Device / navigation row | Native radio row at least 32 points; navigation 34, growing with scale/content |
| Corners | 6 points for buttons; 10 for cards; consistent nesting |
| Focus indicator | Approximately 2-point outline with separation from the boundary |
| Optional icons | 16–18 points in controls, about 20 in navigation; no requirement to add icons to the text-only baseline |

Use a well-supported proportional UI font with clear numerals and Unicode
coverage, plus a code font. Start from bundled egui fonts; only add an open-licensed
font if it materially improves the result, include its license and review binary
size. Do not assume proprietary system fonts are distributable or egui already
uses them. Provide fallbacks for clipboard Unicode and supported layout labels.

Use sentence case throughout. Reserve monospace for source, identifiers and
expanded technical details. Use tabular numerals where available for timers and
progress to avoid width jitter. Hierarchy comes from size, weight, spacing and
contrast rather than oversized headings everywhere.

### Component catalog and interaction states

Extend the existing reusable components for remaining needs: task header,
navigation row, device row, labelled field, primary/secondary/quiet button,
status badge, key/value row, inline validation, issue banner, empty state,
job strip, confirmation dialog and diagnostic row.

- Every interactive component defines idle, hover, pressed, focused, disabled
  and busy states; selectable components also define selected and selected/focused.
- Primary actions use filled accent styling. Secondary actions use a neutral
  bordered style. Infrequent details use quiet text buttons. Avoid several competing
  blue buttons within the same task area.
- Stop is a clearly labelled high-contrast action; a decorative icon is optional. Use a stronger
  danger treatment only while an owned operation is active. It needs no confirmation.
- Disabled controls remain readable. Pair the unavailable action with a visible
  prerequisite/recovery sentence; a mouse-only tooltip is supplemental.
- Icons use one open-licensed, consistent outline set or small vector primitives.
  No emoji as critical status icons. Keep labels on navigation and execution actions.
- Status badges include clear text; optional symbols support rather than replace it. Color alone cannot
  distinguish Ready, Busy, Stale, Unknown or Failed.
- Busy buttons keep their width and show an operation-specific label. An inline
  spinner is optional; stable text feedback is mandatory.

## 5. Connection setup, readiness and recovery

### Device discovery and selection

The Devices workspace owns scanning, choosing a Pico and resolving profile or
connection problems. The toolbar owns persistent session state and quick access.

- Show one prominent “Scan for devices” action initially; use “Scanning…” with a
  way to cancel discovery if the backend supports cancellation through Disconnect.
  Do not present Connect as usable without a selected current discovery result.
- Device rows show display name; full IDs are currently under Device details.
  Accept multiple/identically named devices in U1: expose a short distinguishing
  identifier when needed before Connect instead of making users guess from name
  alone. Signal strength is optional only if reliably supplied. Sort predictably,
  retain selection by ID and avoid jumping rows during interaction.
- Explicitly select a device before Connect. Do not select the first discovered
  device implicitly or treat discovery as consent to connect or execute.
- Make the whole row selectable; distinguish selected, connected and previously
  seen devices. If a discovered device becomes unavailable, mark it accordingly
  rather than silently substituting another device.
- Implemented follow-up (interactive acceptance pending) for the observed
  mock-connection usability defect: make the required
  choice visible with a labelled selection control/clear row affordance and a
  persistent selected-device summary. Place Connect beside or immediately after
  the selection, identify its target, and show why it is disabled inline; a
  tooltip or Help instructions alone are insufficient. Discovery selection must
  be distinguishable from an established connection, including in the toolbar.
  Preserve explicit choice; do not fix this by silently selecting or connecting
  the first result. Apply the same flow to mock and real BLE devices.
  Acceptance: from a fresh launch, a user can discover, select, connect, acquire
  control and reach a running mock job to test Stop without verbal guidance;
  exercise both one-result and multiple-result lists with keyboard and pointer.
- Show empty states specific to never scanned, scanning, scan finished empty,
  adapter unavailable and permission failure. “No devices found” is not the correct
  message for every discovery failure.
- Scan and Connect are distinct actions. Double-click or Enter on a selected row
  may connect as an explicit shortcut, with the same prerequisite checks.

### Session state and next-action matrix

| State | Main presentation | Next useful action |
| --- | --- | --- |
| Not connected, no selection | “Choose a Pico to get started” | Scan for devices |
| Scanning | “Searching for nearby Pico devices…” | Cancel scan where supported |
| Device selected | Name and profile availability | Connect |
| Connecting/authenticating | Name, truthful phase and elapsed time | Cancel connection |
| Public diagnostics only / no profile | “Device visible; control requires a matching profile” | Open Devices; disconnect before importing/selecting the implemented native profile |
| Connected, control released | “Connected · Control needed” | Acquire control; show a separate authenticated claim only if the backend exposes it |
| Control acquired, USB disabled | “Control acquired · USB off” | Enable USB |
| USB enabled but not ready | “USB enabled · Waiting for target” | Explain target USB connection; no fake ready state |
| USB ready, no job | “Ready to type” | Send text from Keyboard |
| Owned job queued/submitting/running | Actual job phase; task actions appropriately gated | Stop typing |
| Cancelling | “Stopping… Waiting for device confirmation” | Keep status visible; apply existing timeout/disconnect behavior |
| Link lost or command timed out | “Connection lost” / specific timeout; last values marked stale | Explicit Reconnect or Scan; never resend the last effect |
| Wrong profile / incompatible firmware | Specific cause and required correction | Choose matching profile or view firmware compatibility help |

If the backend does not expose separate Connecting and Authenticating phases,
show “Connecting securely…” for the combined phase. Add bounded phase metadata
only if useful; never infer authentication from a spinner duration or a bare
Connected enum. Read-only/public diagnostics and trusted sessions need explicit
presentation rules.

Change “Authenticate and acquire control” to “Acquire control” when authentication
already completed on Connect. Explain the authority boundary in setup/details,
not on every screen. Keep acquisition intentional. Release control and Disconnect
remain distinct; Release is unavailable during an active job with an explanation.

### Error and recovery design

Use one current issue banner at the top of the affected workspace, mirrored only
as a compact state indicator in the toolbar. It contains a plain-language cause,
what remains usable, a recovery action, and “Show details” for diagnostic context.
Historical errors remain in Activity without becoming permanent current alarms.

Example for the supplied screenshot:

> Bluetooth is unavailable. You can still edit text. Check Bluetooth and app
> permissions, then retry discovery. [Retry scan] [Troubleshooting]

Only say “Bluetooth is off” or “Permission denied” if the platform/backend can
distinguish it. For unknown failures, retain the honest combined message. An
“Open Bluetooth settings” action is optional and platform-gated; do not show a
button that cannot open the right destination.

- Validation stays beside the field; a session-wide failure uses the issue banner.
- Deduplicate persistent identical failures; bound any transient notices. Suggested
  UI cap: three notices, with durable errors retained in the existing 128-entry log.
- Never use a disappearing toast as the only record of a rejected save or job.
- Clear resolved current errors based on authoritative success/state, not a timer.
  Dismissing a banner does not imply that the capability is available again.
- Show queued work distinctly from completed work; backend queue rejection returns
  a visible result and leaves the draft intact. Do not queue additional clicks locally.
- Reconnect uses fresh authentication and deliberate acquisition. Future automatic
  connection recovery is opt-in, bounded and cancellable, and cannot replay work or
  silently reacquire authority; it belongs to the BLE recovery milestone.

## 6. Workspace specifications

### Overview

Show a compact readiness summary and a useful next action, followed by two or
three grouped sections rather than many decorative metric cards:

- **Device:** display name, firmware summary, compatible/incompatible service.
- **USB target:** enabled/ready state, target agent presence, host OS/name only
  when known, and installation stage if available.
- **Control:** acquired/released status with Acquire or Release, and a direct
  “Open Keyboard” task link when ready.

Keep firmware build, protocol versions, RTT, uptime, execution bounds, heartbeat
age and raw USB compatibility metadata in “Device details.” Allow selecting and
copying non-secret values. Agent absence does not block keyboard typing when USB
HID is ready; show separate prerequisites for later agent-dependent features.

Show data freshness per section. Existing snapshots provide last status and
overview timestamps; use those rather than pretending all metadata refreshes at
the one-second status interval. Mark unavailable values Unknown and stale values
Last known with an age. On disconnect, do not leave green Ready badges next to
last-known values. Overview refresh must not rebuild focused controls or shrink
labels as information arrives.

USB enable/disable uses an explicit pending state and confirmed backend result.
Avoid an optimistic switch that immediately implies success. A labelled button
is sufficient; a switch requires equivalent accessible state and failure rollback.
Do not offer disable as a workaround for stopping a job; Stop remains the right action.

### Keyboard

Rename “Keyboard demonstration” to “Keyboard.” Put the purpose near the header:
“Types on the computer connected to the Pico USB port.” Mock mode has a persistent
“Simulation — no Bluetooth or USB output” badge in the shell, not merely fine print.

| Element | Required behavior |
| --- | --- |
| Layout selector | Friendly labels such as “Windows · English (US)” and “macOS · German”; retain exact IDs in details. Explain that this must match the USB target's input layout, not the control computer's |
| Initial delay | Current entry is 0–5,000 ms, default 0. U2 may present seconds with exact mapping and 0/1/3/5-second presets; any default change must be deliberate and documented. Remember values only after U3 safe preferences exist |
| Composer | Resizable multiline field, generous initial height, selection/copy/paste/undo, real label, explicit scroll. Plain text; Enter inserts a newline |
| Validation | Character count; invalid/unsupported character and layout errors beside the composer; focus the affected field after rejection. Show encoded KBD1 size in expandable details |
| Main action | Send text, with shortcut shown in help/tooltip; disabled reason when connection/control/USB/job/input blocks it |
| Example text | Quiet “Insert example” action; never append unexpectedly. Replace a nonempty draft only after an explicit replace choice, with editing undo where feasible |
| Job result | Durable compact summary: Completed, Cancelled, Rejected, USB unavailable, or Disconnected; device/session and time in details. No automatic resend |

Choose and document a deterministic blocked-reason priority: current session
failure/disconnection, unauthenticated/incompatible connection, released control,
USB not ready, active/pending job, invalid layout/text. Keep input validation
visible independently even when readiness is the reason Send is unavailable.

Keep draft editing available offline. Submission captures a validated immutable
effect; edits afterward affect the next run only. Visibly separate the editable
draft from the active job. Do not persist text drafts by default or send them to
support tooling. An opt-in local draft feature needs clear storage/privacy behavior.

The text character limit does not guarantee bytecode encodability. Preserve strict
lowering validation and report its errors before sending. Oversize paste needs
visible feedback if input is truncated/rejected; do not silently lose clipboard
content. Define Unicode character counting exactly as the existing core does,
without changing limits as part of styling.

Show the actual Queued, Sending, Running and Cancelling states. Do not manufacture
a percentage, “Typing now” phase or an authoritative countdown: the current
backend does not expose every firmware timing boundary. A countdown requires a
defined device event/time origin; until then show “Includes a 3 s initial delay.”
If future estimated progress is useful, label it Estimated and keep real completion
authoritative. Polling can never turn a historic success into present readiness.

### USB identity / device settings

Place device identity in Settings → Device, reachable through a link from Overview.
Separate it clearly from local Appearance and General preferences: saving here
changes the Pico's persisted USB identity.

- Show loaded manufacturer/product values and mark dirty drafts. Inputs remain
  unchanged by ordinary status refreshes.
- Keep validation beside each field; display byte limits, printable ASCII
  restriction and that empty values are allowed. Character-limited widgets alone
  are not a sufficient ASCII validator.
- Show “Turn USB off to save identity” with a deliberate USB action where permitted.
  Never silently disable USB, save and re-enable as a hidden macro.
- Save only when the backend's save predicate permits it. Show Saving, confirmed
  Saved and verified, and Failed; saved identity applies on next USB start.
- Explain “Saved; enable USB to apply” without claiming the USB target has already
  re-enumerated. Retain edits after rejection or persistence failure.
- Ordinary navigation preserves the draft for its device. Explicit Reload while
  dirty offers Keep editing or Discard and reload. Explicit device switching/exit
  prompts only when it would discard meaningful edits.
- Unexpected disconnect preserves at most one unsaved identity draft in memory,
  keyed to the device's stable identity and tagged for revalidation. It is not
  applied to another Pico; restore after reconnect/readback only with explicit
  conflict handling. Never save it automatically.
- Firmware lacking identity support gets an informative unavailable section,
  not empty editable fields. Do not reset persistent USB identity with a local
  “Reset appearance” command.

### Activity and diagnostics

Keep the existing bounded log, filtering, Errors only, Clear and Autoscroll
functions, but present them in a resizable drawer or dedicated expanded workspace.

- Use columns/structured rows for Time, Severity, Operation and Message. Keep
  session IDs and technical fields in expandable row details. Prefer readable
  proportional message text; reserve monospace for identifiers/details.
- Messages wrap; columns have sensible minimum widths. Copy selected non-secret
  entries through an explicit action and expose full text to accessibility tools.
- Scrolling away from the bottom pauses follow behavior and offers “Jump to latest”
  with a bounded new-entry count. Returning to the bottom resumes follow if enabled.
  New messages never steal selection, focus or the user's scroll position.
- Distinguish no events, no events matching the filter, and no errors. Make Clear
  semantics explicit; show discarded-entry count when history has rolled over.
- Current readiness comes from the snapshot, not by interpreting the latest log
  line. A previous session's errors and successes remain visibly historical.
- Support export only for the existing sanitized diagnostic schema. Do not include
  composer text, profile contents, passwords, credentials or arbitrary app state.
  No automatic telemetry or upload is added.

### Appearance, General and Help

Appearance contains System/Light/Dark, UI scale (proposed 100–200%), and high
contrast/reduced motion where OS settings cannot be read reliably. Include Reset
appearance. General contains safe UI preferences and a clear simulation indicator.
Device settings contain only device-backed operations. Profile import/selection is already in Devices. Do not move it to Settings merely
to satisfy the original outline; accept and refine its existing interaction flow.

Help provides a short connection checklist, keyboard shortcuts, target-layout
guidance and links to the operator documentation. Keep secrets out of screenshots
and provide a clear explanation that typing goes to the USB target, potentially
a different computer from the one running the companion.

## 7. Keyboard interaction, focus and native desktop behavior

All current workflows must be possible without a pointer. Preserve familiar text
editing shortcuts and distinguish navigation, activation and executing an effect.
Microsoft's desktop guidance supports complete keyboard paths and predictable
focus/navigation behavior. [Microsoft keyboard interaction guidance](https://learn.microsoft.com/en-us/windows/apps/develop/input/keyboard-interactions)

| Shortcut / input | Proposed behavior |
| --- | --- |
| Tab / Shift+Tab | Logical focus order, matching visual order |
| Arrow keys | Navigate device rows, sidebar/task choices and open menus |
| Enter / Space | Activate the focused button or selected device action; Enter remains newline inside the composer |
| Cmd+Enter on macOS; Ctrl+Enter elsewhere | Send the Keyboard draft only when that workspace owns the action and readiness/validation pass; Start/Restart scripts only in Scripts using the same admission rules as its button; the latter is implemented, Keyboard Send shortcut remains planned |
| Cmd+. on macOS; Ctrl+Shift+. elsewhere | Planned scoped Stop while the app is focused, including in-app popups; native modal-dialog behavior requires platform testing |
| Escape | Dismiss topmost popup/dialog and restore focus. It does not silently execute, disconnect or double as ambiguous global Stop |
| Cmd/Ctrl+F | Focus search/filter in the current workspace where meaningful |
| Cmd+, on macOS; Ctrl+, elsewhere | Open Settings |
| F6 / Shift+F6 | Cycle main regions where platform conventions/support permit |
| Standard copy/paste/undo/select-all | Respect focused text fields and selected diagnostic text |

Confirm shortcut conflicts with OS and keyboard layouts during platform testing.
Keep shortcuts listed in Help and menus; avoid a large undocumented shortcut
surface. No system-wide hotkeys are required. A focused app shortcut cannot stop
a job when the target computer owns focus; keep physical Y Stop and explicit UI
Stop guidance truthful.

- Use stable IDs for text fields, rows, tabs and dialogs so polling does not lose
  cursor positions or keyboard focus. Do not assign focus automatically every frame.
- Opening a dialog moves focus inside it; constrain dialog keyboard navigation
  appropriately and restore focus to the invoker on dismissal.
- Dangerous overwrite/discard dialogs default to the non-destructive action.
  Ordinary connecting, sending and stopping do not need confirmation dialogs.
- Do not let Enter in an unrelated form submit the keyboard draft. Revalidate
  shortcuts through the same predicates as visible buttons.
- If an active job disables its invoking control, move focus to a sensible job
  control only when needed; do not move the user out of an editor arbitrarily.
- Window close/quit triggers existing scoped cleanup and best-effort cancel.
  Never block Stop behind an unsaved-draft prompt; handle job cleanup before a
  prompt about losing edits. Do not claim remote completion if disconnect prevents
  confirmation. Never keep a hidden background typing job running by accident.
- Keep OS title bar and conventional close/minimize/fullscreen behavior. Add
  native application/Edit/View/Device/Help menus where integration is feasible;
  platform-gate this work and retain a keyboard-accessible in-app fallback.
- Accept the implemented native profile-import and destination-picker dialogs.
  Verify whether native modal UI leaves owned Stop reachable; do not promise an
  app shortcut works while the OS dialog owns input. If needed, defer opening the
  dialog until keyboard work is idle. No global key hook is implied. Do not
  replace working native dialogs with custom modals without a demonstrated need.

## 8. Accessibility and inclusive presentation

Use WCAG 2.2 visual/interaction criteria as measurable design targets alongside
native accessibility testing. This is not a claim that a desktop app has achieved
web conformance; actual platform behavior and assistive technology support must
be verified.

- Aim for at least 4.5:1 contrast for normal readable text and 3:1 for qualifying
  large text. Include helper text, placeholders and displayed errors. Disabled
  controls may have different standards, but still need readable reasons.
  [W3C text contrast guidance](https://www.w3.org/WAI/WCAG22/Understanding/contrast-minimum.html)
- Required boundaries, state indicators and focus visuals target at least 3:1
  against relevant adjacent colors. Decorative separators can be subtler when
  they are not the only way to recognize a control.
  [W3C non-text contrast guidance](https://www.w3.org/WAI/WCAG22/Understanding/non-text-contrast.html)
- Encode state with text and shape/icon as well as color. Error, warning and
  selected states remain distinguishable in grayscale.
  [W3C use-of-color guidance](https://www.w3.org/WAI/WCAG22/Understanding/use-of-color.html)
- Every keyboard-operable component has a durable visible focus indicator. Aim
  to keep the entire focused control and its indicator visible, including with
  the activity drawer or a pinned action area open.
  [W3C visible focus guidance](https://www.w3.org/WAI/WCAG22/Understanding/focus-visible.html),
  [focus visibility around overlays](https://www.w3.org/WAI/WCAG22/Understanding/focus-not-obscured-minimum.html)
- Use desktop-sized targets, normally 28–32 points high with adequate spacing;
  aim for at least 24 × 24 logical points for standalone icon actions. This is a
  project target informed by WCAG's 24 CSS-pixel minimum, not an equivalence claim
  between CSS pixels and egui points. Provide larger targets at increased scale.
  [W3C target-size guidance](https://www.w3.org/WAI/WCAG22/Understanding/target-size-minimum.html)
- Keep persistent labels; placeholders never replace them. Name icons and expose
  selected, disabled, busy, checked and expanded states semantically.
- Preserve the existing `eframe` AccessKit feature and explicitly associate
  labels with inputs. Verify the tree for custom navigation, job strips and banners;
  visual custom painting must not replace accessible controls.
- Announce important connection/job/error transitions once with suitable urgency.
  Do not announce every one-second status refresh, elapsed timer or diagnostic row.
  Use supported AccessKit semantics; verify rather than assuming web ARIA behavior.
- Test VoiceOver on macOS first, then Narrator/NVDA on Windows and supported Linux
  screen readers. Record unsupported integration as a release limitation, not a pass.
- Respect OS scale and offer UI scale up to 200%. Avoid clipping, overlapping,
  hidden Stop, and horizontally scrolling ordinary forms at high scale. Test at
  combinations of OS and app scale, not by changing font size alone.
- Respect reduced motion where available; offer an explicit setting otherwise.
  Motion is optional: brief 100–150 ms hover/expansion transitions, no pulsing
  errors, flashing ready states, sliding task pages or animated background effects.
- Support IME composition, Unicode clipboard input and long translated-like labels.
  Keep user-facing strings centralized enough for future localization; do not
  add a localization framework merely for this redesign.

## 9. Existing workflow refinement contracts

These workflows are implemented. Refine their presentation and accept their
actual paths; do not reimplement interpreter, storage or transport stacks as UI work.

| Workflow | Desktop presentation contract | Dependency |
| --- | --- | --- |
| Profiles | Native import/selection, non-secret device/profile names, matching/missing/wrong-profile states; no key display or automatic export | macOS implemented in milestones 0/5; interactive/other-platform acceptance open |
| Scripts | Explicit Start/Stop, declared-demo readiness, custom offline event waits, actionable faults/details; source editing affects next run | Implemented; remaining actual-process/device/platform acceptance in BLE milestone 3 |
| Files | Path/breadcrumb bar, Home/Root/Up, hidden toggle, bounded pagination/selection; clear loading/empty/error states | Implemented; remaining interaction/device/platform acceptance in BLE milestone 2 |
| Transfers | Bounded queue/history, clear progress/verification/save/failure states; explicit destination before queueing and verified no-overwrite saving | Implemented; limits and failure/platform acceptance in BLE milestone 4 |

The same shell, focus rules, issue banners and job components should apply to
these screens. Keep script Stop semantics distinct from transfer Abort. A single
global strip must describe what it will stop rather than ambiguously “Stop all.”
Do not infer filesystem/transfer availability from retained USB protocol versions
or merely seeing a host agent; use negotiated BLE capabilities.

## 10. Implementation structure and engineering boundaries

Refactor incrementally; keep transport and UI changes separate. Current ownership
under `apps/companion/src/`:

```text
app.rs                  snapshot intake, local drafts, remaining workspace controls
ui/theme.rs             shared theme/type/spacing tokens
ui/components.rs        primary/quiet/Stop buttons, badges, focus and section helpers
ui/state.rs             local Workspace value, readiness/admission presentation
ui/shell.rs             toolbar/sidebar/footer/drawer; child module of app
ui/devices.rs           discovery/connected summary; child module of app
ui/scripts.rs           script controls/recovery; child module of app
desktop.rs/profiles.rs  local worker, native dialogs, catalog and credential store
```

Extract overview/keyboard/settings/diagnostics modules only when a concrete change
benefits from that boundary. Do not create placeholder modules or another mutable
navigation/connection state machine to match the old proposed directory outline.

- Apply `egui::Style`, `Visuals`, spacing and font setup through one theme layer.
  Installed egui 0.36.2 provides theme and zoom controls, input labelling and
  disabled-hover helpers. Use the local API version; do not change frameworks or
  dependencies to achieve a visual restyle.
- Separate local view state from `companion_core::Snapshot`. Keep core/backend
  authoritative for permissions, ownership, operation state, results and freshness.
- Derive a compact presentation model for labels, readiness reasons and actions.
  Centralize this mapping so toolbar, workspace and shortcut paths cannot disagree.
  Do not introduce another independently mutable connection state machine in UI code.
- Move shared readiness predicates into core only where they prevent real drift;
  user-facing copy and colors belong in UI. Add bounded metadata to snapshots only
  for genuinely missing information, not guessed progress.
- Preserve `Action` dispatch and epoch tagging. No BLE calls, profile I/O, file I/O,
  dialogs that block, sleeps, interpreter work or asynchronous waits in UI callbacks.
- Cache text lowering/validation when text/layout/delay changes rather than clone,
  lower and allocate the same payload every rendered frame. Revalidate in the
  authoritative submission path. Diagnostic rendering remains bounded to the
  current 128 entries; large future lists require pagination/virtualization.
- Use demand-driven repaint, with a measured cadence only while animation or
  active time display needs it. Do not force continuous 60 fps repaint at idle.
- Add a small versioned local UI preference record if needed; bound its size and
  reject corrupt/oversize input. Store only appearance, window/task/drawer and
  non-secret layout/delay preferences. Profile keys, source/text, host metadata
  and pending jobs do not belong in this record.
- Audit crate features before choosing persistence/native-dialog dependencies;
  current eframe features do not by themselves establish an application settings
  store or native menu implementation. Keep additions narrow and platform-gated.
- Build deterministic view fixtures from existing mock scenarios and bounded
  snapshot fixtures. Include user-facing identity editing/save states without
  editing or replacing unrelated working-tree changes.

## 11. Delivery and acceptance gates

Use U1–U5 above as the active delivery order. Completed shell, theme, navigation
and readiness work belongs in the baseline, not repeated delivery phases.

| Evidence gate | Required before claiming completion |
| --- | --- |
| Software behavior | Relevant admission, draft, session, keyboard and rendered-state regressions pass |
| Native UX | Actual window interaction, themes, resize, focus/keyboard and applicable assistive-technology checks are recorded |
| Device behavior | Explicitly authorized real Pico/USB-target tests establish output, cancellation/key release and reconnect semantics |
| Platform release | Installation, native permissions/store/dialogs, performance and hardware evidence exist for each claimed platform |

Record only the gates exercised. Missing human/platform evidence stays open;
it does not require discarding completed engineering or blocking independent work.

### Concrete usability acceptance

Use the current app as the baseline and repeat the same tasks with the proposed
UI. Run at least five formative desktop sessions if participants are available,
including less experienced and experienced users; record assistance and failure
points. This is qualitative product validation, not a statistical certification.
If only the maintainer can test, document that limitation and retain broader
sessions as pending rather than inventing results.

Proposed success targets:

1. At least four of five participants correctly identify selected/connected device,
   control state and typing target from a brief inspection without opening Activity.
2. At least four of five connect an already provisioned Pico, acquire control,
   enable USB as needed and send a sample without facilitator intervention.
3. All participants can find Stop within two seconds of being asked while an
   owned job is active, independent of the current workspace or drawer state.
4. At least four of five explain the next recovery step for an adapter/permission
   failure and why Send is unavailable, without reading raw diagnostics.
5. Identity edits survive ordinary refresh/navigation, invalid inputs explain the
   violated bound, and a rejected save leaves the draft intact.
6. No accidental text execution, automatic replay, silent draft replacement or
   wrong-device identity save occurs in the exercised workflows.
7. Keyboard-only completion works for discovery, Acquire, USB, composing/sending,
   Stop, identity editing and diagnostic filtering; no focus traps outside dialogs.

For Stop, distinguish finding the UI from remote completion latency. Measure
click-to-local-feedback separately from BLE cancellation/cleanup; maintain the
existing device acceptance criteria. Proposed local feedback target: within
100 ms on the tested desktop without a blocked main thread. Never report this as
proof of instant remote key release.

## 12. Validation matrix and release evidence

### Visual and interaction fixtures

Exercise Light/Dark/High contrast at 480 × 520, 820 × 790, 1,120 × 760 and
1,440 × 900 logical points; include 100%, 150% and 200% UI scale and OS display
scaling. At tiny usable viewports with increased scale, compact scrolling layouts
must retain access to actions rather than promise all content is visible at once.

Fixture states: initial disconnected; scan empty/in progress/multiple devices;
adapter/permission unavailable; connecting; missing/wrong profile;
diagnostics-only; incompatible firmware; authenticated/released;
acquired/USB off; USB enabled/not ready; ready; queued/sending/running/cancelling;
every terminal job outcome; backend queue rejection; stale overview; absent/stale
agent; unsupported identity; loaded/dirty/invalid/saving/saved/failed identity;
empty/filtered/saturated activity; long names and large pasted input.

Capture deterministic screenshots for representative states using the existing
development-only screenshot feature where suitable. Review actual native-window
rendering, focus, IME, selection, resizing and trackpad behavior; screenshots and
headless mocks cannot validate those interactions. Keep screenshots free of keys,
typed secrets and identifying support data. Do not commit generated images/logs
without an explicit artifact policy.

### Targeted automated checks during implementation

Use behavior tests where they verify new readiness mapping, draft lifetime,
shortcut dispatch, notification bounds or stale-session rejection. Avoid tests
that merely duplicate every spacing constant or trivial decorative widget.

```sh
cargo fmt --all -- --check
cargo test -p ble-protocol -p python-runtime -p companion-core -p pico-companion
cargo clippy -p pico-companion -p companion-core -p ble-protocol -p python-runtime --all-targets -- -D warnings
cargo run -p pico-companion -- --mock --self-test
cargo run -p pico-companion -- --python-self-test
cargo build -p pico-companion --release
# For macOS bundle changes/acceptance:
scripts/build-companion
```

Launch the actual native window, starting with mock mode and the existing named
scenarios: normal, empty, permission-denied, busy, usb-unavailable, incompatible,
timeout and link-loss. Add fixtures for uncovered presentation states instead of
claiming the current mock runner already covers them all. Screen-reader checks
and native menu/dialog checks need the real platform.

If keyboard lowering changes, also run the keyboard/executor suites from
[AGENTS.md](AGENTS.md#validation-matrix). If shared wire formats, capacities,
flash or firmware behavior change in a dependent slice, apply the protocol
checklist and embedded release validation there. A pure UI change does not need
an unrelated firmware build or hardware flash.

### Hardware and platform acceptance

With hardware available and device testing explicitly authorized, verify
first-connect/reconnect/reboot authentication, Acquire/Release, USB toggling,
real HID output, delay semantics, cancellation/key release, physical Y Stop,
local-preset independence and disconnect during upload. Verify identity persistence
and next-USB-start behavior for the identity implementation. Do not flash hardware
as routine styling validation.

Record macOS/Pico results separately from mocks, then real Windows and Linux
rendering, permissions, BLE, keyboard navigation and accessibility evidence.
Compiling a platform target is not runtime acceptance. Measure idle CPU/RAM,
repaint rate, startup and release binary size before/after; explain regressions
and dependencies rather than hiding them behind visual improvements.

Update companion operator documentation for navigation, labels, shortcuts,
appearance and changed delay defaults. Put actual hardware/platform evidence in
[docs/DEVICE_TESTING.md](docs/DEVICE_TESTING.md). Keep this plan's completion
status honest and move completed acceptance evidence out of the backlog.

## 13. Immediate next slice

Prioritize U1 acceptance of the current build, including the sidebar selection
correction. Keep its checklist small: select/connect/acquire, start a delayed
mock job, navigate away, Stop; repeat with keyboard and at narrow/light/dark
layouts. Human and real-device results are separate evidence gates.

The next code slice is U2: freshness and last-run/session presentation, then draft
replacement and identity conflict handling. Do not start another general restyle
or reopen completed native feature work. U3 preferences/accessibility and U4/U5
platform/release work follow while keyboard/accessibility correctness applies
throughout. Use the BLE plan for transport, secret lifecycle and download limits.

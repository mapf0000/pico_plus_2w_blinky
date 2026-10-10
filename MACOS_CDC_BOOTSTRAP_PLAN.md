# macOS host-agent installation over USB CDC

Research date: 2026-10-10. Status: production implementation added and flashed; working on the tested Apple Silicon Mac with German input. Broader recovery/platform acceptance remains. Historical isolated-probe experiments are retained below.

## Production implementation status

The source implementation now includes validated optional `PICO_USB_SERIAL`,
generated SHA-256/size/extent metadata, the maintained shell installer, manual-arm
and US/DE CDC presets, an exclusive bounded CDC session, generation/job-owned
cancellation, device-wide Y Stop, and display/Web UI status. The default build
preserves location-based USB naming and leaves CDC presets disabled; provisioning
an identity enables them. See README for build and operator commands.

Firmware rejects new control/HID operations during reservation. After a failed
stream it remains quiet until the host closes the port, then clears the TLV
decoder before normal service. This avoids sending ordinary protocol bytes into
a stalled shell reader. The native macOS installer uses a private directory on
the destination filesystem, exact reads, accumulated-length checks, SHA-256,
atomic replacement, and a 120-second watchdog with reaped child processes.
Serial descriptors close before detached launch, and HUP is ignored before
forking so startup cannot lose the race with shell exit. The first-stage recovery
gap before complete installer delivery remains; use Terminal Control-C there.

Completed software checks:

- `scripts/build-host-agent` packaged the current Apple Silicon agent from source.
- `scripts/test-cdc-installer`: all eight PTY scenarios passed, including native
  executable launch, descriptor audit, corruption, metadata, symlink, EOF,
  watchdog/interrupt cleanup, and the byte-reader fallback.
- `cargo test -p build-support -p firmware-exec`: 10 + 9 tests passed.
- `cargo test -p pico_rust --lib --no-default-features`: 30 tests passed,
  including visible feedback when manual arm blocks a second install and
  automatic retry guidance after cached agent presence expires.
- The full keyboard layout matrix passed, and all generated presets fit KBD1.
  The new CDC US and DE programs are each 552 encoded bytes, below 4,096.
- Frontend wasm tests compile and Trunk release embedding/build succeeds. The new
  bootstrap status/backward-compatibility test passes in headless Firefox; older
  builtin `#[test]` frontend tests are compile-only on wasm and are not counted
  as executed browser tests.
- Build-support and firmware library Clippy checks pass with `-D warnings`;
  the embedded release Clippy check also passes.
- The embedded release build links successfully. One agent copy occurs in the
  ELF; MSC remains 4 MiB, with no sections in the final configuration slots.
  Static SRAM use is 251,260 bytes (about 245 KiB) in the latest reviewed build.
  HELLO/RPC text capacity is now 1,024 bytes; event queue payloads/depths remain
  unchanged. USB packet and request buffers stay 64 bytes.

The production build was verified-flashed using serial `P1234567`. Its full USB
composite enumerates logger `…P12345671` and control `…P12345673`. Selecting the
local manual-arm action completed a real installation into an isolated temporary
directory: all 1,487,440 bytes matched the packaged artifact's SHA-256, the agent
launched, and firmware logged its normal TLV handshake. The receiver took 2.574
seconds. The test stopped only its own agent afterward. This manual-arm test ran
the receiver programmatically and did not open a visible Terminal or send HID
reports. Physical HID US behavior, measured cancellation recovery, unplug during
transfer, and cross-version/topology checks remain hardware gates. The operator
subsequently reported that stopping the agent and reinstalling worked, and that
cancellation worked. The cancellation method and recovery timing were not
recorded; these reports are distinguished from the instrumented acceptance runs.
The subsequent HID attempt opened Terminal but typed `^`/`°` instead of `<`/`>`,
so zsh tried to open a nonexistent filename and never opened the CDC port. This
was a keyboard mapping failure before transfer. Historical probe results below
are not substitutes for the remaining acceptance checks.

The manual and HID actions are alternatives, rather than sequential steps. A
pending manual arm blocks another launch; Y cancels it before switching to the
HID action. Stopped agents remain present in firmware's existing health cache
for up to 25 seconds. The display model now reports an attempted launch while
waiting instead of silently ignoring X. This feedback change is included in the
corrected build alongside the German redirection fix.

The repeated-install report also confirmed confusing stale-presence feedback:
the stopped agent remained marked connected until the existing health timeout
expired. The agent sends keepalives every 10 seconds, and firmware retains
presence for 25 seconds after the last activity. DTR is intentionally deasserted
by the current host agent, so it cannot identify that process's close reliably;
USB endpoint enablement also does not represent application ownership. Shortening
the timeout alone would cause false disconnections with the current keepalive.

A display-only follow-up now says **Agent recently seen; wait 25s after stop**
when cached presence blocks a CDC action. When presence expires, the footer
automatically changes to **Agent not detected; press X to retry**. It does not
automatically queue an installation or send a control probe into an unowned
serial stream. The 25-second health policy and serial protocol stay unchanged.
Validation passed with `cargo test -p pico_rust --lib --no-default-features`
(30 tests), `cargo clippy -p pico_rust --lib --tests --no-default-features -- -D
warnings`, the provisioned embedded release build, formatting, and diff checks.
This follow-up was verified-flashed with `picotool load -u -v -x -t elf
target/thumbv8m.main-none-eabihf/release/pico_rust` and rebooted. Both provisioned
CDC names reappeared. The real agent's restricted self-test completed its
handshake and two keepalive round-trips at the normal 10-second cadence:

```sh
apps/host-agent/artifacts/aarch64-apple-darwin/host-agent --device-self-test --port /dev/cu.usbmodemP12345673 --self-test-keepalives 2 --self-test-interval-ms 10000
```

The test exited normally and left no agent running. The new footer transition
is covered by the display-model tests. After this firmware update, the operator
reported that everything seems to work; the precise footer timing was not
separately recorded. The
operator clarified that the command fragments in their message were an
accidental paste while switching windows, rather than a new HID typing failure.

### Recommended next work

1. Verify physical interruption recovery: unplug during a CDC download, reconnect,
   and complete a fresh install. Confirm no partial executable replaces the
   installed agent and that normal control traffic resumes. Record the exact
   cancellation method when repeating Y Stop or Terminal Control-C checks.
2. Test the automatic US preset with the matching input source, then another
   intended macOS version and a USB hub. Test ambiguous selection with two Picos
   if that setup is available. Keep those results separate from this Mac's DE
   acceptance.
3. Review the source diff and prepare a commit after explicit authorization.
   Retain the working 96-character receiver for this version; Terminal prompt
   and job-message cleanup is optional polish.
4. USB mass-storage removal is now implemented as described below. Reclaiming
   or replacing its internal 4 MiB flash reservation remains a later
   packaging/layout change with its own build and hardware checks.

### Removal of USB mass storage

At the operator's request, the composite builder now exposes only logger CDC,
control CDC, and HID. The MSC class and its bulk-only/SCSI implementation were
removed. `firmware/src/usb/agent_image.rs` retains the internal FAT image as the
CDC artifact source, using the existing `.msc_image` section at `0x10BFE000`.
No artifact offsets, image capacity, or persistent configuration addresses change.
The CDC classes are still allocated first, in the same order.

The generated payload catalog now contains four entries: keyboard test, manual
CDC arm, CDC DE install, and CDC US install. The four obsolete volume-copy
launch/debug presets were removed. CDC installers retain the same 96-character
receiver and encoded keyboard sequence. `scripts/device-test` no longer expects
a mounted volume; its `--msc-label` and `--skip-msc` options were removed.
`PICO_MSC_LABEL` remains a legacy build input for the private FAT container only.

Software validation passed: build-support/executor tests (10 + 9), firmware
library tests (30), build-support and firmware library/embedded Clippy,
provisioned embedded release build, shell syntax/help, formatting, and diff
checks. Static SRAM is 251,036 bytes, 224 fewer than the preceding build. The
image remains exactly 4 MiB and no ELF section overlaps persistent configuration.
The build was verified-flashed and rebooted with:

```sh
picotool load -u -v -x -t elf target/thumbv8m.main-none-eabihf/release/pico_rust
```

macOS IORegistry reports exactly five USB interfaces: logger CDC control/data
(classes 2/10, interfaces 0/1), agent CDC control/data (classes 2/10, interfaces
2/3), and HID keyboard (class 3, interface 4). No class-8 mass-storage interface
or `/Volumes/PICO_AGENT` mount remains. Both provisioned serial names are
unchanged. The existing agent reconnected after reboot and held the control port;
the first `scripts/device-test --port /dev/cu.usbmodemP12345673` attempt therefore
failed at exclusive open with `Device or resource busy`, before protocol checks.
After the operator stopped the agent, the same command passed all 11 raw control
checks, the real restricted agent handshake, and both keepalive round-trips at
the production 10-second cadence. The test exited normally and did not terminate
another process. The operator subsequently verified the result and reported
that it works. A fresh download was not separately instrumented in this removal
follow-up; the prior measured DE installation remains recorded above. The
temporary logger-only observer exited when its USB device became unavailable;
it never opened the control port and is no longer running.

Software validation commands for this removal all passed:

```sh
cargo test -p build-support -p firmware-exec
cargo test -p pico_rust --lib --no-default-features
cargo clippy -p build-support --all-targets -- -D warnings
cargo clippy -p pico_rust --lib --tests --no-default-features -- -D warnings
PICO_USB_SERIAL=P1234567 cargo build -p pico_rust --release --target thumbv8m.main-none-eabihf
PICO_USB_SERIAL=P1234567 cargo clippy -p pico_rust --release --target thumbv8m.main-none-eabihf -- -D warnings
bash -n scripts/device-test
scripts/device-test --help
cargo fmt --all -- --check
git diff --check
```

### German HID correction

The operator's Terminal capture reproduced `exec 3^°$p` and `^&3`/`°&3`. Local
macOS keyboard-type preferences classify this Pico VID/PID as ANSI (type 40).
A native Carbon probe of the active German input source translated virtual key
`0x32` to `<`/`>` and `0x0a` to `^`/`°`; this matches the observed HID failure.
The shared `mac_de-DE` mapping now sends HID usage `0x35` for `<`/`>` instead of
`0x64`. Other input sources and the receiver text are unchanged. This assumes
the tested ANSI classification; ISO-classified Pico behavior remains unverified.
The probe used Apple's [UCKeyTranslate API](https://developer.apple.com/documentation/coreservices/1390584-uckeytranslate).

The firmware asset fingerprint and Cargo rerun inputs now also include
`keyboard-core` and `bytecode-constants`, ensuring mapping changes rebuild the
embedded Worker/frontend instead of retaining a stale Worker bundle. Tests
cover the redirection lowering and dependency invalidation. The full keyboard
matrix (4 tests), native and headless-Firefox release wasm Worker suites (11
tests each), and build-support/executor suites (10 + 9 tests) pass. The corrected
production build was verified-flashed with `picotool load -u -v -x -t elf
target/thumbv8m.main-none-eabihf/release/pico_rust` and rebooted normally.
The physical DE retry passed: the Pico opened Terminal and typed the exact
96-character command, all 1,487,440 downloaded bytes matched the packaged
artifact's size and SHA-256, and the installed real agent completed its normal
TLV handshake. Arm-to-handshake time was 9.407 seconds, including HID launch and
typing. The agent is installed at `~/pico-agent/HOSTAGNT` and left running on the
control port. The operator independently confirmed the correctly typed command
and the installer completion message. The receiver monitor only opened the
logger port; it did not send HID reports or start a receiver on the control port.

The native shell detects its CDC stdin as a terminal. It currently prints `>`
continuation prompts while parsing the compound installer and background `dd`
job notifications during transfer. These are expected output, not failed blocks.
PTY checks of `/bin/sh +i`, `-s`, and `-t` did not disable that interactive mode;
the validated 96-character receiver remains unchanged.

Validation commands for this correction all passed:

```sh
cargo test -p keyboard-core --features 'std layout_win_en_gb layout_win_pt_br layout_win_de_de layout_mac_en_gb layout_mac_pt_br layout_mac_de_de'
cargo test -p python-worker
GECKODRIVER=/tmp/pico-geckodriver/geckodriver MOZ_HEADLESS=1 cargo test -p python-worker --release --target wasm32-unknown-unknown
cargo test -p build-support -p firmware-exec
cargo clippy -p keyboard-core --all-targets --features 'std layout_win_en_gb layout_win_pt_br layout_win_de_de layout_mac_en_gb layout_mac_pt_br layout_mac_de_de' -- -D warnings
cargo clippy -p build-support --all-targets -- -D warnings
PICO_USB_SERIAL=P1234567 cargo build -p pico_rust --release --target thumbv8m.main-none-eabihf
PICO_USB_SERIAL=P1234567 cargo clippy -p pico_rust --release --target thumbv8m.main-none-eabihf -- -D warnings
cargo fmt --all -- --check
git diff --check
```

The GeckoDriver path is local validation tooling, not an installer requirement.

## Proposed result

A user selects a macOS installation payload on the Pico. The existing keyboard service opens Terminal and types fewer than 100 command characters. A small shell receiver downloads the installer and host-agent executable directly from Pico flash over the existing USB CDC control port, verifies the executable, installs it under the user's home directory, and starts it. No public internet, Wi-Fi, mass-storage mount, Python, package manager, or administrator access is needed for the intended path.

Start with native Apple Silicon (`aarch64-apple-darwin`), which is already packaged. Detect and reject an unsupported host architecture before transferring executable bytes. Native Intel macOS support requires adding an `x86_64-apple-darwin` artifact; it must not be implied by the existing macOS directory.

Keep mass-storage delivery available during initial implementation. Removing MSC or changing its flash reservation is a separate change after this path works.

The 96-character receiver has now downloaded the real 1,487,440-byte packaged agent from the Pico's existing flash image, with matching size/SHA-256 and clean descriptor handoff. The diagnostic firmware exposed only two CDC interfaces: no USB mass-storage function, HID traffic, or Wi-Fi was used. A test helper was launched after verification; the downloaded agent was deleted without being installed or executed. See the hardware results below for the limits of this proof.

## Recommended first-stage command

The following candidate is **96 ASCII characters**, excluding the final Return:

```zsh
(p=(/dev/cu.usbmodemP*3);(($#p==1))&&exec 3<>$p&&stty raw -echo<&3&&echo B1>&3&&exec /bin/sh<&3)
```

This command passed against the isolated probe firmware and is now supported by production firmware when built with a validated `PICO_USB_SERIAL` and explicitly armed through a local CDC installation action. Its prerequisites are a fresh macOS Terminal running zsh, a usable Pico USB serial identity starting with `P`, and a control data interface appearing with suffix `3`. That suffix was confirmed on this Mac with logger CDC first and control CDC second. Repeat the check against the full production composite and each supported macOS version.

The subshell contains the temporary variable and descriptor changes. The array collects matching control ports, and the arithmetic check refuses multiple matches. Default zsh also rejects an unmatched glob; with nullglob enabled the zero-length array fails the check. No matching port is opened until the selection succeeds. `exec 3<>$p` opens the port once for reading and writing; `stty` configures that descriptor; `B1\n` requests bootstrap version 1; `/bin/sh` then reads the delivered installer from the same connection. `&&` stops the sequence if opening, configuration, or requesting fails. The array syntax is zsh-specific. [zsh array parameters](https://zsh.sourceforge.io/Doc/Release/Parameters.html#Array-Parameters).

Do not shorten this to an unguarded wildcard: the device exposes two CDC functions, and another Pico may be attached. The guard also lets one compiled preset work with different device serial values without runtime keyboard-program generation.

| Candidate | Typed command characters | Condition |
| --- | ---: | --- |
| Guarded selector and explicit `/bin/sh`, above | 96 | Passed on diagnostic and full production firmware; real agent handshake confirmed |
| Same command using `sh` | 91 | Depends on the user's executable search path |
| Exact `/dev/cu.usbmodemP12345673` port, compact redirections | 78 | Requires matching per-device preset and firmware identity |
| Exact `/dev/cu.usbmodemP0123456789ABCDEF3` port, compact redirections | 87 | Rejected design: this 17-character serial fell back to location-based naming on this Mac |

The budget counts command characters, not USB HID reports or physical key transitions. Shifted punctuation, launcher chords, Return, and the eight characters in `Terminal` are additional keyboard actions. Measure the generated KBD1 operations and elapsed time separately; do not advertise the entire launch sequence as fewer than 100 physical keystrokes.

The compact first stage has no independent host watchdog before it receives the installer. If firmware never responds, or delivers an incomplete compound command, the host shell can remain waiting despite a device-side timeout. Process-group SIGINT recovery passed on the physical link both before installer delivery and during a stalled binary block: the port reopened and a fresh bootstrap succeeded. Still document and test actual Terminal Control-C; Y Stop is not yet proven to terminate the waiting host shell. Do not claim fully automatic, bounded host recovery until that gap is resolved. If an autonomous first-stage watchdog is mandatory, revise the command and remeasure its budget rather than assuming a serial ZLP or port close supplies EOF.

## Research findings

### macOS chooses the serial-device name

Apple's published CDC ACM driver constructs a device suffix from a usable USB serial string, otherwise from USB location, and appends the data-interface number. That historical implementation accepts only short serial strings; current behavior must be measured. Manufacturer and product strings do not directly select the BSD device path. [Apple CDC driver, `createSuffix` and `createSerialStream`](https://raw.githubusercontent.com/apple-oss-distributions/AppleUSBCDCDriver/main/AppleUSBCDCACM/DataDriver/Classes/AppleUSBCDCACMData.cpp).

Repository evidence: `firmware/src/usb/task.rs` creates logger CDC first, then control CDC, and originally set `cfg.serial_number = None` (now an optional validated build input). An isolated probe preserved that CDC order. `P1234567` produced `/dev/cu.usbmodemP12345671` for logger and `/dev/cu.usbmodemP12345673` for control; active request/response probing confirmed their roles. On macOS 26.6.2, tested serial lengths 8, 9, 12, and 14 retained their identities in the device name; lengths 15, 16, and 17 fell back to `/dev/cu.usbmodem31101` and `/dev/cu.usbmodem31103`. I/O Registry still reported the long USB serial strings, so descriptor delivery itself worked. Treat 14 as the longest tested working length on this system, not a cross-version guarantee.

For the first implementation, add a build-supplied serial identity, provisioned per device, and validate its alphabet and length. The implemented input is `PICO_USB_SERIAL`; omitting it preserves location-based naming and disables CDC installation entries. Prefer a `P`-prefixed ASCII alphanumeric identity of at most eight characters for compatibility with the historical implementation as well as the tested Mac. Longer identities up to 14 require an explicit supported-macOS matrix. Do not use a full 16-hex-character identifier plus a `P` prefix: that 17-character value demonstrably breaks the selector. Track the input in Cargo rerun hints and share its validation/generation between USB configuration and preset construction. Assign distinct identities when provisioning multiple boards. A later device-derived identity can replace provisioning after its length and initialization path are verified; the generic selector requires no runtime keyboard-program patching.

### USB connection is not a bootstrap request

The workspace uses `embassy-usb` 0.6.0. Its local source shows that `wait_connection()` waits for the endpoint to be enabled, not for a host application to open the serial port. DTR is separately available. Arm the installer explicitly through the hardware preset and require `B1\n`; neither USB enumeration nor DTR alone should send executable shell text. [Embassy CDC API](https://docs.embassy.dev/embassy-usb/git/default/class/cdc_acm/struct.CdcAcmClass.html).

CDC writes must fit the maximum packet size, currently 64 bytes. Follow Embassy's documented requirement to terminate responses ending on a full packet boundary with a short packet/ZLP. A ZLP completes a USB transaction; it does **not** provide a serial EOF. [Embassy CDC packet constraints](https://docs.embassy.dev/embassy-usb/git/default/class/cdc_acm/struct.CdcAcmClass.html). This Mac delivered complete 64-, 1,024-, and 16,384-byte diagnostic responses even without a ZLP before the next request; do not infer portable behavior or remove explicit response termination from that observation.

### Shell parsing and binary reception need separate phases

Deliver the installer as one complete compound command, such as `{ ...; }\n`, with no subsequent shell commands on the serial stream. The shell must parse that complete unit before the unit requests executable bytes. Firmware must wait for the request before transmitting metadata or binary. The installer receives data through descriptor 3, terminates explicitly on every path, and must never return to parsing the binary stream as shell source.

The redirection syntax and `exec` descriptor changes are documented shell features; macOS behavior was also tested locally. [GNU Bash redirections](https://www.gnu.org/s/bash/manual/html_node/Redirections.html).

One additional macOS finding affects handoff: attaching `</dev/null 3>&-` directly to `exec agent` left a saved serial descriptor open in the tested `/bin/sh`. The passing sequence changes descriptors with a standalone `exec` first, then executes the program:

```sh
exec </dev/null 3>&-
exec "$agent" --port "$port"
```

These commands must be inside the already-parsed compound installer. For a detached production launch, close descriptors first, then use the existing `nohup` pattern with explicit standard-stream redirections and exit the installer. `/bin/sh` should not rely on zsh's `disown`. Test every inherited descriptor, rather than checking only descriptors 0 and 3.

### Ordinary `dd count=` does not guarantee a byte count

A local PTY experiment asked `dd bs=4096 count=1` to receive 4,096 bytes. It returned successfully after a 37-byte fragment. Input blocks can be short; USB packets and serial reads do not correspond to full `dd` blocks.

The installed macOS `dd` supports `iflag=fullblock`, also present in Apple's published source. Feature-detect it in the downloaded installer using an empty local input, before requesting binary bytes. Do not assume every supported macOS version has the flag. [Apple `dd` argument handling](https://raw.githubusercontent.com/apple-oss-distributions/file_cmds/main/dd/args.c), [Apple `dd` manual source](https://raw.githubusercontent.com/apple-oss-distributions/file_cmds/main/dd/dd.1).

Use full-block reads for complete blocks and an exact remainder read. A portable fallback is `dd bs=1 count=N`, followed by explicit byte-count and digest checks, with a watchdog; benchmark its full-agent performance. Do not use `conv=sync` to hide short reads, unbounded `cat`, or EOF as the executable boundary.

## Integration into the existing project

| Area | Planned change |
| --- | --- |
| `crates/build-support/src/presets.rs` | Add US and macOS DE installation presets using the existing Terminal launcher. Associate the preset with a bootstrap action. Enforce the character and KBD1 budgets. |
| `crates/build-support/src/msc_image.rs` | Produce artifact size, SHA-256, architecture, and bounded flash extent metadata while building the existing FAT image. |
| `crates/build-support/src/lib.rs` | Track installer source, artifact changes, serial identity, and generator inputs. Keep `firmware/build.rs` thin. |
| New installer source under `crates/build-support/` | Maintain a readable shell source template; generate and syntax-check its bounded compound-command wire form. Never edit generated output manually. |
| `firmware/src/usb/task.rs` | Supply stable serial descriptor storage and the validated identity. Preserve interface order. |
| `firmware/src/usb/ctrl.rs`, new bounded bootstrap module | Keep one CDC owner; route exclusively between existing TLV traffic and explicitly armed bootstrap phases. |
| `firmware/src/display/services.rs`, `display_core` | Coordinate bootstrap reservation with successful HID admission, report transfer/handoff separately from typing completion, and connect Y Stop to cancellation. |
| `firmware/src/capabilities.rs`, frontend status handling | Expose installation state without requiring a running agent; keep display and Web UI status consistent. |
| `README.md`, `docs/PROTOCOL.md`, `docs/HARDWARE.md`, `docs/ARCHITECTURE.md` | Document the implemented workflow, stream version, identity requirements, bounds, and compatibility. |

Avoid embedding a second copy of the executable. The FAT builder can expose a bounded table of file extents, and the CDC sender can read those slices from the existing 4 MiB image. Generate extents while allocating clusters, and verify that reconstructing the bytes produces the packaged artifact's digest. Do not assume file contiguity or introduce a firmware FAT parser. `firmware/src/usb/msc.rs::image()` already exposes the flash-backed image. The physical experiment verified this approach: a host-side FAT metadata walk found the existing agent extent, and the probe streamed that flash range using bounded packet writes, with no duplicate executable in its ELF. Production metadata must be generated from the current image, not copy the experiment's offsets.

For v1, preserve `memory.x`, the 4 MiB MSC region, and persistent configuration slots. Add only small manifests, installer text, and bounded transfer state. Review task-stack placement and linked size. Missing artifacts make the installation preset unavailable; an Intel host must get an explicit unsupported/missing-artifact outcome until an Intel binary is packaged.

The agent already accepts `--port`, so the installer should launch with the actual selected control port rather than make the agent rediscover logger versus control. The downloaded script can resolve the single matching path again before download; stop if the selection has changed.

## Proposed bootstrap v1 protocol

Use a dedicated, documented stream mode while the hardware action is armed. `B1\n` is outside the normal TLV grammar and is recognized only at a clean frame boundary in that mode. Do not reinterpret an existing tag or reuse the existing host-to-browser file-transfer messages. Ordinary firmware, host-agent, and frontend protocols continue unchanged outside this mode.

Suggested phases:

```text
Idle -> Armed -> SendInstaller -> AwaitManifestRequest
     -> SendManifest -> AwaitBlockRequest -> SendBlock
     -> AwaitBlockRequest ... -> AwaitVerification
     -> SendCompletion -> Idle -> ordinary agent handshake
```

1. Accept local installation only when USB and the matching artifact are ready and no host agent, relay, benchmark, or bootstrap owns the control connection. Reserve bootstrap state and submit the HID preset atomically enough to prevent a busy submission leaving installation armed. Use generation-owned cancellation so a stale completion cannot cancel a new install.
2. `B1\n` requests the installer. Send its complete compound command and terminate the USB response properly. The installer preflights utilities, determines host architecture, creates private staging, and starts its watchdog.
3. The installer requests a manifest with version and architecture. Return a bounded canonical line containing protocol version, architecture, executable length, transfer block size, and SHA-256. Validate every field; reject missing artifacts before binary transfer.
4. Transfer **raw binary** in requested blocks, recommended maximum **16,384 bytes** based on the physical measurements. Each bounded text request contains the expected block index. Firmware returns exactly the known block length; the host reads exactly that many bytes, appends them, checks the accumulated size, then requests the next block. There is no binary data until a block request, and no unsolicited trailing status after a block. The sender streams 64-byte flash slices; a 16 KiB block does not require a 16 KiB firmware buffer.
5. After the final block, verify exact file length and SHA-256. Send a verification result and consume a final completion line. Firmware returns to normal TLV service before the installer closes all serial handles and launches the agent.
6. On any failure, abandon the partial file and return a terminal error. Firmware clears decoder/transfer state and invalidates the generation. No partial executable is launched.

Suggested initial bounds: one session, 64-byte request lines, 256-byte manifest/completion lines, 8 KiB installer source, 16,384-byte transfer blocks, artifact length no larger than the packaged image, 30-second arm deadline, 5-second per-phase inactivity deadline, and 120-second total install deadline. The actual-agent transfer completed in 1.615 seconds on this link with fullblock reads; the timeout budget remains deliberately conservative pending slower-host tests and complete installation integration. Stream installer text and artifact slices directly from flash; do not put the complete script, block, or executable on a firmware task stack.

Bootstrap has its own bounds; the existing TLV maximum remains **2,048 bytes**. The normal TLV parser must be reset at every transition. During active bootstrap, prevent `CTRL_CHAN` traffic, heartbeat probes, or relay responses from being emitted into the installer stream. Define explicit busy outcomes for new host operations, and preserve the existing browser-transfer backpressure outside bootstrap. Additive UI status must be tested across all endpoints that consume it.

DTR loss, USB disable, device-wide Y Stop, deadline expiry, or a failed HID job cancels the corresponding session. Do not rely solely on closing a host port to deliver a firmware endpoint error. After failure, return to a documented resynchronization condition; require a fresh generation/request before serving any installer bytes again.

## Installer behavior

- Use only macOS-provided shell and file utilities. Preflight `stty`, `dd`, `shasum`, `mktemp`, and the required file commands; do not install missing dependencies.
- Set a private umask and create staging with `mktemp -d` under the intended destination filesystem. Prefer the existing `~/pico-agent/HOSTAGNT` destination, with quoted paths and an atomic replacement after successful verification. Reject unexpected symlinks rather than following them. Leave an existing installation usable on failure.
- Accept only the supported architecture tokens and strictly bounded manifest numbers/digest. Treat serial data as untrusted; never `eval` protocol fields. The initial shell source itself is trusted device software, just as the current packaged executable is.
- Receive requested blocks with an exact reader, a host watchdog, and explicit final size/digest checks. A `dd` exit status alone is insufficient. Ensure watchdog children cannot inherit CDC descriptors and cancel/reap them before launch.
- Clean temporary files on errors, interruption, and success. Print concise progress/errors to Terminal; do not log transferred contents.
- Close serial descriptors as a separate command before the foreground/debug or detached launch. Pass `--port` explicitly. Successful installation and successful agent handshake are distinct states.

SHA-256 detects damaged transfers; a hash delivered by the same device does not authenticate a malicious replacement device. This plan uses the existing trusted-device distribution model. It does not bypass macOS execution policy or establish code signing/notarization; check ordinary launch behavior on supported machines.

## Implementation sequence and acceptance gates

### 1. Establish the device-name contract

The connected Mac/Pico checks established the short serial and logger/control naming contract. Repeat against the full production composite and supported macOS versions. Extend coverage to alternate ports/hubs, two Picos, and unrelated serial devices. Preserve the 96-character selector's ambiguity guard and reject unsupported serial identities at build time. See the results section for physical reconnect coverage.

The user explicitly authorized the board experiments and waived restoration of the original firmware. The diagnostic firmware has now been replaced by a verified production build with CDC installation support. Historical probe sources/captures remain outside the repository.

### 2. Build and validate artifact metadata

Generate extents, architecture, size, digest, availability, installer wire text, and preset metadata from one artifact source. Add generator tests for missing artifacts, image limits, extent reconstruction, serial validation, shell syntax, and the strict `<100` character budget. Record US/DE HID operations and KBD1 size; preserve the existing 4,096-byte program limit.

### 3. Implement and test the stream state machine

Separate pure bounded parsing/state transitions from Embassy I/O. Test fragmented/coalesced requests, exact packet boundaries/ZLPs, malformed versions/lengths, wrong block indices, duplicate requests with deliberate retry behavior, cancellation, stale generations, timeouts, disconnects, queue ownership, and return to a normal TLV handshake. No existing tag is renumbered or silently reinterpreted.

### 4. Implement the installer and PTY integration tests

Maintain the readable source template and test the exact generated compound command with macOS PTYs. Cover complete and corrupted binaries, 0/1/63/64/65/1,023/1,024/1,025-byte boundaries, short reads, fragmented script delivery, early EOF, stalled peers, unsupported architecture, disk errors, spaces in paths, existing files/symlinks, zero/multiple port matches, and cancellation. Check **all** child descriptors and process cleanup. Measure a real packaged binary with both fullblock and fallback readers.

### 5. Integrate admission, status, and keyboard presets

Preserve one HID job through key-release cleanup. Arm before the bootstrap command can arrive, roll back on failed admission, and avoid mistaking HID completion for transfer completion. Y Stop must cancel transfer as well as typing. Add US and DE installation entries, source-derived availability, and coherent display/Web UI status.

### 6. Validate the complete delivery path

Applicable implementation checks:

```sh
cargo fmt --all -- --check
cargo test -p firmware-exec -p build-support
cargo test -p keyboard-core --features "std layout_win_en_gb layout_win_pt_br layout_win_de_de layout_mac_en_gb layout_mac_pt_br layout_mac_de_de"
cargo test -p pico_rust --lib --no-default-features
cargo clippy -p pico_rust --lib --tests --no-default-features -- -D warnings
cargo build -p pico_rust --release --target thumbv8m.main-none-eabihf
```

Run the new macOS PTY installer suite. If host-agent code changes, also run its test and Clippy commands. If frontend status changes, compile its wasm tests and run available browser tests; run Trunk if assets change. No RustPython behavior change is planned, so Worker tests are not needed solely for adding a hardware preset.

An explicitly authorized board smoke test must demonstrate installation with MSC unused and networking unavailable, final digest equality, clean agent handshake, repeat installation, interrupted download, Y Stop, and logger/control isolation. Check supported macOS versions and native Apple Silicon first; Intel remains unsupported until its artifact and platform checks are added. Inspect linked flash/SRAM usage and prove the executable was not embedded twice.

## Research validation already performed

The initial checks below used synthetic data and temporary files outside the repository. They preceded the explicitly authorized physical-board experiments in the next section.

| Local experiment | Result |
| --- | --- |
| Count the recommended command as ASCII | 96 characters |
| macOS 26.6.2 utilities; `/bin/sh` reports Bash 3.2.57; zsh 5.9 | Available; `dd iflag=fullblock` accepted |
| zsh guarded selector with a simulated control `P…3` and logger `P…1` | Selected control only |
| Guarded selector with zero or two control matches | Refused to proceed |
| Fragmented compound installer, then 8,193 binary bytes covering all byte values | SHA-256 matched with zsh and Bash launchers |
| Exact receiver using `bs=1`, and fullblock receiver plus remainder | Both passed on each launcher |
| Normal `dd bs=4096 count=1` with a 37-byte fragment | Short read reproduced |
| Close serial redirections on the program-launching `exec` | Failed descriptor audit: a saved serial descriptor was inherited |
| Standalone descriptor-closing `exec`, then program-launching `exec` | Passed audit of descriptors 0–255; no serial descriptor inherited |

The ad hoc harness was run with `python3 /tmp/pico_cdc_bootstrap_research.py`; Python was only a research tool, not an installer dependency. Its scenarios should become maintained integration tests during implementation. PTYs alone cannot validate USB naming, DTR, or packet termination; the board results below cover part of that gap. Terminal launch timing, physical HID layout behavior, and production board cancellation remain acceptance gates.

## Physical-board results

The test project and captures are under `/tmp/pico-cdc-board-research.ZexmFw/`, outside the repository. The probe uses RP2350 Embassy USB with logger CDC allocated before control CDC, matching production order. It deliberately omits HID, MSC, Wi-Fi, display, application TLV routing, and persistent writes. Its request handlers are diagnostic scaffolding, not a completed production bootstrap protocol.

Each build used the real embedded release target and each flash used verified `picotool load -u -v -x ... -t elf`. The final ELF contained 28,492 bytes of flash load data, ending at address `0x10006f4c`, below the retained MSC region and final two configuration slots. No persistent configuration slots were written. The user stopped the already-running `HOSTAGNT` process after it was found holding the control port; the tests did not kill it. That conflict confirms that installer admission/port ownership needs an explicit busy outcome.

| Physical check on macOS 26.6.2 | Result |
| --- | --- |
| Eight-character serial `P1234567` | Logger `…P12345671`, control `…P12345673`; roles actively probed |
| Serial lengths 9, 12, 14 | Identity-based names retained |
| Serial lengths 15, 16, 17 | Location-based `…31101`/`…31103`; long-serial descriptor still visible in I/O Registry |
| Physical unplug/reconnect to the same USB port | Both names retained with serial `P0123456789ABC`; control health probe and fresh 96-character bootstrap passed |
| 96-character first-stage receiver | Received and executed the shell stage from the physical Pico |
| Raw lengths 0, 1, 63, 64, 65, 1,023, 1,024, 1,025, 16,384 | Exact synthetic bytes; subsequent health probe passed |
| Default open / explicit DTR clear / explicit DTR set | Firmware reported 1 / 0 / 1 |
| Full packet responses without ZLP | This Mac delivered all requested bytes before a subsequent short response; keep portable ZLP handling |
| Corrupted 1 MiB synthetic download | Digest failed; launch helper not executed; next bootstrap succeeded |
| Missing installer and stalled real-binary block | Process-group SIGINT terminated the receiver; port reopened; health probe and fresh bootstrap passed |
| Launch descriptor audit | Host helper checked descriptors 0–255; none referenced the CDC device |
| Existing agent bytes streamed directly from retained MSC flash | 1,487,440 bytes; size and SHA-256 matched the local packaged artifact; no agent execution |

The experimental real-file installer was 1,081 bytes delivered over CDC; its source size adds no keyboard keystrokes. It staged and deleted the file using native macOS shell utilities, then executed a locally compiled Rust descriptor-audit helper instead of the downloaded agent. Python orchestrated the tests but was not used by the delivered shell receiver.

Measured shell download times include shell startup, requested blocks, file staging, SHA-256 verification, cleanup, and audit-helper handoff; they exclude keyboard typing, opening Terminal, final installation, and real agent startup. These are individual local measurements, not a supported-host performance guarantee.

| Payload | Block size | Exact reader | Seconds |
| --- | ---: | --- | ---: |
| 1,048,641 synthetic bytes | 1 KiB | `dd iflag=fullblock` | 1.885 |
| 1,048,641 synthetic bytes | 4 KiB | `dd iflag=fullblock` | 1.151 |
| 1,048,641 synthetic bytes | 16 KiB | `dd iflag=fullblock` | 1.094 |
| 1,048,641 synthetic bytes | 16 KiB | `dd bs=1` fallback | 1.582 |
| 1,487,440 actual agent bytes | 1 KiB | `dd iflag=fullblock` | 2.244 |
| 1,487,440 actual agent bytes | 16 KiB | `dd iflag=fullblock` | 1.615 |
| 1,487,440 actual agent bytes | 16 KiB | `dd bs=1` fallback | 2.406 |
| 1,487,440 actual agent bytes, after physical reconnect | 16 KiB | `dd iflag=fullblock` | 1.626 |

The retained image's tested agent was one contiguous extent at flash offset 12,625,408, with SHA-256 `fd29a16a1fcda646474e89177bda88fd8e4972a94dd1c819747113a788ca40f6`. Those are evidence from this image only. The production generator must support bounded multiple extents and regenerate size/digest/offsets whenever artifacts or FAT allocation change.

Research commands and files:

```sh
python3 /tmp/pico-cdc-board-research.ZexmFw/board_checks.py
python3 /tmp/pico-cdc-board-research.ZexmFw/bootstrap_checks.py
python3 /tmp/pico-cdc-board-research.ZexmFw/real_download_checks.py
```

`bootstrap_checks.py` targeted the earlier synthetic-installer firmware revision. The current probe instead serves the real-file test installer; do not rerun that harness expecting its recorded synthetic payload without rebuilding the matching revision. The current build sources are in `probe/`; JSON captures are `board-results.json`, `bootstrap-results.json`, `real-download-results.json`, and `reconnect-results.json`. These temporary research assets are not a maintained repository test suite.

Remaining integration gates: the full USB composite, HID-driven Terminal opening/typing on US and DE layouts, production arming/admission, correlated cancellation and Y Stop, safe watchdog inheritance, actual Terminal Control-C, unplug during a transfer, atomic installation, real host-agent launch and TLV handshake, multiple attached Picos, alternate USB topology, and other supported macOS versions. The prototype does not prove these behaviors or enforce all planned session/manifest/timeout checks.

# macOS host-agent installation over USB CDC

Research date: 2026-10-10. Status: implementation plan; no firmware or installer changes have been made.

## Proposed result

A user selects a macOS installation payload on the Pico. The existing keyboard service opens Terminal and types fewer than 100 command characters. A small shell receiver downloads the installer and host-agent executable directly from Pico flash over the existing USB CDC control port, verifies the executable, installs it under the user's home directory, and starts it. No public internet, Wi-Fi, mass-storage mount, Python, package manager, or administrator access is needed for the intended path.

Start with native Apple Silicon (`aarch64-apple-darwin`), which is already packaged. Detect and reject an unsupported host architecture before transferring executable bytes. Native Intel macOS support requires adding an `x86_64-apple-darwin` artifact; it must not be implied by the existing macOS directory.

Keep mass-storage delivery available during initial implementation. Removing MSC or changing its flash reservation is a separate change after this path works.

## Recommended first-stage command

The following candidate is **96 ASCII characters**, excluding the final Return:

```zsh
(p=(/dev/cu.usbmodemP*3);(($#p==1))&&exec 3<>$p&&stty raw -echo<&3&&echo B1>&3&&exec /bin/sh<&3)
```

This is a proposed command, not an installer supported by current firmware. Its prerequisites are a fresh macOS Terminal running zsh, a Pico USB serial identity starting with `P`, and confirmation that the control data interface appears with suffix `3` on the supported macOS versions.

The subshell contains the temporary variable and descriptor changes. The array collects matching control ports, and the arithmetic check refuses multiple matches. Default zsh also rejects an unmatched glob; with nullglob enabled the zero-length array fails the check. No matching port is opened until the selection succeeds. `exec 3<>$p` opens the port once for reading and writing; `stty` configures that descriptor; `B1\n` requests bootstrap version 1; `/bin/sh` then reads the delivered installer from the same connection. `&&` stops the sequence if opening, configuration, or requesting fails. The array syntax is zsh-specific. [zsh array parameters](https://zsh.sourceforge.io/Doc/Release/Parameters.html#Array-Parameters).

Do not shorten this to an unguarded wildcard: the device exposes two CDC functions, and another Pico may be attached. The guard also lets one compiled preset work with different device serial values without runtime keyboard-program generation.

| Candidate | Typed command characters | Condition |
| --- | ---: | --- |
| Guarded selector and explicit `/bin/sh`, above | 96 | Recommended production candidate after device-name validation |
| Same command using `sh` | 91 | Depends on the user's executable search path |
| Exact `/dev/cu.usbmodemP12345673` port, compact redirections | 78 | Requires matching per-device preset and firmware identity |
| Exact `/dev/cu.usbmodemP0123456789ABCDEF3` port, compact redirections | 87 | Long serial naming is unverified on supported systems |

The budget counts command characters, not USB HID reports or physical key transitions. Shifted punctuation, launcher chords, Return, and the eight characters in `Terminal` are additional keyboard actions. Measure the generated KBD1 operations and elapsed time separately; do not advertise the entire launch sequence as fewer than 100 physical keystrokes.

The compact first stage has no independent host watchdog before it receives the installer. If firmware never responds, or delivers an incomplete compound command, the host shell can remain waiting despite a device-side timeout. Document Terminal's Control-C recovery and test it; Y Stop must release device state but is not yet proven to terminate the waiting host shell. Do not claim fully automatic, bounded host recovery until that gap is resolved. If an autonomous first-stage watchdog is mandatory, revise the command and remeasure its budget rather than assuming a serial ZLP or port close supplies EOF.

## Research findings

### macOS chooses the serial-device name

Apple's published CDC ACM driver constructs a device suffix from a usable USB serial string, otherwise from USB location, and appends the data-interface number. That historical implementation accepts only short serial strings, so it is evidence for the naming strategy rather than a guarantee for current macOS. Use an ASCII alphanumeric `P`-prefixed serial of at most eight characters for the first experiment and verify the resulting nodes on actual hardware. Manufacturer and product strings do not directly select the BSD device path. [Apple CDC driver, `createSuffix` and `createSerialStream`](https://raw.githubusercontent.com/apple-oss-distributions/AppleUSBCDCDriver/main/AppleUSBCDCACM/DataDriver/Classes/AppleUSBCDCACMData.cpp).

Repository evidence: `firmware/src/usb/task.rs` creates logger CDC first, then control CDC, and currently sets `cfg.serial_number = None`. The expected data interfaces are therefore 1 and 3, but the exact descriptor-to-device-node mapping remains a board-test gate.

For the first implementation, add a build-supplied serial identity, provisioned per device, and validate its alphabet and length. A proposed input is `PICO_USB_SERIAL`; it does not exist today. Track it in Cargo rerun inputs and share its validation/generation between USB configuration and preset construction. Assign distinct identities when provisioning multiple boards. A later device-derived identity can replace provisioning after its macOS naming behavior and initialization path are verified; do not add runtime bytecode patching just to shorten this command.

### USB connection is not a bootstrap request

The workspace uses `embassy-usb` 0.6.0. Its local source shows that `wait_connection()` waits for the endpoint to be enabled, not for a host application to open the serial port. DTR is separately available. Arm the installer explicitly through the hardware preset and require `B1\n`; neither USB enumeration nor DTR alone should send executable shell text. [Embassy CDC API](https://docs.embassy.dev/embassy-usb/git/default/class/cdc_acm/struct.CdcAcmClass.html).

CDC writes must fit the maximum packet size, currently 64 bytes. A response ending on a full packet boundary needs a short packet or zero-length packet so the host can receive that phase before sending its next request. A ZLP completes a USB transaction; it does **not** provide a serial EOF. [Embassy CDC packet constraints](https://docs.embassy.dev/embassy-usb/git/default/class/cdc_acm/struct.CdcAcmClass.html).

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

Avoid embedding a second copy of the executable. The FAT builder can expose a bounded table of file extents, and the CDC sender can read those slices from the existing 4 MiB image. Generate extents while allocating clusters, and verify that reconstructing the bytes produces the packaged artifact's digest. Do not assume file contiguity or introduce a firmware FAT parser. `firmware/src/usb/msc.rs::image()` already exposes the flash-backed image.

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
4. Transfer **raw binary** in requested blocks, proposed maximum 1,024 bytes. Each bounded text request contains the expected block index. Firmware returns exactly the known block length; the host reads exactly that many bytes, appends them, checks the accumulated size, then requests the next block. There is no binary data until a block request, and no unsolicited trailing status after a block.
5. After the final block, verify exact file length and SHA-256. Send a verification result and consume a final completion line. Firmware returns to normal TLV service before the installer closes all serial handles and launches the agent.
6. On any failure, abandon the partial file and return a terminal error. Firmware clears decoder/transfer state and invalidates the generation. No partial executable is launched.

Suggested initial bounds, to be measured and enforced: one session, 64-byte request lines, 256-byte manifest/completion lines, 8 KiB installer source, 1,024-byte transfer blocks, artifact length no larger than the packaged image, 30-second arm deadline, 5-second per-phase inactivity deadline, and 120-second total install deadline. Tune deadlines only from measured full-agent transfers. Stream installer text and artifact slices directly from flash; do not put the complete script or executable on a firmware task stack.

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

Test a short alphanumeric serial on the board with logger and control interfaces present. Record actual `/dev/cu.*` names and I/O Registry interface numbers. Test direct ports, hubs, unplug/replug, two Picos, and unrelated serial devices. Gate the 96-character selector on evidence that it selects only the correct control interface. If macOS naming differs, revise the selector and remeasure before implementation proceeds.

Hardware flashing/testing is a separate explicitly authorized activity; it was not performed for this research task.

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

These checks used synthetic data and temporary files outside the repository; they did not execute or install the real host agent, open a physical USB port, or flash hardware.

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

The ad hoc harness was run with `python3 /tmp/pico_cdc_bootstrap_research.py`; Python was only a research tool, not an installer dependency. Its scenarios should become maintained integration tests during implementation. PTYs validate shell behavior and fragmented streams, but cannot validate USB device naming, DTR, endpoint backpressure, packet termination, Terminal launch timing, HID layout behavior, or board cancellation. Those remain explicit acceptance gates.

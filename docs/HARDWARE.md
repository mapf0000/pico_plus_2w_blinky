# Hardware, flashing, and recovery

This document records the board assumptions and physical validation workflow implemented by the firmware. Protocol details are in [PROTOCOL.md](PROTOCOL.md), and system lifecycle details are in [ARCHITECTURE.md](ARCHITECTURE.md).

## Supported hardware

The primary and currently wired target is:

- **Controller board:** Pimoroni Pico Plus 2 W
- **MCU:** RP2350B, Cortex-M33
- **Flash:** 16 MiB external QSPI/XIP flash
- **PSRAM:** 8 MiB APS6404L-class external PSRAM
- **On-chip SRAM:** 520 KiB exposed as 512 KiB striped RAM plus two 4 KiB direct-mapped banks
- **Wireless:** onboard CYW43439-class Wi-Fi/Bluetooth device, used for Wi-Fi AP mode
- **Display board:** Pimoroni Pico Display 2.8, ST7789, 320×240 logical orientation

The Cargo target is `thumbv8m.main-none-eabihf`. Embassy is configured with the `rp235xb` feature, which matches current B-silicon boards.

Although parts of the code and README refer generally to Pico 2/Pico 2 W, this firmware is not a drop-in build for every RP2350 board. The 16 MiB linker layout, PSRAM wiring, CYW43 PIO pins/divider, display wiring, and `rp235xb` selection are board-specific. Porting requires reviewing every item in this document.

Primary implementation locations:

- Board and peripheral construction: `firmware/src/main.rs`
- MCU/silicon feature: `firmware/Cargo.toml`
- Linker memory map: `firmware/memory.x`
- Display driver and input: `firmware/src/display/`
- USB composition: `firmware/src/usb/task.rs`
- Wi-Fi/DHCP: `firmware/src/main.rs`, `firmware/src/dhcp.rs`

## Pin map

The table describes the current `main.rs` assignments, not a generic Pico header recommendation.

| GPIO/peripheral | Function | Electrical/software notes | Implementation |
| --- | --- | --- | --- |
| Internal GPIO47 + `QMI_CS1` | External PSRAM chip select/interface | Pico Plus 2 W board wiring; uses `QmiCs1` with APS6404L configuration | `firmware/src/psram_pool.rs` |
| GP12 | Display button A | Input with pull-up; button is active-low | `firmware/src/display/mod.rs` |
| GP13 | Display button B | Input with pull-up; button is active-low | `firmware/src/display/mod.rs` |
| GP14 | Display button X | Input with pull-up; button is active-low | `firmware/src/display/mod.rs` |
| GP15 | Display button Y | Input with pull-up; button is active-low | `firmware/src/display/mod.rs` |
| GP16 | ST7789 D/C | Output | `firmware/src/display/backend.rs` |
| GP17 | ST7789 chip select | SPI device CS, idle high | `firmware/src/display/backend.rs` |
| GP18 | ST7789 SPI clock | SPI0 SCK, mode 0, 62.5 MHz | `firmware/src/display/backend.rs` |
| GP19 | ST7789 SPI data | SPI0 MOSI; transmit-only, no MISO | `firmware/src/display/backend.rs` |
| GP20 | Display backlight | Output driven high while the display task lives | `firmware/src/display/mod.rs` |
| GP23 | CYW43 power | Output, starts low | `firmware/src/main.rs` |
| GP24 | CYW43 PIO-SPI data | Bidirectional data on PIO0 state machine 0 | `firmware/src/main.rs` |
| GP25 | CYW43 PIO-SPI chip select | Output, idle high | `firmware/src/main.rs` |
| GP26 | Display RGB LED red | Active-low output | `firmware/src/display/mod.rs` |
| GP27 | Display RGB LED green | Active-low output | `firmware/src/display/mod.rs` |
| GP28 | Display RGB LED blue | Active-low output | `firmware/src/display/mod.rs` |
| GP29 | CYW43 PIO-SPI clock | PIO0 clock; RM2 divider | `firmware/src/main.rs` |
| ADC temperature sensor | RP2350 die temperature | Internal ADC channel; no external GPIO | `firmware/src/display/mod.rs` |
| DMA channel 0 | CYW43 PIO-SPI DMA | Bound to `DMA_IRQ_0` | `firmware/src/main.rs` |
| USB controller | Composite USB device | Dedicated USB pins; not assigned as GPIO in code | `firmware/src/usb/task.rs` |

The ST7789 uses no hardware reset pin in the current driver. Initialization performs a software reset and uses an orientation equivalent to a 90-degree rotation of the panel's 240×320 framebuffer, producing a 320×240 drawing area. Display initialization is best effort: on failure, the task keeps buttons, LED, and backlight alive.

### Pin-conflict rule

Before adding a peripheral, check both `main.rs` construction and the display/CYW43/PSRAM modules. Embassy peripheral values are uniquely owned, so most direct conflicts fail at construction or compile time, but board-internal functions can still be electrically incompatible even when a pin appears available in a schematic abstraction.

## Flash and RAM layout

### 16 MiB XIP flash

The linker divides the range `0x10000000..0x11000000` as follows:

| Region | Start | End exclusive | Size | Purpose |
| --- | ---: | ---: | ---: | --- |
| `FLASH` | `0x10000000` | `0x107FE000` | 8184 KiB | Boot metadata, executable firmware, embedded frontend, generated payloads, read-only data |
| `MSC` | `0x107FE000` | `0x10FFE000` | 8192 KiB | Exact 8 MiB USB mass-storage image in `.msc_image` |
| `PERSIST` | `0x10FFE000` | `0x11000000` | 8 KiB | Two alternating 4 KiB device-configuration slots |

```text
0x10000000                                                        0x11000000
    |---------------- 8184 KiB FLASH ----------------|-- 8 MiB MSC --|-- 8 KiB --|
                                                                        PERSIST
```

`firmware/src/device_config.rs` must agree with this map:

- `FLASH_CAPACITY = 16 * 1024 * 1024`
- Two slots of one 4096-byte erase block each
- Persistent offset is `FLASH_CAPACITY - 8192`
- Each slot carries magic, schema version, sequence number, payload length, CRC32, and USB identity strings
- On boot, firmware selects the newest valid sequence; on save, it erases/writes the other slot

The MSC image is generated by `crates/build-support/src/msc_image.rs` and placed by the `.msc_image` linker section. Changes to the image size require coordinated edits to the generator, firmware MSC implementation, linker region, size calculations, and documentation.

### On-chip SRAM

| Region | Address | Size | Linker name |
| --- | ---: | ---: | --- |
| Striped SRAM banks | `0x20000000` | 512 KiB | `RAM` |
| Direct-mapped bank | `0x20080000` | 4 KiB | `SRAM4` |
| Direct-mapped bank | `0x20081000` | 4 KiB | `SRAM5` |

Total on-chip SRAM represented by the linker map is 520 KiB. Most normal data, stacks, static buffers, and Embassy state use `RAM`; the direct banks are available for explicitly placed sections but are not broadly allocated by current code.

Large fixed allocations to review before changing memory use include:

- Three HTTP asset workers and two WebSocket acceptors. Only the newest WebSocket is the active logical session; the singleton transfer pump remains the only batching owner.
- HTTP RX/TX/request buffers and SRAM fallbacks.
- USB descriptor, logger, class, and control buffers.
- TLV decoder payload buffer of 2048 bytes.
- Display SPI staging buffer and render state.
- Network stack resources for 16 sockets.

The RustPython cutover release build measures 357,248 bytes of `.bss`, 2,644 bytes of `.data`, and 1,024 bytes of `.uninit` in the 512 KiB `RAM` region. Including linker alignment, the last static allocation ends at byte 360,928, leaving 163,360 bytes before runtime stack use. The one-slot HID command channel owns one bounded 4,096-byte effect; the Python VM and its heap live in the browser and consume no device SRAM. A prior failed pooled-batching build used 465,888 bytes of `.bss`: batching state had become part of all four HTTP task futures. Keeping batching in one dedicated transfer task and placing its uniquely owned batch slot in PSRAM avoids that multiplication. The two WebSocket acceptors share the single logical transfer path; they duplicate only their bounded connection/task state. Treat async future sizes and task-pool multiplicity as part of every SRAM review; successful linking alone does not guarantee enough runtime headroom.

### External PSRAM

The default `psram` feature probes the external memory through QMI CS1 using an APS6404L configuration. The reported board capacity is 8 MiB.

The PSRAM pool reserves address ranges for the network and transfer data planes:

- Three disjoint 8 KiB HTTP receive and 32 KiB transmit buffers.
- Two disjoint 8 KiB WebSocket receive and 32 KiB transmit buffers.
- One 16,402-byte encrypted-chunk batch buffer.
- Five 4 KiB SRAM receive/transmit fallback slots, plus five 4 KiB request buffers.

In the default `psram` build, detection or capacity failure makes HTTP and WebSocket workers continue with their SRAM buffers while the transfer pump sends individual chunks. A build compiled without the `psram` feature reserves one SRAM batch slot instead. The rest of PSRAM is not a general allocator; code cannot assume `Vec`/heap allocation becomes available merely because the feature is enabled.

## Composite USB device

Firmware exposes one USB device containing four functions:

| Function | Purpose | Important behavior |
| --- | --- | --- |
| CDC-ACM logger | Human-readable firmware logs | Used by `scripts/pico-run`; log level is Info in the USB logger |
| CDC-ACM control | Host-agent TLV protocol | Host agent probes candidates to distinguish it from logger/noisy ports |
| HID boot keyboard | Executes `KBD1` scripts | IN reports, 8-byte keyboard report, 10 ms poll interval |
| Mass storage | Distributes host-agent binaries | 8 MiB read-only FAT16 volume |

Default USB descriptor values:

| Property | Value |
| --- | --- |
| VID | `0x1209` |
| PID | `0x0001` normally; optional boot-varying code can choose `0x0001`/`0x0002` |
| Manufacturer | Persisted setting; default `Pico 2W` |
| Product | Persisted setting; default `Logger + Keyboard` |
| Serial number | None by default |
| Descriptor max power | 100 mA |
| EP0 max packet | 64 bytes |

The volume label defaults to `PICO_AGENT` and can be overridden at build time with `PICO_MSC_LABEL` (maximum 11 valid FAT-label characters after normalization). Expected contents:

```text
/README.TXT
/MAC/HOSTAGNT
/WIN/HOSTAGNT.EXE
/LINUX/HOSTAGNT
```

The aarch64 macOS artifact is considered required by the image builder; Windows and Linux artifacts are optional. Missing artifacts are represented by `MISSING.TXT` in the corresponding directory. The custom MSC implementation rejects writes and should be treated as immutable distribution media: copy the agent to a writable host directory before running it.

Safe, no-Wi-Fi automated coverage for enumeration, CDC framing, encrypted-transfer rejection, restricted production host-agent handshake/keepalive behavior, and read-only mass-storage metadata is provided by `scripts/device-test`. See [DEVICE_TESTING.md](DEVICE_TESTING.md) for its safeguards, commands, and coverage limits.

USB starts automatically during boot in the current implementation. `USB_UNREGISTER` detaches the entire composite device; re-registration rebuilds all classes and descriptors. Changing identity while USB is enabled is rejected because descriptors are fixed for the active session.

Implementation:

- Composite builder/session: `firmware/src/usb/task.rs`
- Start/stop lifecycle: `firmware/src/usb/usb_supervisor.rs`
- HID: `firmware/src/usb/hid.rs`, `crates/firmware-exec`
- Control CDC: `firmware/src/usb/ctrl.rs`
- MSC device: `firmware/src/usb/msc.rs`
- MSC image generator: `crates/build-support/src/msc_image.rs`

## Wi-Fi access point and local network

Current compile-time configuration:

| Property | Value |
| --- | --- |
| SSID | `PicoEndpoint` |
| WPA mode | WPA2 |
| Password | `pico12345` |
| Channel | 6 |
| Device/gateway address | `192.168.4.1/24` |
| DHCP pool | `192.168.4.100`–`192.168.4.200` |
| DHCP lease capacity | 16 clients |
| Advertised router/DNS | `192.168.4.1` |
| HTTP port | 80 |
| UI | `http://192.168.4.1/` |
| Health check | `http://192.168.4.1/health` |
| WebSocket | `ws://192.168.4.1:81/ws` |

The device does not provide an upstream internet route or DNS forwarding. The DNS address is supplied to satisfy client network configuration. Some operating systems may warn that the Pico network has no internet access.

The password is compiled into firmware and present in source. Treat this AP as a local control network, not a strong security boundary. Changing SSID/password/channel currently requires a firmware edit in `firmware/src/main.rs` and rebuild.

The CYW43 path uses:

- Firmware, CLM, NVRAM, and Bluetooth firmware blobs under `firmware/cyw43-firmware/`.
- PIO0 state machine 0 and DMA channel 0.
- `RM2_CLOCK_DIVIDER`, which is important for the RM2-based Pico Plus 2 W.
- Power management disabled while operating as an access point.

## Build prerequisites

From the repository root:

```sh
rustup target add thumbv8m.main-none-eabihf
rustup target add wasm32-unknown-unknown
cargo install trunk
```

Install `picotool` for USB flashing. On macOS:

```sh
brew install picotool
```

The firmware build requires Trunk because it embeds the current frontend. To package a real macOS host-agent binary into the MSC image before flashing, use:

```sh
scripts/fw-deploy-with-agent
```

## Flashing paths

### Normal USB flash and logs

Build, flash, reboot, wait for CDC, and stream logs:

```sh
cargo run -p pico_rust --release --target thumbv8m.main-none-eabihf
```

The root Cargo runner calls:

```sh
picotool load -u -v -x -t elf <firmware-elf>
```

Relevant runner controls:

- `--no-wait`: flash and exit without waiting for CDC.
- `--timeout=<seconds>`: override the default 120-second CDC wait.
- `PICO_WAIT_USB_TIMEOUT=<seconds>`: environment equivalent.

The runner prints an ELF flash/RAM estimate when it can find an appropriate size tool or parse the ELF itself. After flashing it chooses the first matching macOS/Linux CDC device and streams it; with two CDC functions present, the host agent's active probe is more reliable than filename order for finding the control port.

Flash only, without opening the runner, by invoking `picotool` directly on the release ELF under `target/thumbv8m.main-none-eabihf/release/`.

### BOOTSEL recovery

Use physical BOOTSEL mode when the application is not running, USB descriptors are broken, normal `picotool -u` cannot reboot the board, or a bad firmware image prevents enumeration:

1. Disconnect USB power.
2. Hold the board's BOOTSEL button.
3. Reconnect USB while continuing to hold BOOTSEL briefly.
4. Confirm that `picotool info` can see an RP2350 boot-ROM device.
5. Re-run the normal Cargo flash command or the direct `picotool load` command.

BOOTSEL runs ROM code and does not depend on the application USB stack, Wi-Fi, display, or host agent.

Normal ELF flashing writes the sections present in the ELF. The persistent-config region has no normal firmware section, so do not assume an ordinary firmware update resets USB identity. A deliberate full-flash erase is destructive and should only be used when recovery requires it and persisted state may be discarded.

### SWD/debug probe

`firmware/Embed.toml` contains a `cargo-embed`/probe-rs configuration for RP235x over SWD at 20 MHz, including RTT/defmt settings. Use this path when USB boot recovery is insufficient or when debugging early startup. It requires a compatible external probe and correct SWD wiring; the normal project runner does not use it.

## Recovery guide

| Symptom | Checks and recovery |
| --- | --- |
| `picotool` cannot find the board | Try a data-capable cable/port; enter physical BOOTSEL; run `picotool info`; then flash again. |
| Flash succeeds but no CDC appears | Wait through re-enumeration; inspect both CDC devices; use `--no-wait` to separate flash from serial diagnosis; verify the USB task starts in early logs/RTT. |
| Host agent selects the logger port | Pass `--port`, or let active probing evaluate all USB CDC candidates; remove stale cached selection by reconnecting/restarting after the failed dispatch. |
| UI is unreachable | Join `PicoEndpoint`, verify the client has `192.168.4.x`, open `/health`, then inspect CYW43/AP/DHCP logs. |
| UI loads but WebSocket reconnects | Open port 81 `/ws` through the served page host, check `HELLO`, confirm both WebSocket acceptors started, and inspect firmware logs. A newer page receives session ownership and closes the previous page with code 4001. |
| Display is blank | Confirm the Pico Display 2.8 is seated correctly; verify GP16–GP20; inspect ST7789 initialization logs. Buttons/LED should remain functional on display init failure. |
| PSRAM is not detected | Verify this is the Pico Plus 2 W variant and internal GPIO47/QMI CS1 wiring; firmware should fall back to SRAM and log the condition. |
| MSC mounts but agent is missing | Rebuild with `scripts/fw-deploy-with-agent`; inspect `README.TXT`/`MISSING.TXT`; confirm the artifact target path. |
| USB identity change does not appear | Detach/re-register USB or power-cycle so descriptors are rebuilt; config changes are rejected while USB is enabled. |
| Persistent config appears corrupt | Firmware should choose the other valid CRC-checked slot or defaults. Avoid erasing flash until both slots and linker boundaries have been checked. |

## Hardware smoke-test checklist

Use this checklist for changes to startup, pins, memory, network, USB, transfer, or the build pipeline. Record skipped steps in the handoff.

### Build and boot

- [ ] `cargo build -p pico_rust --release --target thumbv8m.main-none-eabihf` succeeds.
- [ ] The firmware and MSC sections fit their linker regions; size output is plausible.
- [ ] `picotool` flashes and executes the ELF without verification errors.
- [ ] The board re-enumerates over USB after reset.
- [ ] Logs reach the USB logger CDC without repeated task failures or panics.

### Display and local peripherals

- [ ] ST7789 initialization reports a 320×240 drawing area and the UI renders correctly.
- [ ] Buttons A, B, X, and Y register one debounced action each.
- [ ] RGB LED red/green/blue channels illuminate correctly and turn fully off.
- [ ] Backlight remains enabled.
- [ ] System page temperature, flash, PSRAM, and uptime values are credible.

### Wi-Fi and HTTP

- [ ] `PicoEndpoint` appears on channel 6 and accepts the configured WPA2 password.
- [ ] A client receives an address within `192.168.4.100`–`192.168.4.200`.
- [ ] `http://192.168.4.1/health` returns `ok`.
- [ ] The embedded UI, JavaScript, WebAssembly, CSS, and IndexedDB helper load without 404/integrity errors.
- [ ] Port 81 `/ws` opens and the first application message is a compatible `HELLO`.
- [ ] Disconnect/reconnect restores status/config and does not leave stale pending actions.

### Composite USB

- [ ] `scripts/device-test` passes its no-Wi-Fi raw CDC, host-agent, and mass-storage checks.
- [ ] Both CDC-ACM interfaces enumerate.
- [ ] Logger output is readable and the host agent identifies the control interface.
- [ ] HID enumerates as a boot keyboard.
- [ ] The `PICO_AGENT` mass-storage volume mounts read-only and expected files are present.
- [ ] USB detach/re-register causes clean re-enumeration.
- [ ] Persisted manufacturer/product values survive a power cycle and appear after re-enumeration.

### Host agent and transfers

- [ ] Host-agent `HELLO` presence/version/hostname becomes visible and `STATUS.host_os` matches the host; after stopping the agent and waiting 25 seconds, a fresh/reconnected `HELLO` reports it absent.
- [ ] Keepalive traffic survives at least several intervals without reconnect churn.
- [ ] Filesystem browsing handles home, root, pagination, hidden entries, cancellation, and permission errors.
- [ ] Unattended negotiation establishes an encrypted session without exposing bootstrap or session secrets in structured logs.
- [ ] A small encrypted transfer completes with the correct name, size, authenticated SHA-256 receipt, and downloaded bytes.
- [ ] A multi-window transfer exercises backpressure and completes without out-of-order errors.
- [ ] Browser disconnect during transfer aborts cleanly and does not deadlock the USB sender.
- [ ] Plaintext transfer RPCs/tags and simulation mode are rejected without terminating the host reconnect loop.

### Keyboard safety

- [ ] Test only with focus in a disposable text editor or controlled test machine.
- [ ] A simple tap/text script produces the expected layout-specific characters.
- [ ] Invalid magic, malformed bytecode, and oversize bytecode are rejected without a panic.
- [ ] After a script failure or USB detach, no key remains logically pressed.

### Recovery

- [ ] Physical BOOTSEL mode is reachable with the current enclosure/cabling.
- [ ] `picotool info` recognizes the ROM device.
- [ ] A known-good release can be reflashed from BOOTSEL and returns to normal boot.

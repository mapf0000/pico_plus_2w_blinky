# File Transfer

This repository implements file transfer from the computer connected to the
Pico over USB to a browser connected to the Pico over Wi-Fi.

The Pico is a streaming relay. It does not store the complete transferred file.

```text
USB-connected computer
  host-agent reads a local file
        |
        | USB CDC, binary TLV frames
        v
Pico firmware
        |
        | WebSocket over the Pico access point
        v
Receiving browser
  chunks are stored in IndexedDB
        |
        v
  user saves the completed file
```

## Current status

The transfer path is implemented across the host-agent, firmware, and web
frontend:

- The host-agent reads files, calculates SHA-256, sends chunks, processes ACKs,
  and retries timed-out chunks.
- The firmware incrementally decodes USB frames, validates transfer metadata and
  chunks, and forwards accepted chunks to the active WebSocket client.
- The browser validates chunk CRC32 values, stores chunks in IndexedDB, displays
  progress/rate/ETA, verifies the complete SHA-256, and saves the file.
- The browser can explore the source computer's filesystem through the running
  host-agent, including folders, files, symlinks, metadata, hidden files, and
  paginated directory listings.
- Transfers can be started from the browser, from the Pico display, or when the
  host-agent starts with `--send-file`.

This is a connected-session transfer mechanism. Resume after a browser reload
or WebSocket disconnect is not supported.

## Supported setup

### Pico hardware

The primary target is the Pimoroni Pico Plus 2 W (RP2350B). The transfer uses:

- USB composite device: mass storage plus CDC control interfaces
- CYW43 Wi-Fi in access-point mode
- The embedded Yew web frontend

### Source computer

The source computer is connected to the Pico by USB and runs `host-agent`.

The firmware exposes a read-only FAT16 volume named `PICO_AGENT`. The current
image packages:

| Platform | Path on USB volume | Status |
| --- | --- | --- |
| macOS ARM64 | `/MAC/HOSTAGNT` | Supported and packaged |
| Windows x86-64 | `/WIN/HOSTAGNT.EXE` | Optional placeholder |
| Linux x86-64 | `/LINUX/HOSTAGNT` | Optional placeholder |

The agent must run on the source computer because a USB device cannot directly
read files from its host's filesystem.

### Receiving computer

The receiving computer connects directly to the Pico access point:

| Setting | Value |
| --- | --- |
| SSID | `PicoEndpoint` |
| Password | `pico12345` |
| Web UI | `http://192.168.4.1/` |

The receiving browser must remain open and connected for the duration of the
transfer.

## Build and deploy

From the repository root, build the local host-agent, include it in the USB
mass-storage image, build the frontend and firmware, and flash the Pico:

```sh
scripts/fw-deploy-with-agent
```

The script detects the current Rust host target. Override it when necessary:

```sh
HOST_AGENT_TARGET=aarch64-apple-darwin scripts/fw-deploy-with-agent
```

To build only the macOS ARM64 host-agent artifact:

```sh
scripts/build-host-agent aarch64-apple-darwin
```

The artifact is copied to:

```text
apps/host-agent/artifacts/aarch64-apple-darwin/host-agent
```

The normal firmware build then embeds it in `host-agent.img`.

## Transfer workflow

### 1. Connect the receiving computer

1. Power the Pico.
2. Join the `PicoEndpoint` Wi-Fi network using `pico12345`.
3. Open `http://192.168.4.1/`.
4. Keep the page open during the transfer.

### 2. Start USB

USB is started on demand from the web UI:

1. Select the source computer's operating system.
2. Use **Start USB** or **Start USB on macOS (Assistant)**.
3. Wait until the UI reports that USB is ready.

The source computer should now see the `PICO_AGENT` volume and the CDC serial
interfaces.

### 3. Run the host-agent on the source computer

On macOS ARM64:

```sh
cp /Volumes/PICO_AGENT/MAC/HOSTAGNT ./host-agent
chmod +x ./host-agent
./host-agent
```

The agent discovers and probes serial ports automatically. If discovery is
ambiguous, provide a port explicitly:

```sh
./host-agent --port /dev/cu.usbmodemXXXX
```

VID/PID filtering is also supported:

```sh
./host-agent vid=1209 pid=1234
```

### 4. Start a transfer

There are three supported ways to start.

#### Start from the browser

Use the **Source Files** card to explore the source computer:

1. **Home** opens the host user's home directory.
2. **/** opens the filesystem root; mounted macOS volumes are under `/Volumes`.
3. Select a folder or breadcrumb to navigate.
4. Use **Show hidden** when dotfiles should be included.
5. Select **Transfer** beside a regular file to queue it immediately.
6. **Select** copies the path into the manual **Start Transfer** field.

Directories are returned in bounded pages. Use **Load more** when a directory
contains more entries than fit in the current page.

The manual **Start Transfer** card remains available: enter an absolute path
that exists on the source computer and select **Queue Transfer**. In both cases,
the browser sends the path through the Pico to the running host-agent. The
host-agent opens that path locally and starts sending it.

**Set Default** updates the running host-agent's default path without starting
a transfer. The default path is used by the Pico display's start action.

#### Start when launching the host-agent

```sh
./host-agent --send-file /absolute/path/to/file.bin
```

Each `--send-file` argument is queued when the agent connects. The first path
also becomes the default path for later default-start requests.

#### Start from the Pico display

Open the **Transfer** page:

- `A/B`: select an action
- `X` on **Start Transfer (default)**: request the host-agent's default path
- `X` on the mode row: toggle relay and simulation modes

Relay mode requires an active browser WebSocket connection. Simulation mode
executes the USB protocol and progress accounting but intentionally drops chunk
data instead of forwarding it to the browser.

### 5. Save the received file

During transfer, the browser displays:

- status
- received and total bytes
- progress percentage
- smoothed transfer rate
- estimated time remaining
- finished, retrying, and failed chunk counts

Incoming chunks are persisted in IndexedDB. When the transfer is complete,
select **Download**:

1. The browser waits for pending IndexedDB writes.
2. It verifies the complete SHA-256 against the value supplied by the
   host-agent.
3. It writes the file through the File System Access API when available.
4. Otherwise it uses a Blob download, limited to 128 MiB.
5. Successfully saved transfer chunks are removed from IndexedDB.

Canceling or failing the save operation leaves the completed transfer available
for another download attempt.

## Host-agent options

Relevant command-line options are:

| Option | Purpose |
| --- | --- |
| `--send-file <path>` | Queue a file and set the first file as the default |
| `--port <path>` | Use a specific serial port |
| `--vid <value>` / `vid=<value>` | Filter USB serial ports by VID |
| `--pid <value>` / `pid=<value>` | Filter USB serial ports by PID |
| `--cwd <path>` / `cwd=<path>` | Change the agent working directory |
| `--probe-timeout-ms <ms>` | Configure serial-port probing |
| `--debug-log <path>` | Store debug messages received from the Pico |
| `--raw` | Log raw serial input |
| `--debug` | Enable debug-level logging |

`--send-file` can be repeated. Transfers are processed serially by the current
host-agent transfer worker.

## Protocol

### USB CDC framing

Every USB control message uses this TLV frame:

```text
+--------+----------------------+------------------+
| tag:u8 | payload_len:u32 LE   | payload          |
+--------+----------------------+------------------+
```

The maximum TLV payload is 2048 bytes.

### Transfer tags

| Tag | Name | Direction | Purpose |
| ---: | --- | --- | --- |
| 20 | `FILE_OPEN` | agent → Pico | Declare transfer metadata |
| 21 | `FILE_CHUNK` | agent → Pico | Send one file chunk |
| 22 | `FILE_ACK` | Pico → agent | Acknowledge contiguous chunks and grant credit |
| 23 | `FILE_CLOSE` | agent → Pico | Finish the byte stream |
| 24 | `FILE_RESULT` | Pico → agent | Report the Pico relay result |
| 25 | `FILE_ABORT` | both | Terminate a transfer with a reason |
| 26 | `FILE_HEARTBEAT` | reserved | Long-transfer heartbeat |
| 27 | `FILE_START_REQUEST` | Pico → agent | Start a path or the default path |
| 28 | `FILE_SET_DEFAULT_PATH` | Pico → agent | Update the default source path |
| 29 | `FS_LIST_REQUEST` | Pico → agent | Request one directory page |
| 30 | `FS_LIST_PAGE` | agent → Pico | Return directory entries or an error |
| 31 | `FS_LIST_CANCEL` | Pico → agent | Mark a directory request as stale |

All multibyte integers are little-endian.

### Filesystem listing

`FS_LIST_REQUEST` contains:

```text
protocol_version : u16
request_id       : u64
cursor           : u32
entry_limit      : u16
flags            : u8
path_len         : u16
path             : UTF-8 bytes
```

An empty path resolves to the host user's home directory. Flag bit 0 includes
hidden dotfiles. The browser currently requests up to 64 entries per page.

`FS_LIST_PAGE` contains the request ID, status, continuation cursor, canonical
directory, optional error, and repeated entry records. Each entry includes:

```text
kind             : u8
flags            : u8
size             : u64
modified_secs    : u64
name_len         : u16
name             : UTF-8 bytes
```

Kinds distinguish files, directories, file/directory symlinks, and other
filesystem objects. Listings are sorted with directories first and then by
case-insensitive name. Non-UTF-8 names are omitted because the current browser
and command protocol address paths as UTF-8.

Each page is generated from a fresh directory read. If a directory changes
between continuation requests, entries can move between pages; refresh the
directory to obtain a consistent current view.

### `FILE_OPEN`

```text
protocol_version : u16
transfer_id      : u64
total_size       : u64
chunk_size       : u16
chunk_count      : u32
sha256           : [u8; 32]
file_name_len    : u16
file_name        : UTF-8 bytes
```

The current protocol version is `1`. Firmware accepts file names up to 96
bytes. A duplicate open is accepted only when its metadata matches the active
transfer.

### `FILE_CHUNK`

```text
transfer_id : u64
chunk_index : u32
offset      : u64
payload_len : u16
payload     : bytes
crc32       : u32
```

The fixed overhead is 26 bytes, so the maximum data portion is 2022 bytes. The
host-agent currently uses that maximum as its default chunk size.

Firmware accepts chunks in order. It validates:

- transfer ID
- chunk index and offset
- declared and actual payload length
- total-size bounds
- CRC32

Duplicates are re-ACKed. A CRC mismatch causes the chunk to be retried.
Protocol and bounds violations abort the transfer.

### `FILE_ACK`

```text
transfer_id             : u64
highest_contiguous_chunk: u32
next_expected_offset    : u64
window_credit           : u16
```

`u32::MAX` means that no chunk has been acknowledged. Firmware currently grants
a fixed credit of eight chunks after accepted input. The host-agent limits
credit to 64 and retries the oldest outstanding chunk after a five-second ACK
timeout, up to five retries.

### `FILE_CLOSE`, `FILE_RESULT`, and `FILE_ABORT`

`FILE_CLOSE` contains the transfer ID, sent chunk count, and sent byte count.
Firmware returns a result after validating counts and total size.

Result codes are:

| Code | Meaning |
| ---: | --- |
| 0 | OK |
| 1 | SHA-256 mismatch |
| 2 | Size mismatch |
| 3 | Aborted |
| 4 | Internal error |

The Pico does not calculate the complete SHA-256 because it does not retain the
file. Final SHA-256 verification occurs in the browser during download.
Consequently, an OK result to the host-agent means that the Pico accepted and
queued all bytes, not that the browser has saved the file.

## WebSocket messages

The firmware and frontend share one WebSocket connection for command responses
and transfers.

Control events are JSON text messages:

- `transfer/open`
- `transfer/progress`
- `transfer/chunk_status`
- `transfer/finished`
- `transfer/failed`
- `transfer/aborted`

Binary WebSocket messages begin with a one-byte kind:

| Kind | Payload |
| ---: | --- |
| 1 | File-transfer chunk |
| 2 | Filesystem listing page |

A kind-1 transfer payload contains:

```text
transfer_id : u64
chunk_index : u32
offset      : u64
payload_len : u16
crc32       : u32
payload     : bytes
```

The firmware maintains a bounded 16-event WebSocket queue. Starting relay mode
without a browser, disconnecting the browser, or filling this queue aborts the
transfer rather than buffering the complete file on the Pico.

## Storage and integrity

Integrity is checked at two levels:

- CRC32 detects corruption of each chunk at the Pico and again in the browser.
- SHA-256 verifies the complete ordered file in the browser before saving.

The browser requires ordered chunks and validates indexes, offsets, sizes, and
the final byte count. IndexedDB holds chunk data only; transfer progress state
is kept in memory.

Reloading the page discards the active in-memory transfer state. Start the
transfer again after reconnecting.

## Security model

The current deployment assumes a controlled physical and wireless environment:

- Wi-Fi uses WPA2 with credentials compiled into the firmware.
- HTTP and WebSocket traffic are not protected by TLS.
- The web UI has no separate user authentication.
- A connected browser can request any path readable by the running host-agent.
- The host-agent also supports other privileged USB commands, including command
  execution.

Run the host-agent only while this access is intended, and stop USB or terminate
the agent when finished.

## Operational limits

| Limit | Current value |
| --- | ---: |
| USB TLV payload | 2048 bytes |
| File data per chunk | 2022 bytes |
| Firmware active transfer records | 4 |
| Firmware WebSocket transfer queue | 16 events |
| Browser Blob fallback | 128 MiB |
| Transfer path sent by firmware | 512 bytes |
| Filesystem path sent by firmware | 512 bytes |
| Filesystem page request | 64 entries |
| Host filesystem page maximum | 128 entries |
| Browser filesystem timeout | 15 seconds |
| File name accepted by firmware | 96 bytes |
| Host ACK timeout | 5 seconds |
| Host chunk retry limit | 5 |

The File System Access API path streams chunks from IndexedDB to the destination
and is preferred for large files. Browser storage quotas and free disk space
remain platform-dependent.

## Implementation map

| Component | Location |
| --- | --- |
| Host TLV codec | `apps/host-agent/src/tlv.rs` |
| Host sender and transfer codec | `apps/host-agent/src/file_transfer.rs` |
| Host filesystem listing | `apps/host-agent/src/filesystem.rs` |
| Host request dispatch | `apps/host-agent/src/dispatch.rs` |
| Firmware USB decoder and relay | `firmware/src/usb/ctrl.rs` |
| Firmware WebSocket endpoint | `firmware/src/http/routes/ws.rs` |
| Browser WebSocket client | `apps/frontend/src/api.rs` |
| Browser filesystem state/codec | `apps/frontend/src/filesystem.rs` |
| Browser transfer state/integrity | `apps/frontend/src/transfer.rs` |
| Browser IndexedDB and download | `apps/frontend/ui/idb.js` |
| Browser transfer UI | `apps/frontend/src/lib.rs` |
| Pico display transfer page | `firmware/src/display/page_transfer.rs` |

## Verification

Run host-agent unit and macOS PTY integration tests:

```sh
cargo test -p host-agent
```

Run frontend filesystem/transfer state, CRC, SHA-256, and rate/ETA tests:

```sh
cargo test -p frontend
```

Check the firmware for the embedded target:

```sh
cargo check -p pico_rust --target thumbv8m.main-none-eabihf
```

The automated suite does not currently exercise a complete
host-agent-to-firmware-to-browser hardware transfer. Validate the complete flow
on hardware after protocol, USB, WebSocket, or storage changes.

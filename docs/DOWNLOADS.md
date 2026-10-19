# Encrypted native downloads

The companion receives files from the computer connected to the Pico's USB CDC
port. That computer must run the updated host agent. The companion uses Bluetooth;
no Wi-Fi or browser is needed. The host file session uses transfer-v2 Noise and
per-file authenticated encryption, independently of the provisioned BLE control
key. The Pico relays ciphertext and never receives file keys or plaintext bytes.
This does not authenticate the source computer against a malicious Pico/USB relay;
see THREAT_MODEL.md for the existing unattended bootstrap boundary.

Acquire control, then open **Encrypted file downloads** and select **Connect
encrypted host session**. Enter the source path on the USB computer (or use a
selected filesystem path) and a **new absolute local filename** on the control
computer. Select **Queue file**. **Set host default path** and **Queue host default**
reuse the agent's in-memory default candidate; they do not change local saving.
Existing destinations are rejected, including symlinks. Host filenames are display
metadata and never choose the local destination. Downloaded files are never opened
or executed automatically.

One file receives at a time; up to four requests and eight history entries are
retained. Progress shows accepted bytes, approximate rate/ETA, retries and terminal
errors. Start/default/queue commands require acquired control, a present agent and
an advertised download-capable firmware. Stop an active keyboard effect before
issuing download controls. Keyboard execution may continue alongside an already
receiving file, and keyboard Cancel keeps its priority.

Each file is limited to **1 MiB** for this prototype. At default ATT MTU the current
16-bit connection counters and four-fragment credit window limit a connection's
usable lifetime; larger files need transport/lifetime work and measured acceptance.
Re-authenticate a fresh BLE connection before exhausting counters. No transfer
resumes or retries automatically after disconnect.

Plaintext streams into an anonymous temporary file in the chosen destination's
parent directory. The OS removes that staging file when its last handle closes,
including abrupt app exit. Each chunk must authenticate and have the expected
index and length before writing; exact retries are idempotent. The final encrypted
close must match the manifest's size/count and the incrementally computed SHA-256.
After verification, a bounded copy (with a second streamed SHA-256 check) goes into a private named save file, is flushed
and synced, and is persisted without replacing an existing destination.
Only then is the encrypted completion receipt sent. Saving can temporarily require
twice the file size of free disk space; only one staging/copy is active. An abrupt
exit during the final save may leave a verified `.pico-download-*` temporary file.
Ordinary cancellation, disconnect and app shutdown clean up staging/save files.
Saved files are ordinary plaintext local files; transport encryption is not
at-rest encryption. Filesystem and crash-durability semantics depend on the OS. No-overwrite
persistence may leave a second hard link on some platforms/filesystems; full
transaction atomicity is not claimed. The chosen local directory is trusted.

The receiver withholds the final BLE record credit until authentication and the
bounded disk work complete. The Pico withholds its USB chunk ACK until that final
credit arrives. There is one bounded relay record and one owned native disk task;
no whole-file memory buffer or PSRAM batch pool. One retained 20-byte ATT write
finishes before a newer command token enters the platform queue; the entire
two-fragment ACK retains one owner and sealed token across refresh preemption. The sender starts with one chunk
credit and waits up to 60 seconds for an ACK (five exact-ciphertext retries).
Download control/session-restart requests allow 30 seconds to drain a cancelled
record; priority Cancel retains a four-second radio I/O guard.
The BLE stream inactivity limit is 30 seconds; the USB relay/receiver inactivity
limit is 60 seconds. Slow/full disk and malformed records fail closed.

**Cancel downloads** discards queued work, resets the host session and releases the
Pico relay wait. A BLE-only barrier drains the old stream before another session
starts. Cancellation does not undo a completed save and can race the final persistence
operation; the app reports that uncertainty if final saving was outstanding.
Check the explicitly chosen filename before retrying. Reconnect the host
session to retry explicitly. USB-off, control loss, app exit and BLE loss terminate
active downloads; saved files remain available.

Headless board acceptance writes only the explicit new destination and does not
emit keyboard effects or change persistent identity/keys:

```sh
cargo run -p pico-companion -- --profile /private/path/pico.json \
  --download-self-test --test-file /host/disposable/sample.bin \
  --test-destination /control/new-sample.bin
# Optional cancellation check instead of saving:
# add --test-cancel-after-ms 2000
# To restart the host session on the same BLE link and cancel again:
# also add --test-restart-after-cancel
```

Evidence and remaining hardware/platform checks are in DEVICE_TESTING.md and
NATIVE_COMPANION_BLE_PLAN.md. Native code: downloads.rs and download_crypto.rs;
connection/credit ownership: ble.rs; device bridge: ble_download_core.rs and
ble_download.rs. Existing USB transfer-v2 encodings are unchanged.

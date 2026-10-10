# Current Bluetooth security boundary

The provisioning PC, control PC/profile and Pico firmware are trusted; the USB
target may be hostile. A unique random 32-byte device key and device ID must be
installed through a trusted provisioning route before unattended deployment.
There is no universal default, unauthenticated BLE enrollment/reset, USB key
retrieval or physical-confirmation requirement. No SMP pairing/bond is used.

A fixed `Noise_NNpsk0_25519_ChaChaPoly_SHA256` handshake establishes fresh
directional record keys with empty payloads and a service/version/device-bound
prologue. Its first message is replayable, so it cannot grant authority or execute
commands. A subsequent authenticated record proves possession of fresh session
keys; encrypted Acquire grants control. All commands/results/status snapshots
thereafter use authenticated records with strictly newer directional sequence
numbers, including Cancel and Poll. Reconnect changes keys and never replays
keyboard effects. Public Info/Status are unauthenticated diagnostics and must not
be treated as peer identity or authority. See PROTOCOL.md for exact framing.

Key possession authorizes the device, rather than identifying a particular PC.
The prototype stores keys in private profile files on the trusted control PC and
plaintext dedicated Pico flash; CRC only detects corruption. Unix loaders reject
group/world-readable profiles. OS credential-store integration, Windows ACL
acceptance, remote authenticated rotation and revocation are unfinished. Both the
JSON profile and provisioning image contain the key and need private handling;
back up credentials outside build output before deployment. Losing all copies
requires another trusted provisioning route. Whole-flash reading, compromised
firmware, physical extraction, control-PC compromise and radio traffic analysis
are outside this protection. No secure boot/flash encryption claim is made.

Fresh ephemeral randomness comes from OS getrandom on the PC and Embassy RP's
ROSC RNG on the Pico. ROSC must remain running as configured; physical entropy
quality/fault attacks are not validated. The no_std session core uses fixed
buffers and zeroizing key types; caller-owned key/plaintext buffers are cleared.
Upstream Noise scratch is not comprehensively zeroized or independently audited.
TrouBLE logs stay disabled, and application diagnostics omit keys/text/bytecode.

Only acquired control, USB enable/disable and bounded keyboard effects are exposed.
No shell, credentials, file contents or filesystem browsing is routed over BLE.
Disconnect cancels this connection's effects, and stale IDs cannot update a new
connection. Local jobs and physical Y Stop remain independent.

One connection can occupy the radio. Unauthenticated access expires 30 seconds
after accept even under continuous reads/fragments, and disconnect has a 250-ms
cooldown. These bounds do not prevent repeated connection attempts or radio
jamming; strong denial-of-service resistance is outside the prototype. Crypto
unit tests/compilation do not establish hardware/platform acceptance.

The WLAN/AP/browser frontend and Python Worker have been removed. The following
sections retain the previous USB bulk-crypto design as a historical reference.
Browser/Wi-Fi authorization descriptions are not current functionality. The
host-agent bulk protocol is preserved, but firmware refuses delivery without a
receiver and does not acknowledge file chunks. Bluetooth bulk transfer requires
a separate authenticated receiver and backpressure design.

# Threat model

This document defines the project's security boundary and the limits of its current protections. Wire formats are specified in [PROTOCOL.md](PROTOCOL.md); the encryption lifecycle is described in [ARCHITECTURE.md](ARCHITECTURE.md#file-transfer-path-and-backpressure).

## Trust boundary

The PC connected to the Pico over USB is considered hostile. Its OS, processes, filesystem, and host-agent instance may be observed, modified, or replaced by an adversary. A correctly behaving host agent is needed for normal operation, but its presence, version, hostname, and protocol responses are not evidence of trust.

The Pico firmware, its embedded frontend, and devices connecting over Wi-Fi are considered trusted. This assumes trusted firmware provisioning, an uncompromised browser/device, and a Wi-Fi environment restricted to trusted clients. The current unattended session protocol does not enforce client identity; every client able to reach the Web UI can negotiate a transfer session.

```text
Hostile PC / host agent <-- untrusted USB input --> Trusted Pico
                                                       |
                                             Trusted Wi-Fi clients
                                             and browser frontend
```

Trusting the Pico and browser does not make host-provided data trustworthy. USB frames, agent status, directory listings, filenames, file bytes, command results, credential responses, and diagnostics remain untrusted at every layer that consumes them, including after successful decryption.

## Adversary capabilities

Assume an adversary on the PC can:

- Read or change source files before or during transfer.
- Inspect or replace the agent, read its memory and session keys, and alter its responses.
- Generate valid encrypted records containing malicious content and a matching SHA-256.
- Send malformed, oversized, duplicated, reordered, stale, or unsolicited USB messages; flood the link or disconnect it.
- Observe commands, paths, and other information intentionally sent to the PC, along with USB timing and transfer activity.

A passive USB observer is a separate, narrower threat: it records traffic without controlling either cryptographic endpoint or modifying negotiation. An active USB intermediary can modify negotiation and impersonate an endpoint because the bootstrap secret is sent over the same link and there is no independent identity authentication.

## What transfer encryption guarantees

With correctly behaving cryptographic endpoints, v2 encrypts transfer requests/paths, manifests, file chunks, and close records before they cross USB. Noise establishes the session; per-file ChaCha20-Poly1305 protects records. The Pico normally relays ciphertext and does not receive the session master or file keys.

Passive recording, including recording the plaintext bootstrap secret, does not reveal the established session's file contents: Noise also uses ephemeral Diffie-Hellman key agreement. Record authentication, ordering checks, and the final streamed hash detect corruption or alteration by a party without the session keys. These properties do not authenticate the file's origin or make a hostile sender's data safe.

| Location | Visibility and limitation |
| --- | --- |
| Source PC | The source file is plaintext and the agent holds session/file keys. Encryption cannot hide these from the PC controlling the process. |
| USB link and normal Pico relay | File-transfer records are ciphertext. Timing, lengths, identifiers, chunk size/count, and unencrypted control protocols remain observable. |
| Trusted receiving browser | The browser decrypts chunks, stages plaintext in IndexedDB, and saves a local download. Connection through the Pico does not grant the source PC access to that storage. |

The browser-generated session master is shared with the host agent. It is not a browser-only secret. Do not reuse it to protect information that must remain confidential from the source PC.

## Accepted limitations

- Unattended negotiation has no independent endpoint/client authentication. Active interception during negotiation is outside the confidentiality guarantee.
- Filesystem browsing, shell commands/results, credential requests/responses, diagnostics, and transfer flow control remain plaintext. Information sent to a hostile PC is exposed to that PC even if a future protocol encrypts the link.
- File size can be estimated from ciphertext lengths and chunk counts; activity and agent execution are not concealed. A PC controlling its OS can inspect processes, USB traffic, and agent memory.
- A sender-supplied hash proves consistency with what that sender supplied, not provenance. Authenticity requires an expected digest or signature obtained independently through a trusted channel.
- Encryption does not prevent malicious file contents. Saving a file is distinct from opening or executing it on the trusted device.
- Browser staging/downloads are plaintext at rest. Compromise of the Pico firmware, frontend, Wi-Fi environment, or receiving device invalidates the corresponding trust assumption.
- Availability is not guaranteed against a hostile PC that drops traffic, resets the connection, or removes power.

## Receiver defenses and review priorities

The current design uses bounded firmware frames/queues and relay states, strict transfer codecs, browser record authentication and order/length/hash checks, and USB-to-WebSocket backpressure. The download UI renders filenames as text and uses a save picker or an `application/octet-stream` Blob instead of executing received content. These controls are useful containment measures, not a completed audit against a malicious sender.

Future security work should focus on the trusted receivers:

1. Fuzz USB and browser decoders/state machines; exercise malformed lengths, stale identities, duplicates, flooding, and disconnect/reconnect behavior. Invalid input must not panic, bypass admission, or contaminate another session.
2. Review limits across the entire path: file sizes, transfer history/count, chunk bookkeeping, browser persistence queues, storage quotas, and timeouts. A bounded USB frame alone does not bound cumulative browser memory or storage.
3. Validate host-provided filenames and other display metadata, including control characters and misleading Unicode. Preserve explicit user action for downloads and avoid automatic content preview/execution.
4. Audit all USB-accessible firmware replacement, boot/reboot, and debug paths. Evaluate signed boot and a trusted recovery/provisioning process. Trusted firmware installation and continued integrity are assumptions here, not guarantees established by transfer encryption.
5. Keep host responses as data. A host status/result must not grant authority to change trusted settings, run browser code, or disclose unrelated browser data.

These are review priorities, not claims that every listed defense is implemented or validated. Host-agent hardening can improve normal behavior but cannot enforce this boundary when the PC can replace the agent. Parser, browser, and hardware validation must be reported separately; a host unit test is not proof of physical USB isolation.


## Read-only BLE prototype

Historical v1 behavior (superseded by the current BLE v3 boundary above):
the optional `ble` firmware feature advertises a public read-only GATT service.
Anyone in radio range may connect and read a short firmware build label, device
uptime, USB enabled/HID-ready flags and recent host-agent presence. It exposes no
hostname, filesystem, credentials, logs, typed text or host/device control.
One connection can occupy the service and deny other BLE observers; WLAN remains
available. Random addresses and local names are discovery hints, not identity or
authorization. Pairing, bonding and encrypted/authenticated links are not claimed
for this prototype. Control stays absent until shared firmware session ownership
and physically confirmed authenticated pairing are implemented and tested.

The native client validates exact versioned GATT values, bounds application-owned
discovery and queues, scopes work to a connection generation and terminates on
status failures. OS BLE libraries own their internal discovery caches. Read-only
BLE success does not establish the security of a later command transport.

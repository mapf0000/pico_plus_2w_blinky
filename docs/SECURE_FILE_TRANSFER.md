# Secure file-transfer design and implementation plan

This document records the security scope, design decisions, implementation phases, and acceptance criteria for encrypted host-to-browser file transfer. The exact wire layouts are in [PROTOCOL.md](PROTOCOL.md).

## Scope

Protected in this phase:

- Browser requests that start a file transfer or update its default path.
- The selected host path while it crosses the browser, firmware relay, and USB CDC link.
- File name, exact size, contents, and final SHA-256 from the host-agent process to the receiving frontend.
- End-to-end proof to the host that the connected browser decrypted, ordered, and hashed the complete file.

Explicitly out of scope:

- Filesystem browsing, command execution, credentials, device logs, and other TLV commands.
- Traffic-analysis resistance. An observer can see timing, ciphertext lengths, session/transfer identifiers, chunk size, and chunk count, and can estimate file size to within one chunk.
- Protection from a compromised host agent, frontend bundle, browser, or host OS kernel.
- Browser-side encryption at rest. The frontend currently stages decrypted chunks in IndexedDB before download; abandoned staging is cleared on the next page/database initialization.
- Durable keys, authenticated client identity, recovery of a session after refresh/reconnect, and multi-browser broadcast.

Filesystem browsing is especially important: its current plaintext pages can reveal names and paths before a transfer starts. Users requiring path confidentiality must avoid the browser filesystem view until that separate protocol is encrypted.

## Threat model and trust assumptions

After unattended negotiation completes, modification or replay of file records fails AEAD verification or ordering/final-hash checks. A passive observer that did not participate in negotiation cannot decrypt the established session. Dropping, delaying, forged ACKs, and disconnects can still deny service; encryption cannot provide availability.

The user trusts the host-agent binary, Pico firmware, and frontend code executing in the browser. The Pico receives the ephemeral bootstrap secret and normally serves the frontend bundle, but it does not receive the session master or per-file keys.

There is no out-of-band authentication channel. Every client that can access the Pico Web UI can request a bootstrap secret, establish a session, replace an existing session, and request files from the host. An active firmware/USB relay can also impersonate either endpoint during negotiation. This authorization exposure is an explicit tradeoff of unattended operation.

## Design decisions

### Session establishment

- Use the established Noise framework via `snow`, not a custom handshake.
- Pattern: `Noise_NNpsk0_25519_ChaChaPoly_SHA256`.
- Host produces a 128-bit random, single-use bootstrap secret and a random 128-bit session ID. Negotiation attempts expire after five minutes and requests are rate-limited.
- Host returns the bootstrap secret through the Pico relay; the browser immediately starts the handshake without user input.
- Browser and host derive a 256-bit PSK from the decoded bootstrap secret, session ID, and a protocol-domain string.
- Browser creates the 256-bit session master with the browser cryptographic random source after Noise authentication and transports it inside Noise.
- Host returns an encrypted fixed confirmation before either side marks the session established.
- No session master or file key is flashed, written to configuration, included in structured logs, or sent to firmware. The ephemeral bootstrap secret necessarily passes through firmware.

This meets the request for frontend-generated session keys and unattended operation. It deliberately does not authenticate the browser or relay; an active man-in-the-middle can establish its own session.

### Per-file isolation

Per-file isolation is retained because its cost is small: one HKDF-SHA-256 expansion and one random public salt per file. One ChaCha20-Poly1305 key is used for all records of that file, with record-type/index nonce domains. This avoids a complex four-key hierarchy while ensuring that a key/nonce mistake or future file-specific exposure does not directly reuse the session master as a data-encryption key for every file.

Per-file derivation is defense in depth, not a substitute for unique nonces and authenticated metadata. Removing it would simplify only a few lines and would enlarge the blast radius of session-key misuse, so it is appropriate here.

### Streaming rather than whole-file buffering

- Open one regular-file handle and use its metadata as the initial length.
- Read once in bounded 2002-byte plaintext chunks.
- Update SHA-256, encrypt immediately, and best-effort zeroize the plaintext buffer.
- Keep only ciphertext frames in the bounded retry window (at most 64 frames, normally 8).
- Retransmit the identical ciphertext after timeout; never invoke AEAD twice for the same record nonce.
- Recheck bytes read and file-handle length before close.
- Do not log the path, file name, content, digest, bootstrap secret, session master, file key, or session PSK.

The implementation does not automatically ZIP/compress. Compression is not required for confidentiality, can amplify CPU/memory use, often provides no gain for already compressed files, and leaks content-dependent compressed length. If compression is added later, it should be an explicit streaming option inside the authenticated manifest with strict decompression size/ratio limits in the browser.

### End-to-end completion

Firmware ACK means only that an opaque ciphertext chunk passed structural/order checks and entered the bounded WebSocket queue. It does not mean the browser authenticated or persisted it. Firmware `FILE_RESULT(OK)` is therefore necessary for relay health but insufficient for transfer success.

After the authenticated close, the browser verifies exact count, exact size, and rolling SHA-256, then returns `transfer_id || sha256` inside the Noise transport. The host succeeds only after it receives both that receipt and firmware `FILE_RESULT(OK)`.

## Implementation phases

1. **Shared, versioned protocol — implemented**
   - Add a `no_std` format crate containing v2 constants, exact bounded encoders/decoders, binary-kind allocation, and maximum-size assertions.
   - Reserve USB tags 32/33 and WebSocket kinds 3–6 without reinterpreting legacy values.
   - Reject truncation, invalid versions/fields/lengths, and trailing bytes.

2. **Cryptographic core — implemented**
   - Add bootstrap-secret derivation, Noise initiator/responder wrappers, session-master generation, HKDF per-file derivation, ChaCha20-Poly1305 record protection, and manifest/close codecs.
   - Domain-separate keys, AAD, nonces, and session-bootstrap context.
   - Use OS/Web Crypto randomness through supported `getrandom` backends.
   - Zeroize secrets/plaintext on a best-effort basis and avoid secret-bearing error text.

3. **Host-agent sender — implemented**
   - Add single-use unattended negotiation state and encrypted controls.
   - Disable plaintext start/default tags.
   - Replace pre-hash/whole-file behavior with one-pass bounded read/hash/encrypt.
   - Cache exact ciphertext for retries, validate ACK bounds/offset/monotonicity, and require an authenticated receipt.
   - Keep queues, retries, and timeouts bounded and preserve serial reconnect behavior.

4. **Firmware opaque relay — implemented**
   - Keep firmware `no_std`; add no cryptographic dependency or secret storage.
   - Validate only v2 public envelope structure, active session/transfer identity, strict chunk ordering, and bounded ciphertext lengths.
   - Await browser queue capacity before ACK to preserve backpressure.
   - Forward open/chunk/close bytes unchanged and relay negotiation/session envelopes.
   - Disable plaintext WebSocket transfer RPCs and simulation/drop mode for authenticated transfers.

5. **Frontend receiver and UX — implemented**
   - Automatically request a session when a compatible host appears and expose connection/retry state without a code input.
   - Generate the session master in WebAssembly, establish Noise, and clear session state on disconnect.
   - Gate all transfer buttons on a compatible host plus an established encrypted session.
   - Decrypt/authenticate manifest, ordered chunks, and close before staging plaintext.
   - Reject plaintext transfer-open/chunk paths and send the authenticated final receipt.

6. **Compatibility and operations — implemented**
   - Advertise transfer protocol v2 in `HELLO` and require exact frontend compatibility.
   - Keep legacy tag numbers reserved but fail closed instead of silently downgrading.
   - Update the device transfer page to describe unattended session negotiation.
   - Document observable metadata and the plaintext filesystem/browser-storage limitations.

7. **Validation and security review — required for release**
   - Run formatting, shared crypto/protocol tests, host unit/e2e tests, host Clippy with warnings denied, frontend wasm compile, Trunk release build, firmware release check, and final diff/status review.
   - Test malformed/trailing/oversized frames, malformed bootstrap secrets, AEAD tampering, record-domain separation, out-of-order/duplicate chunks, exact maximum chunks, empty files, source length changes, ACK regression/future offsets, retry ciphertext identity, disconnect/reconnect, and receipt mismatch/timeouts.
   - Perform a hardware/browser smoke test when a board and browser/WebDriver are available; do not treat compile-only validation as a physical USB security test.

## Operational flow

1. Open the Web UI. When a compatible host is present, the browser requests a session automatically.
2. The host returns an ephemeral bootstrap secret and the browser immediately completes the Noise handshake.
3. Wait for “Encrypted session ready.”
4. Enter or select a file and queue the transfer. The transfer path is sent only inside Noise.
   Alternatively, queue the host default set by `--send-file` or the encrypted “Set as default” action; the default path never crosses the link when started.
5. The host reads, hashes, encrypts, and sends the file as v2 records. Firmware relays ciphertext with backpressure.
6. The frontend decrypts and verifies the file, stages it for download, and returns the authenticated receipt.
7. The browser negotiates a new session automatically after a refresh, WebSocket reconnect, or host-agent restart.

## Residual risks and follow-up work

- File lengths/timing remain visible. Padding would reduce leakage but adds bandwidth and a padding policy.
- Unattended mode authorizes every Pico Web UI client to request host files. Restoring client authentication requires a pre-provisioned key or another independent trust channel.
- Frontend code is delivered over the device's HTTP origin. Protecting against malicious firmware requires an independently authenticated frontend distribution model.
- Decrypted browser chunks are stored in IndexedDB. A later at-rest phase can wrap staged chunks with a browser-only ephemeral key, but download assembly must still expose plaintext to the browser.
- Best-effort zeroization cannot erase OS page cache, kernel/driver buffers, swap, browser copies, allocator remnants, or JavaScript engine copies. Avoid core dumps and swap, lock down log access, and use OS hardening where that threat matters.
- Filesystem browsing and other commands remain plaintext by explicit scope. They require separate versioned security work and must not reuse record nonces or reinterpret these file-transfer controls.
- Independent cryptographic review, dependency monitoring, fuzzing, and hardware fault/disconnect testing are appropriate before high-assurance deployment.

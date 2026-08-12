# Transfer throughput experiments

This document records the USB and transfer-path throughput work performed on the Pimoroni Pico Plus 2 W. It is intended to prevent repeating disproven optimizations and to keep future measurements comparable.

## Test setup

- Device: RP2350B-based Pimoroni Pico Plus 2 W
- USB: full-speed composite device with logger CDC, control CDC, HID, and MSC interfaces
- Host used for the recorded measurements: macOS, direct CDC access through `serialport`
- Firmware USB stack: `embassy-rp 0.10.0` and `embassy-usb 0.6.0`
- Payload per variant: 4 MiB of deterministic synthetic data
- Wi-Fi, browser storage, HID reports, MSC writes, and host-file reads were not used
- Reported device rates start at the first data packet and stop after the final data packet
- Every hardware run ended with a normal TLV health probe

Run the reproducible USB diagnostics with:

```sh
scripts/device-test --usb-throughput-benchmark --benchmark-mib 4 --skip-host-agent --skip-msc
scripts/device-test --usb-raw-throughput-benchmark --benchmark-mib 4 --skip-host-agent --skip-msc
```

## Results

### Framed TLV ingress and flow control

The production-shaped benchmark sends maximum-size 2,048-byte TLV payloads. Each data payload carries an 8-byte benchmark header, leaving 2,040 synthetic data bytes. Firmware performs normal streaming TLV decoding plus a wrapping byte checksum.

| ACK cadence | Window | Host KiB/s | Device KiB/s |
|---:|---:|---:|---:|
| 1 | 8 | 490.4 | 490.7 |
| 4 | 16 | 490.3 | 490.5 |
| 8 | 16 | 492.5 | 492.8 |
| 16 | 32 | 492.6 | 492.9 |

The best result is only 0.45% faster than ACK-every-frame/window-8. ACK cadence and window credit are not the current bottleneck.

An earlier run of the same byte-wise decoder produced device rates of 490.9, 492.0, 491.5, and 493.6 KiB/s. The repeated result shows that the approximately 491–494 KiB/s range is stable.

### Bulk slice-copy TLV decoder

The firmware decoder was changed from its byte-wise state machine to bulk slice copying while preserving fragmented frames, coalesced frames, the 2,048-byte bound, and sliding oversized-header recovery. Two complete hardware runs produced these device rates:

| Run | ACK 1 / W8 | ACK 4 / W16 | ACK 8 / W16 | ACK 16 / W32 |
|---:|---:|---:|---:|---:|
| 1 | 481.7 | 481.3 | 481.8 | 481.5 |
| 2 | 481.7 | 482.2 | 481.6 | 481.2 |

The bulk decoder averaged 481.6 KiB/s, about 2.1% below the restored byte-wise implementation. The likely reason is that the firmware still receives only one 64-byte USB packet at a time, so bulk-copy bookkeeping has no sufficiently large slice over which to amortize itself. The change was reverted.

### Raw CDC OUT ceiling and host write size

The raw diagnostic switches the control OUT endpoint into a bounded count-only mode after a TLV start handshake. Firmware then receives exactly 4 MiB without TLV decoding, per-byte checksum work, retention, or forwarding. It counts USB packets and returns to TLV mode on completion, overflow, or a five-second idle timeout.

| Host write size | Host KiB/s | Device KiB/s | Packets | Full packets | Short packets |
|---:|---:|---:|---:|---:|---:|
| 64 B | 462.3 | 462.3 | 65,536 | 65,536 | 0 |
| 512 B | 499.7 | 499.7 | 65,550 | 65,522 | 28 |
| 2,048 B | 499.9 | 499.9 | 65,552 | 65,520 | 32 |
| 16,384 B | 499.7 | 499.7 | 65,550 | 65,523 | 27 |

The raw ceiling is about 499.9 KiB/s. It is only 1.4% above the best framed result, so TLV parsing, frame dispatch, and the benchmark checksum together account for very little of the current limit.

Writing one host syscall per 64-byte USB packet is about 7.5% slower. At 512 bytes and above, write size has no measurable effect. The production host currently writes one approximately 2 KiB TLV frame at a time, which is already in the saturated range; concatenating more frames into 16 KiB serial writes is therefore unlikely to improve this CDC implementation.

The non-64-byte packet counts in the larger-write variants show that the macOS CDC stack sometimes terminates packets short at internal boundaries. This did not reduce aggregate throughput.

## Current conclusion

The remaining USB limit is below the TLV and flow-control layers. The strongest current candidate is the single-buffered RP USB endpoint path:

- `embassy-rp 0.10.0` allocates one 64-byte DPRAM buffer per non-isochronous endpoint.
- Its OUT implementation uses only buffer-control slot 0.
- After every packet, firmware copies from DPRAM and rewrites/re-enables that same buffer before the host can deliver another packet.
- The RP2350 USB registers expose double-buffered endpoint controls, but the pinned Embassy driver does not use them.

At 499.9 KiB/s, the device processes roughly 8,000 full 64-byte packets per second, or one packet every 125 microseconds. The single-buffer rearm gap and/or the macOS CDC/TTY stack is therefore a much more plausible ceiling than application framing. This remains a measured inference until a double-buffer or alternate-host comparison is run.

## Experiments not yet tried

Ordered by expected information value:

1. **Double-buffer the control bulk OUT endpoint.** Prototype support in a narrowly pinned `embassy-rp` fork, allocate the additional 64-byte DPRAM slot, handle buffer toggling and short packets, then rerun both matrices. This is the most direct test of the current leading hypothesis.
2. **Run the raw matrix from Linux and Windows.** A materially higher result would implicate macOS CDC/TTY behavior; an unchanged result would strengthen the device-driver diagnosis. This needs no firmware change.
3. **Benchmark the production Tokio transport.** Send the raw diagnostic through `tokio-serial`, then add a sink for production-sized encrypted chunks. This separates synchronous test-harness behavior from the host agent's file-read, encryption, allocation, channel, clone, and async-write costs.
4. **Build a minimal USB composite variant.** Temporarily omit logger CDC, HID, and MSC while retaining the control endpoint. Idle composite interfaces should cost little, but this A/B test will reveal any host scheduling or interrupt-polling penalty.
5. **Measure device-to-host asymmetry.** A raw bulk IN benchmark will show whether the same single-buffer ceiling applies in the opposite direction. It is diagnostic rather than directly useful for the current host-to-browser file flow.
6. **Stage the real relay path.** Measure USB-to-bounded-queue, bounded-queue-to-WebSocket, browser decrypt/hash, and IndexedDB persistence separately. The current raw benchmark deliberately stops before all of these stages.

## Throughput improvements still worth considering

### Recommended next implementation

Add double-buffer support for the dedicated control OUT endpoint and keep CDC for compatibility. It is the smallest architectural change that directly targets the measured ceiling. It should be treated as an experimental driver patch until it passes fragmentation, short-packet, disconnect, overflow, control-health, and full transfer tests.

### Long-term high-throughput architecture

If double-buffered CDC remains insufficient, add a vendor-specific bulk interface for file data while retaining CDC for control and logs. A host transport based on queued asynchronous USB transfers can avoid the serial/TTY layer and keep several transfers submitted at once. The tradeoffs are a new cross-platform host backend, USB permissions/driver packaging, interface discovery, and a protocol migration path.

### End-to-end optimizations

These should be measured only after locating the slowest post-USB stage:

- Batch multiple encrypted chunk envelopes into one USB application record so firmware dispatches and acknowledges less often. This will not reduce the number of 64-byte USB packets.
- Use a bounded PSRAM ring or ownership-transfer buffers to reduce firmware copies between the USB decoder and WebSocket writer.
- Increase WebSocket batch depth only if Wi-Fi frame overhead is measured as limiting. The current batch already carries up to eight approximately 2 KiB chunks and uses a 32 KiB TCP transmit buffer.
- Stream browser output directly to a writable file or OPFS where supported, avoiding the later IndexedDB readback pass. Keep IndexedDB as the compatibility path.
- Offer optional streaming compression for compressible inputs. This improves effective file throughput, not raw link throughput, and must retain bounded memory and authenticated metadata.

Increasing ACK windows, increasing serial writes beyond 512 bytes, replacing the decoder with bulk slice copying, or changing the nominal CDC baud rate are not promising next steps based on the recorded evidence.

## Diagnostic footprint and safety

Adding the raw benchmark state increased firmware `.bss` by 64 bytes. The measured image uses 362,816 bytes of `.bss`, 2,644 bytes of `.data`, and 1,024 bytes of `.uninit`; 157,792 bytes remain before runtime stack use.

Raw mode accepts only declared sizes from 1 through 64 MiB, begins only after an explicit diagnostic tag, retains no payload, and returns to TLV mode after completion, overflow, or five seconds without a packet. It does not use Wi-Fi or access host files.

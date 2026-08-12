use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail, ensure};
use clap::Parser;
use serialport::{
    ClearBuffer, DataBits, FlowControl, Parity, SerialPort, SerialPortInfo, SerialPortType,
    StopBits,
};
use transfer_protocol::{
    FILE_OPEN_HEADER_LEN, FILE_TAG_LEN, MANIFEST_PLAINTEXT_BASE_LEN, MAX_PLAINTEXT_CHUNK,
    TAG_USB_BENCHMARK_DATA, TAG_USB_BENCHMARK_FINISH, TAG_USB_BENCHMARK_RESULT,
    TAG_USB_BENCHMARK_START, TAG_USB_RAW_BENCHMARK_RESULT, TAG_USB_RAW_BENCHMARK_START,
    TLV_MAX_PAYLOAD, USB_BENCHMARK_DATA_HEADER_LEN, USB_BENCHMARK_RESULT_LEN,
    USB_BENCHMARK_STATUS_COMPLETE, USB_BENCHMARK_STATUS_PROGRESS, USB_BENCHMARK_STATUS_STARTED,
    USB_BENCHMARK_VERSION, USB_RAW_BENCHMARK_RESULT_LEN, USB_RAW_BENCHMARK_VERSION,
    WEBSOCKET_CLOSE_SESSION_REPLACED, WS_BINARY_KIND_SECURE_CHUNK,
    WS_BINARY_KIND_SECURE_CHUNK_BATCH, decode_secure_chunk, decode_secure_chunk_batch,
    encode_secure_chunk, encode_secure_open,
};

const BAUD_RATE: u32 = 115_200;
const TLV_HEADER_LEN: usize = 5;
const TAG_DEBUG_MSG: u8 = 2;
const TAG_REQUEST_AGENT_STATUS: u8 = 7;
const TAG_FILE_OPEN: u8 = 20;
const TAG_FILE_CHUNK: u8 = 21;
const TAG_FILE_ACK: u8 = 22;
const TAG_FILE_RESULT: u8 = 24;
const TAG_FILE_ABORT: u8 = 25;
const UNKNOWN_TAG: u8 = 0xfe;

const FILE_ABORT_REASON_UNKNOWN_TRANSFER: u8 = 4;
const FILE_ABORT_REASON_CAPACITY: u8 = 5;
const PROBE_OK: &[u8] = b"probe-ok";
const HANDSHAKE: &[u8] = b"handshake";
const HANDSHAKE_OK: &[u8] = b"handshake-ok";
const IO_POLL: Duration = Duration::from_millis(50);

#[derive(Debug, Parser)]
#[command(
    name = "device-test",
    about = "Safe, opt-in USB device tests for Pico file-transfer transport"
)]
struct Args {
    /// USB vendor ID. Decimal and 0x-prefixed values are accepted.
    #[arg(long, default_value = "0x1209", value_parser = parse_u16)]
    vid: u16,

    /// USB product ID. Decimal and 0x-prefixed values are accepted.
    #[arg(long, default_value = "0x0001", value_parser = parse_u16)]
    pid: u16,

    /// Test this serial port instead of probing matching USB CDC interfaces.
    #[arg(long)]
    port: Option<String>,

    /// How long to wait for the application USB device to enumerate.
    #[arg(long, default_value_t = 45)]
    wait_secs: u64,

    /// Per-response timeout for device protocol assertions.
    #[arg(long, default_value_t = 2500)]
    response_timeout_ms: u64,

    /// Print matching USB CDC metadata without opening a port or sending data.
    #[arg(long)]
    list: bool,

    /// Exercise WebSocket refresh handoff and encrypted chunk batching.
    #[arg(long)]
    websocket_batch_test: bool,

    /// Socket address used by --websocket-batch-test.
    #[arg(long, default_value = "192.168.4.1:81")]
    websocket_address: String,

    /// Measure framed USB CDC ingress without Wi-Fi or host-file access.
    #[arg(long)]
    usb_throughput_benchmark: bool,

    /// Measure raw USB CDC ingress across host write sizes without TLV decoding.
    #[arg(long)]
    usb_raw_throughput_benchmark: bool,

    /// Synthetic payload size per USB benchmark variant.
    #[arg(long, default_value_t = 4)]
    benchmark_mib: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Frame {
    tag: u8,
    payload: Vec<u8>,
}

#[derive(Default)]
struct FrameDecoder {
    bytes: Vec<u8>,
}

impl FrameDecoder {
    fn push(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    fn next(&mut self) -> Option<Frame> {
        loop {
            if self.bytes.len() < TLV_HEADER_LEN {
                return None;
            }

            let payload_len =
                u32::from_le_bytes([self.bytes[1], self.bytes[2], self.bytes[3], self.bytes[4]])
                    as usize;
            if payload_len > TLV_MAX_PAYLOAD {
                self.bytes.remove(0);
                continue;
            }

            let frame_len = TLV_HEADER_LEN + payload_len;
            if self.bytes.len() < frame_len {
                return None;
            }

            let tag = self.bytes[0];
            let payload = self.bytes[TLV_HEADER_LEN..frame_len].to_vec();
            self.bytes.drain(..frame_len);
            return Some(Frame { tag, payload });
        }
    }

    fn clear(&mut self) {
        self.bytes.clear();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProbeOutcome {
    Control,
    Noisy,
    Quiet,
}

struct DeviceRunner {
    port: Box<dyn SerialPort>,
    decoder: FrameDecoder,
    response_timeout: Duration,
    transfer_id_seed: u64,
    websocket_address: Option<String>,
    websocket_wait: Duration,
    benchmark_mib: Option<u32>,
    raw_benchmark_mib: Option<u32>,
    passed: usize,
    failures: Vec<(&'static str, String)>,
}

impl DeviceRunner {
    fn new(
        port: Box<dyn SerialPort>,
        response_timeout: Duration,
        websocket_address: Option<String>,
        websocket_wait: Duration,
        benchmark_mib: Option<u32>,
        raw_benchmark_mib: Option<u32>,
    ) -> Self {
        let transfer_id_seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64
            | (1_u64 << 63);
        Self {
            port,
            decoder: FrameDecoder::default(),
            response_timeout,
            transfer_id_seed,
            websocket_address,
            websocket_wait,
            benchmark_mib,
            raw_benchmark_mib,
            passed: 0,
            failures: Vec::new(),
        }
    }

    fn run(mut self) -> Result<()> {
        println!("[INFO] Opened the control CDC with exclusive access");
        println!("[INFO] Tests send only TLV control data and synthetic encrypted envelopes");

        self.case("baseline control probe", Self::test_probe);
        self.case("agent handshake response", Self::test_handshake);
        self.case("fragmented TLV frame", Self::test_fragmented_frame);
        self.case("coalesced TLV frames", Self::test_coalesced_frames);
        self.case("unknown tag isolation", Self::test_unknown_tag);
        self.case(
            "truncated encrypted FILE_OPEN rejection",
            Self::test_malformed_secure_open,
        );
        self.case(
            "legacy FILE_OPEN version rejection",
            Self::test_legacy_secure_open,
        );
        self.case(
            "unknown encrypted FILE_CHUNK rejection",
            Self::test_unknown_secure_chunk,
        );
        self.case(
            "encrypted FILE_OPEN rejection without browser",
            Self::test_secure_open_without_browser,
        );
        self.case(
            "oversized-header TLV resynchronization",
            Self::test_oversized_header_resync,
        );
        self.case("final control health probe", Self::test_probe);
        if self.benchmark_mib.is_some() {
            self.case(
                "USB framed-ingress throughput matrix",
                Self::test_usb_throughput,
            );
            self.case("post-benchmark control health probe", Self::test_probe);
        }
        if self.raw_benchmark_mib.is_some() {
            self.case(
                "USB raw-ingress host-write matrix",
                Self::test_usb_raw_throughput,
            );
            self.case("post-raw-benchmark control health probe", Self::test_probe);
        }
        if self.websocket_address.is_some() {
            self.case(
                "WebSocket refresh handoff and encrypted chunk batching",
                Self::test_websocket_batching,
            );
            self.case("post-WebSocket control health probe", Self::test_probe);
        }

        println!(
            "[SUMMARY] {} passed; {} failed",
            self.passed,
            self.failures.len()
        );
        if self.failures.is_empty() {
            return Ok(());
        }

        let names = self
            .failures
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>()
            .join(", ");
        bail!("device suite failed: {names}")
    }

    fn case(&mut self, name: &'static str, test: fn(&mut Self) -> Result<()>) {
        let started = Instant::now();
        match test(self) {
            Ok(()) => {
                self.passed += 1;
                println!("[PASS] {name} ({} ms)", started.elapsed().as_millis());
            }
            Err(error) => {
                println!("[FAIL] {name}: {error:#}");
                self.failures.push((name, format!("{error:#}")));
            }
        }
    }

    fn reset_input(&mut self) {
        self.decoder.clear();
        let _ = self.port.clear(ClearBuffer::Input);
    }

    fn send_frame(&mut self, tag: u8, payload: &[u8]) -> Result<()> {
        let frame = encode_frame(tag, payload)?;
        self.port
            .write_all(&frame)
            .with_context(|| format!("write TLV tag {tag}"))?;
        self.port.flush().context("flush serial output")
    }

    fn send_raw(&mut self, bytes: &[u8]) -> Result<()> {
        self.port
            .write_all(bytes)
            .context("write raw serial bytes")?;
        self.port.flush().context("flush raw serial bytes")
    }

    fn read_frame_until(&mut self, deadline: Instant) -> Result<Option<Frame>> {
        loop {
            if let Some(frame) = self.decoder.next() {
                return Ok(Some(frame));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            let _ = self.port.set_timeout(remaining.min(IO_POLL));
            let mut buf = [0_u8; 256];
            match self.port.read(&mut buf) {
                Ok(0) => {}
                Ok(count) => self.decoder.push(&buf[..count]),
                Err(error)
                    if matches!(
                        error.kind(),
                        ErrorKind::TimedOut | ErrorKind::WouldBlock | ErrorKind::Interrupted
                    ) => {}
                Err(error) => return Err(error).context("read control CDC"),
            }
        }
    }

    fn expect_exact(&mut self, tag: u8, payload: &[u8]) -> Result<()> {
        let deadline = Instant::now() + self.response_timeout;
        while let Some(frame) = self.read_frame_until(deadline)? {
            if frame.tag == tag && frame.payload == payload {
                return Ok(());
            }
        }
        bail!(
            "timed out waiting for tag {tag} with {}-byte expected payload",
            payload.len()
        )
    }

    fn expect_transfer_response(&mut self, transfer_id: u64) -> Result<Frame> {
        let deadline = Instant::now() + self.response_timeout;
        while let Some(frame) = self.read_frame_until(deadline)? {
            if !matches!(frame.tag, TAG_FILE_ACK | TAG_FILE_RESULT | TAG_FILE_ABORT) {
                continue;
            }
            if frame.payload.len() >= 8
                && u64::from_le_bytes(frame.payload[0..8].try_into().expect("length checked"))
                    == transfer_id
            {
                return Ok(frame);
            }
        }
        bail!("timed out waiting for transfer {transfer_id} response")
    }

    fn assert_no_transfer_response(&mut self, duration: Duration) -> Result<()> {
        let deadline = Instant::now() + duration;
        while let Some(frame) = self.read_frame_until(deadline)? {
            ensure!(
                !matches!(frame.tag, TAG_FILE_ACK | TAG_FILE_RESULT | TAG_FILE_ABORT),
                "unexpected transfer response tag {}",
                frame.tag
            );
        }
        Ok(())
    }

    fn test_probe(&mut self) -> Result<()> {
        self.reset_input();
        self.send_frame(TAG_REQUEST_AGENT_STATUS, &[])?;
        self.expect_exact(TAG_DEBUG_MSG, PROBE_OK)
    }

    fn test_handshake(&mut self) -> Result<()> {
        self.reset_input();
        self.send_frame(TAG_REQUEST_AGENT_STATUS, HANDSHAKE)?;
        self.expect_exact(TAG_DEBUG_MSG, HANDSHAKE_OK)
    }

    fn test_fragmented_frame(&mut self) -> Result<()> {
        self.reset_input();
        let frame = encode_frame(TAG_REQUEST_AGENT_STATUS, HANDSHAKE)?;
        for byte in frame {
            self.send_raw(&[byte])?;
            thread::sleep(Duration::from_millis(2));
        }
        self.expect_exact(TAG_DEBUG_MSG, HANDSHAKE_OK)
    }

    fn test_coalesced_frames(&mut self) -> Result<()> {
        self.reset_input();
        let mut frames = encode_frame(TAG_REQUEST_AGENT_STATUS, &[])?;
        frames.extend_from_slice(&encode_frame(TAG_REQUEST_AGENT_STATUS, HANDSHAKE)?);
        self.send_raw(&frames)?;
        self.expect_exact(TAG_DEBUG_MSG, PROBE_OK)?;
        self.expect_exact(TAG_DEBUG_MSG, HANDSHAKE_OK)
    }

    fn test_unknown_tag(&mut self) -> Result<()> {
        self.reset_input();
        self.send_frame(UNKNOWN_TAG, &[])?;
        self.assert_no_transfer_response(Duration::from_millis(150))?;
        self.send_frame(TAG_REQUEST_AGENT_STATUS, &[])?;
        self.expect_exact(TAG_DEBUG_MSG, PROBE_OK)
    }

    fn test_malformed_secure_open(&mut self) -> Result<()> {
        self.reset_input();
        self.send_frame(TAG_FILE_OPEN, &[2, 0])?;
        self.assert_no_transfer_response(Duration::from_millis(150))?;
        self.send_frame(TAG_REQUEST_AGENT_STATUS, &[])?;
        self.expect_exact(TAG_DEBUG_MSG, PROBE_OK)
    }

    fn test_legacy_secure_open(&mut self) -> Result<()> {
        self.reset_input();
        let mut payload = [0_u8; FILE_OPEN_HEADER_LEN];
        payload[0..2].copy_from_slice(&1_u16.to_le_bytes());
        self.send_frame(TAG_FILE_OPEN, &payload)?;
        self.assert_no_transfer_response(Duration::from_millis(150))?;
        self.send_frame(TAG_REQUEST_AGENT_STATUS, &[])?;
        self.expect_exact(TAG_DEBUG_MSG, PROBE_OK)
    }

    fn test_unknown_secure_chunk(&mut self) -> Result<()> {
        self.reset_input();
        let transfer_id = self.transfer_id_seed ^ 0x21;
        let mut payload = [0_u8; TLV_MAX_PAYLOAD];
        let payload_len = encode_secure_chunk(
            &mut payload,
            &[0x31; 16],
            transfer_id,
            0,
            &[0xa5; FILE_TAG_LEN],
        )
        .map_err(|error| anyhow!("encode synthetic FILE_CHUNK: {error:?}"))?;
        self.send_frame(TAG_FILE_CHUNK, &payload[..payload_len])?;

        let response = self.expect_transfer_response(transfer_id)?;
        ensure!(response.tag == TAG_FILE_ABORT, "expected FILE_ABORT tag");
        let abort = decode_abort(&response.payload)?;
        ensure!(
            abort.reason == FILE_ABORT_REASON_UNKNOWN_TRANSFER,
            "expected unknown-transfer reason {}, got {}",
            FILE_ABORT_REASON_UNKNOWN_TRANSFER,
            abort.reason
        );
        Ok(())
    }

    fn test_secure_open_without_browser(&mut self) -> Result<()> {
        self.reset_input();
        let transfer_id = self.transfer_id_seed ^ 0x20;
        let mut payload = [0_u8; TLV_MAX_PAYLOAD];
        let ciphertext = [0x5a; MANIFEST_PLAINTEXT_BASE_LEN + FILE_TAG_LEN];
        let payload_len = encode_secure_open(
            &mut payload,
            &[0x30; 16],
            transfer_id,
            &[0x40; 32],
            MAX_PLAINTEXT_CHUNK as u16,
            1,
            &ciphertext,
        )
        .map_err(|error| anyhow!("encode synthetic FILE_OPEN: {error:?}"))?;
        self.send_frame(TAG_FILE_OPEN, &payload[..payload_len])?;

        let response = self.expect_transfer_response(transfer_id)?;
        if response.tag == TAG_FILE_ACK {
            let cleanup = encode_abort(transfer_id, FILE_ABORT_REASON_CAPACITY, b"test cleanup")?;
            self.send_frame(TAG_FILE_ABORT, &cleanup)?;
            bail!("received FILE_ACK; a browser relay appears to be active")
        }
        ensure!(response.tag == TAG_FILE_ABORT, "expected FILE_ABORT tag");
        let abort = decode_abort(&response.payload)?;
        ensure!(
            abort.reason == FILE_ABORT_REASON_CAPACITY,
            "expected relay-unavailable reason {}, got {}",
            FILE_ABORT_REASON_CAPACITY,
            abort.reason
        );
        ensure!(
            abort.detail == "secure browser relay unavailable",
            "unexpected safe abort detail"
        );
        Ok(())
    }

    fn test_oversized_header_resync(&mut self) -> Result<()> {
        self.reset_input();

        // The firmware's sliding header decoder temporarily recognizes a valid
        // 2047-byte unknown frame while resynchronizing this stream. Supplying
        // that bounded payload lets it reach the following known frame without
        // depending on timing or USB packet boundaries.
        self.send_raw(&[0x99, 0xff, 0xff, 0xff, 0xff, 0x07, 0x00, 0x00])?;
        self.send_raw(&vec![0_u8; TLV_MAX_PAYLOAD - 1])?;
        self.send_frame(TAG_REQUEST_AGENT_STATUS, &[])?;
        self.expect_exact(TAG_DEBUG_MSG, PROBE_OK)
    }

    fn test_usb_throughput(&mut self) -> Result<()> {
        let mib = self
            .benchmark_mib
            .expect("benchmark test runs only when configured");
        ensure!(mib > 0 && mib <= 64, "benchmark size must be 1..=64 MiB");
        println!("[INFO] USB benchmark uses {mib} MiB of deterministic synthetic data per variant");
        for (ack_every, window) in [(1_u16, 8_u32), (4, 16), (8, 16), (16, 32)] {
            let measurement = self.run_usb_benchmark(mib, ack_every, window)?;
            println!(
                "[BENCH] ack_every={ack_every:>2} window={window:>2} host={:.1} KiB/s device={:.1} KiB/s frames={} bytes={} checksum={:#010x}",
                measurement.host_kib_per_second,
                measurement.device_kib_per_second,
                measurement.frames,
                measurement.bytes,
                measurement.checksum,
            );
        }
        Ok(())
    }

    fn run_usb_benchmark(
        &mut self,
        mib: u32,
        ack_every: u16,
        window: u32,
    ) -> Result<UsbBenchmarkMeasurement> {
        self.reset_input();
        let token = (self.transfer_id_seed as u32) ^ (u32::from(ack_every) << 16) ^ window;
        let mut start = Vec::with_capacity(8);
        start.extend_from_slice(&USB_BENCHMARK_VERSION.to_le_bytes());
        start.extend_from_slice(&token.to_le_bytes());
        start.extend_from_slice(&ack_every.to_le_bytes());
        self.send_frame(TAG_USB_BENCHMARK_START, &start)?;
        let started = self.expect_benchmark_result(token, Duration::from_secs(2))?;
        ensure!(
            started.status == USB_BENCHMARK_STATUS_STARTED,
            "benchmark start was rejected with status {}",
            started.status
        );

        let target_bytes = u64::from(mib) * 1024 * 1024;
        let data_capacity = TLV_MAX_PAYLOAD - USB_BENCHMARK_DATA_HEADER_LEN;
        let frame_count = target_bytes.div_ceil(data_capacity as u64) as u32;
        let mut sent_frames = 0_u32;
        let mut acknowledged_frames = 0_u32;
        let mut sent_bytes = 0_u64;
        let mut checksum = 0_u32;
        let started_at = Instant::now();

        while sent_frames < frame_count {
            while sent_frames < frame_count
                && sent_frames.saturating_sub(acknowledged_frames) < window
            {
                let remaining = target_bytes.saturating_sub(sent_bytes);
                let data_len = remaining.min(data_capacity as u64) as usize;
                let mut payload = vec![0_u8; USB_BENCHMARK_DATA_HEADER_LEN + data_len];
                payload[0..4].copy_from_slice(&token.to_le_bytes());
                payload[4..8].copy_from_slice(&sent_frames.to_le_bytes());
                for (offset, byte) in payload[USB_BENCHMARK_DATA_HEADER_LEN..]
                    .iter_mut()
                    .enumerate()
                {
                    *byte = (sent_frames as u8).wrapping_add(offset as u8);
                    checksum = checksum.wrapping_add(u32::from(*byte));
                }
                let frame = encode_frame(TAG_USB_BENCHMARK_DATA, &payload)?;
                self.port
                    .write_all(&frame)
                    .context("write USB benchmark data")?;
                sent_frames = sent_frames.saturating_add(1);
                sent_bytes = sent_bytes.saturating_add(data_len as u64);
            }

            if sent_frames < frame_count {
                let progress = self.expect_benchmark_result(token, Duration::from_secs(5))?;
                ensure!(
                    progress.status == USB_BENCHMARK_STATUS_PROGRESS,
                    "benchmark data was rejected with status {}",
                    progress.status
                );
                acknowledged_frames = progress.frames;
            }
        }

        let mut finish = Vec::with_capacity(8);
        finish.extend_from_slice(&token.to_le_bytes());
        finish.extend_from_slice(&frame_count.to_le_bytes());
        self.send_frame(TAG_USB_BENCHMARK_FINISH, &finish)?;
        let completed = loop {
            let result = self.expect_benchmark_result(token, Duration::from_secs(10))?;
            if result.status == USB_BENCHMARK_STATUS_COMPLETE {
                break result;
            }
            ensure!(
                result.status == USB_BENCHMARK_STATUS_PROGRESS,
                "benchmark finish failed with status {}",
                result.status
            );
        };
        let host_elapsed = started_at.elapsed();
        ensure!(
            completed.frames == frame_count,
            "firmware frame count mismatch"
        );
        ensure!(
            completed.bytes == target_bytes,
            "firmware byte count mismatch"
        );
        ensure!(completed.checksum == checksum, "firmware checksum mismatch");
        ensure!(
            completed.elapsed_us > 0,
            "firmware reported zero elapsed time"
        );

        Ok(UsbBenchmarkMeasurement {
            frames: frame_count,
            bytes: target_bytes,
            checksum,
            host_kib_per_second: target_bytes as f64 / 1024.0 / host_elapsed.as_secs_f64(),
            device_kib_per_second: target_bytes as f64
                / 1024.0
                / (completed.elapsed_us as f64 / 1_000_000.0),
        })
    }

    fn expect_benchmark_result(
        &mut self,
        token: u32,
        timeout: Duration,
    ) -> Result<UsbBenchmarkResult> {
        let deadline = Instant::now() + timeout;
        while let Some(frame) = self.read_frame_until(deadline)? {
            if frame.tag != TAG_USB_BENCHMARK_RESULT {
                continue;
            }
            let result = decode_usb_benchmark_result(&frame.payload)?;
            if result.token == token {
                return Ok(result);
            }
        }
        bail!("timed out waiting for USB benchmark result")
    }

    fn test_usb_raw_throughput(&mut self) -> Result<()> {
        let mib = self
            .raw_benchmark_mib
            .expect("raw benchmark test runs only when configured");
        ensure!(mib > 0 && mib <= 64, "benchmark size must be 1..=64 MiB");
        println!(
            "[INFO] Raw USB benchmark uses {mib} MiB per host write-size variant and bypasses TLV decoding"
        );
        for write_size in [64_usize, 512, 2048, 16 * 1024] {
            let measurement = self.run_usb_raw_benchmark(mib, write_size)?;
            println!(
                "[BENCH-RAW] write={write_size:>5} host={:.1} KiB/s device={:.1} KiB/s packets={} full={} short={}",
                measurement.host_kib_per_second,
                measurement.device_kib_per_second,
                measurement.packets,
                measurement.full_packets,
                measurement.short_packets,
            );
        }
        Ok(())
    }

    fn run_usb_raw_benchmark(
        &mut self,
        mib: u32,
        write_size: usize,
    ) -> Result<RawUsbBenchmarkMeasurement> {
        self.reset_input();
        let target_bytes = u64::from(mib) * 1024 * 1024;
        let token = (self.transfer_id_seed as u32)
            ^ 0x5241_5700
            ^ u32::try_from(write_size).context("raw benchmark write size conversion")?;
        let mut start = Vec::with_capacity(14);
        start.extend_from_slice(&USB_RAW_BENCHMARK_VERSION.to_le_bytes());
        start.extend_from_slice(&token.to_le_bytes());
        start.extend_from_slice(&target_bytes.to_le_bytes());
        self.send_frame(TAG_USB_RAW_BENCHMARK_START, &start)?;
        let started = self.expect_raw_benchmark_result(token, Duration::from_secs(2))?;
        ensure!(
            started.status == USB_BENCHMARK_STATUS_STARTED,
            "raw benchmark start was rejected with status {}",
            started.status
        );

        let buffer = vec![0xa5_u8; write_size];
        let started_at = Instant::now();
        let mut sent = 0_u64;
        while sent < target_bytes {
            let count = usize::try_from((target_bytes - sent).min(write_size as u64))
                .context("raw benchmark write length conversion")?;
            self.port
                .write_all(&buffer[..count])
                .with_context(|| format!("write {write_size}-byte raw USB benchmark block"))?;
            sent = sent.saturating_add(count as u64);
        }
        self.port.flush().context("flush raw USB benchmark data")?;

        let completed = self.expect_raw_benchmark_result(token, Duration::from_secs(10))?;
        let host_elapsed = started_at.elapsed();
        ensure!(
            completed.status == USB_BENCHMARK_STATUS_COMPLETE,
            "raw benchmark failed with status {}",
            completed.status
        );
        ensure!(
            completed.bytes == target_bytes,
            "raw benchmark firmware byte count mismatch"
        );
        ensure!(
            completed.packets == completed.full_packets + completed.short_packets,
            "raw benchmark packet accounting mismatch"
        );
        ensure!(
            completed.elapsed_us > 0,
            "firmware reported zero raw benchmark elapsed time"
        );

        Ok(RawUsbBenchmarkMeasurement {
            packets: completed.packets,
            full_packets: completed.full_packets,
            short_packets: completed.short_packets,
            host_kib_per_second: target_bytes as f64 / 1024.0 / host_elapsed.as_secs_f64(),
            device_kib_per_second: target_bytes as f64
                / 1024.0
                / (completed.elapsed_us as f64 / 1_000_000.0),
        })
    }

    fn expect_raw_benchmark_result(
        &mut self,
        token: u32,
        timeout: Duration,
    ) -> Result<RawUsbBenchmarkResult> {
        let deadline = Instant::now() + timeout;
        while let Some(frame) = self.read_frame_until(deadline)? {
            if frame.tag != TAG_USB_RAW_BENCHMARK_RESULT {
                continue;
            }
            let result = decode_raw_usb_benchmark_result(&frame.payload)?;
            if result.token == token {
                return Ok(result);
            }
        }
        bail!("timed out waiting for raw USB benchmark result")
    }

    fn test_websocket_batching(&mut self) -> Result<()> {
        let address = self
            .websocket_address
            .as_deref()
            .expect("test is called only when a WebSocket address is configured");
        let mut displaced = WebSocketProbe::connect(address, self.websocket_wait)?;
        for handoff in 1..=3 {
            let next = WebSocketProbe::connect(address, self.websocket_wait)
                .with_context(|| format!("open replacement WebSocket {handoff}"))?;
            displaced
                .expect_close_code(
                    Instant::now() + self.response_timeout.max(Duration::from_secs(5)),
                    WEBSOCKET_CLOSE_SESSION_REPLACED,
                )
                .with_context(|| {
                    format!("replacement WebSocket {handoff} did not displace old session")
                })?;
            displaced = next;
        }
        let mut websocket = displaced;

        self.reset_input();
        let transfer_id = self.transfer_id_seed ^ 0x81;
        let session_id = [0x51; 16];
        let mut payload = [0_u8; TLV_MAX_PAYLOAD];
        let manifest_ciphertext = [0x5a; MANIFEST_PLAINTEXT_BASE_LEN + FILE_TAG_LEN];
        let open_len = encode_secure_open(
            &mut payload,
            &session_id,
            transfer_id,
            &[0x61; 32],
            1,
            8,
            &manifest_ciphertext,
        )
        .map_err(|error| anyhow!("encode batch-test FILE_OPEN: {error:?}"))?;
        self.send_frame(TAG_FILE_OPEN, &payload[..open_len])?;
        ensure!(
            self.expect_transfer_response(transfer_id)?.tag == TAG_FILE_ACK,
            "batch-test FILE_OPEN was not acknowledged"
        );

        for chunk_index in 0..8 {
            let ciphertext = [chunk_index as u8; FILE_TAG_LEN + 1];
            let chunk_len = encode_secure_chunk(
                &mut payload,
                &session_id,
                transfer_id,
                chunk_index,
                &ciphertext,
            )
            .map_err(|error| anyhow!("encode batch-test FILE_CHUNK: {error:?}"))?;
            self.send_frame(TAG_FILE_CHUNK, &payload[..chunk_len])?;
        }
        for _ in 0..8 {
            ensure!(
                self.expect_transfer_response(transfer_id)?.tag == TAG_FILE_ACK,
                "batch-test FILE_CHUNK was not acknowledged"
            );
        }

        let deadline = Instant::now() + self.response_timeout.max(Duration::from_secs(5));
        let mut received_chunks = 0;
        let mut saw_batch = false;
        while received_chunks < 8 {
            let frame = websocket
                .next_frame(deadline)?
                .ok_or_else(|| anyhow!("timed out waiting for relayed WebSocket chunks"))?;
            if frame.opcode != 2 {
                continue;
            }
            let Some((&kind, body)) = frame.payload.split_first() else {
                continue;
            };
            match kind {
                WS_BINARY_KIND_SECURE_CHUNK => {
                    let chunk = decode_secure_chunk(body)
                        .map_err(|error| anyhow!("decode individual relayed chunk: {error:?}"))?;
                    if chunk.transfer_id == transfer_id {
                        received_chunks += 1;
                    }
                }
                WS_BINARY_KIND_SECURE_CHUNK_BATCH => {
                    let batch = decode_secure_chunk_batch(body)
                        .map_err(|error| anyhow!("decode relayed chunk batch: {error:?}"))?;
                    let mut matching_chunks = 0;
                    for encoded_chunk in batch.chunks() {
                        let chunk = decode_secure_chunk(encoded_chunk)
                            .map_err(|error| anyhow!("decode chunk in relayed batch: {error:?}"))?;
                        if chunk.transfer_id == transfer_id {
                            matching_chunks += 1;
                        }
                    }
                    if matching_chunks > 0 {
                        ensure!(
                            matching_chunks >= 2,
                            "batch contained fewer than two chunks"
                        );
                        received_chunks += matching_chunks;
                        saw_batch = true;
                    }
                }
                _ => {}
            }
        }
        ensure!(saw_batch, "firmware relayed all chunks without batching");
        ensure!(received_chunks == 8, "received duplicate relayed chunks");

        let cleanup = encode_abort(transfer_id, FILE_ABORT_REASON_CAPACITY, b"test cleanup")?;
        self.send_frame(TAG_FILE_ABORT, &cleanup)?;
        ensure!(
            self.expect_transfer_response(transfer_id)?.tag == TAG_FILE_RESULT,
            "batch-test cleanup did not complete"
        );
        Ok(())
    }
}

struct WebSocketFrame {
    opcode: u8,
    payload: Vec<u8>,
}

struct UsbBenchmarkMeasurement {
    frames: u32,
    bytes: u64,
    checksum: u32,
    host_kib_per_second: f64,
    device_kib_per_second: f64,
}

struct UsbBenchmarkResult {
    status: u8,
    token: u32,
    frames: u32,
    bytes: u64,
    elapsed_us: u64,
    checksum: u32,
}

struct RawUsbBenchmarkMeasurement {
    packets: u32,
    full_packets: u32,
    short_packets: u32,
    host_kib_per_second: f64,
    device_kib_per_second: f64,
}

struct RawUsbBenchmarkResult {
    status: u8,
    token: u32,
    bytes: u64,
    elapsed_us: u64,
    packets: u32,
    full_packets: u32,
    short_packets: u32,
}

struct WebSocketProbe {
    stream: TcpStream,
    bytes: Vec<u8>,
}

impl WebSocketProbe {
    fn connect(address: &str, wait: Duration) -> Result<Self> {
        let deadline = Instant::now() + wait;
        let mut last_error = None;
        let stream = loop {
            let socket_addresses = address
                .to_socket_addrs()
                .with_context(|| format!("resolve WebSocket address {address}"))?;
            let mut connected = None;
            for socket_address in socket_addresses {
                match TcpStream::connect_timeout(&socket_address, Duration::from_secs(1)) {
                    Ok(stream) => {
                        connected = Some(stream);
                        break;
                    }
                    Err(error) => last_error = Some(error),
                }
            }
            if let Some(stream) = connected {
                break stream;
            }
            if Instant::now() >= deadline {
                return Err(last_error
                    .map(anyhow::Error::from)
                    .unwrap_or_else(|| anyhow!("no socket addresses resolved for {address}")))
                .with_context(|| format!("connect to WebSocket server {address}"));
            }
            thread::sleep(Duration::from_millis(200));
        };

        stream
            .set_read_timeout(Some(IO_POLL))
            .context("set WebSocket read timeout")?;
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .context("set WebSocket write timeout")?;
        let mut probe = Self {
            stream,
            bytes: Vec::new(),
        };
        let request = format!(
            "GET /ws HTTP/1.1\r\nHost: {address}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        probe
            .stream
            .write_all(request.as_bytes())
            .context("write WebSocket upgrade request")?;
        probe.stream.flush().context("flush WebSocket upgrade")?;
        probe.read_upgrade_response(Instant::now() + Duration::from_secs(5))?;
        Ok(probe)
    }

    fn read_upgrade_response(&mut self, deadline: Instant) -> Result<()> {
        loop {
            if let Some(header_end) = self.bytes.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                let frame_start = header_end + 4;
                let headers = std::str::from_utf8(&self.bytes[..header_end])
                    .context("WebSocket upgrade response is not UTF-8")?;
                ensure!(
                    headers.starts_with("HTTP/1.1 101") || headers.starts_with("HTTP/1.0 101"),
                    "WebSocket upgrade failed: {headers}"
                );
                self.bytes.drain(..frame_start);
                return Ok(());
            }
            ensure!(
                self.bytes.len() <= 8192,
                "WebSocket upgrade headers exceed 8192 bytes"
            );
            ensure!(
                Instant::now() < deadline,
                "timed out waiting for WebSocket upgrade response"
            );
            self.read_more()?;
        }
    }

    fn next_frame(&mut self, deadline: Instant) -> Result<Option<WebSocketFrame>> {
        loop {
            if let Some(frame) = decode_websocket_frame(&mut self.bytes)? {
                return Ok(Some(frame));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            self.read_more()?;
        }
    }

    fn expect_close_code(&mut self, deadline: Instant, expected_code: u16) -> Result<()> {
        loop {
            let frame = self
                .next_frame(deadline)?
                .ok_or_else(|| anyhow!("timed out waiting for WebSocket close frame"))?;
            if frame.opcode != 8 {
                continue;
            }
            ensure!(
                frame.payload.len() >= 2,
                "WebSocket close frame omitted its status code"
            );
            let code = u16::from_be_bytes([frame.payload[0], frame.payload[1]]);
            ensure!(
                code == expected_code,
                "expected WebSocket close code {expected_code}, got {code}"
            );
            return Ok(());
        }
    }

    fn read_more(&mut self) -> Result<()> {
        let mut buffer = [0_u8; 4096];
        match self.stream.read(&mut buffer) {
            Ok(0) => bail!("WebSocket server closed the connection"),
            Ok(count) => self.bytes.extend_from_slice(&buffer[..count]),
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::TimedOut | ErrorKind::WouldBlock | ErrorKind::Interrupted
                ) => {}
            Err(error) => return Err(error).context("read WebSocket frame"),
        }
        Ok(())
    }
}

fn decode_websocket_frame(bytes: &mut Vec<u8>) -> Result<Option<WebSocketFrame>> {
    if bytes.len() < 2 {
        return Ok(None);
    }
    let opcode = bytes[0] & 0x0f;
    let masked = bytes[1] & 0x80 != 0;
    let mut at = 2;
    let payload_len = match bytes[1] & 0x7f {
        len @ 0..=125 => len as usize,
        126 => {
            if bytes.len() < at + 2 {
                return Ok(None);
            }
            let len = u16::from_be_bytes(bytes[at..at + 2].try_into().expect("length checked"));
            at += 2;
            len as usize
        }
        127 => {
            if bytes.len() < at + 8 {
                return Ok(None);
            }
            let len = u64::from_be_bytes(bytes[at..at + 8].try_into().expect("length checked"));
            at += 8;
            usize::try_from(len).context("WebSocket frame is too large for this host")?
        }
        _ => unreachable!("the seven-bit WebSocket length discriminator is at most 127"),
    };
    ensure!(
        payload_len <= 64 * 1024,
        "WebSocket frame exceeds test limit"
    );

    let mask = if masked {
        if bytes.len() < at + 4 {
            return Ok(None);
        }
        let mask: [u8; 4] = bytes[at..at + 4].try_into().expect("length checked");
        at += 4;
        Some(mask)
    } else {
        None
    };
    let frame_len = at
        .checked_add(payload_len)
        .context("WebSocket frame length overflow")?;
    if bytes.len() < frame_len {
        return Ok(None);
    }

    let mut payload = bytes[at..frame_len].to_vec();
    if let Some(mask) = mask {
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[index % mask.len()];
        }
    }
    bytes.drain(..frame_len);
    Ok(Some(WebSocketFrame { opcode, payload }))
}

#[derive(Debug, PartialEq, Eq)]
struct Abort<'a> {
    transfer_id: u64,
    reason: u8,
    detail: &'a str,
}

fn main() -> Result<()> {
    let args = Args::parse();
    ensure!(args.wait_secs > 0, "--wait-secs must be greater than zero");
    ensure!(
        args.response_timeout_ms > 0,
        "--response-timeout-ms must be greater than zero"
    );
    ensure!(
        !(args.usb_throughput_benchmark || args.usb_raw_throughput_benchmark)
            || (1..=64).contains(&args.benchmark_mib),
        "--benchmark-mib must be 1..=64"
    );

    let ports = wait_for_matching_ports(args.vid, args.pid, Duration::from_secs(args.wait_secs))?;
    if !args.list {
        ensure!(
            ports.len() >= 2,
            "expected both logger and control CDC interfaces; found {} matching port(s)",
            ports.len()
        );
    }
    print_port_inventory(args.vid, args.pid, &ports);
    if args.list {
        return Ok(());
    }

    let port_name = discover_control_port(
        &ports,
        args.port.as_deref(),
        Duration::from_millis(args.response_timeout_ms),
    )?;
    println!("[PASS] Discovered exactly one responsive control CDC: {port_name}");

    let port = open_port(&port_name)
        .with_context(|| format!("open control CDC {port_name} exclusively"))?;
    let websocket_address = args.websocket_batch_test.then_some(args.websocket_address);
    DeviceRunner::new(
        port,
        Duration::from_millis(args.response_timeout_ms),
        websocket_address,
        Duration::from_secs(args.wait_secs),
        args.usb_throughput_benchmark.then_some(args.benchmark_mib),
        args.usb_raw_throughput_benchmark
            .then_some(args.benchmark_mib),
    )
    .run()
}

fn parse_u16(raw: &str) -> std::result::Result<u16, String> {
    let raw = raw.trim();
    if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
        u16::from_str_radix(hex, 16).map_err(|error| error.to_string())
    } else {
        raw.parse::<u16>().map_err(|error| error.to_string())
    }
}

fn wait_for_matching_ports(vid: u16, pid: u16, timeout: Duration) -> Result<Vec<SerialPortInfo>> {
    let deadline = Instant::now() + timeout;
    loop {
        let ports = matching_ports(vid, pid).context("enumerate serial ports")?;
        if !ports.is_empty() {
            return Ok(prefer_callout_ports(ports));
        }
        if Instant::now() >= deadline {
            bail!("no USB CDC ports with VID {vid:#06x} PID {pid:#06x} enumerated");
        }
        thread::sleep(Duration::from_millis(200));
    }
}

fn matching_ports(vid: u16, pid: u16) -> serialport::Result<Vec<SerialPortInfo>> {
    let mut ports = serialport::available_ports()?
        .into_iter()
        .filter(|info| {
            matches!(
                &info.port_type,
                SerialPortType::UsbPort(usb) if usb.vid == vid && usb.pid == pid
            )
        })
        .collect::<Vec<_>>();
    ports.sort_by(|left, right| left.port_name.cmp(&right.port_name));
    Ok(ports)
}

fn prefer_callout_ports(ports: Vec<SerialPortInfo>) -> Vec<SerialPortInfo> {
    #[cfg(target_os = "macos")]
    {
        let callout = ports
            .iter()
            .filter(|port| port.port_name.starts_with("/dev/cu."))
            .cloned()
            .collect::<Vec<_>>();
        if !callout.is_empty() {
            return callout;
        }
    }
    ports
}

fn print_port_inventory(vid: u16, pid: u16, ports: &[SerialPortInfo]) {
    println!(
        "[PASS] Enumerated {} CDC port(s) for VID {vid:#06x} PID {pid:#06x}",
        ports.len()
    );
    for info in ports {
        let SerialPortType::UsbPort(usb) = &info.port_type else {
            continue;
        };
        println!(
            "[INFO] {} interface={:?} manufacturer={} product={} serial={}",
            info.port_name,
            usb.interface,
            presence(&usb.manufacturer),
            presence(&usb.product),
            if usb.serial_number.is_some() {
                "present"
            } else {
                "absent"
            }
        );
    }
}

fn presence(value: &Option<String>) -> &'static str {
    if value.is_some() { "present" } else { "absent" }
}

fn discover_control_port(
    ports: &[SerialPortInfo],
    requested: Option<&str>,
    timeout: Duration,
) -> Result<String> {
    let candidates = if let Some(requested) = requested {
        let port = ports
            .iter()
            .find(|port| port.port_name == requested)
            .ok_or_else(|| {
                anyhow!("requested port {requested} does not match the selected VID/PID")
            })?;
        vec![port]
    } else {
        ports.iter().collect::<Vec<_>>()
    };

    let mut controls = Vec::new();
    for info in candidates {
        match probe_port(&info.port_name, timeout) {
            Ok(ProbeOutcome::Control) => controls.push(info.port_name.clone()),
            Ok(ProbeOutcome::Noisy) => {
                println!(
                    "[INFO] {} classified as non-control/noisy CDC",
                    info.port_name
                );
            }
            Ok(ProbeOutcome::Quiet) => {
                println!(
                    "[INFO] {} classified as non-control/quiet CDC",
                    info.port_name
                );
            }
            Err(error) => {
                println!("[INFO] {} probe failed: {error:#}", info.port_name);
            }
        }
    }

    ensure!(
        controls.len() == 1,
        "expected exactly one responsive control CDC; found {}",
        controls.len()
    );
    Ok(controls.remove(0))
}

fn probe_port(name: &str, timeout: Duration) -> Result<ProbeOutcome> {
    let mut port = open_port(name).with_context(|| format!("open candidate {name}"))?;
    let _ = port.clear(ClearBuffer::Input);
    let frame = encode_frame(TAG_REQUEST_AGENT_STATUS, &[])?;
    port.write_all(&frame).context("write control probe")?;
    port.flush().context("flush control probe")?;

    let deadline = Instant::now() + timeout;
    let mut decoder = FrameDecoder::default();
    let mut saw_bytes = false;
    loop {
        while let Some(frame) = decoder.next() {
            if frame.tag == TAG_DEBUG_MSG && frame.payload == PROBE_OK {
                return Ok(ProbeOutcome::Control);
            }
        }
        if Instant::now() >= deadline {
            break;
        }

        let remaining = deadline.saturating_duration_since(Instant::now());
        let _ = port.set_timeout(remaining.min(IO_POLL));
        let mut buf = [0_u8; 256];
        match port.read(&mut buf) {
            Ok(0) => {}
            Ok(count) => {
                saw_bytes = true;
                decoder.push(&buf[..count]);
            }
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::TimedOut | ErrorKind::WouldBlock | ErrorKind::Interrupted
                ) => {}
            Err(error) => return Err(error).context("read candidate CDC"),
        }
    }

    Ok(if saw_bytes {
        ProbeOutcome::Noisy
    } else {
        ProbeOutcome::Quiet
    })
}

fn open_port(name: &str) -> serialport::Result<Box<dyn SerialPort>> {
    let builder = serialport::new(name, BAUD_RATE)
        .data_bits(DataBits::Eight)
        .flow_control(FlowControl::None)
        .parity(Parity::None)
        .stop_bits(StopBits::One)
        .timeout(IO_POLL)
        .dtr_on_open(false);
    #[cfg(unix)]
    let builder = builder.exclusive(true);
    let mut port = builder.open()?;
    let _ = port.write_data_terminal_ready(false);
    let _ = port.write_request_to_send(false);
    Ok(port)
}

fn encode_frame(tag: u8, payload: &[u8]) -> Result<Vec<u8>> {
    ensure!(
        payload.len() <= TLV_MAX_PAYLOAD,
        "TLV payload exceeds {TLV_MAX_PAYLOAD} bytes"
    );
    let payload_len = u32::try_from(payload.len()).context("TLV payload length conversion")?;
    let mut frame = Vec::with_capacity(TLV_HEADER_LEN + payload.len());
    frame.push(tag);
    frame.extend_from_slice(&payload_len.to_le_bytes());
    frame.extend_from_slice(payload);
    Ok(frame)
}

fn decode_abort(payload: &[u8]) -> Result<Abort<'_>> {
    ensure!(payload.len() >= 11, "truncated FILE_ABORT payload");
    let transfer_id = u64::from_le_bytes(payload[0..8].try_into().expect("length checked"));
    let reason = payload[8];
    let detail_len =
        u16::from_le_bytes(payload[9..11].try_into().expect("length checked")) as usize;
    ensure!(
        payload.len() == 11 + detail_len,
        "FILE_ABORT detail length mismatch"
    );
    let detail = std::str::from_utf8(&payload[11..]).context("FILE_ABORT detail is not UTF-8")?;
    Ok(Abort {
        transfer_id,
        reason,
        detail,
    })
}

fn decode_usb_benchmark_result(payload: &[u8]) -> Result<UsbBenchmarkResult> {
    ensure!(
        payload.len() == USB_BENCHMARK_RESULT_LEN,
        "USB benchmark result length mismatch"
    );
    let version = u16::from_le_bytes(payload[0..2].try_into().expect("length checked"));
    ensure!(
        version == USB_BENCHMARK_VERSION,
        "unsupported USB benchmark result version {version}"
    );
    Ok(UsbBenchmarkResult {
        status: payload[2],
        token: u32::from_le_bytes(payload[3..7].try_into().expect("length checked")),
        frames: u32::from_le_bytes(payload[11..15].try_into().expect("length checked")),
        bytes: u64::from_le_bytes(payload[15..23].try_into().expect("length checked")),
        elapsed_us: u64::from_le_bytes(payload[23..31].try_into().expect("length checked")),
        checksum: u32::from_le_bytes(payload[31..35].try_into().expect("length checked")),
    })
}

fn decode_raw_usb_benchmark_result(payload: &[u8]) -> Result<RawUsbBenchmarkResult> {
    ensure!(
        payload.len() == USB_RAW_BENCHMARK_RESULT_LEN,
        "raw USB benchmark result length mismatch"
    );
    let version = u16::from_le_bytes(payload[0..2].try_into().expect("length checked"));
    ensure!(
        version == USB_RAW_BENCHMARK_VERSION,
        "unsupported raw USB benchmark result version {version}"
    );
    Ok(RawUsbBenchmarkResult {
        status: payload[2],
        token: u32::from_le_bytes(payload[3..7].try_into().expect("length checked")),
        bytes: u64::from_le_bytes(payload[7..15].try_into().expect("length checked")),
        elapsed_us: u64::from_le_bytes(payload[15..23].try_into().expect("length checked")),
        packets: u32::from_le_bytes(payload[23..27].try_into().expect("length checked")),
        full_packets: u32::from_le_bytes(payload[27..31].try_into().expect("length checked")),
        short_packets: u32::from_le_bytes(payload[31..35].try_into().expect("length checked")),
    })
}

fn encode_abort(transfer_id: u64, reason: u8, detail: &[u8]) -> Result<Vec<u8>> {
    let detail_len = u16::try_from(detail.len()).context("FILE_ABORT detail is too long")?;
    let mut payload = Vec::with_capacity(11 + detail.len());
    payload.extend_from_slice(&transfer_id.to_le_bytes());
    payload.push(reason);
    payload.extend_from_slice(&detail_len.to_le_bytes());
    payload.extend_from_slice(detail);
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_codec_handles_fragmentation_and_coalescing() {
        let first = encode_frame(TAG_REQUEST_AGENT_STATUS, &[]).unwrap();
        let second = encode_frame(TAG_DEBUG_MSG, PROBE_OK).unwrap();
        let split = second.len() / 2;
        let mut decoder = FrameDecoder::default();

        decoder.push(&first);
        decoder.push(&second[..split]);
        assert_eq!(
            decoder.next(),
            Some(Frame {
                tag: TAG_REQUEST_AGENT_STATUS,
                payload: Vec::new()
            })
        );
        assert_eq!(decoder.next(), None);

        decoder.push(&second[split..]);
        assert_eq!(
            decoder.next(),
            Some(Frame {
                tag: TAG_DEBUG_MSG,
                payload: PROBE_OK.to_vec()
            })
        );
        assert_eq!(decoder.next(), None);
    }

    #[test]
    fn frame_decoder_resynchronizes_after_oversized_length() {
        let mut bytes = vec![0x99, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
        bytes.extend_from_slice(&encode_frame(TAG_DEBUG_MSG, PROBE_OK).unwrap());
        let mut decoder = FrameDecoder::default();
        decoder.push(&bytes);

        let mut found = None;
        while let Some(frame) = decoder.next() {
            if frame.tag == TAG_DEBUG_MSG && frame.payload == PROBE_OK {
                found = Some(frame);
                break;
            }
        }
        assert!(found.is_some());
    }

    #[test]
    fn abort_codec_is_exact_and_rejects_trailing_data() {
        let encoded = encode_abort(42, FILE_ABORT_REASON_CAPACITY, b"unavailable").unwrap();
        assert_eq!(
            decode_abort(&encoded).unwrap(),
            Abort {
                transfer_id: 42,
                reason: FILE_ABORT_REASON_CAPACITY,
                detail: "unavailable"
            }
        );

        let mut trailing = encoded;
        trailing.push(0);
        assert!(decode_abort(&trailing).is_err());
    }

    #[test]
    fn numeric_usb_ids_accept_hex_and_decimal() {
        assert_eq!(parse_u16("0x1209").unwrap(), 0x1209);
        assert_eq!(parse_u16("4617").unwrap(), 0x1209);
        assert!(parse_u16("0x10000").is_err());
    }

    #[test]
    fn raw_usb_benchmark_result_decoder_is_exact() {
        let mut payload = [0_u8; USB_RAW_BENCHMARK_RESULT_LEN];
        payload[0..2].copy_from_slice(&USB_RAW_BENCHMARK_VERSION.to_le_bytes());
        payload[2] = USB_BENCHMARK_STATUS_COMPLETE;
        payload[3..7].copy_from_slice(&7_u32.to_le_bytes());
        payload[7..15].copy_from_slice(&4096_u64.to_le_bytes());
        payload[15..23].copy_from_slice(&8000_u64.to_le_bytes());
        payload[23..27].copy_from_slice(&64_u32.to_le_bytes());
        payload[27..31].copy_from_slice(&64_u32.to_le_bytes());

        let result = decode_raw_usb_benchmark_result(&payload).unwrap();
        assert_eq!(result.status, USB_BENCHMARK_STATUS_COMPLETE);
        assert_eq!(result.token, 7);
        assert_eq!(result.bytes, 4096);
        assert_eq!(result.elapsed_us, 8000);
        assert_eq!(result.packets, 64);
        assert_eq!(result.full_packets, 64);
        assert_eq!(result.short_packets, 0);

        assert!(decode_raw_usb_benchmark_result(&payload[..payload.len() - 1]).is_err());
    }

    #[test]
    fn websocket_decoder_handles_extended_binary_frames() {
        let payload = vec![0x5a; 512];
        let mut encoded = vec![0x82, 126];
        encoded.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        encoded.extend_from_slice(&payload);

        let frame = decode_websocket_frame(&mut encoded).unwrap().unwrap();
        assert_eq!(frame.opcode, 2);
        assert_eq!(frame.payload, payload);
        assert!(encoded.is_empty());
    }

    #[test]
    fn websocket_decoder_waits_for_complete_payload() {
        let mut encoded = vec![0x82, 3, 1, 2];
        assert!(decode_websocket_frame(&mut encoded).unwrap().is_none());
        encoded.push(3);
        assert_eq!(
            decode_websocket_frame(&mut encoded)
                .unwrap()
                .unwrap()
                .payload,
            vec![1, 2, 3]
        );
    }

    #[test]
    fn websocket_decoder_preserves_application_close_code() {
        let mut encoded = vec![0x88, 2];
        encoded.extend_from_slice(&WEBSOCKET_CLOSE_SESSION_REPLACED.to_be_bytes());

        let frame = decode_websocket_frame(&mut encoded).unwrap().unwrap();
        assert_eq!(frame.opcode, 8);
        assert_eq!(
            u16::from_be_bytes(frame.payload.try_into().unwrap()),
            WEBSOCKET_CLOSE_SESSION_REPLACED
        );
        assert!(encoded.is_empty());
    }
}

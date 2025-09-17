use core::sync::atomic::Ordering;

use embassy_usb::class::cdc_acm::CdcAcmClass as UsbCdcAcmClass;
use embassy_usb::driver::{Driver, EndpointError};
use heapless::String;

use crate::USB_ENABLED;
use crate::hid::{HID_CHAN, HidCommand, USB_READY};
use crate::script_dsl;

const OPCODE_RUN_DSL: u8 = 0x01;
const OPCODE_STATUS: u8 = 0x80;

const STATUS_OK: u8 = 0x00;
const STATUS_BUSY: u8 = 0x01;
const STATUS_BAD_OPCODE: u8 = 0x02;
const STATUS_BAD_LENGTH: u8 = 0x03;
const STATUS_BAD_CHECKSUM: u8 = 0x04;
const STATUS_INVALID_PAYLOAD: u8 = 0x05;
const STATUS_USB_NOT_READY: u8 = 0x06;

const HEADER_LEN: usize = 3;
const CHECKSUM_LEN: usize = 1;
const MAX_PAYLOAD: usize = 512;

/// Dedicated USB CDC command processor for host->device data transfers.
pub async fn run<'d, D>(mut class: UsbCdcAcmClass<'d, D>) -> !
where
    D: Driver<'d>,
{
    class.wait_connection().await;
    log::info!("usb_ctrl: host connection established");

    let mut decoder = FrameDecoder::new();
    let mut packet = [0u8; 64];

    loop {
        match class.read_packet(&mut packet).await {
            Ok(n) => {
                for &byte in &packet[..n] {
                    match decoder.feed(byte) {
                        DecoderAction::None => {}
                        DecoderAction::LengthExceeded => {
                            log::warn!("usb_ctrl: payload length exceeded");
                            let _ = send_status(&mut class, STATUS_BAD_LENGTH).await;
                        }
                        DecoderAction::ChecksumMismatch => {
                            log::warn!("usb_ctrl: checksum mismatch");
                            let _ = send_status(&mut class, STATUS_BAD_CHECKSUM).await;
                        }
                        DecoderAction::FrameReady { opcode, payload } => {
                            {
                                let status = handle_opcode(payload, opcode).await;
                                let _ = send_status(&mut class, status).await;
                            }
                            // Now that `payload` is out of scope, it's safe to reset the decoder.
                            decoder.reset();
                        }
                    }
                }
            }
            Err(e) => {
                log::warn!("usb_ctrl: read error: {:?}", e);
            }
        }
    }
}

async fn handle_opcode(payload: &[u8], opcode: u8) -> u8 {
    match opcode {
        OPCODE_RUN_DSL => handle_run_dsl(payload).await,
        _ => STATUS_BAD_OPCODE,
    }
}

async fn handle_run_dsl(payload: &[u8]) -> u8 {
    if payload.len() > MAX_PAYLOAD {
        return STATUS_BAD_LENGTH;
    }
    if !USB_ENABLED.load(Ordering::SeqCst) || !USB_READY.load(Ordering::SeqCst) {
        return STATUS_USB_NOT_READY;
    }
    if !payload.iter().all(|b| matches!(b, 9 | 10 | 13 | 32..=126)) {
        return STATUS_INVALID_PAYLOAD;
    }
    let mut body: String<MAX_PAYLOAD> = String::new();
    for &ch in payload {
        if body.push(ch as char).is_err() {
            return STATUS_BAD_LENGTH;
        }
    }
    if let Err(e) = script_dsl::compile_dsl(&body) {
        log::warn!("usb_ctrl: compile error: {:?}", e);
        return STATUS_INVALID_PAYLOAD;
    }
    match HID_CHAN.try_send(HidCommand::RunDsl { dsl: body }) {
        Ok(()) => STATUS_OK,
        Err(_) => STATUS_BUSY,
    }
}

async fn send_status<'d, D>(
    class: &mut UsbCdcAcmClass<'d, D>,
    status: u8,
) -> Result<(), EndpointError>
where
    D: Driver<'d>,
{
    let mut frame = [0u8; HEADER_LEN + 1 + CHECKSUM_LEN];
    frame[0] = OPCODE_STATUS;
    frame[1] = 0;
    frame[2] = 1;
    frame[HEADER_LEN] = status;
    frame[HEADER_LEN + CHECKSUM_LEN] = checksum(frame[0], 1, &frame[HEADER_LEN..HEADER_LEN + 1]);

    class
        .write_packet(&frame[..HEADER_LEN + 1 + CHECKSUM_LEN])
        .await
}

fn checksum(opcode: u8, len: u16, payload: &[u8]) -> u8 {
    let mut sum = opcode
        .wrapping_add((len >> 8) as u8)
        .wrapping_add(len as u8);
    for &b in payload {
        sum = sum.wrapping_add(b);
    }
    sum
}

// Removed unused helper `verify_checksum` (checksum is validated inline).

struct FrameDecoder {
    state: DecoderState,
    header: [u8; HEADER_LEN],
    header_len: usize,
    payload: [u8; MAX_PAYLOAD],
    payload_len: usize,
    expected_len: usize,
    opcode: u8,
    checksum: u8,
    drop_remaining: usize,
}

enum DecoderState {
    Header,
    Payload,
    Checksum,
    Drop,
}

enum DecoderAction<'a> {
    None,
    LengthExceeded,
    ChecksumMismatch,
    FrameReady { opcode: u8, payload: &'a [u8] },
}

impl FrameDecoder {
    const fn new() -> Self {
        Self {
            state: DecoderState::Header,
            header: [0; HEADER_LEN],
            header_len: 0,
            payload: [0; MAX_PAYLOAD],
            payload_len: 0,
            expected_len: 0,
            opcode: 0,
            checksum: 0,
            drop_remaining: 0,
        }
    }

    fn reset(&mut self) {
        self.state = DecoderState::Header;
        self.header_len = 0;
        self.payload_len = 0;
        self.expected_len = 0;
        self.opcode = 0;
        self.checksum = 0;
        self.drop_remaining = 0;
    }

    fn feed(&mut self, byte: u8) -> DecoderAction<'_> {
        match self.state {
            DecoderState::Header => {
                self.header[self.header_len] = byte;
                self.header_len += 1;
                if self.header_len == HEADER_LEN {
                    self.opcode = self.header[0];
                    self.expected_len =
                        u16::from_be_bytes([self.header[1], self.header[2]]) as usize;
                    self.checksum = self
                        .opcode
                        .wrapping_add(self.header[1])
                        .wrapping_add(self.header[2]);
                    self.header_len = 0;
                    if self.expected_len > MAX_PAYLOAD {
                        // Drop payload + checksum
                        self.state = DecoderState::Drop;
                        self.drop_remaining = self.expected_len + CHECKSUM_LEN;
                        DecoderAction::LengthExceeded
                    } else if self.expected_len == 0 {
                        self.state = DecoderState::Checksum;
                        DecoderAction::None
                    } else {
                        self.state = DecoderState::Payload;
                        DecoderAction::None
                    }
                } else {
                    DecoderAction::None
                }
            }
            DecoderState::Payload => {
                if self.payload_len < self.payload.len() {
                    self.payload[self.payload_len] = byte;
                }
                self.payload_len += 1;
                self.checksum = self.checksum.wrapping_add(byte);
                if self.payload_len == self.expected_len {
                    self.state = DecoderState::Checksum;
                }
                DecoderAction::None
            }
            DecoderState::Checksum => {
                // At this point, `self.checksum` already equals
                // checksum(self.opcode, expected_len, &self.payload[..expected_len]).
                let ok = self.checksum == byte;
                if ok {
                    let opcode = self.opcode;
                    let payload = &self.payload[..self.expected_len];
                    // Do not reset here since `payload` borrows from `self`.
                    // The caller will reset after handling the frame.
                    DecoderAction::FrameReady { opcode, payload }
                } else {
                    self.reset();
                    DecoderAction::ChecksumMismatch
                }
            }
            DecoderState::Drop => {
                if self.drop_remaining > 0 {
                    self.drop_remaining -= 1;
                }
                if self.drop_remaining == 0 {
                    self.reset();
                }
                DecoderAction::None
            }
        }
    }
}

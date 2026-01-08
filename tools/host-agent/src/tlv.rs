use anyhow::{bail, Result};
use bytes::{Buf, BufMut, Bytes, BytesMut};

pub const HEADER_LEN: usize = 5;
pub const MAX_PAYLOAD_LEN: usize = 2048;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub tag: u8,
    pub payload: Bytes,
}

impl Frame {
    pub fn new(tag: u8, payload: Bytes) -> Self {
        Self { tag, payload }
    }
}

pub fn write_frame(frame: &Frame, dst: &mut BytesMut) -> Result<()> {
    if frame.payload.len() > MAX_PAYLOAD_LEN {
        bail!(
            "payload length {} exceeds max {}",
            frame.payload.len(),
            MAX_PAYLOAD_LEN
        );
    }

    dst.reserve(HEADER_LEN + frame.payload.len());
    dst.put_u8(frame.tag);
    dst.put_u32_le(frame.payload.len() as u32);
    dst.extend_from_slice(&frame.payload);
    Ok(())
}

pub fn decode_next(buffer: &mut BytesMut) -> Option<Frame> {
    loop {
        if buffer.len() < HEADER_LEN {
            return None;
        }

        let tag = buffer[0];
        let len = u32::from_le_bytes([buffer[1], buffer[2], buffer[3], buffer[4]]) as usize;

        if len > MAX_PAYLOAD_LEN {
            buffer.advance(1);
            continue;
        }

        let total = HEADER_LEN + len;
        if buffer.len() < total {
            if let Some(offset) = find_resync_offset(buffer) {
                buffer.advance(offset);
                continue;
            }
            return None;
        }

        buffer.advance(HEADER_LEN);
        let payload = buffer.split_to(len).freeze();
        return Some(Frame { tag, payload });
    }
}

fn find_resync_offset(buffer: &BytesMut) -> Option<usize> {
    let max_start = buffer.len().saturating_sub(HEADER_LEN);
    for offset in 1..=max_start {
        let len = u32::from_le_bytes([
            buffer[offset + 1],
            buffer[offset + 2],
            buffer[offset + 3],
            buffer[offset + 4],
        ]) as usize;
        if len > MAX_PAYLOAD_LEN {
            continue;
        }
        let total = offset + HEADER_LEN + len;
        if total <= buffer.len() {
            return Some(offset);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_roundtrip() {
        let frame = Frame::new(7, Bytes::from_static(b"ping"));
        let mut buf = BytesMut::new();
        write_frame(&frame, &mut buf).unwrap();

        let decoded = decode_next(&mut buf).expect("frame should decode");
        assert_eq!(decoded, frame);
        assert!(buf.is_empty());
    }

    #[test]
    fn encode_endianness() {
        let payload = vec![0u8; 0x0102];
        let frame = Frame::new(1, Bytes::from(payload));
        let mut buf = BytesMut::new();
        write_frame(&frame, &mut buf).unwrap();

        assert_eq!(buf[0], 1);
        assert_eq!(buf[1], 0x02);
        assert_eq!(buf[2], 0x01);
        assert_eq!(buf[3], 0x00);
        assert_eq!(buf[4], 0x00);
    }

    #[test]
    fn decode_empty_payload() {
        let mut buf = BytesMut::from(&[2u8, 0, 0, 0, 0][..]);
        let decoded = decode_next(&mut buf).expect("frame should decode");
        assert_eq!(decoded.tag, 2);
        assert!(decoded.payload.is_empty());
    }

    #[test]
    fn reject_over_max() {
        let payload = vec![0u8; MAX_PAYLOAD_LEN + 1];
        let frame = Frame::new(1, Bytes::from(payload));
        let mut buf = BytesMut::new();
        let err = write_frame(&frame, &mut buf).unwrap_err();
        assert!(err.to_string().contains("exceeds max"));
    }

    #[test]
    fn resync_on_invalid_length() {
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&[0x99, 0xFF, 0xFF, 0xFF, 0x7F]);
        buf.extend_from_slice(&[0x00, 0x01, 0x02]);
        let good = Frame::new(9, Bytes::from_static(b"ok"));
        write_frame(&good, &mut buf).unwrap();

        let mut frames = Vec::new();
        while let Some(frame) = decode_next(&mut buf) {
            frames.push(frame);
        }

        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0], good);
    }
}

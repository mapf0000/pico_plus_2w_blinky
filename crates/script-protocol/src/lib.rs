#![no_std]

pub use bytecode_constants::MAX_BYTECODE;

pub const WS_BINARY_KIND_SCRIPT_EFFECT: u8 = 8;
pub const VERSION: u8 = 1;
pub const OP_RUN_EFFECT: u8 = 1;
pub const OP_CANCEL_EFFECT: u8 = 2;

const IDS_LEN: usize = 8 * 3;
pub const HEADER_LEN: usize = 1 + 1 + 1 + IDS_LEN + 2;
pub const MAX_MESSAGE_LEN: usize = HEADER_LEN + MAX_BYTECODE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectId {
    pub request_id: u64,
    pub process_id: u64,
    pub effect_id: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Message<'a> {
    Run { id: EffectId, bytecode: &'a [u8] },
    Cancel { id: EffectId },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    BadKind,
    BadVersion,
    BadOperation,
    BadLength,
    BytecodeTooLong,
}

pub fn decode(frame: &[u8]) -> Result<Message<'_>, DecodeError> {
    if frame.len() < HEADER_LEN {
        return Err(DecodeError::BadLength);
    }
    if frame[0] != WS_BINARY_KIND_SCRIPT_EFFECT {
        return Err(DecodeError::BadKind);
    }
    if frame[1] != VERSION {
        return Err(DecodeError::BadVersion);
    }
    let id = EffectId {
        request_id: read_u64(frame, 3),
        process_id: read_u64(frame, 11),
        effect_id: read_u64(frame, 19),
    };
    let bytecode_len = u16::from_le_bytes([frame[27], frame[28]]) as usize;
    if bytecode_len > MAX_BYTECODE {
        return Err(DecodeError::BytecodeTooLong);
    }
    if frame.len() != HEADER_LEN + bytecode_len {
        return Err(DecodeError::BadLength);
    }
    match frame[2] {
        OP_RUN_EFFECT if bytecode_len != 0 => Ok(Message::Run {
            id,
            bytecode: &frame[HEADER_LEN..],
        }),
        OP_CANCEL_EFFECT if bytecode_len == 0 => Ok(Message::Cancel { id }),
        OP_RUN_EFFECT | OP_CANCEL_EFFECT => Err(DecodeError::BadLength),
        _ => Err(DecodeError::BadOperation),
    }
}

pub fn encode_header(
    output: &mut [u8],
    operation: u8,
    id: EffectId,
    bytecode_len: usize,
) -> Result<usize, DecodeError> {
    if bytecode_len > MAX_BYTECODE || bytecode_len > u16::MAX as usize {
        return Err(DecodeError::BytecodeTooLong);
    }
    let total = HEADER_LEN + bytecode_len;
    if output.len() < total {
        return Err(DecodeError::BadLength);
    }
    output[0] = WS_BINARY_KIND_SCRIPT_EFFECT;
    output[1] = VERSION;
    output[2] = operation;
    output[3..11].copy_from_slice(&id.request_id.to_le_bytes());
    output[11..19].copy_from_slice(&id.process_id.to_le_bytes());
    output[19..27].copy_from_slice(&id.effect_id.to_le_bytes());
    output[27..29].copy_from_slice(&(bytecode_len as u16).to_le_bytes());
    Ok(total)
}

fn read_u64(frame: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        frame[offset],
        frame[offset + 1],
        frame[offset + 2],
        frame[offset + 3],
        frame[offset + 4],
        frame[offset + 5],
        frame[offset + 6],
        frame[offset + 7],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_round_trip_and_exact_length() {
        let id = EffectId {
            request_id: 1,
            process_id: 2,
            effect_id: 3,
        };
        let mut frame = [0u8; HEADER_LEN + 4];
        let len = encode_header(&mut frame, OP_RUN_EFFECT, id, 4).unwrap();
        frame[HEADER_LEN..].copy_from_slice(b"KBD1");
        assert_eq!(len, frame.len());
        assert_eq!(
            decode(&frame),
            Ok(Message::Run {
                id,
                bytecode: b"KBD1"
            })
        );

        let mut with_trailing = frame.to_vec();
        with_trailing.push(0);
        assert_eq!(decode(&with_trailing), Err(DecodeError::BadLength));
    }

    #[test]
    fn cancel_has_no_payload() {
        let id = EffectId {
            request_id: 9,
            process_id: 8,
            effect_id: 7,
        };
        let mut frame = [0u8; HEADER_LEN];
        encode_header(&mut frame, OP_CANCEL_EFFECT, id, 0).unwrap();
        assert_eq!(decode(&frame), Ok(Message::Cancel { id }));
    }
}

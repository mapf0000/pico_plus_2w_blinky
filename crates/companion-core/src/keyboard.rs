use crate::Failure;
use keyboard_core::{FlatOp, OpOwned, ProgramOwned, bytecode, lower_to_flat};

pub const MAX_TEXT_CHARS: usize = 1_024;
pub const MAX_TEXT_BYTES: usize = 4_096;
pub const MAX_INITIAL_DELAY_MS: u32 = 5_000;

pub fn layouts() -> &'static [&'static str] {
    keyboard_core::available_layouts()
}

pub struct Effect {
    pub bytecode: Vec<u8>,
    pub estimated_duration_ms: u64,
}

/// Bound source before lowering and validate the encoded effect with the same
/// strict executor validator used by firmware. No text enters diagnostics.
pub fn text_effect(text: String, layout: &str, delay_ms: u32) -> Result<Effect, Failure> {
    if text.is_empty()
        || text.len() > MAX_TEXT_BYTES
        || text.chars().count() > MAX_TEXT_CHARS
        || delay_ms > MAX_INITIAL_DELAY_MS
    {
        return Err(Failure::InvalidText);
    }
    let layout = layout.parse().map_err(|_| Failure::UnsupportedLayout)?;
    let mut program = ProgramOwned::new();
    program.ops.push(OpOwned::Layout(layout));
    if delay_ms != 0 {
        program.ops.push(OpOwned::DelayMs(delay_ms));
    }
    program.ops.push(OpOwned::Text {
        s: text,
        delay_ms: 20,
    });
    let flat = lower_to_flat(&program).map_err(|_| Failure::InvalidText)?;
    let estimated_duration_ms = flat
        .ops
        .iter()
        .map(|op| match op {
            FlatOp::DelayMs(ms) => u64::from(*ms),
            FlatOp::Tap { .. } => 20,
        })
        .sum();
    let bytecode = bytecode::encode(&flat).map_err(|_| Failure::InvalidText)?;
    firmware_exec::validate_bytecode(&bytecode).map_err(|_| Failure::InvalidText)?;
    Ok(Effect {
        bytecode,
        estimated_duration_ms,
    })
}

pub fn envelope(id: script_protocol::EffectId, bytecode: &[u8]) -> Result<Vec<u8>, Failure> {
    let mut frame = vec![0; script_protocol::HEADER_LEN + bytecode.len()];
    script_protocol::encode_header(
        &mut frame,
        script_protocol::OP_RUN_EFFECT,
        id,
        bytecode.len(),
    )
    .map_err(|_| Failure::InvalidData)?;
    frame[script_protocol::HEADER_LEN..].copy_from_slice(bytecode);
    Ok(frame)
}

pub fn cancel_envelope(id: script_protocol::EffectId) -> Result<Vec<u8>, Failure> {
    let mut frame = vec![0; script_protocol::HEADER_LEN];
    script_protocol::encode_header(&mut frame, script_protocol::OP_CANCEL_EFFECT, id, 0)
        .map_err(|_| Failure::InvalidData)?;
    Ok(frame)
}

use crate::{
    CharMapping, CompileError, FlatProgram, KeyTap, LayoutId, OpOwned, ProgramOwned, Span,
    message_for_code,
};

/// Lower an owned, call-free program into a FlatProgram using a default layout.
/// - Expands `Text` to Tap+Delay
/// - Coalesces adjacent delays
pub fn lower_to_flat_with_layout(
    p: &ProgramOwned,
    default_layout: LayoutId,
) -> Result<FlatProgram, CompileError> {
    let mut out = FlatProgram::new();
    let mut current_layout = default_layout;
    for (idx, op) in p.ops.iter().enumerate() {
        match op {
            OpOwned::Tap(KeyTap { usage, mods }) => {
                out.push_tap(*usage, *mods)
                    .map_err(|_| lower_error("TooManyLines", 0))?;
            }
            OpOwned::DelayMs(ms) => {
                out.push_delay(*ms)
                    .map_err(|_| lower_error("TooManyLines", 0))?;
            }
            OpOwned::Text { s, delay_ms } => {
                for ch in s.chars() {
                    let mapping = current_layout
                        .map_char(ch)
                        .ok_or_else(|| lower_error("ParseKey", (idx as u16) + 1))?;
                    match mapping {
                        CharMapping::Tap { usage, mods } => {
                            out.push_tap(usage, mods)
                                .map_err(|_| lower_error("TooManyLines", 0))?;
                            out.push_delay(*delay_ms as u32)
                                .map_err(|_| lower_error("TooManyLines", 0))?;
                        }
                        CharMapping::Seq(seq) => {
                            for (idx, tap) in seq.iter().enumerate() {
                                out.push_tap(tap.usage, tap.mods)
                                    .map_err(|_| lower_error("TooManyLines", 0))?;
                                if idx + 1 == seq.len() {
                                    out.push_delay(*delay_ms as u32)
                                        .map_err(|_| lower_error("TooManyLines", 0))?;
                                }
                            }
                        }
                    }
                }
            }
            OpOwned::Layout(layout) => {
                current_layout = *layout;
            }
        }
    }
    Ok(out)
}

/// Lower an owned, call-free program into a US-only FlatProgram.
pub fn lower_to_flat_us(p: &ProgramOwned) -> Result<FlatProgram, CompileError> {
    lower_to_flat_with_layout(p, LayoutId::Us)
}

fn lower_error(code: &'static str, line: u16) -> CompileError {
    CompileError::error(code, message_for_code(code), Span::line(line))
}

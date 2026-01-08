use crate::limits::MAX_TOTAL_FLAT_OPS;
use crate::{
    DslError, DslErrorAt, FlatOp, FlatProgram, KeyTap, LayoutId, OpOwned, ProgramOwned,
};

/// Lower an owned, call-free program into a FlatProgram using a default layout.
/// - Expands `Text` to Tap+Delay
/// - Coalesces adjacent delays
pub fn lower_to_flat_with_layout(
    p: &ProgramOwned,
    default_layout: LayoutId,
) -> Result<FlatProgram, DslErrorAt> {
    let mut out = FlatProgram::new();
    let mut current_layout = default_layout;
    for (idx, op) in p.ops.iter().enumerate() {
        match op {
            OpOwned::Tap(KeyTap { usage, mods }) => out.ops.push(FlatOp::Tap {
                usage: *usage,
                mods: *mods,
            }),
            OpOwned::DelayMs(ms) => push_delay(&mut out, *ms),
            OpOwned::Text { s, delay_ms } => {
                for ch in s.chars() {
                    let (u, m) = current_layout.map_char(ch).ok_or(DslErrorAt {
                        kind: DslError::ParseKey,
                        line: (idx as u16) + 1,
                    })?;
                    out.ops.push(FlatOp::Tap { usage: u, mods: m });
                    if *delay_ms != 0 {
                        push_delay(&mut out, *delay_ms as u32);
                    }
                }
            }
            OpOwned::Layout(layout) => {
                current_layout = *layout;
            }
        }
        if out.ops.len() > MAX_TOTAL_FLAT_OPS {
            return Err(DslErrorAt {
                kind: DslError::TooManyLines,
                line: 0,
            });
        }
    }
    Ok(out)
}

/// Lower an owned, call-free program into a US-only FlatProgram.
pub fn lower_to_flat_us(p: &ProgramOwned) -> Result<FlatProgram, DslErrorAt> {
    lower_to_flat_with_layout(p, LayoutId::Us)
}

fn push_delay(out: &mut FlatProgram, ms: u32) {
    if ms == 0 {
        return;
    }
    if let Some(FlatOp::DelayMs(prev)) = out.ops.last_mut() {
        // coalesce
        let sum = (*prev as u64 + ms as u64).min(u32::MAX as u64) as u32;
        *prev = sum;
    } else {
        out.ops.push(FlatOp::DelayMs(ms));
    }
}

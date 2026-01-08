// -------- Public Limits --------

pub const MAX_DSL_LINES: usize = 256;
pub const MAX_DSL_DELAY_MS: u64 = 5000;
pub const MAX_TOTAL_FLAT_OPS: usize = 10_000;

// Internal caps for Phase 1 preprocessor (repeat/let) — not public API.
pub(crate) const MAX_REPEAT_N: u32 = 100;
pub(crate) const MAX_EXPANDED_LINES: usize = 4096;

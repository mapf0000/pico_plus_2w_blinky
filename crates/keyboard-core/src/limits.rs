//! Resource limits shared by keyboard lowering and validation.

// -------- Public Limits --------

pub const MAX_DEVICE_DELAY_MS: u64 = bytecode_constants::MAX_DEVICE_DELAY_MS as u64;
pub const MAX_EFFECT_DELAY_TOTAL_MS: u64 = bytecode_constants::MAX_EFFECT_DELAY_TOTAL_MS;
pub const MAX_TOTAL_FLAT_OPS: usize = bytecode_constants::MAX_BYTECODE_OPS;

#![no_std]

/// Maximum allowed bytecode length in bytes.
pub const MAX_BYTECODE: usize = 4096;

/// Maximum number of decoded keyboard operations in one program.
pub const MAX_BYTECODE_OPS: usize = 10_000;

/// Maximum delay encoded in one device-side operation.
pub const MAX_DEVICE_DELAY_MS: u32 = 5_000;

/// Maximum sum of encoded delays in one queued keyboard effect.
pub const MAX_EFFECT_DELAY_TOTAL_MS: u64 = 5 * 60 * 1_000;

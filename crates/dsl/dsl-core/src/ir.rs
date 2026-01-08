#[cfg(feature = "std")]
use std::{string::String, vec::Vec};

#[cfg(not(feature = "std"))]
use alloc::{string::String, vec::Vec};

use crate::limits::MAX_TOTAL_FLAT_OPS;
use crate::{Mods, Usage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyTap {
    pub usage: Usage,
    pub mods: Mods,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Op<'a> {
    Tap(KeyTap),
    DelayMs(u32),
    Text { s: &'a str, delay_ms: u16 },
    Layout(crate::LayoutId),
    Call { id: &'a str },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Program<'a> {
    pub ops: Vec<Op<'a>>,
}

impl<'a> Program<'a> {
    pub fn new() -> Self {
        Self { ops: Vec::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpOwned {
    Tap(KeyTap),
    DelayMs(u32),
    Text { s: String, delay_ms: u16 },
    Layout(crate::LayoutId),
    // Calls are fully inlined during linking; not present in OpOwned
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramOwned {
    pub ops: Vec<OpOwned>,
}

impl ProgramOwned {
    pub fn new() -> Self {
        Self { ops: Vec::new() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlatOp {
    Tap { usage: Usage, mods: Mods },
    DelayMs(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlatProgramError {
    TooManyOps,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlatProgram {
    pub ops: Vec<FlatOp>,
}

impl FlatProgram {
    pub fn new() -> Self {
        Self { ops: Vec::new() }
    }

    pub(crate) fn push_tap(&mut self, usage: Usage, mods: Mods) -> Result<(), FlatProgramError> {
        self.ops.push(FlatOp::Tap { usage, mods });
        self.check_limit()
    }

    pub(crate) fn push_delay(&mut self, ms: u32) -> Result<(), FlatProgramError> {
        if ms == 0 {
            return Ok(());
        }
        if let Some(FlatOp::DelayMs(prev)) = self.ops.last_mut() {
            let sum = (*prev as u64 + ms as u64).min(u32::MAX as u64) as u32;
            *prev = sum;
        } else {
            self.ops.push(FlatOp::DelayMs(ms));
        }
        self.check_limit()
    }

    pub(crate) fn push_op(&mut self, op: FlatOp) -> Result<(), FlatProgramError> {
        match op {
            FlatOp::Tap { usage, mods } => self.push_tap(usage, mods),
            FlatOp::DelayMs(ms) => self.push_delay(ms),
        }
    }

    pub(crate) fn normalize_in_place(&mut self) -> Result<(), FlatProgramError> {
        let mut out = FlatProgram::new();
        for op in self.ops.iter().copied() {
            out.push_op(op)?;
        }
        self.ops = out.ops;
        Ok(())
    }

    fn check_limit(&self) -> Result<(), FlatProgramError> {
        if self.ops.len() > MAX_TOTAL_FLAT_OPS {
            Err(FlatProgramError::TooManyOps)
        } else {
            Ok(())
        }
    }
}

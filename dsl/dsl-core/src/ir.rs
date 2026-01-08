#[cfg(feature = "std")]
use std::{string::String, vec::Vec};

#[cfg(not(feature = "std"))]
use alloc::{string::String, vec::Vec};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyTap {
    pub usage: u8,
    pub mods: u8,
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
    Tap { usage: u8, mods: u8 },
    DelayMs(u32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlatProgram {
    pub ops: Vec<FlatOp>,
}

impl FlatProgram {
    pub fn new() -> Self {
        Self { ops: Vec::new() }
    }
}

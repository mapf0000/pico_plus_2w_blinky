#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc;

#[cfg(feature = "std")]
use std::vec::Vec;
#[cfg(feature = "std")]
use std::string::String;

#[cfg(not(feature = "std"))]
use alloc::{vec::Vec, string::String};

//
// -------- Public Limits --------
//
pub const MAX_DSL_LINES: usize = 256;
pub const MAX_DSL_DELAY_MS: u64 = 5000;
pub const MAX_TOTAL_FLAT_OPS: usize = 10_000;

//
// -------- Errors & Diagnostics --------
//
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DslError {
    TooManyLines,
    UnknownCommand,
    InvalidLine,
    ParseKey,
    ParseMod,
    ParseDelay,
    TextEmpty,
    UnknownScript,
    RecursionTooDeep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DslErrorAt {
    pub kind: DslError,
    /// 1-based line number in the current DSL being compiled.
    pub line: u16,
}

//
// -------- HID constants (subset) --------
// (stable per HID Usage Tables, page 0x07)
pub const KEY_A: u8 = 0x04;
pub const KEY_1: u8 = 0x1e; pub const KEY_2: u8 = 0x1f; pub const KEY_3: u8 = 0x20;
pub const KEY_4: u8 = 0x21; pub const KEY_5: u8 = 0x22; pub const KEY_6: u8 = 0x23;
pub const KEY_7: u8 = 0x24; pub const KEY_8: u8 = 0x25; pub const KEY_9: u8 = 0x26; pub const KEY_0: u8 = 0x27;
pub const KEY_ENTER: u8 = 0x28; pub const KEY_ESC: u8 = 0x29; pub const KEY_BACKSPACE: u8 = 0x2a; pub const KEY_TAB: u8 = 0x2b; pub const KEY_SPACE: u8 = 0x2c;
pub const KEY_MINUS: u8 = 0x2d; pub const KEY_EQUAL: u8 = 0x2e; pub const KEY_LEFT_BRACKET: u8 = 0x2f; pub const KEY_RIGHT_BRACKET: u8 = 0x30;
pub const KEY_BACKSLASH: u8 = 0x31; pub const KEY_NON_US_HASH: u8 = 0x32; pub const KEY_SEMICOLON: u8 = 0x33; pub const KEY_APOSTROPHE: u8 = 0x34;
pub const KEY_GRAVE: u8 = 0x35; pub const KEY_COMMA: u8 = 0x36; pub const KEY_DOT: u8 = 0x37; pub const KEY_SLASH: u8 = 0x38; pub const KEY_CAPS_LOCK: u8 = 0x39;
pub const KEY_F1: u8 = 0x3a; pub const KEY_F2: u8 = 0x3b; pub const KEY_F3: u8 = 0x3c; pub const KEY_F4: u8 = 0x3d; pub const KEY_F5: u8 = 0x3e; pub const KEY_F6: u8 = 0x3f;
pub const KEY_F7: u8 = 0x40; pub const KEY_F8: u8 = 0x41; pub const KEY_F9: u8 = 0x42; pub const KEY_F10: u8 = 0x43; pub const KEY_F11: u8 = 0x44; pub const KEY_F12: u8 = 0x45;
pub const KEY_PRINT_SCREEN: u8 = 0x46; pub const KEY_SCROLL_LOCK: u8 = 0x47; pub const KEY_PAUSE: u8 = 0x48; pub const KEY_INSERT: u8 = 0x49; pub const KEY_HOME: u8 = 0x4a;
pub const KEY_PAGE_UP: u8 = 0x4b; pub const KEY_DELETE: u8 = 0x4c; pub const KEY_END: u8 = 0x4d; pub const KEY_PAGE_DOWN: u8 = 0x4e;
pub const KEY_RIGHT: u8 = 0x4f; pub const KEY_LEFT: u8 = 0x50; pub const KEY_DOWN: u8 = 0x51; pub const KEY_UP: u8 = 0x52;
pub const KEY_NUM_LOCK: u8 = 0x53; pub const KEY_KP_SLASH: u8 = 0x54; pub const KEY_KP_ASTERISK: u8 = 0x55; pub const KEY_KP_MINUS: u8 = 0x56; pub const KEY_KP_PLUS: u8 = 0x57;
pub const KEY_KP_ENTER: u8 = 0x58; pub const KEY_KP_1: u8 = 0x59; pub const KEY_KP_2: u8 = 0x5a; pub const KEY_KP_3: u8 = 0x5b; pub const KEY_KP_4: u8 = 0x5c; pub const KEY_KP_5: u8 = 0x5d;
pub const KEY_KP_6: u8 = 0x5e; pub const KEY_KP_7: u8 = 0x5f; pub const KEY_KP_8: u8 = 0x60; pub const KEY_KP_9: u8 = 0x61; pub const KEY_KP_0: u8 = 0x62; pub const KEY_KP_DOT: u8 = 0x63;
pub const KEY_NON_US_BACKSLASH: u8 = 0x64;

pub const MOD_LCTRL: u8 = 0x01; pub const MOD_LSHIFT: u8 = 0x02; pub const MOD_LALT: u8 = 0x04; pub const MOD_LGUI: u8 = 0x08;
pub const MOD_RCTRL: u8 = 0x10; pub const MOD_RSHIFT: u8 = 0x20; pub const MOD_RALT: u8 = 0x40; pub const MOD_RGUI: u8 = 0x80;

//
// -------- High-level IR (AST) --------
//
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyTap { pub usage: u8, pub mods: u8 }

#[derive(Debug, Clone, PartialEq)]
pub enum Op<'a> {
    Tap(KeyTap),
    DelayMs(u32),
    Text { s: &'a str, delay_ms: u16 },
    Call { id: &'a str },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Program<'a> { pub ops: Vec<Op<'a>> }

impl<'a> Program<'a> {
    pub fn new() -> Self { Self { ops: Vec::new() } }
}

//
// -------- Owned IR (for linking & lowering) --------
//
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpOwned {
    Tap(KeyTap),
    DelayMs(u32),
    Text { s: String, delay_ms: u16 }, // owned
    // Calls are fully inlined during linking; not present in OpOwned
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramOwned { pub ops: Vec<OpOwned> }
impl ProgramOwned { pub fn new() -> Self { Self { ops: Vec::new() } } }

//
// -------- Flat IR (lowered; only Tap/Delay) --------
//
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlatOp {
    Tap { usage: u8, mods: u8 },
    DelayMs(u32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlatProgram { pub ops: Vec<FlatOp> }
impl FlatProgram { pub fn new() -> Self { Self { ops: Vec::new() } } }

//
// -------- Parser (DSL -> Program<'a>) --------
//

pub trait ScriptExists { fn exists(&self, id: &str) -> bool; }
impl<F> ScriptExists for F where F: Fn(&str) -> bool { fn exists(&self, id: &str) -> bool { (self)(id) } }

fn split_head(s: &str) -> Option<(&str, &str)> {
    let mut it = s.splitn(2, char::is_whitespace);
    let head = it.next()?;
    let tail = it.next().unwrap_or("");
    Some((head, tail))
}
fn eq_ci(a: &str, b: &str) -> bool { a.eq_ignore_ascii_case(b) }

fn upper_ascii<const N: usize>(s: &str) -> ArrayString<N> {
    let mut out: ArrayString<N> = ArrayString::new();
    for b in s.bytes() {
        let up = if b'a' <= b && b <= b'z' { b - 32 } else { b };
        let _ = out.push(up as char);
    }
    out
}

pub fn compile_dsl_with_diag<'a>(dsl: &'a str, exists: impl ScriptExists) -> Result<Program<'a>, DslErrorAt> {
    let mut prog = Program::new();
    let mut counted_nonempty = 0usize;
    for (lineno0, raw) in dsl.lines().enumerate() {
        let line_no = (lineno0 + 1) as u16;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        counted_nonempty += 1;
        if counted_nonempty > MAX_DSL_LINES { return Err(DslErrorAt { kind: DslError::TooManyLines, line: line_no }); }

        let (cmd, rest) = split_head(line).ok_or(DslErrorAt { kind: DslError::InvalidLine, line: line_no })?;
        if eq_ci(cmd, "tap") {
            let key_name = rest.trim();
            if key_name.is_empty() { return Err(DslErrorAt { kind: DslError::InvalidLine, line: line_no }); }
            let usage = parse_key(key_name).ok_or(DslErrorAt { kind: DslError::ParseKey, line: line_no })?;
            prog.ops.push(Op::Tap(KeyTap { usage, mods: 0 }));
        } else if eq_ci(cmd, "modtap") {
            let arg = rest.trim();
            if arg.is_empty() { return Err(DslErrorAt { kind: DslError::InvalidLine, line: line_no }); }
            let (mods, usage) = parse_modtap(arg).map_err(|k| DslErrorAt { kind: k, line: line_no })?;
            prog.ops.push(Op::Tap(KeyTap { usage, mods }));
        } else if eq_ci(cmd, "delay") {
            let ms: u64 = rest.trim().parse::<u64>().map_err(|_| DslErrorAt { kind: DslError::ParseDelay, line: line_no })?;
            let ms = core::cmp::min(ms, MAX_DSL_DELAY_MS) as u32;
            if ms != 0 { prog.ops.push(Op::DelayMs(ms)); }
        } else if eq_ci(cmd, "text") {
            let (text, delay_ms) = parse_text_args(rest).map_err(|k| DslErrorAt { kind: k, line: line_no })?;
            let delay_ms = core::cmp::min(delay_ms as u64, MAX_DSL_DELAY_MS) as u16;
            if !text.is_empty() { prog.ops.push(Op::Text { s: text, delay_ms }); }
        } else if eq_ci(cmd, "call") {
            let id = rest.trim();
            if id.is_empty() { return Err(DslErrorAt { kind: DslError::InvalidLine, line: line_no }); }
            if !exists.exists(id) { return Err(DslErrorAt { kind: DslError::UnknownScript, line: line_no }); }
            prog.ops.push(Op::Call { id });
        } else {
            return Err(DslErrorAt { kind: DslError::UnknownCommand, line: line_no });
        }
    }
    Ok(prog)
}

// --- parse helpers (mods, keys, text) ---

fn parse_modtap(s: &str) -> Result<(u8, u8), DslError> {
    let mut mods: u8 = 0;
    let mut parts = s.split('+').peekable();
    let mut last_is_key = false;
    let mut usage: u8 = 0;
    while let Some(p) = parts.next() {
        let t = p.trim();
        if t.is_empty() { return Err(DslError::InvalidLine); }
        if parts.peek().is_none() {
            usage = parse_key(t).ok_or(DslError::ParseKey)?;
            last_is_key = true;
        } else {
            let m = parse_mod(t).ok_or(DslError::ParseMod)?;
            mods |= m;
        }
    }
    if !last_is_key { return Err(DslError::ParseKey); }
    Ok((mods, usage))
}

fn parse_text_args(rest: &str) -> Result<(&str, u64), DslError> {
    let r = rest.trim();
    if r.is_empty() { return Err(DslError::TextEmpty); }
    let mut delay_ms: u64 = 10;
    if let Some(idx) = r.rfind(char::is_whitespace) {
        let (lhs, rhs) = r.split_at(idx);
        let maybe = rhs.trim();
        if !maybe.is_empty() {
            if let Ok(n) = maybe.parse::<u64>() {
                delay_ms = core::cmp::min(n, MAX_DSL_DELAY_MS);
                let text = lhs.trim_end();
                return Ok((text, delay_ms));
            }
        }
    }
    Ok((r, delay_ms))
}

fn parse_mod(s: &str) -> Option<u8> {
    let up = upper_ascii::<16>(s);
    let u = up.as_str();
    Some(match u {
        "LCTRL" | "CONTROL" | "CTRL" => MOD_LCTRL,
        "RCTRL" => MOD_RCTRL,
        "LSHIFT" | "SHIFT" => MOD_LSHIFT,
        "RSHIFT" => MOD_RSHIFT,
        "LALT" | "ALT" | "OPTION" => MOD_LALT,
        "RALT" => MOD_RALT,
        "LGUI" | "GUI" | "CMD" | "WIN" | "META" | "SUPER" => MOD_LGUI,
        "RGUI" => MOD_RGUI,
        _ => return None,
    })
}

fn parse_key(s: &str) -> Option<u8> {
    let up = upper_ascii::<32>(s);
    let u = up.as_str();

    if u.len() == 1 {
        let b = u.as_bytes()[0];
        if b'A' <= b && b <= b'Z' { return Some(KEY_A + (b - b'A')); }
    }
    if let Some(rest) = u.strip_prefix('F') {
        if let Ok(n) = rest.parse::<u8>() {
            return match n {
                1 => Some(KEY_F1), 2 => Some(KEY_F2), 3 => Some(KEY_F3), 4 => Some(KEY_F4),
                5 => Some(KEY_F5), 6 => Some(KEY_F6), 7 => Some(KEY_F7), 8 => Some(KEY_F8),
                9 => Some(KEY_F9), 10 => Some(KEY_F10), 11 => Some(KEY_F11), 12 => Some(KEY_F12),
                _ => None,
            };
        }
    }
    if let Some(rest) = u.strip_prefix("KP_") {
        return match rest {
            "ENTER" => Some(KEY_KP_ENTER),
            "+" | "PLUS" => Some(KEY_KP_PLUS),
            "-" | "MINUS" => Some(KEY_KP_MINUS),
            "*" | "ASTERISK" => Some(KEY_KP_ASTERISK),
            "/" | "SLASH" => Some(KEY_KP_SLASH),
            "." | "DOT" | "DECIMAL" => Some(KEY_KP_DOT),
            "0" => Some(KEY_KP_0),
            "1" => Some(KEY_KP_1),
            "2" => Some(KEY_KP_2),
            "3" => Some(KEY_KP_3),
            "4" => Some(KEY_KP_4),
            "5" => Some(KEY_KP_5),
            "6" => Some(KEY_KP_6),
            "7" => Some(KEY_KP_7),
            "8" => Some(KEY_KP_8),
            "9" => Some(KEY_KP_9),
            _ => None,
        };
    }
    if u.len() == 1 {
        let b = u.as_bytes()[0];
        if b'1' <= b && b <= b'9' { return Some(KEY_1 + (b - b'1')); }
        if b == b'0' { return Some(KEY_0); }
    }
    Some(match u {
        "ENTER" | "RETURN" => KEY_ENTER,
        "ESC" | "ESCAPE" => KEY_ESC,
        "BACKSPACE" | "BKSP" => KEY_BACKSPACE,
        "TAB" => KEY_TAB,
        "SPACE" | "SPACEBAR" => KEY_SPACE,
        "CAPS_LOCK" | "CAPS" => KEY_CAPS_LOCK,
        "PRINT_SCREEN" | "PRTSCR" => KEY_PRINT_SCREEN,
        "SCROLL_LOCK" => KEY_SCROLL_LOCK,
        "PAUSE" | "BREAK" => KEY_PAUSE,
        "INSERT" | "INS" => KEY_INSERT,
        "DELETE" | "DEL" => KEY_DELETE,
        "HOME" => KEY_HOME,
        "END" => KEY_END,
        "PAGE_UP" | "PGUP" => KEY_PAGE_UP,
        "PAGE_DOWN" | "PGDN" => KEY_PAGE_DOWN,
        "LEFT" => KEY_LEFT,
        "RIGHT" => KEY_RIGHT,
        "UP" => KEY_UP,
        "DOWN" => KEY_DOWN,

        "MINUS" | "HYPHEN" => KEY_MINUS,
        "EQUAL" | "EQUALS" | "PLUS" => KEY_EQUAL,
        "LEFT_BRACKET" | "LBRACKET" | "LBRACE" => KEY_LEFT_BRACKET,
        "RIGHT_BRACKET" | "RBRACKET" | "RBRACE" => KEY_RIGHT_BRACKET,
        "BACKSLASH" | "BSLASH" | "PIPE" => KEY_BACKSLASH,
        "NON_US_HASH" => KEY_NON_US_HASH,
        "SEMICOLON" | "SEMI" | ":" => KEY_SEMICOLON,
        "APOSTROPHE" | "QUOTE" | "'" | "\"" => KEY_APOSTROPHE,
        "GRAVE" | "BACKTICK" | "TILDE" => KEY_GRAVE,
        "COMMA" | "," | "<" => KEY_COMMA,
        "DOT" | "PERIOD" | "." | ">" => KEY_DOT,
        "SLASH" | "/" | "?" => KEY_SLASH,

        "NON_US_BACKSLASH" => KEY_NON_US_BACKSLASH,
        "NUM_LOCK" => KEY_NUM_LOCK,
        _ => return None,
    })
}

// tiny array string used by upper_ascii (no heap)
#[derive(Debug, Clone)]
pub struct ArrayString<const N: usize> { buf: [u8; N], len: usize }
impl<const N: usize> ArrayString<N> {
    pub fn new() -> Self { Self { buf: [0; N], len: 0 } }
    pub fn push(&mut self, ch: char) -> Result<(), ()> {
        if self.len < N { self.buf[self.len] = ch as u8; self.len += 1; Ok(()) } else { Err(()) }
    }
    pub fn as_str(&self) -> &str { core::str::from_utf8(&self.buf[..self.len]).unwrap_or("") }
}

//
// -------- Linking (resolve/inlines calls) --------
//

/// Simple provider used during linking. Returns DSL text for a script id.
pub trait ScriptProvider {
    fn get(&self, id: &str) -> Option<&str>;
}

impl<F> ScriptProvider for F
where
    F: Fn(&str) -> Option<&str>,
{
    fn get(&self, id: &str) -> Option<&str> { (self)(id) }
}

/// Compile `entry_dsl`, resolve & inline all `call`s using `provider`,
/// and return an **owned** program with no `Call` ops.
pub fn compile_and_link<'a>(
    entry_dsl: &'a str,
    provider: &impl ScriptProvider,
) -> Result<ProgramOwned, DslErrorAt> {
    let exists = |id: &str| provider.get(id).is_some();
    let ast = compile_dsl_with_diag(entry_dsl, exists)?;
    let mut out = ProgramOwned::new();
    let mut stack: Vec<String> = Vec::new();
    inline_into_owned(&ast, provider, &mut out, &mut stack)?;
    Ok(out)
}

fn inline_into_owned(
    ast: &Program<'_>,
    provider: &impl ScriptProvider,
    out: &mut ProgramOwned,
    stack: &mut Vec<String>,
) -> Result<(), DslErrorAt> {
    for (idx, op) in ast.ops.iter().enumerate() {
        match op {
            Op::Tap(k) => out.ops.push(OpOwned::Tap(*k)),
            Op::DelayMs(ms) => if *ms != 0 { out.ops.push(OpOwned::DelayMs(*ms)); },
            Op::Text { s, delay_ms } => out.ops.push(OpOwned::Text { s: (*s).to_owned(), delay_ms: *delay_ms }),
            Op::Call { id } => {
                // cycle detection
                if stack.iter().any(|s| s == id) {
                    return Err(DslErrorAt { kind: DslError::RecursionTooDeep, line: (idx as u16)+1 });
                }
                stack.push((*id).to_owned());
                let Some(text) = provider.get(id) else {
                    return Err(DslErrorAt { kind: DslError::UnknownScript, line: (idx as u16)+1 });
                };
                let exists = |sid: &str| provider.get(sid).is_some();
                let sub = compile_dsl_with_diag(text, exists)?;
                inline_into_owned(&sub, provider, out, stack)?;
                stack.pop();
            }
        }
        if out.ops.len() > MAX_TOTAL_FLAT_OPS { // reusing the cap for owned too
            return Err(DslErrorAt { kind: DslError::TooManyLines, line: 0 });
        }
    }
    Ok(())
}

//
// -------- US ANSI text lowering (Owned -> Flat) --------
//

pub fn char_to_key_us(c: char) -> Option<(u8, u8)> {
    match c {
        '\n' | '\r' => Some((KEY_ENTER, 0)),
        '\t' => Some((KEY_TAB, 0)),
        ' '  => Some((KEY_SPACE, 0)),
        'a'..='z' => { let i = (c as u8) - b'a'; Some((KEY_A + i, 0)) }
        'A'..='Z' => { let i = (c as u8) - b'A'; Some((KEY_A + i, MOD_LSHIFT)) }
        '1' => Some((KEY_1, 0)), '2' => Some((KEY_2, 0)), '3' => Some((KEY_3, 0)), '4' => Some((KEY_4, 0)), '5' => Some((KEY_5, 0)),
        '6' => Some((KEY_6, 0)), '7' => Some((KEY_7, 0)), '8' => Some((KEY_8, 0)), '9' => Some((KEY_9, 0)), '0' => Some((KEY_0, 0)),
        '!' => Some((KEY_1, MOD_LSHIFT)), '@' => Some((KEY_2, MOD_LSHIFT)), '#' => Some((KEY_3, MOD_LSHIFT)), '$' => Some((KEY_4, MOD_LSHIFT)),
        '%' => Some((KEY_5, MOD_LSHIFT)), '^' => Some((KEY_6, MOD_LSHIFT)), '&' => Some((KEY_7, MOD_LSHIFT)), '*' => Some((KEY_8, MOD_LSHIFT)),
        '(' => Some((KEY_9, MOD_LSHIFT)), ')' => Some((KEY_0, MOD_LSHIFT)),
        '-' => Some((KEY_MINUS, 0)), '_' => Some((KEY_MINUS, MOD_LSHIFT)),
        '=' => Some((KEY_EQUAL, 0)), '+' => Some((KEY_EQUAL, MOD_LSHIFT)),
        '[' => Some((KEY_LEFT_BRACKET, 0)), '{' => Some((KEY_LEFT_BRACKET, MOD_LSHIFT)),
        ']' => Some((KEY_RIGHT_BRACKET, 0)), '}' => Some((KEY_RIGHT_BRACKET, MOD_LSHIFT)),
        '\\'=> Some((KEY_BACKSLASH, 0)), '|' => Some((KEY_BACKSLASH, MOD_LSHIFT)),
        ';' => Some((KEY_SEMICOLON, 0)), ':' => Some((KEY_SEMICOLON, MOD_LSHIFT)),
        '\''=> Some((KEY_APOSTROPHE, 0)), '"' => Some((KEY_APOSTROPHE, MOD_LSHIFT)),
        '`' => Some((KEY_GRAVE, 0)),       '~' => Some((KEY_GRAVE, MOD_LSHIFT)),
        ',' => Some((KEY_COMMA, 0)),       '<' => Some((KEY_COMMA, MOD_LSHIFT)),
        '.' => Some((KEY_DOT, 0)),         '>' => Some((KEY_DOT, MOD_LSHIFT)),
        '/' => Some((KEY_SLASH, 0)),       '?' => Some((KEY_SLASH, MOD_LSHIFT)),
        _ => None,
    }
}

/// Lower an owned, call-free program into a US-only FlatProgram.
/// - Expands `Text` to Tap+Delay
/// - Coalesces adjacent delays
pub fn lower_to_flat_us(p: &ProgramOwned) -> Result<FlatProgram, DslErrorAt> {
    let mut out = FlatProgram::new();
    for (idx, op) in p.ops.iter().enumerate() {
        match op {
            OpOwned::Tap(KeyTap { usage, mods }) => out.ops.push(FlatOp::Tap { usage: *usage, mods: *mods }),
            OpOwned::DelayMs(ms) => push_delay(&mut out, *ms),
            OpOwned::Text { s, delay_ms } => {
                for ch in s.chars() {
                    let (u, m) = char_to_key_us(ch).ok_or(DslErrorAt{ kind: DslError::ParseKey, line: (idx as u16)+1 })?;
                    out.ops.push(FlatOp::Tap { usage: u, mods: m });
                    if *delay_ms != 0 { push_delay(&mut out, *delay_ms as u32); }
                }
            }
        }
        if out.ops.len() > MAX_TOTAL_FLAT_OPS {
            return Err(DslErrorAt { kind: DslError::TooManyLines, line: 0 });
        }
    }
    Ok(out)
}

fn push_delay(out: &mut FlatProgram, ms: u32) {
    if ms == 0 { return; }
    if let Some(FlatOp::DelayMs(prev)) = out.ops.last_mut() {
        // coalesce
        let sum = (*prev as u64 + ms as u64).min(u32::MAX as u64) as u32;
        *prev = sum;
    } else {
        out.ops.push(FlatOp::DelayMs(ms));
    }
}

//
// -------- Bytecode (encoder + streaming reader) --------
//

pub mod bytecode {
    #[cfg(not(feature = "std"))]
    extern crate alloc;
    #[cfg(not(feature = "std"))]
    use alloc::vec::Vec;

    #[cfg(feature = "std")]
    use std::vec::Vec;

    use super::{FlatProgram, FlatOp};

    pub const MAGIC: [u8; 4] = *b"KBD1";
    pub const OP_DELAY: u8 = 0x01;
    pub const OP_TAP:   u8 = 0x02;
    pub const OP_END:   u8 = 0xFF;

    /// Encode a FlatProgram into compact, versioned bytecode with CRC32 trailer.
    pub fn encode(prog: &FlatProgram) -> Vec<u8> {
        let mut buf = Vec::with_capacity(4 + 1 + prog.ops.len() * 4 + 5);
        buf.extend_from_slice(&MAGIC);
        buf.push(0x00); // flags (reserved)

        for op in &prog.ops {
            match *op {
                FlatOp::DelayMs(ms) => { buf.push(OP_DELAY); write_varu32(&mut buf, ms); }
                FlatOp::Tap { usage, mods } => { buf.push(OP_TAP); buf.push(usage); buf.push(mods); }
            }
        }
        buf.push(OP_END);

        let crc = crc32(&buf);
        buf.extend_from_slice(&crc.to_le_bytes());
        buf
    }

    /// Streaming reader for firmware side (no allocation required).
    pub struct Reader<'a> {
        data: &'a [u8],
        pos: usize,
        crc_range_end: usize, // position of CRC (exclusive)
    }

    #[derive(Debug)]
    pub enum DecodeError {
        BadMagic,
        UnexpectedEof,
        BadVarint,
        BadOpcode,
        BadCrc,
    }

    impl<'a> Reader<'a> {
        pub fn new(data: &'a [u8]) -> Result<Self, DecodeError> {
            if data.len() < 4 + 1 + 4 { return Err(DecodeError::UnexpectedEof); }
            if &data[0..4] != &MAGIC { return Err(DecodeError::BadMagic); }
            // flags = data[4], ignore for now
            // Find CRC at end (last 4 bytes)
            let crc_range_end = data.len().checked_sub(4).ok_or(DecodeError::UnexpectedEof)?;
            Ok(Self { data, pos: 5, crc_range_end })
        }

        pub fn read_u8(&mut self) -> Result<u8, DecodeError> {
            if self.pos >= self.crc_range_end { return Err(DecodeError::UnexpectedEof); }
            let b = self.data[self.pos]; self.pos += 1; Ok(b)
        }

        pub fn read_varu32(&mut self) -> Result<u32, DecodeError> {
            let mut result: u32 = 0;
            let mut shift = 0;
            for _ in 0..5 {
                let b = self.read_u8()?;
                result |= ((b & 0x7F) as u32) << shift;
                if (b & 0x80) == 0 { return Ok(result); }
                shift += 7;
            }
            Err(DecodeError::BadVarint)
        }

        /// After finishing the stream, verify the CRC32 trailer.
        pub fn verify_crc(self) -> Result<(), DecodeError> {
            // Must be positioned exactly at crc_range_end now or later (we ignore extra padding),
            // but we always compute CRC over [0 .. crc_range_end]
            if self.data.len() < self.crc_range_end + 4 { return Err(DecodeError::UnexpectedEof); }
            let trailer = &self.data[self.crc_range_end .. self.crc_range_end + 4];
            let expected = u32::from_le_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]);
            let actual = crc32(&self.data[..self.crc_range_end]);
            if expected == actual { Ok(()) } else { Err(DecodeError::BadCrc) }
        }
    }

    // Convenience (host/testing): decode into a FlatProgram (alloc)
    pub fn decode_to_flat(data: &[u8]) -> Result<FlatProgram, DecodeError> {
        let mut rd = Reader::new(data)?;
        let mut out = FlatProgram::new();
        loop {
            let op = rd.read_u8()?;
            match op {
                OP_DELAY => { let ms = rd.read_varu32()?; if ms != 0 { out.ops.push(FlatOp::DelayMs(ms)); } }
                OP_TAP => {
                    let usage = rd.read_u8()?;
                    let mods = rd.read_u8()?;
                    out.ops.push(FlatOp::Tap { usage, mods });
                }
                OP_END => break,
                _ => return Err(DecodeError::BadOpcode),
            }
            if out.ops.len() > super::MAX_TOTAL_FLAT_OPS { return Err(DecodeError::BadOpcode); }
        }
        rd.verify_crc()?;
        Ok(out)
    }

    fn write_varu32(buf: &mut Vec<u8>, mut v: u32) {
        loop {
            let mut b = (v & 0x7F) as u8;
            v >>= 7;
            if v != 0 { b |= 0x80; }
            buf.push(b);
            if v == 0 { break; }
        }
    }

    // Small, table-free CRC32 (IEEE) for tiny code size.
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in bytes {
            crc ^= b as u32;
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB88320u32 & mask);
            }
        }
        !crc
    }
}

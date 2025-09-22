#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc;

#[cfg(feature = "std")]
use std::{vec::Vec, string::String, borrow::ToOwned, string::ToString, format};

#[cfg(not(feature = "std"))]
use alloc::{vec::Vec, string::String, borrow::ToOwned, string::ToString, format};

//
// -------- Public Limits --------
//
pub const MAX_DSL_LINES: usize = 256;
pub const MAX_DSL_DELAY_MS: u64 = 5000;
pub const MAX_TOTAL_FLAT_OPS: usize = 10_000;

// Internal caps for Phase 1 preprocessor (repeat/let) — not public API.
const MAX_REPEAT_N: u32 = 100;
const MAX_EXPANDED_LINES: usize = 4096;

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
    fn get<'a>(&self, id: &'a str) -> Option<&'a str>;
}

impl<F> ScriptProvider for F
where
    for<'a> F: Fn(&'a str) -> Option<&'a str>,
{
    fn get<'a>(&self, id: &'a str) -> Option<&'a str> { (self)(id) }
}

/// Compile `entry_dsl`, resolve & inline all `call`s using `provider`,
/// and return an **owned** program with no `Call` ops.
pub fn compile_and_link<'a>(
    entry_dsl: &'a str,
    provider: &impl ScriptProvider,
) -> Result<ProgramOwned, DslErrorAt> {
    // Preprocess entry script (repeat/let), then parse.
    let pre = preprocess(entry_dsl, &PreprocessOptions::default())
        .map_err(|e| DslErrorAt { kind: DslError::InvalidLine, line: e.line })?;
    let exists = |id: &str| provider.get(id).is_some();
    let ast = match compile_dsl_with_diag(&pre.text, exists) {
        Ok(p) => p,
        Err(e) => {
            let line = map_pre_line(e.line, &pre.sourcemap);
            return Err(DslErrorAt { kind: e.kind, line });
        }
    };
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
                // Preprocess callee independently (file-local scope), then parse and inline.
                let pre = preprocess(text, &PreprocessOptions::default())
                    .map_err(|e| DslErrorAt { kind: DslError::InvalidLine, line: e.line })?;
                let exists = |sid: &str| provider.get(sid).is_some();
                let sub = match compile_dsl_with_diag(&pre.text, exists) {
                    Ok(p) => p,
                    Err(e) => {
                        let line = map_pre_line(e.line, &pre.sourcemap);
                        return Err(DslErrorAt { kind: e.kind, line });
                    }
                };
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

// -------- Phase 1 Preprocessor (repeat, let, limited lints) --------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity { Warning, Error }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: &'static str,
    pub message: String,
    pub line: u16,
    pub col: u16,
    pub span_len: u16,
    pub suggestion: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrigLoc { pub line: u16, pub col: u16 }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreprocessOutput {
    pub text: String,
    pub sourcemap: Vec<OrigLoc>,
    pub diags: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreError { pub code: &'static str, pub message: String, pub line: u16, pub col: u16 }

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreprocessOptions { pub max_repeat_n: u32, pub max_expanded_lines: usize, pub near_cap_ratio: f32 }
impl Default for PreprocessOptions {
    fn default() -> Self { Self { max_repeat_n: MAX_REPEAT_N, max_expanded_lines: MAX_EXPANDED_LINES, near_cap_ratio: 0.9 } }
}

fn map_pre_line(pre_line: u16, sm: &[OrigLoc]) -> u16 {
    let idx = (pre_line as usize).saturating_sub(1);
    if idx < sm.len() { sm[idx].line } else { pre_line }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ConstVal { Str(String), Num(u64) }

#[derive(Debug, Default)]
struct LetEnv { items: Vec<(String, ConstVal)> }
impl LetEnv {
    fn get(&self, name: &str) -> Option<&ConstVal> { self.items.iter().rev().find(|(n, _)| n.as_str() == name).map(|(_, v)| v) }
    fn insert(&mut self, name: String, val: ConstVal) -> Result<(), ()> {
        if self.items.iter().any(|(n, _)| n == &name) { return Err(()); }
        self.items.push((name, val));
        Ok(())
    }
    fn clone_from_parent(parent: &LetEnv) -> Self { Self { items: parent.items.clone() } }
}

pub fn preprocess(src: &str, opts: &PreprocessOptions) -> Result<PreprocessOutput, PreError> {
    let mut out_lines: Vec<String> = Vec::new();
    let mut sm: Vec<OrigLoc> = Vec::new();
    let mut diags: Vec<Diagnostic> = Vec::new();
    let mut env = LetEnv::default();

    let mut lines: Vec<&str> = Vec::new();
    for l in src.split('\n') { lines.push(l); }

    let mut i = 0usize;
    while i < lines.len() {
        let raw = lines[i];
        let trimmed = raw.trim();
        let line_no = (i + 1) as u16;
        if trimmed.is_empty() || trimmed.starts_with('#') {
            // Skip non-semantic lines in output; mapping is for semantic lines only.
            i += 1;
            continue;
        }
        if let Some(rest) = starts_with_ci(trimmed, "let") {
            // let NAME = VALUE
            match parse_let(rest) {
                Ok((name, val)) => {
                    if !is_upper_name(&name) {
                        return Err(PreError { code: "LetInvalidName", message: format!("invalid constant name '{}': must be [A-Z_][A-Z0-9_]*", name), line: line_no, col: 1 });
                    }
                    if env.insert(name, val).is_err() {
                        return Err(PreError { code: "LetRedefinition", message: "constant already defined".into(), line: line_no, col: 1 });
                    }
                }
                Err(pe) => { return Err(PreError { code: pe.0, message: pe.1, line: line_no, col: 1 }); }
            }
            i += 1;
            continue;
        }
        if let Some(rest) = starts_with_ci(trimmed, "repeat") {
            // repeat N {  ...  }
            let (n, has_brace) = match parse_repeat_header(rest) {
                Ok((n, b)) => (n, b),
                Err((code, msg)) => { return Err(PreError { code, message: msg, line: line_no, col: 1 }); }
            };
            if !has_brace { return Err(PreError { code: "RepeatMissingBrace", message: "expected '{' after repeat N".into(), line: line_no, col: 1 }); }
            if n > opts.max_repeat_n { return Err(PreError { code: "RepeatNTooLarge", message: format!("repeat count {} exceeds cap {}", n, opts.max_repeat_n), line: line_no, col: 1 }); }
            // Collect body until matching single-line '}' with nesting on nested repeat headers.
            let mut body_start = i + 1;
            let mut depth: i32 = 1;
            let mut j = i + 1;
            while j < lines.len() {
                let t = lines[j].trim();
                if t.is_empty() || t.starts_with('#') { j += 1; continue; }
                if let Some(r2) = starts_with_ci(t, "repeat") {
                    if let Ok((_cn, has)) = parse_repeat_header(r2) { if has { depth += 1; } }
                } else if t == "}" {
                    depth -= 1;
                    if depth == 0 { break; }
                }
                j += 1;
            }
            if depth != 0 { return Err(PreError { code: "RepeatMissingBrace", message: "missing closing '}' for repeat block".into(), line: line_no, col: 1 }); }
            let body_end = j; // exclusive of '}'

            // Preprocess body once using a cloned environment (block-local additions do not leak out).
            let child_env = LetEnv::clone_from_parent(&env);
            let body = preprocess_block(&lines, body_start, body_end, opts, child_env)?;

            // Projected expansion cap
            if out_lines.len() + body.lines.len().saturating_mul(n as usize) > opts.max_expanded_lines {
                return Err(PreError { code: "RepeatExpansionTooLarge", message: "expanded lines exceed cap".into(), line: line_no, col: 1 });
            }
            for _ in 0..n { append_block(&body, &mut out_lines, &mut sm); }

            i = j + 1; // skip body and closing brace
            continue;
        }

        // Regular command line: perform targeted substitutions.
        match substitute_and_emit(trimmed, line_no, &env, &mut out_lines, &mut sm, &mut diags) {
            Ok(()) => { /* ok */ }
            Err(pe) => { return Err(PreError { code: pe.0, message: pe.1, line: line_no, col: 1 }); }
        }
        // Caps & basic lints
        if out_lines.len() > opts.max_expanded_lines { return Err(PreError { code: "ExpandedLinesTooLarge", message: "expanded lines exceed cap".into(), line: line_no, col: 1 }); }
        i += 1;
    }

    // Near-cap lint for lines vs MAX_DSL_LINES (non-empty logical lines)
    let approx_nonempty = out_lines.len();
    if approx_nonempty as f32 >= (MAX_DSL_LINES as f32 * opts.near_cap_ratio) {
        diags.push(Diagnostic { severity: Severity::Warning, code: "NearCapLines", message: format!("lines approaching cap: {} / {}", approx_nonempty, MAX_DSL_LINES), line: 0, col: 0, span_len: 0, suggestion: None });
    }

    Ok(PreprocessOutput { text: join_lines(&out_lines), sourcemap: sm, diags })
}

#[derive(Debug)]
struct PreBlock { lines: Vec<String>, map: Vec<OrigLoc> }

fn preprocess_block(lines: &Vec<&str>, start: usize, end: usize, opts: &PreprocessOptions, mut env: LetEnv) -> Result<PreBlock, PreError> {
    let mut out: Vec<String> = Vec::new();
    let mut sm: Vec<OrigLoc> = Vec::new();
    let mut diags: Vec<Diagnostic> = Vec::new();
    let mut i = start;
    while i < end {
        let raw = lines[i];
        let t = raw.trim();
        let line_no = (i + 1) as u16;
        if t.is_empty() || t.starts_with('#') { i += 1; continue; }
        if let Some(rest) = starts_with_ci(t, "let") {
            match parse_let(rest) {
                Ok((name, val)) => {
                    if !is_upper_name(&name) {
                        return Err(PreError { code: "LetInvalidName", message: format!("invalid constant name '{}': must be [A-Z_][A-Z0-9_]*", name), line: line_no, col: 1 });
                    }
                    if env.insert(name, val).is_err() {
                        return Err(PreError { code: "LetRedefinition", message: "constant already defined".into(), line: line_no, col: 1 });
                    }
                }
                Err(pe) => { return Err(PreError { code: pe.0, message: pe.1, line: line_no, col: 1 }); }
            }
            i += 1; continue;
        }
        if let Some(rest) = starts_with_ci(t, "repeat") {
            let (n, has) = match parse_repeat_header(rest) { Ok(v) => v, Err((c, m)) => return Err(PreError { code: c, message: m, line: line_no, col: 1 }) };
            if !has { return Err(PreError { code: "RepeatMissingBrace", message: "expected '{' after repeat N".into(), line: line_no, col: 1 }); }
            if n > opts.max_repeat_n { return Err(PreError { code: "RepeatNTooLarge", message: format!("repeat count {} exceeds cap {}", n, opts.max_repeat_n), line: line_no, col: 1 }); }
            // Find matching '}'
            let mut depth: i32 = 1; let mut j = i + 1; let body_start = i + 1;
            while j < end { let tt = lines[j].trim(); if tt.is_empty() || tt.starts_with('#') { j += 1; continue; }
                if let Some(r2) = starts_with_ci(tt, "repeat") { if let Ok((_cn, hb)) = parse_repeat_header(r2) { if hb { depth += 1; } } }
                else if tt == "}" { depth -= 1; if depth == 0 { break; } }
                j += 1; }
            if depth != 0 { return Err(PreError { code: "RepeatMissingBrace", message: "missing closing '}' for repeat block".into(), line: line_no, col: 1 }); }
            let body = preprocess_block(lines, body_start, j, opts, LetEnv::clone_from_parent(&env))?;
            if out.len() + body.lines.len().saturating_mul(n as usize) > opts.max_expanded_lines { return Err(PreError { code: "RepeatExpansionTooLarge", message: "expanded lines exceed cap".into(), line: line_no, col: 1 }); }
            for _ in 0..n { append_block(&body, &mut out, &mut sm); }
            i = j + 1; continue;
        }
        // Regular line
        match substitute_and_emit(t, line_no, &env, &mut out, &mut sm, &mut diags) {
            Ok(()) => {}
            Err(pe) => { return Err(PreError { code: pe.0, message: pe.1, line: line_no, col: 1 }); }
        }
        if out.len() > opts.max_expanded_lines { return Err(PreError { code: "ExpandedLinesTooLarge", message: "expanded lines exceed cap".into(), line: line_no, col: 1 }); }
        i += 1;
    }
    Ok(PreBlock { lines: out, map: sm })
}

fn append_block(b: &PreBlock, out_lines: &mut Vec<String>, sm: &mut Vec<OrigLoc>) {
    for (k, l) in b.lines.iter().enumerate() {
        out_lines.push(l.clone());
        sm.push(b.map.get(k).copied().unwrap_or(OrigLoc { line: 0, col: 0 }));
    }
}

fn join_lines(lines: &[String]) -> String {
    let mut out = String::new();
    for (i, l) in lines.iter().enumerate() {
        if i != 0 { out.push('\n'); }
        out.push_str(l);
    }
    out
}

fn is_upper_name(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() { return false; }
    let mut it = bytes.iter();
    let b0 = *it.next().unwrap();
    if !(b'A'..=b'Z').contains(&b0) && b0 != b'_' { return false; }
    for &b in it { if !(b'A'..=b'Z').contains(&b) && !(b'0'..=b'9').contains(&b) && b != b'_' { return false; } }
    true
}

fn starts_with_ci<'a>(line: &'a str, kw: &str) -> Option<&'a str> {
    let mut it = line.splitn(2, char::is_whitespace);
    let head = it.next()?;
    if head.eq_ignore_ascii_case(kw) { Some(it.next().unwrap_or("").trim_start()) } else { None }
}

fn parse_let(rest: &str) -> Result<(String, ConstVal), (&'static str, String)> {
    // rest: NAME = VALUE
    let mut parts = rest.splitn(2, '=');
    let lhs = parts.next().unwrap_or("").trim();
    let rhs = parts.next().ok_or(("LetMissingEquals", "expected '='".into()))?.trim();
    if lhs.is_empty() { return Err(("LetInvalidName", "missing name".into())); }
    if rhs.is_empty() { return Err(("LetMissingValue", "missing value".into())); }
    if rhs.starts_with('"') {
        // string literal: consume until closing unescaped '"'
        if !rhs.ends_with('"') || rhs.len() < 2 { return Err(("InvalidString", "unterminated string literal".into())); }
        let inner = &rhs[1..rhs.len()-1];
        let s = unescape_string(inner).map_err(|e| ("InvalidStringEscape", e))?;
        return Ok((lhs.to_string(), ConstVal::Str(s)));
    } else {
        // number
        let n = rhs.parse::<u64>().map_err(|_| ("LetValueNotNumber", "expected number".into()))?;
        return Ok((lhs.to_string(), ConstVal::Num(n)));
    }
}

fn unescape_string(s: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut iter = s.chars();
    while let Some(ch) = iter.next() {
        if ch == '\\' {
            match iter.next() {
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('t') => out.push('\t'),
                Some('n') => return Err("\\n not allowed in Phase 1".into()),
                Some(other) => return Err(format!("unsupported escape \\{}", other)),
                None => return Err("dangling escape".into()),
            }
        } else {
            out.push(ch);
        }
    }
    Ok(out)
}

fn parse_repeat_header(rest: &str) -> Result<(u32, bool), (&'static str, String)> {
    // rest: N {   (brace required on same line)
    let mut it = rest.trim().split_whitespace();
    let n_tok = it.next().ok_or(("RepeatMissingCount", "missing count".into()))?;
    let n = n_tok.parse::<u32>().map_err(|_| ("RepeatCountNotNumber", "repeat count must be a number".into()))?;
    // detect '{'
    let has_brace = rest.contains('{');
    Ok((n, has_brace))
}

fn substitute_and_emit(
    trimmed: &str,
    orig_line: u16,
    env: &LetEnv,
    out_lines: &mut Vec<String>,
    sm: &mut Vec<OrigLoc>,
    diags: &mut Vec<Diagnostic>,
) -> Result<(), (&'static str, String)> {
    // hold/release warnings
    if starts_with_ci(trimmed, "hold").is_some() || starts_with_ci(trimmed, "release").is_some() {
        diags.push(Diagnostic { severity: Severity::Warning, code: "HoldReleaseNotSupported", message: "hold/release require firmware update; try modtap/tap".into(), line: orig_line, col: 1, span_len: 0, suggestion: Some("Use 'modtap MOD+KEY' or 'tap KEY'".into()) });
    }

    // delay substitution
    if let Some(rest) = starts_with_ci(trimmed, "delay") {
        let tok = rest.trim();
        let val = if is_upper_name(tok) {
            match env.get(tok) {
                Some(ConstVal::Num(n)) => n.to_string(),
                Some(ConstVal::Str(_)) => return Err(("LetTypeMismatch", "delay expects a number".into())),
                None => return Err(("LetUndefined", format!("undefined constant '{}'", tok))),
            }
        } else {
            tok.to_string()
        };
        // Lints: delay 0 and adjacent delays — detect here (adjacent check via last emitted)
        if val.trim() == "0" {
            diags.push(Diagnostic { severity: Severity::Warning, code: "UselessDelayZero", message: "delay 0 is a no-op".into(), line: orig_line, col: 1, span_len: 0, suggestion: Some("Remove this line".into()) });
        }
        let prev_is_delay = out_lines.last().map(|l| l.trim_start().to_ascii_lowercase().starts_with("delay ")).unwrap_or(false);
        if prev_is_delay { diags.push(Diagnostic { severity: Severity::Warning, code: "AdjacentDelays", message: "adjacent delays will be coalesced".into(), line: orig_line, col: 1, span_len: 0, suggestion: Some("Combine into one delay".into()) }); }
        out_lines.push(format!("delay {}", val));
        sm.push(OrigLoc { line: orig_line, col: 1 });
        return Ok(());
    }

    // text substitution
    if let Some(rest) = starts_with_ci(trimmed, "text") {
        let rest = rest.trim();
        if rest.is_empty() { return Err(("TextEmpty", "missing text".into())); }
        // Split last whitespace to detect optional delay token
        let mut text_part = rest;
        let mut delay_part = "";
        if let Some(idx) = rest.rfind(char::is_whitespace) {
            let (lhs, rhs) = rest.split_at(idx);
            let maybe = rhs.trim();
            if !maybe.is_empty() { delay_part = maybe; text_part = lhs.trim_end(); }
        }
        let mut new_text: String = String::new();
        // First token may be a NAME for substitution
        if let Some((first, tail)) = split2(text_part) {
            if is_upper_name(first) {
                match env.get(first) {
                    Some(ConstVal::Str(s)) => { new_text.push_str(s); text_part = tail.trim_start(); }
                    Some(ConstVal::Num(_)) => { return Err(("LetTypeMismatch", "text expects a string".into())); }
                    None => { return Err(("LetUndefined", format!("undefined constant '{}'", first))); }
                }
            } else {
                new_text.push_str(text_part);
                text_part = ""; // consumed as literal
            }
        } else if is_upper_name(text_part) {
            match env.get(text_part) {
                Some(ConstVal::Str(s)) => { new_text.push_str(s); text_part = ""; }
                Some(ConstVal::Num(_)) => { return Err(("LetTypeMismatch", "text expects a string".into())); }
                None => { return Err(("LetUndefined", format!("undefined constant '{}'", text_part))); }
            }
        } else {
            new_text.push_str(text_part);
            text_part = "";
        }
        if !text_part.is_empty() {
            if !new_text.is_empty() { new_text.push(' '); }
            new_text.push_str(text_part);
        }
        // Substitute trailing delay if it's a NAME
        let mut out_line = String::new(); out_line.push_str("text "); out_line.push_str(&new_text);
        if !delay_part.is_empty() {
            let dval = if is_upper_name(delay_part) {
                match env.get(delay_part) {
                    Some(ConstVal::Num(n)) => n.to_string(),
                    Some(ConstVal::Str(_)) => return Err(("LetTypeMismatch", "text delay expects a number".into())),
                    None => return Err(("LetUndefined", format!("undefined constant '{}'", delay_part))),
                }
            } else { delay_part.to_string() };
            out_line.push(' '); out_line.push_str(&dval);
        }
        out_lines.push(out_line);
        sm.push(OrigLoc { line: orig_line, col: 1 });
        return Ok(());
    }

    // Pass-through other commands unchanged (tap/modtap/call/etc.).
    out_lines.push(trimmed.to_string());
    sm.push(OrigLoc { line: orig_line, col: 1 });
    Ok(())
}

fn split2(s: &str) -> Option<(&str, &str)> {
    let mut it = s.splitn(2, char::is_whitespace);
    let a = it.next()?;
    let b = it.next()?;
    Some((a, b))
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

// -------- Tests (Phase 1 preprocessor) --------

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;

    #[test]
    fn repeat_unroll_simple() {
        let entry = "repeat 3 {\n  tap A\n}";
        let provider = |_id: &str| -> Option<&str> { None };
        let owned = compile_and_link(entry, &provider).expect("compile_and_link");
        let flat = lower_to_flat_us(&owned).expect("lower_to_flat_us");
        let taps = flat.ops.iter().filter(|op| matches!(op, FlatOp::Tap { .. })).count();
        assert_eq!(taps, 3);
    }

    #[test]
    fn repeat_unroll_nested() {
        let entry = "repeat 2 {\n  repeat 2 {\n    tap A\n  }\n}";
        let provider = |_id: &str| -> Option<&str> { None };
        let owned = compile_and_link(entry, &provider).expect("compile_and_link");
        let flat = lower_to_flat_us(&owned).expect("lower_to_flat_us");
        let taps = flat.ops.iter().filter(|op| matches!(op, FlatOp::Tap { .. })).count();
        assert_eq!(taps, 4);
    }

    #[test]
    fn let_number_in_delay() {
        let entry = "let D = 150\n delay D";
        let provider = |_id: &str| -> Option<&str> { None };
        let owned = compile_and_link(entry, &provider).expect("compile_and_link");
        assert!(matches!(owned.ops.as_slice(), [OpOwned::DelayMs(150)]));
    }

    #[test]
    fn let_string_in_text() {
        let entry = "let S = \"Hi\"\n text S 5";
        let provider = |_id: &str| -> Option<&str> { None };
        let owned = compile_and_link(entry, &provider).expect("compile_and_link");
        match owned.ops.as_slice() {
            [OpOwned::Text { s, delay_ms }] => { assert_eq!(s, "Hi"); assert_eq!(*delay_ms, 5); }
            other => panic!("unexpected ops: {:?}", other),
        }
    }

    #[test]
    fn repeat_missing_brace_errors() {
        let entry = "repeat 2 {\n tap A\n"; // missing closing brace
        let provider = |_id: &str| -> Option<&str> { None };
        let err = compile_and_link(entry, &provider).unwrap_err();
        assert_eq!(err.line, 1); // header line
        assert!(matches!(err.kind, DslError::InvalidLine));
    }

    #[test]
    fn let_redefinition_errors() {
        let entry = "let A = 1\nlet A = 2\n tap A";
        let provider = |_id: &str| -> Option<&str> { None };
        let err = compile_and_link(entry, &provider).unwrap_err();
        assert_eq!(err.line, 2);
        assert!(matches!(err.kind, DslError::InvalidLine));
    }

    #[test]
    fn repeat_expansion_cap_errors() {
        // 100 * 100 * 1 line = 10_000 > MAX_EXPANDED_LINES (4096)
        let entry = "repeat 100 {\n  repeat 100 {\n    tap A\n  }\n}";
        let provider = |_id: &str| -> Option<&str> { None };
        let err = compile_and_link(entry, &provider).unwrap_err();
        // Error reported at the outer repeat header
        assert_eq!(err.line, 1);
        assert!(matches!(err.kind, DslError::InvalidLine));
    }

    #[test]
    fn sourcemap_maps_error_inside_repeat_body() {
        // 'zzz' is an unknown command on line 2; it repeats but we expect
        // the first error to map back to original line 2.
        let entry = "repeat 2 {\n  zzz\n}";
        let provider = |_id: &str| -> Option<&str> { None };
        let err = compile_and_link(entry, &provider).unwrap_err();
        assert_eq!(err.line, 2);
        assert!(matches!(err.kind, DslError::UnknownCommand));
    }

    #[test]
    fn nested_sourcemap_deep_error() {
        // Error occurs inside inner repeat body at original line 3
        let entry = "repeat 2 {\n  repeat 3 {\n    zzz\n  }\n}";
        let provider = |_id: &str| -> Option<&str> { None };
        let err = compile_and_link(entry, &provider).unwrap_err();
        assert_eq!(err.line, 3);
        assert!(matches!(err.kind, DslError::UnknownCommand));
    }

    #[test]
    fn redefinition_inside_nested_block_errors() {
        let entry = "let A = \"X\"\nrepeat 2 {\n  let A = \"Y\"\n  tap A\n}";
        let provider = |_id: &str| -> Option<&str> { None };
        let err = compile_and_link(entry, &provider).unwrap_err();
        // Error should point to the nested 'let A = "Y"' at original line 3
        assert_eq!(err.line, 3);
        assert!(matches!(err.kind, DslError::InvalidLine));
    }

    #[test]
    fn cross_script_error_maps_to_callee_line() {
        let entry = "call sub";
        let sub = "# sub\nzzz\n"; // error at line 2
        let provider = move |id: &str| -> Option<&str> { if id == "sub" { Some(sub) } else { None } };
        let err = compile_and_link(entry, &provider).unwrap_err();
        assert_eq!(err.line, 2);
        assert!(matches!(err.kind, DslError::UnknownCommand));
    }

    #[test]
    fn nested_cross_script_error_maps_deep() {
        let entry = "call A";
        let a = "call B";
        let b = "zzz"; // error at line 1 in B
        let provider = move |id: &str| -> Option<&str> {
            match id { "A" => Some(a), "B" => Some(b), _ => None }
        };
        let err = compile_and_link(entry, &provider).unwrap_err();
        assert_eq!(err.line, 1);
        assert!(matches!(err.kind, DslError::UnknownCommand));
    }
}

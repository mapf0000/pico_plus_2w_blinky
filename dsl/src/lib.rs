#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc;

#[cfg(feature = "std")]
use std::vec::Vec;
#[cfg(feature = "std")]
use std::string::String;

#[cfg(not(feature = "std"))]
use heapless as _; // ensure dependency present when no_std

#[cfg(feature = "ui")]
use serde::Serialize;

// Public DSL limits for consistency across targets
pub const MAX_DSL_LINES: usize = 256;
pub const MAX_DSL_DELAY_MS: u64 = 5000;
pub const MAX_OWNED_PROGRAMS: usize = 8; // used by executor call stack
pub const MAX_CALL_STACK_FRAMES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ui", derive(Serialize))]
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

/// A platform-agnostic representation of keys and modifiers.
/// Values follow HID Usage Tables (page 0x07) to make integration trivial.
#[cfg_attr(feature = "ui", derive(Serialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyTap {
    pub usage: u8,
    pub mods: u8,
}

#[cfg_attr(feature = "ui", derive(Serialize))]
#[derive(Debug, Clone, PartialEq)]
pub enum Op<'a> {
    Tap(KeyTap),
    DelayMs(u32),
    Text { s: &'a str, delay_ms: u16 },
    Call { id: &'a str },
}

#[cfg(feature = "std")]
type ProgVec<'a> = Vec<Op<'a>>;
#[cfg(not(feature = "std"))]
type ProgVec<'a> = heapless::Vec<Op<'a>, 256>;

#[cfg_attr(feature = "ui", derive(Serialize))]
#[derive(Debug, Clone, PartialEq)]
pub struct Program<'a> { pub ops: ProgVec<'a> }

impl<'a> Program<'a> {
    pub fn new() -> Self {
        #[cfg(feature = "std")] { Self { ops: Vec::new() } }
        #[cfg(not(feature = "std"))] { Self { ops: ProgVec::new() } }
    }
}

#[cfg(feature = "std")]
fn try_push<'a>(ops: &mut ProgVec<'a>, op: Op<'a>) -> Result<(), DslError> { ops.push(op); Ok(()) }
#[cfg(not(feature = "std"))]
fn try_push<'a>(ops: &mut ProgVec<'a>, op: Op<'a>) -> Result<(), DslError> { ops.push(op).map_err(|_| DslError::TooManyLines) }

/// Compile the DSL into a platform-agnostic program.
///
/// Note: `call` targets are not validated here; if needed, use the
/// `compile_dsl_with` variant with a `ScriptExists` callback.
pub fn compile_dsl<'a>(dsl: &'a str) -> Result<Program<'a>, DslError> {
    struct AcceptAll;
    impl ScriptExists for AcceptAll { fn exists(&self, _id: &str) -> bool { true } }
    compile_dsl_with(dsl, AcceptAll)
}

/// Callback trait to validate script existence when compiling.
pub trait ScriptExists {
    fn exists(&self, id: &str) -> bool;
}

impl<F> ScriptExists for F
where
    F: for<'a> Fn(&'a str) -> bool,
{
    fn exists(&self, id: &str) -> bool { self(id) }
}

pub fn compile_dsl_with<'a>(dsl: &'a str, exists: impl ScriptExists) -> Result<Program<'a>, DslError> {
    let mut prog = Program::new();
    let mut count = 0usize;
    for raw in dsl.lines() {
        if count >= MAX_DSL_LINES { return Err(DslError::TooManyLines); }
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        count += 1;

        let (cmd, rest) = split_head(line).ok_or(DslError::InvalidLine)?;
        if eq_ci(cmd, "tap") {
            let key_name = rest.trim();
            if key_name.is_empty() { return Err(DslError::InvalidLine); }
            let usage = parse_key(key_name).ok_or(DslError::ParseKey)?;
            try_push(&mut prog.ops, Op::Tap(KeyTap { usage, mods: 0 }))?;
        } else if eq_ci(cmd, "modtap") {
            let arg = rest.trim();
            if arg.is_empty() { return Err(DslError::InvalidLine); }
            let (mods, usage) = parse_modtap(arg)?;
            try_push(&mut prog.ops, Op::Tap(KeyTap { usage, mods }))?;
        } else if eq_ci(cmd, "delay") {
            let ms: u64 = rest.trim().parse::<u64>().map_err(|_| DslError::ParseDelay)?;
            let ms = core::cmp::min(ms, MAX_DSL_DELAY_MS) as u32;
            if ms != 0 { try_push(&mut prog.ops, Op::DelayMs(ms))?; }
        } else if eq_ci(cmd, "text") {
            let (text, delay_ms) = parse_text_args(rest)?;
            let delay_ms = core::cmp::min(delay_ms as u64, MAX_DSL_DELAY_MS) as u16;
            if !text.is_empty() { try_push(&mut prog.ops, Op::Text { s: text, delay_ms })?; }
        } else if eq_ci(cmd, "call") {
            let id = rest.trim();
            if id.is_empty() { return Err(DslError::InvalidLine); }
            if !exists.exists(id) { return Err(DslError::UnknownScript); }
            try_push(&mut prog.ops, Op::Call { id })?;
        } else {
            return Err(DslError::UnknownCommand);
        }
    }
    Ok(prog)
}

fn split_head(s: &str) -> Option<(&str, &str)> {
    let mut it = s.splitn(2, char::is_whitespace);
    let head = it.next()?;
    let tail = it.next().unwrap_or("");
    Some((head, tail))
}

fn eq_ci(a: &str, b: &str) -> bool { a.eq_ignore_ascii_case(b) }

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

fn upper_ascii<const N: usize>(s: &str) -> arrayvec::ArrayString<N> {
    // Use arrayvec to avoid heap here; fine for small temps even with std.
    let mut out: arrayvec::ArrayString<N> = arrayvec::ArrayString::new();
    for b in s.bytes() {
        let up = if b'a' <= b && b <= b'z' { b - 32 } else { b };
        let _ = out.push(up as char);
    }
    out
}

// Minimal internal dep: arrayvec for uppercasing temporary buffers.
// We embed it here to keep `no_std` friendly behavior with no allocator.
mod arrayvec {
    #[derive(Debug, Clone)]
    pub struct ArrayString<const N: usize> { buf: [u8; N], len: usize }
    impl<const N: usize> ArrayString<N> {
        pub fn new() -> Self { Self { buf: [0; N], len: 0 } }
        pub fn push(&mut self, ch: char) -> Result<(), ()> {
            if self.len < N { self.buf[self.len] = ch as u8; self.len += 1; Ok(()) } else { Err(()) }
        }
        pub fn as_str(&self) -> &str { core::str::from_utf8(&self.buf[..self.len]).unwrap_or("") }
    }
}

// --- HID constants (subset of src/keyboard.rs, stable values) ---
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

    // Single letter a-z
    if u.len() == 1 {
        let b = u.as_bytes()[0];
        if b'A' <= b && b <= b'Z' { return Some(KEY_A + (b - b'A')); }
    }

    // Function keys
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

    // KP_*
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

    // Number row
    if u.len() == 1 {
        let b = u.as_bytes()[0];
        if b'1' <= b && b <= b'9' { return Some(KEY_1 + (b - b'1')); }
        if b == b'0' { return Some(KEY_0); }
    }

    Some(match u {
        // Common control
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

        // Punctuation cluster by name
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

        // Non-US backslash for ISO layouts
        "NON_US_BACKSLASH" => KEY_NON_US_BACKSLASH,

        // Number lock
        "NUM_LOCK" => KEY_NUM_LOCK,

        _ => return None,
    })
}

// --- Optional Pi/firmware executor (feature = "pi") ---
#[cfg(feature = "pi")]
pub mod pi_exec {
    use super::*;
    use embassy_time::Timer;
    use embassy_usb::class::hid::HidWriter as UsbHidWriter;
    use usbd_hid::descriptor::KeyboardReport;

    const DELAY_MOD_DOWN_MS: u64 = 8;
    const DELAY_TAP_HOLD_MS: u64 = 20;

    pub trait ScriptText {
        fn get(&mut self, id: &str) -> Option<&'static str>;
    }

    impl<F> ScriptText for F where F: FnMut(&str) -> Option<&'static str> {
        fn get(&mut self, id: &str) -> Option<&'static str> { (self)(id) }
    }

    pub async fn tap_with_mod<'d, D>(w: &mut UsbHidWriter<'d, D, 8>, usage: u8, modifier: u8)
    where
        D: embassy_usb::driver::Driver<'d>,
    {
        if modifier != 0 {
            let mod_down = KeyboardReport { keycodes: [0, 0, 0, 0, 0, 0], leds: 0, modifier, reserved: 0 };
            let _ = w.write_serialize(&mod_down).await; Timer::after_millis(DELAY_MOD_DOWN_MS).await;
            let both_down = KeyboardReport { keycodes: [usage, 0, 0, 0, 0, 0], leds: 0, modifier, reserved: 0 };
            let _ = w.write_serialize(&both_down).await; Timer::after_millis(DELAY_TAP_HOLD_MS).await;
            let key_up = KeyboardReport { keycodes: [0, 0, 0, 0, 0, 0], leds: 0, modifier, reserved: 0 };
            let _ = w.write_serialize(&key_up).await; Timer::after_millis(DELAY_MOD_DOWN_MS).await;
            let mod_up = KeyboardReport { keycodes: [0, 0, 0, 0, 0, 0], leds: 0, modifier: 0, reserved: 0 };
            let _ = w.write_serialize(&mod_up).await;
        } else {
            let press = KeyboardReport { keycodes: [usage, 0, 0, 0, 0, 0], leds: 0, modifier: 0, reserved: 0 };
            let release = KeyboardReport { keycodes: [0, 0, 0, 0, 0, 0], leds: 0, modifier: 0, reserved: 0 };
            let _ = w.write_serialize(&press).await; Timer::after_millis(DELAY_TAP_HOLD_MS).await;
            let _ = w.write_serialize(&release).await;
        }
    }

    pub fn char_to_key(c: char) -> Option<(u8, u8)> {
        match c {
            '\n' | '\r' => Some((KEY_ENTER, 0)),
            '\t' => Some((KEY_TAB, 0)),
            ' ' => Some((KEY_SPACE, 0)),
            'a'..='z' => { let idx = (c as u8) - b'a'; Some((KEY_A + idx, 0)) }
            'A'..='Z' => { let idx = (c as u8) - b'A'; Some((KEY_A + idx, MOD_LSHIFT)) }
            '1' => Some((KEY_1, 0)), '2' => Some((KEY_2, 0)), '3' => Some((KEY_3, 0)), '4' => Some((KEY_4, 0)), '5' => Some((KEY_5, 0)),
            '6' => Some((KEY_6, 0)), '7' => Some((KEY_7, 0)), '8' => Some((KEY_8, 0)), '9' => Some((KEY_9, 0)), '0' => Some((KEY_0, 0)),
            '!' => Some((KEY_1, MOD_LSHIFT)), '@' => Some((KEY_2, MOD_LSHIFT)), '#' => Some((KEY_3, MOD_LSHIFT)), '$' => Some((KEY_4, MOD_LSHIFT)),
            '%' => Some((KEY_5, MOD_LSHIFT)), '^' => Some((KEY_6, MOD_LSHIFT)), '&' => Some((KEY_7, MOD_LSHIFT)), '*' => Some((KEY_8, MOD_LSHIFT)),
            '(' => Some((KEY_9, MOD_LSHIFT)), ')' => Some((KEY_0, MOD_LSHIFT)),
            '-' => Some((KEY_MINUS, 0)), '_' => Some((KEY_MINUS, MOD_LSHIFT)), '=' => Some((KEY_EQUAL, 0)), '+' => Some((KEY_EQUAL, MOD_LSHIFT)),
            '[' => Some((KEY_LEFT_BRACKET, 0)), '{' => Some((KEY_LEFT_BRACKET, MOD_LSHIFT)), ']' => Some((KEY_RIGHT_BRACKET, 0)), '}' => Some((KEY_RIGHT_BRACKET, MOD_LSHIFT)),
            '\\' => Some((KEY_BACKSLASH, 0)), '|' => Some((KEY_BACKSLASH, MOD_LSHIFT)), ';' => Some((KEY_SEMICOLON, 0)), ':' => Some((KEY_SEMICOLON, MOD_LSHIFT)),
            '\'' => Some((KEY_APOSTROPHE, 0)), '"' => Some((KEY_APOSTROPHE, MOD_LSHIFT)), '`' => Some((KEY_GRAVE, 0)), '~' => Some((KEY_GRAVE, MOD_LSHIFT)),
            ',' => Some((KEY_COMMA, 0)), '<' => Some((KEY_COMMA, MOD_LSHIFT)), '.' => Some((KEY_DOT, 0)), '>' => Some((KEY_DOT, MOD_LSHIFT)), '/' => Some((KEY_SLASH, 0)), '?' => Some((KEY_SLASH, MOD_LSHIFT)),
            _ => None,
        }
    }

    pub async fn type_str<'d, D>(w: &mut UsbHidWriter<'d, D, 8>, s: &str, delay_ms: u64)
    where
        D: embassy_usb::driver::Driver<'d>,
    {
        for ch in s.chars() {
            if let Some((usage, modifier)) = char_to_key(ch) {
                tap_with_mod(w, usage, modifier).await;
                if delay_ms != 0 { Timer::after_millis(delay_ms).await; }
            } else {
                #[cfg(feature = "pi")]
                log::debug!("hid: skipping unsupported char: {:?}", ch);
            }
        }
    }

    /// Execute a program against USB HID writer.
    pub async fn exec_program<'d, 'a, D, L>(w: &mut UsbHidWriter<'d, D, 8>, prog: &Program<'a>, mut lookup: L) -> Result<(), DslError>
    where
        D: embassy_usb::driver::Driver<'d>,
        L: ScriptText,
    {
        #[derive(Copy, Clone)] enum Ctx { Top, Owned(usize) }
        struct Frame { ctx: Ctx, ip: usize }

        let mut owned: heapless::Vec<Program<'static>, { MAX_OWNED_PROGRAMS }> = heapless::Vec::new();
        let mut stack: heapless::Vec<Frame, { MAX_CALL_STACK_FRAMES }> = heapless::Vec::new();

        let mut ctx = Ctx::Top;
        let mut ip: usize = 0;
        loop {
            match ctx {
                Ctx::Top => {
                    if ip >= prog.ops.len() {
                        match stack.pop() { Some(Frame { ctx: pctx, ip: pip }) => { ctx = pctx; ip = pip; continue; }, None => break }
                    }
                    let op = &prog.ops[ip]; ip += 1;
                    match *op {
                        Op::Tap(KeyTap { usage, mods }) => { tap_with_mod(w, usage, mods).await; }
                        Op::DelayMs(ms) => { if ms != 0 { Timer::after_millis(ms as u64).await; } }
                        Op::Text { s, delay_ms } => { type_str(w, s, delay_ms as u64).await; }
                        Op::Call { id } => {
                            let dsl = lookup.get(id).ok_or(DslError::UnknownScript)?;
                            let sub = compile_dsl(dsl)?;
                            let ix = owned.len();
                            if owned.push(sub).is_err() || stack.push(Frame { ctx, ip }).is_err() { return Err(DslError::RecursionTooDeep); }
                            ctx = Ctx::Owned(ix); ip = 0;
                        }
                    }
                }
                Ctx::Owned(ix) => {
                    if ip >= owned[ix].ops.len() {
                        match stack.pop() { Some(Frame { ctx: pctx, ip: pip }) => { ctx = pctx; ip = pip; continue; }, None => break }
                    }
                    let op = &owned[ix].ops[ip]; ip += 1;
                    match *op {
                        Op::Tap(KeyTap { usage, mods }) => { tap_with_mod(w, usage, mods).await; }
                        Op::DelayMs(ms) => { if ms != 0 { Timer::after_millis(ms as u64).await; } }
                        Op::Text { s, delay_ms } => { type_str(w, s, delay_ms as u64).await; }
                        Op::Call { id } => {
                            let dsl = lookup.get(id).ok_or(DslError::UnknownScript)?;
                            let sub = compile_dsl(dsl)?;
                            let ix2 = owned.len();
                            if owned.push(sub).is_err() || stack.push(Frame { ctx, ip }).is_err() { return Err(DslError::RecursionTooDeep); }
                            ctx = Ctx::Owned(ix2); ip = 0;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Convenience wrapper: compile and execute DSL text.
    pub async fn run_dsl<'d, D, L>(w: &mut UsbHidWriter<'d, D, 8>, dsl: &str, lookup: L) -> Result<(), DslError>
    where
        D: embassy_usb::driver::Driver<'d>,
        L: ScriptText,
    {
        let prog = compile_dsl(dsl)?;
        exec_program(w, &prog, lookup).await
    }
}

// --- Tests (host/std) ---
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_text_args_variants() {
        assert_eq!(parse_text_args("Hello 25").unwrap(), ("Hello", 25));
        assert_eq!(parse_text_args("Hello").unwrap(), ("Hello", 10));
        assert_eq!(parse_text_args("Hello   ").unwrap(), ("Hello", 10));
        assert_eq!(parse_text_args("Hello 20 30").unwrap(), ("Hello 20", 30));
        assert!(matches!(parse_text_args(""), Err(DslError::TextEmpty)));
    }

    #[test]
    fn compile_simple_program() {
        let src = "tap A\nmodtap LCTRL+LALT+DELETE\ntext Hello 25\ndelay 0\ncall hello_world\n";
        let prog = compile_dsl_with(src, |id| id == "hello_world").expect("compile ok");
        let mut it = prog.ops.iter();
        match it.next() { Some(Op::Tap(KeyTap { usage, mods })) => { assert_eq!((*usage, *mods), (KEY_A, 0)); }, _ => panic!() }
        match it.next() { Some(Op::Tap(KeyTap { usage, mods })) => { assert_eq!((*usage, *mods), (KEY_DELETE, MOD_LCTRL | MOD_LALT)); }, _ => panic!() }
        match it.next() { Some(Op::Text { s, delay_ms }) => { assert_eq!((*s, *delay_ms), ("Hello", 25)); }, _ => panic!() }
        match it.next() { Some(Op::Call { id }) => { assert_eq!(*id, "hello_world"); }, _ => panic!() }
        assert!(it.next().is_none());
    }

    #[test]
    fn compile_unknowns_and_limits() {
        match compile_dsl("noop X\n") { Err(DslError::UnknownCommand) => {}, _ => panic!() }
        match compile_dsl_with("call not_a_script\n", |_| false) { Err(DslError::UnknownScript) => {}, _ => panic!() }
        let mut s = String::new();
        for _ in 0..(MAX_DSL_LINES) { s.push_str("tap A\n"); }
        s.push_str("tap A\n");
        match compile_dsl(&s) { Err(DslError::TooManyLines) => {}, _ => panic!() }
    }
}

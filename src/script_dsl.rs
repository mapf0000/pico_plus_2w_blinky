use embassy_time::Timer;
use embassy_usb::class::hid::HidWriter as UsbHidWriter;

use crate::scripts;
use crate::usb::keyboard;

// Maintainability: centralized DSL limits
const MAX_DSL_LINES: usize = 256;
const MAX_DSL_DELAY_MS: u64 = 5000;
const MAX_OWNED_PROGRAMS: usize = 8;
const MAX_CALL_STACK_FRAMES: usize = 8;

#[derive(Debug)]
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

/// Intermediate representation for a compiled DSL program.
pub enum Op<'a> {
    Tap { key: u8, mods: u8 },
    DelayMs(u32),
    Text { s: &'a str, delay_ms: u16 },
    Call { id: &'a str },
}

pub struct Program<'a> {
    pub ops: heapless::Vec<Op<'a>, 256>,
}

impl<'a> Program<'a> {
    pub const fn new() -> Self {
        Self {
            ops: heapless::Vec::new(),
        }
    }
}

/// Compile the full DSL into a `Program` (no side effects).
pub fn compile_dsl<'a>(dsl: &'a str) -> Result<Program<'a>, DslError> {
    let mut prog = Program::new();
    let mut count = 0usize;
    for raw in dsl.lines() {
        if count >= MAX_DSL_LINES {
            return Err(DslError::TooManyLines);
        }
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        count += 1;

        let (cmd, rest) = split_head(line).ok_or(DslError::InvalidLine)?;
        if eq_ci(cmd, "tap") {
            let key_name = rest.trim();
            if key_name.is_empty() {
                return Err(DslError::InvalidLine);
            }
            let key = parse_key(key_name).ok_or(DslError::ParseKey)?;
            prog.ops
                .push(Op::Tap { key, mods: 0 })
                .map_err(|_| DslError::TooManyLines)?;
        } else if eq_ci(cmd, "modtap") {
            let arg = rest.trim();
            if arg.is_empty() {
                return Err(DslError::InvalidLine);
            }
            let (mods, key) = parse_modtap(arg)?;
            prog.ops
                .push(Op::Tap { key, mods })
                .map_err(|_| DslError::TooManyLines)?;
        } else if eq_ci(cmd, "delay") {
            let ms: u64 = rest
                .trim()
                .parse::<u64>()
                .map_err(|_| DslError::ParseDelay)?;
            let ms = core::cmp::min(ms, MAX_DSL_DELAY_MS) as u32;
            if ms != 0 {
                prog.ops
                    .push(Op::DelayMs(ms))
                    .map_err(|_| DslError::TooManyLines)?;
            }
        } else if eq_ci(cmd, "text") {
            let r = rest;
            let (text, delay_ms) = parse_text_args(r)?;
            let delay_ms = core::cmp::min(delay_ms as u64, MAX_DSL_DELAY_MS) as u16;
            if !text.is_empty() {
                prog.ops
                    .push(Op::Text { s: text, delay_ms })
                    .map_err(|_| DslError::TooManyLines)?;
            }
        } else if eq_ci(cmd, "call") {
            let id = rest.trim();
            if id.is_empty() {
                return Err(DslError::InvalidLine);
            }
            // Validate at compile time that the target exists.
            if scripts::parse_id(id).is_none() {
                return Err(DslError::UnknownScript);
            }
            prog.ops
                .push(Op::Call { id })
                .map_err(|_| DslError::TooManyLines)?;
        } else {
            return Err(DslError::UnknownCommand);
        }
    }
    Ok(prog)
}

// Public wrapper removed (unused); call `run_dsl` or `exec_program_inner` via
// module-local helpers instead.

/// Internal executor using an explicit call stack to avoid async recursion.
async fn exec_program_inner<'d, 'a, D>(
    w: &mut UsbHidWriter<'d, D, 8>,
    prog: &Program<'a>,
    _depth: u8,
) -> Result<(), DslError>
where
    D: embassy_usb::driver::Driver<'d>,
{
    // We implement an explicit call stack so we can support `call <id>` without
    // recursive async functions (which are not allowed without boxing).
    // Stack stores return addresses and which program to resume (Top or an Owned compiled one).
    #[derive(Copy, Clone)]
    enum Ctx {
        Top,
        Owned(usize),
    }

    struct Frame {
        ctx: Ctx,
        ip: usize,
    }

    // Owned compiled programs for called builtins (their DSL is 'static).
    let mut owned: heapless::Vec<Program<'static>, MAX_OWNED_PROGRAMS> = heapless::Vec::new();
    let mut stack: heapless::Vec<Frame, MAX_CALL_STACK_FRAMES> = heapless::Vec::new();

    let mut ctx = Ctx::Top;
    let mut ip: usize = 0;

    loop {
        match ctx {
            Ctx::Top => {
                if ip >= prog.ops.len() {
                    match stack.pop() {
                        Some(Frame {
                            ctx: prev_ctx,
                            ip: prev_ip,
                        }) => {
                            ctx = prev_ctx;
                            ip = prev_ip;
                            continue;
                        }
                        None => break,
                    }
                }
                let op = &prog.ops[ip];
                ip += 1;
                match *op {
                    Op::Tap { key, mods } => {
                        keyboard::tap_with_mod(w, key, mods).await;
                    }
                    Op::DelayMs(ms) => {
                        if ms != 0 {
                            Timer::after_millis(ms as u64).await;
                        }
                    }
                    Op::Text { s, delay_ms } => {
                        keyboard::type_str(w, s, delay_ms as u64).await;
                    }
                    Op::Call { id } => {
                        let dsl = match scripts::dsl_for_id_str(id) {
                            Some(s) => s,
                            None => return Err(DslError::UnknownScript),
                        };
                        let sub = compile_dsl(dsl)?;
                        let ix = owned.len();
                        if owned.push(sub).is_err() || stack.push(Frame { ctx, ip }).is_err() {
                            return Err(DslError::RecursionTooDeep);
                        }
                        ctx = Ctx::Owned(ix);
                        ip = 0;
                    }
                }
            }
            Ctx::Owned(ix) => {
                if ip >= owned[ix].ops.len() {
                    match stack.pop() {
                        Some(Frame {
                            ctx: prev_ctx,
                            ip: prev_ip,
                        }) => {
                            ctx = prev_ctx;
                            ip = prev_ip;
                            continue;
                        }
                        None => break,
                    }
                }
                let op = &owned[ix].ops[ip];
                ip += 1;
                match *op {
                    Op::Tap { key, mods } => {
                        keyboard::tap_with_mod(w, key, mods).await;
                    }
                    Op::DelayMs(ms) => {
                        if ms != 0 {
                            Timer::after_millis(ms as u64).await;
                        }
                    }
                    Op::Text { s, delay_ms } => {
                        keyboard::type_str(w, s, delay_ms as u64).await;
                    }
                    Op::Call { id } => {
                        let dsl = match scripts::dsl_for_id_str(id) {
                            Some(s) => s,
                            None => return Err(DslError::UnknownScript),
                        };
                        let sub = compile_dsl(dsl)?;
                        let ix2 = owned.len();
                        if owned.push(sub).is_err() || stack.push(Frame { ctx, ip }).is_err() {
                            return Err(DslError::RecursionTooDeep);
                        }
                        ctx = Ctx::Owned(ix2);
                        ip = 0;
                    }
                }
            }
        }
    }

    Ok(())
}

/// Execute a small line-based DSL against the HID writer.
///
/// Supported commands (case-insensitive):
/// - `tap("KEY")` (legacy `tap KEY` still accepted)
/// - `modtap("MOD+MOD+KEY")` (MOD: LCTRL, LSHIFT, LALT, LGUI, RCTRL, RSHIFT, RALT, RGUI; aliases: CTRL, SHIFT, ALT, GUI, CMD, WIN, OPTION, CONTROL)
/// - "delay N" (milliseconds, clamped)
/// - "text STRING [DELAY]" (optional per-char delay in ms)
/// - "call ID" (ID from `GET /kb/scripts`)
pub async fn run_dsl<'d, D>(w: &mut UsbHidWriter<'d, D, 8>, dsl: &str) -> Result<(), DslError>
where
    D: embassy_usb::driver::Driver<'d>,
{
    let prog = compile_dsl(dsl)?;
    exec_program_inner(w, &prog, 0u8).await
}

fn split_head(s: &str) -> Option<(&str, &str)> {
    let mut it = s.splitn(2, char::is_whitespace);
    let head = it.next()?;
    let tail = it.next().unwrap_or("");
    Some((head, tail))
}

fn eq_ci(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn parse_modtap(s: &str) -> Result<(u8, u8), DslError> {
    let mut mods: u8 = 0;
    let mut parts = s.split('+').peekable();
    let mut last_is_key = false;
    let mut key: u8 = 0;
    while let Some(p) = parts.next() {
        let t = p.trim();
        if t.is_empty() {
            return Err(DslError::InvalidLine);
        }
        // If this is the last segment, treat as key.
        if parts.peek().is_none() {
            key = parse_key(t).ok_or(DslError::ParseKey)?;
            last_is_key = true;
        } else {
            let m = parse_mod(t).ok_or(DslError::ParseMod)?;
            mods |= m;
        }
    }
    if !last_is_key {
        return Err(DslError::ParseKey);
    }
    Ok((mods, key))
}

fn parse_text_args(rest: &str) -> Result<(&str, u64), DslError> {
    let r = rest.trim();
    if r.is_empty() {
        return Err(DslError::TextEmpty);
    }
    // Optional trailing integer for delay.
    // Find last space; if suffix is integer, use it as delay.
    let mut delay_ms: u64 = 10;
    if let Some(idx) = r.rfind(char::is_whitespace) {
        let (lhs, rhs) = r.split_at(idx);
        let maybe = rhs.trim();
        if !maybe.is_empty() {
            if let Ok(n) = maybe.parse::<u64>() {
                delay_ms = core::cmp::min(n, 5000);
                let text = lhs.trim_end();
                return Ok((text, delay_ms));
            }
        }
    }
    Ok((r, delay_ms))
}

fn parse_mod(s: &str) -> Option<u8> {
    // Normalize to upper ASCII in a small buffer
    let up = upper_ascii::<16>(s);
    let u = up.as_str();
    Some(match u {
        "LCTRL" | "CONTROL" | "CTRL" => keyboard::MOD_LCTRL,
        "RCTRL" => keyboard::MOD_RCTRL,
        "LSHIFT" | "SHIFT" => keyboard::MOD_LSHIFT,
        "RSHIFT" => keyboard::MOD_RSHIFT,
        "LALT" | "ALT" | "OPTION" => keyboard::MOD_LALT,
        "RALT" => keyboard::MOD_RALT,
        "LGUI" | "GUI" | "CMD" | "WIN" => keyboard::MOD_LGUI,
        "RGUI" => keyboard::MOD_RGUI,
        _ => return None,
    })
}

fn parse_key(s: &str) -> Option<u8> {
    // Single letter A..Z
    if s.len() == 1 {
        let b = s.as_bytes()[0];
        if b'A' <= b && b <= b'Z' {
            return Some(keyboard::KEY_A + (b - b'A'));
        }
        if b'a' <= b && b <= b'z' {
            return Some(keyboard::KEY_A + (b - b'a'));
        }
    }

    let up = upper_ascii::<24>(s);
    let u = up.as_str();

    // Function keys
    if let Some(num) = u.strip_prefix('F').and_then(|t| t.parse::<u8>().ok()) {
        return match num {
            1 => Some(keyboard::KEY_F1),
            2 => Some(keyboard::KEY_F2),
            3 => Some(keyboard::KEY_F3),
            4 => Some(keyboard::KEY_F4),
            5 => Some(keyboard::KEY_F5),
            6 => Some(keyboard::KEY_F6),
            7 => Some(keyboard::KEY_F7),
            8 => Some(keyboard::KEY_F8),
            9 => Some(keyboard::KEY_F9),
            10 => Some(keyboard::KEY_F10),
            11 => Some(keyboard::KEY_F11),
            12 => Some(keyboard::KEY_F12),
            _ => None,
        };
    }

    // KP_ keys
    if let Some(rest) = u.strip_prefix("KP_") {
        return match rest {
            "ENTER" => Some(keyboard::KEY_KP_ENTER),
            "+" | "PLUS" => Some(keyboard::KEY_KP_PLUS),
            "-" | "MINUS" => Some(keyboard::KEY_KP_MINUS),
            "*" | "ASTERISK" => Some(keyboard::KEY_KP_ASTERISK),
            "/" | "SLASH" => Some(keyboard::KEY_KP_SLASH),
            "." | "DOT" | "DECIMAL" => Some(keyboard::KEY_KP_DOT),
            "0" => Some(keyboard::KEY_KP_0),
            "1" => Some(keyboard::KEY_KP_1),
            "2" => Some(keyboard::KEY_KP_2),
            "3" => Some(keyboard::KEY_KP_3),
            "4" => Some(keyboard::KEY_KP_4),
            "5" => Some(keyboard::KEY_KP_5),
            "6" => Some(keyboard::KEY_KP_6),
            "7" => Some(keyboard::KEY_KP_7),
            "8" => Some(keyboard::KEY_KP_8),
            "9" => Some(keyboard::KEY_KP_9),
            _ => None,
        };
    }

    // Number row
    if u.len() == 1 {
        let b = u.as_bytes()[0];
        if b'1' <= b && b <= b'9' {
            return Some(keyboard::KEY_1 + (b - b'1'));
        }
        if b == b'0' {
            return Some(keyboard::KEY_0);
        }
    }

    Some(match u {
        // Common control keys
        "ENTER" | "RETURN" => keyboard::KEY_ENTER,
        "ESC" | "ESCAPE" => keyboard::KEY_ESC,
        "BACKSPACE" | "BKSP" => keyboard::KEY_BACKSPACE,
        "TAB" => keyboard::KEY_TAB,
        "SPACE" | "SPACEBAR" => keyboard::KEY_SPACE,
        "CAPS_LOCK" | "CAPS" => keyboard::KEY_CAPS_LOCK,
        "PRINT_SCREEN" | "PRTSCR" => keyboard::KEY_PRINT_SCREEN,
        "SCROLL_LOCK" => keyboard::KEY_SCROLL_LOCK,
        "PAUSE" | "BREAK" => keyboard::KEY_PAUSE,
        "INSERT" | "INS" => keyboard::KEY_INSERT,
        "DELETE" | "DEL" => keyboard::KEY_DELETE,
        "HOME" => keyboard::KEY_HOME,
        "END" => keyboard::KEY_END,
        "PAGE_UP" | "PGUP" => keyboard::KEY_PAGE_UP,
        "PAGE_DOWN" | "PGDN" => keyboard::KEY_PAGE_DOWN,
        "LEFT" => keyboard::KEY_LEFT,
        "RIGHT" => keyboard::KEY_RIGHT,
        "UP" => keyboard::KEY_UP,
        "DOWN" => keyboard::KEY_DOWN,

        // Punctuation cluster by name
        "MINUS" | "HYPHEN" => keyboard::KEY_MINUS,
        "EQUAL" | "EQUALS" | "PLUS" => keyboard::KEY_EQUAL,
        "LEFT_BRACKET" | "LBRACKET" | "LBRACE" => keyboard::KEY_LEFT_BRACKET,
        "RIGHT_BRACKET" | "RBRACKET" | "RBRACE" => keyboard::KEY_RIGHT_BRACKET,
        "BACKSLASH" | "BSLASH" | "PIPE" => keyboard::KEY_BACKSLASH,
        "NON_US_HASH" => keyboard::KEY_NON_US_HASH,
        "SEMICOLON" | "SEMI" | ":" => keyboard::KEY_SEMICOLON,
        "APOSTROPHE" | "QUOTE" | "'" | "\"" => keyboard::KEY_APOSTROPHE,
        "GRAVE" | "BACKTICK" | "TILDE" => keyboard::KEY_GRAVE,
        "COMMA" | "," | "<" => keyboard::KEY_COMMA,
        "DOT" | "PERIOD" | "." | ">" => keyboard::KEY_DOT,
        "SLASH" | "/" | "?" => keyboard::KEY_SLASH,

        // Non-US backslash for ISO layouts
        "NON_US_BACKSLASH" => keyboard::KEY_NON_US_BACKSLASH,

        // Number lock
        "NUM_LOCK" => keyboard::KEY_NUM_LOCK,

        _ => return None,
    })
}

fn upper_ascii<const N: usize>(s: &str) -> heapless::String<N> {
    let mut out: heapless::String<N> = heapless::String::new();
    for b in s.bytes() {
        let up = if b'a' <= b && b <= b'z' { b - 32 } else { b };
        let _ = out.push(up as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyboard;

    #[test]
    fn parse_text_args_variants() {
        assert_eq!(parse_text_args("Hello 25").unwrap(), ("Hello", 25));
        assert_eq!(parse_text_args("Hello").unwrap(), ("Hello", 10));
        assert_eq!(parse_text_args("Hello   ").unwrap(), ("Hello", 10));
        // Delay is taken from the last whitespace-separated token if numeric
        assert_eq!(parse_text_args("Hello 20 30").unwrap(), ("Hello 20", 30));
        assert!(matches!(parse_text_args(""), Err(DslError::TextEmpty)));
    }

    #[test]
    fn parse_modtap_and_key_aliases() {
        // LCTRL + LALT + DELETE
        let (mods, key) = parse_modtap("LCTRL+LALT+DELETE").unwrap();
        assert_eq!(mods, keyboard::MOD_LCTRL | keyboard::MOD_LALT);
        assert_eq!(key, keyboard::KEY_DELETE);

        // Synonyms for GUI/CMD and key names
        let (mods2, key2) = parse_modtap("cmd+space").unwrap();
        assert_eq!(mods2, keyboard::MOD_LGUI);
        assert_eq!(key2, keyboard::KEY_SPACE);

        // Single letters and function keys
        assert_eq!(parse_key("a"), Some(keyboard::KEY_A));
        assert_eq!(parse_key("A"), Some(keyboard::KEY_A));
        assert_eq!(parse_key("F12"), Some(keyboard::KEY_F12));

        // Common aliases
        assert_eq!(parse_key("Return"), Some(keyboard::KEY_ENTER));
        assert_eq!(parse_key("Escape"), Some(keyboard::KEY_ESC));

        // Numpad and punctuation
        assert_eq!(parse_key("KP_ENTER"), Some(keyboard::KEY_KP_ENTER));
        assert_eq!(parse_key("PLUS"), Some(keyboard::KEY_EQUAL));
    }

    #[test]
    fn compile_simple_program() {
        let src = "tap(\"A\")\nmodtap(\"LCTRL+LALT+DELETE\")\ntext(\"Hello\", 25)\ndelay(0)\ncall hello_world\n";
        let prog = compile_dsl(src).expect("compile_dsl ok");
        // Expect: Tap(A), Tap(DELETE with mods), Text("Hello",25), Call("hello_world")
        let mut it = prog.ops.iter();
        match it.next() {
            Some(Op::Tap { key, mods }) => {
                assert_eq!((*key, *mods), (keyboard::KEY_A, 0));
            }
            _ => panic!("op0"),
        }
        match it.next() {
            Some(Op::Tap { key, mods }) => {
                assert_eq!(
                    (*key, *mods),
                    (
                        keyboard::KEY_DELETE,
                        keyboard::MOD_LCTRL | keyboard::MOD_LALT
                    )
                );
            }
            _ => panic!("op1"),
        }
        match it.next() {
            Some(Op::Text { s, delay_ms }) => {
                assert_eq!((*s, *delay_ms), ("Hello", 25));
            }
            _ => panic!("op2"),
        }
        match it.next() {
            Some(Op::Call { id }) => {
                assert_eq!(*id, "hello_world");
            }
            _ => panic!("op3"),
        }
        // No DelayMs for "delay 0"
        assert!(it.next().is_none());
    }

    #[test]
    fn compile_unknowns_and_limits() {
        // Unknown command
        match compile_dsl("noop X\n") {
            Err(DslError::UnknownCommand) => {}
            _ => panic!("expected UnknownCommand"),
        }

        // Unknown script id
        match compile_dsl("call not_a_script\n") {
            Err(DslError::UnknownScript) => {}
            _ => panic!("expected UnknownScript"),
        }

        // Too many lines (256 allowed, 257th should fail)
        let mut s = heapless::String::<{ MAX_DSL_LINES * 8 }>::new();
        for _ in 0..(MAX_DSL_LINES) {
            let _ = s.push_str("tap(\"A\")\n");
        }
        // Add one more non-empty command
        let _ = s.push_str("tap(\"A\")\n");
        match compile_dsl(&s) {
            Err(DslError::TooManyLines) => {}
            _ => panic!("expected TooManyLines"),
        }
    }

    #[test]
    fn text_delay_is_clamped() {
        // 60000 should clamp to MAX_DSL_DELAY_MS (5000)
        let p = compile_dsl("text A 60000\n").unwrap();
        match &p.ops[0] {
            Op::Text { s, delay_ms } => {
                assert_eq!(*s, "A");
                assert_eq!(*delay_ms as u64, MAX_DSL_DELAY_MS);
            }
            _ => panic!("expected text op"),
        }
    }

    #[test]
    fn split_head_and_upper_ascii() {
        assert_eq!(split_head("text Hello 10").unwrap(), ("text", "Hello 10"));
        let u = upper_ascii::<8>("aBc!z");
        assert_eq!(u.as_str(), "ABC!Z");
    }
}

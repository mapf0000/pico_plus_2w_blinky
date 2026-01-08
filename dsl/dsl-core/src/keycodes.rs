use crate::DslError;

// -------- HID constants (subset) --------
// (stable per HID Usage Tables, page 0x07)
pub const KEY_A: u8 = 0x04;
pub const KEY_1: u8 = 0x1e;
pub const KEY_2: u8 = 0x1f;
pub const KEY_3: u8 = 0x20;
pub const KEY_4: u8 = 0x21;
pub const KEY_5: u8 = 0x22;
pub const KEY_6: u8 = 0x23;
pub const KEY_7: u8 = 0x24;
pub const KEY_8: u8 = 0x25;
pub const KEY_9: u8 = 0x26;
pub const KEY_0: u8 = 0x27;
pub const KEY_ENTER: u8 = 0x28;
pub const KEY_ESC: u8 = 0x29;
pub const KEY_BACKSPACE: u8 = 0x2a;
pub const KEY_TAB: u8 = 0x2b;
pub const KEY_SPACE: u8 = 0x2c;
pub const KEY_MINUS: u8 = 0x2d;
pub const KEY_EQUAL: u8 = 0x2e;
pub const KEY_LEFT_BRACKET: u8 = 0x2f;
pub const KEY_RIGHT_BRACKET: u8 = 0x30;
pub const KEY_BACKSLASH: u8 = 0x31;
pub const KEY_NON_US_HASH: u8 = 0x32;
pub const KEY_SEMICOLON: u8 = 0x33;
pub const KEY_APOSTROPHE: u8 = 0x34;
pub const KEY_GRAVE: u8 = 0x35;
pub const KEY_COMMA: u8 = 0x36;
pub const KEY_DOT: u8 = 0x37;
pub const KEY_SLASH: u8 = 0x38;
pub const KEY_CAPS_LOCK: u8 = 0x39;
pub const KEY_F1: u8 = 0x3a;
pub const KEY_F2: u8 = 0x3b;
pub const KEY_F3: u8 = 0x3c;
pub const KEY_F4: u8 = 0x3d;
pub const KEY_F5: u8 = 0x3e;
pub const KEY_F6: u8 = 0x3f;
pub const KEY_F7: u8 = 0x40;
pub const KEY_F8: u8 = 0x41;
pub const KEY_F9: u8 = 0x42;
pub const KEY_F10: u8 = 0x43;
pub const KEY_F11: u8 = 0x44;
pub const KEY_F12: u8 = 0x45;
pub const KEY_PRINT_SCREEN: u8 = 0x46;
pub const KEY_SCROLL_LOCK: u8 = 0x47;
pub const KEY_PAUSE: u8 = 0x48;
pub const KEY_INSERT: u8 = 0x49;
pub const KEY_HOME: u8 = 0x4a;
pub const KEY_PAGE_UP: u8 = 0x4b;
pub const KEY_DELETE: u8 = 0x4c;
pub const KEY_END: u8 = 0x4d;
pub const KEY_PAGE_DOWN: u8 = 0x4e;
pub const KEY_RIGHT: u8 = 0x4f;
pub const KEY_LEFT: u8 = 0x50;
pub const KEY_DOWN: u8 = 0x51;
pub const KEY_UP: u8 = 0x52;
pub const KEY_NUM_LOCK: u8 = 0x53;
pub const KEY_KP_SLASH: u8 = 0x54;
pub const KEY_KP_ASTERISK: u8 = 0x55;
pub const KEY_KP_MINUS: u8 = 0x56;
pub const KEY_KP_PLUS: u8 = 0x57;
pub const KEY_KP_ENTER: u8 = 0x58;
pub const KEY_KP_1: u8 = 0x59;
pub const KEY_KP_2: u8 = 0x5a;
pub const KEY_KP_3: u8 = 0x5b;
pub const KEY_KP_4: u8 = 0x5c;
pub const KEY_KP_5: u8 = 0x5d;
pub const KEY_KP_6: u8 = 0x5e;
pub const KEY_KP_7: u8 = 0x5f;
pub const KEY_KP_8: u8 = 0x60;
pub const KEY_KP_9: u8 = 0x61;
pub const KEY_KP_0: u8 = 0x62;
pub const KEY_KP_DOT: u8 = 0x63;
pub const KEY_NON_US_BACKSLASH: u8 = 0x64;

pub const MOD_LCTRL: u8 = 0x01;
pub const MOD_LSHIFT: u8 = 0x02;
pub const MOD_LALT: u8 = 0x04;
pub const MOD_LGUI: u8 = 0x08;
pub const MOD_RCTRL: u8 = 0x10;
pub const MOD_RSHIFT: u8 = 0x20;
pub const MOD_RALT: u8 = 0x40;
pub const MOD_RGUI: u8 = 0x80;

fn upper_ascii<const N: usize>(s: &str) -> ArrayString<N> {
    let mut out: ArrayString<N> = ArrayString::new();
    for b in s.bytes() {
        let up = if b'a' <= b && b <= b'z' { b - 32 } else { b };
        let _ = out.push(up as char);
    }
    out
}

pub(crate) fn parse_modtap(s: &str) -> Result<(u8, u8), DslError> {
    let mut mods: u8 = 0;
    let mut parts = s.split('+').peekable();
    let mut last_is_key = false;
    let mut usage: u8 = 0;
    while let Some(p) = parts.next() {
        let t = p.trim();
        if t.is_empty() {
            return Err(DslError::InvalidLine);
        }
        if parts.peek().is_none() {
            usage = parse_key(t).ok_or(DslError::ParseKey)?;
            last_is_key = true;
        } else {
            let m = parse_mod(t).ok_or(DslError::ParseMod)?;
            mods |= m;
        }
    }
    if !last_is_key {
        return Err(DslError::ParseKey);
    }
    Ok((mods, usage))
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

pub(crate) fn parse_key(s: &str) -> Option<u8> {
    let up = upper_ascii::<32>(s);
    let u = up.as_str();

    if u.len() == 1 {
        let b = u.as_bytes()[0];
        if b'A' <= b && b <= b'Z' {
            return Some(KEY_A + (b - b'A'));
        }
    }
    if let Some(rest) = u.strip_prefix('F') {
        if let Ok(n) = rest.parse::<u8>() {
            return match n {
                1 => Some(KEY_F1),
                2 => Some(KEY_F2),
                3 => Some(KEY_F3),
                4 => Some(KEY_F4),
                5 => Some(KEY_F5),
                6 => Some(KEY_F6),
                7 => Some(KEY_F7),
                8 => Some(KEY_F8),
                9 => Some(KEY_F9),
                10 => Some(KEY_F10),
                11 => Some(KEY_F11),
                12 => Some(KEY_F12),
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
        if b'1' <= b && b <= b'9' {
            return Some(KEY_1 + (b - b'1'));
        }
        if b == b'0' {
            return Some(KEY_0);
        }
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
pub struct ArrayString<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> ArrayString<N> {
    pub fn new() -> Self {
        Self {
            buf: [0; N],
            len: 0,
        }
    }
    pub fn push(&mut self, ch: char) -> Result<(), ()> {
        if self.len < N {
            self.buf[self.len] = ch as u8;
            self.len += 1;
            Ok(())
        } else {
            Err(())
        }
    }
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

// -------- US ANSI text lowering --------

pub fn char_to_key_us(c: char) -> Option<(u8, u8)> {
    match c {
        '\n' | '\r' => Some((KEY_ENTER, 0)),
        '\t' => Some((KEY_TAB, 0)),
        ' ' => Some((KEY_SPACE, 0)),
        'a'..='z' => {
            let i = (c as u8) - b'a';
            Some((KEY_A + i, 0))
        }
        'A'..='Z' => {
            let i = (c as u8) - b'A';
            Some((KEY_A + i, MOD_LSHIFT))
        }
        '1' => Some((KEY_1, 0)),
        '2' => Some((KEY_2, 0)),
        '3' => Some((KEY_3, 0)),
        '4' => Some((KEY_4, 0)),
        '5' => Some((KEY_5, 0)),
        '6' => Some((KEY_6, 0)),
        '7' => Some((KEY_7, 0)),
        '8' => Some((KEY_8, 0)),
        '9' => Some((KEY_9, 0)),
        '0' => Some((KEY_0, 0)),
        '!' => Some((KEY_1, MOD_LSHIFT)),
        '@' => Some((KEY_2, MOD_LSHIFT)),
        '#' => Some((KEY_3, MOD_LSHIFT)),
        '$' => Some((KEY_4, MOD_LSHIFT)),
        '%' => Some((KEY_5, MOD_LSHIFT)),
        '^' => Some((KEY_6, MOD_LSHIFT)),
        '&' => Some((KEY_7, MOD_LSHIFT)),
        '*' => Some((KEY_8, MOD_LSHIFT)),
        '(' => Some((KEY_9, MOD_LSHIFT)),
        ')' => Some((KEY_0, MOD_LSHIFT)),
        '-' => Some((KEY_MINUS, 0)),
        '_' => Some((KEY_MINUS, MOD_LSHIFT)),
        '=' => Some((KEY_EQUAL, 0)),
        '+' => Some((KEY_EQUAL, MOD_LSHIFT)),
        '[' => Some((KEY_LEFT_BRACKET, 0)),
        '{' => Some((KEY_LEFT_BRACKET, MOD_LSHIFT)),
        ']' => Some((KEY_RIGHT_BRACKET, 0)),
        '}' => Some((KEY_RIGHT_BRACKET, MOD_LSHIFT)),
        '\\' => Some((KEY_BACKSLASH, 0)),
        '|' => Some((KEY_BACKSLASH, MOD_LSHIFT)),
        ';' => Some((KEY_SEMICOLON, 0)),
        ':' => Some((KEY_SEMICOLON, MOD_LSHIFT)),
        '\'' => Some((KEY_APOSTROPHE, 0)),
        '"' => Some((KEY_APOSTROPHE, MOD_LSHIFT)),
        '`' => Some((KEY_GRAVE, 0)),
        '~' => Some((KEY_GRAVE, MOD_LSHIFT)),
        ',' => Some((KEY_COMMA, 0)),
        '<' => Some((KEY_COMMA, MOD_LSHIFT)),
        '.' => Some((KEY_DOT, 0)),
        '>' => Some((KEY_DOT, MOD_LSHIFT)),
        '/' => Some((KEY_SLASH, 0)),
        '?' => Some((KEY_SLASH, MOD_LSHIFT)),
        _ => None,
    }
}

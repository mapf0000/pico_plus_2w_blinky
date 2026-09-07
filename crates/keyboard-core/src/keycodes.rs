//! HID key and modifier parsing.

// -------- HID constants (subset) --------
// (stable per HID Usage Tables, page 0x07)

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Usage(u8);

impl Usage {
    pub const fn from_u8(value: u8) -> Self {
        Self(value)
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn add(self, offset: u8) -> Self {
        Self(self.0 + offset)
    }
}

impl From<Usage> for u8 {
    fn from(value: Usage) -> Self {
        value.0
    }
}

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Mods(u8);

impl Mods {
    pub const fn from_u8(value: u8) -> Self {
        Self(value)
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn or(self, other: Mods) -> Mods {
        Mods(self.0 | other.0)
    }
}

impl Default for Mods {
    fn default() -> Self {
        Mods::empty()
    }
}

impl From<Mods> for u8 {
    fn from(value: Mods) -> Self {
        value.0
    }
}

impl core::ops::BitOr for Mods {
    type Output = Mods;

    fn bitor(self, rhs: Mods) -> Self::Output {
        Mods(self.0 | rhs.0)
    }
}

impl core::ops::BitOrAssign for Mods {
    fn bitor_assign(&mut self, rhs: Mods) {
        self.0 |= rhs.0;
    }
}
pub const KEY_A: Usage = Usage(0x04);
pub const KEY_1: Usage = Usage(0x1e);
pub const KEY_2: Usage = Usage(0x1f);
pub const KEY_3: Usage = Usage(0x20);
pub const KEY_4: Usage = Usage(0x21);
pub const KEY_5: Usage = Usage(0x22);
pub const KEY_6: Usage = Usage(0x23);
pub const KEY_7: Usage = Usage(0x24);
pub const KEY_8: Usage = Usage(0x25);
pub const KEY_9: Usage = Usage(0x26);
pub const KEY_0: Usage = Usage(0x27);
pub const KEY_ENTER: Usage = Usage(0x28);
pub const KEY_ESC: Usage = Usage(0x29);
pub const KEY_BACKSPACE: Usage = Usage(0x2a);
pub const KEY_TAB: Usage = Usage(0x2b);
pub const KEY_SPACE: Usage = Usage(0x2c);
pub const KEY_MINUS: Usage = Usage(0x2d);
pub const KEY_EQUAL: Usage = Usage(0x2e);
pub const KEY_LEFT_BRACKET: Usage = Usage(0x2f);
pub const KEY_RIGHT_BRACKET: Usage = Usage(0x30);
pub const KEY_BACKSLASH: Usage = Usage(0x31);
pub const KEY_NON_US_HASH: Usage = Usage(0x32);
pub const KEY_SEMICOLON: Usage = Usage(0x33);
pub const KEY_APOSTROPHE: Usage = Usage(0x34);
pub const KEY_GRAVE: Usage = Usage(0x35);
pub const KEY_COMMA: Usage = Usage(0x36);
pub const KEY_DOT: Usage = Usage(0x37);
pub const KEY_SLASH: Usage = Usage(0x38);
pub const KEY_CAPS_LOCK: Usage = Usage(0x39);
pub const KEY_F1: Usage = Usage(0x3a);
pub const KEY_F2: Usage = Usage(0x3b);
pub const KEY_F3: Usage = Usage(0x3c);
pub const KEY_F4: Usage = Usage(0x3d);
pub const KEY_F5: Usage = Usage(0x3e);
pub const KEY_F6: Usage = Usage(0x3f);
pub const KEY_F7: Usage = Usage(0x40);
pub const KEY_F8: Usage = Usage(0x41);
pub const KEY_F9: Usage = Usage(0x42);
pub const KEY_F10: Usage = Usage(0x43);
pub const KEY_F11: Usage = Usage(0x44);
pub const KEY_F12: Usage = Usage(0x45);
pub const KEY_F13: Usage = Usage(0x68);
pub const KEY_F14: Usage = Usage(0x69);
pub const KEY_F15: Usage = Usage(0x6a);
pub const KEY_F16: Usage = Usage(0x6b);
pub const KEY_F17: Usage = Usage(0x6c);
pub const KEY_F18: Usage = Usage(0x6d);
pub const KEY_F19: Usage = Usage(0x6e);
pub const KEY_F20: Usage = Usage(0x6f);
pub const KEY_F21: Usage = Usage(0x70);
pub const KEY_F22: Usage = Usage(0x71);
pub const KEY_F23: Usage = Usage(0x72);
pub const KEY_F24: Usage = Usage(0x73);
pub const KEY_PRINT_SCREEN: Usage = Usage(0x46);
pub const KEY_SCROLL_LOCK: Usage = Usage(0x47);
pub const KEY_PAUSE: Usage = Usage(0x48);
pub const KEY_INSERT: Usage = Usage(0x49);
pub const KEY_HOME: Usage = Usage(0x4a);
pub const KEY_PAGE_UP: Usage = Usage(0x4b);
pub const KEY_DELETE: Usage = Usage(0x4c);
pub const KEY_END: Usage = Usage(0x4d);
pub const KEY_PAGE_DOWN: Usage = Usage(0x4e);
pub const KEY_RIGHT: Usage = Usage(0x4f);
pub const KEY_LEFT: Usage = Usage(0x50);
pub const KEY_DOWN: Usage = Usage(0x51);
pub const KEY_UP: Usage = Usage(0x52);
pub const KEY_NUM_LOCK: Usage = Usage(0x53);
pub const KEY_KP_SLASH: Usage = Usage(0x54);
pub const KEY_KP_ASTERISK: Usage = Usage(0x55);
pub const KEY_KP_MINUS: Usage = Usage(0x56);
pub const KEY_KP_PLUS: Usage = Usage(0x57);
pub const KEY_KP_ENTER: Usage = Usage(0x58);
pub const KEY_KP_1: Usage = Usage(0x59);
pub const KEY_KP_2: Usage = Usage(0x5a);
pub const KEY_KP_3: Usage = Usage(0x5b);
pub const KEY_KP_4: Usage = Usage(0x5c);
pub const KEY_KP_5: Usage = Usage(0x5d);
pub const KEY_KP_6: Usage = Usage(0x5e);
pub const KEY_KP_7: Usage = Usage(0x5f);
pub const KEY_KP_8: Usage = Usage(0x60);
pub const KEY_KP_9: Usage = Usage(0x61);
pub const KEY_KP_0: Usage = Usage(0x62);
pub const KEY_KP_DOT: Usage = Usage(0x63);
pub const KEY_NON_US_BACKSLASH: Usage = Usage(0x64);

pub const MOD_LCTRL: Mods = Mods(0x01);
pub const MOD_LSHIFT: Mods = Mods(0x02);
pub const MOD_LALT: Mods = Mods(0x04);
pub const MOD_LGUI: Mods = Mods(0x08);
pub const MOD_RCTRL: Mods = Mods(0x10);
pub const MOD_RSHIFT: Mods = Mods(0x20);
pub const MOD_RALT: Mods = Mods(0x40);
pub const MOD_RGUI: Mods = Mods(0x80);

fn upper_ascii<const N: usize>(s: &str) -> ArrayString<N> {
    let mut out: ArrayString<N> = ArrayString::new();
    for b in s.bytes() {
        let up = if b.is_ascii_lowercase() { b - 32 } else { b };
        let _ = out.push(up as char);
    }
    out
}

pub fn parse_modtap(s: &str) -> Result<(Mods, Usage), &'static str> {
    let mut mods = Mods::empty();
    let mut parts = s.split('+').peekable();
    let mut usage: Option<Usage> = None;
    while let Some(p) = parts.next() {
        let t = p.trim();
        if t.is_empty() {
            return Err("InvalidLine");
        }
        if parts.peek().is_none() {
            usage = Some(parse_key(t).ok_or("ParseKey")?);
        } else {
            let m = parse_mod(t).ok_or("ParseMod")?;
            mods |= m;
        }
    }
    let usage = usage.ok_or("ParseKey")?;
    Ok((mods, usage))
}

fn parse_mod(s: &str) -> Option<Mods> {
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

pub fn parse_key(s: &str) -> Option<Usage> {
    let up = upper_ascii::<32>(s);
    let u = up.as_str();

    if u.len() == 1 {
        let b = u.as_bytes()[0];
        if b.is_ascii_uppercase() {
            return Some(KEY_A.add(b - b'A'));
        }
    }
    if let Some(rest) = u.strip_prefix('F')
        && let Ok(n) = rest.parse::<u8>()
    {
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
            13 => Some(KEY_F13),
            14 => Some(KEY_F14),
            15 => Some(KEY_F15),
            16 => Some(KEY_F16),
            17 => Some(KEY_F17),
            18 => Some(KEY_F18),
            19 => Some(KEY_F19),
            20 => Some(KEY_F20),
            21 => Some(KEY_F21),
            22 => Some(KEY_F22),
            23 => Some(KEY_F23),
            24 => Some(KEY_F24),
            _ => None,
        };
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
        if (b'1'..=b'9').contains(&b) {
            return Some(KEY_1.add(b - b'1'));
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

impl<const N: usize> Default for ArrayString<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> ArrayString<N> {
    pub fn new() -> Self {
        Self {
            buf: [0; N],
            len: 0,
        }
    }
    pub fn push(&mut self, ch: char) -> bool {
        if self.len < N {
            self.buf[self.len] = ch as u8;
            self.len += 1;
            true
        } else {
            false
        }
    }
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

// -------- US ANSI text lowering --------

pub fn char_to_key_us(c: char) -> Option<(Usage, Mods)> {
    match c {
        '\n' | '\r' => Some((KEY_ENTER, Mods::empty())),
        '\t' => Some((KEY_TAB, Mods::empty())),
        ' ' => Some((KEY_SPACE, Mods::empty())),
        'a'..='z' => {
            let i = (c as u8) - b'a';
            Some((KEY_A.add(i), Mods::empty()))
        }
        'A'..='Z' => {
            let i = (c as u8) - b'A';
            Some((KEY_A.add(i), MOD_LSHIFT))
        }
        '1' => Some((KEY_1, Mods::empty())),
        '2' => Some((KEY_2, Mods::empty())),
        '3' => Some((KEY_3, Mods::empty())),
        '4' => Some((KEY_4, Mods::empty())),
        '5' => Some((KEY_5, Mods::empty())),
        '6' => Some((KEY_6, Mods::empty())),
        '7' => Some((KEY_7, Mods::empty())),
        '8' => Some((KEY_8, Mods::empty())),
        '9' => Some((KEY_9, Mods::empty())),
        '0' => Some((KEY_0, Mods::empty())),
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
        '-' => Some((KEY_MINUS, Mods::empty())),
        '_' => Some((KEY_MINUS, MOD_LSHIFT)),
        '=' => Some((KEY_EQUAL, Mods::empty())),
        '+' => Some((KEY_EQUAL, MOD_LSHIFT)),
        '[' => Some((KEY_LEFT_BRACKET, Mods::empty())),
        '{' => Some((KEY_LEFT_BRACKET, MOD_LSHIFT)),
        ']' => Some((KEY_RIGHT_BRACKET, Mods::empty())),
        '}' => Some((KEY_RIGHT_BRACKET, MOD_LSHIFT)),
        '\\' => Some((KEY_BACKSLASH, Mods::empty())),
        '|' => Some((KEY_BACKSLASH, MOD_LSHIFT)),
        ';' => Some((KEY_SEMICOLON, Mods::empty())),
        ':' => Some((KEY_SEMICOLON, MOD_LSHIFT)),
        '\'' => Some((KEY_APOSTROPHE, Mods::empty())),
        '"' => Some((KEY_APOSTROPHE, MOD_LSHIFT)),
        '`' => Some((KEY_GRAVE, Mods::empty())),
        '~' => Some((KEY_GRAVE, MOD_LSHIFT)),
        ',' => Some((KEY_COMMA, Mods::empty())),
        '<' => Some((KEY_COMMA, MOD_LSHIFT)),
        '.' => Some((KEY_DOT, Mods::empty())),
        '>' => Some((KEY_DOT, MOD_LSHIFT)),
        '/' => Some((KEY_SLASH, Mods::empty())),
        '?' => Some((KEY_SLASH, MOD_LSHIFT)),
        _ => None,
    }
}

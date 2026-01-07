use core::str::FromStr;

use crate::{
    char_to_key_us, KEY_0, KEY_2, KEY_5, KEY_6, KEY_7, KEY_8, KEY_9, KEY_APOSTROPHE,
    KEY_BACKSLASH, KEY_COMMA, KEY_DOT, KEY_GRAVE, KEY_LEFT_BRACKET, KEY_MINUS,
    KEY_NON_US_BACKSLASH, KEY_RIGHT_BRACKET, KEY_SLASH, KEY_SPACE, MOD_LALT, MOD_LSHIFT,
    MOD_RALT,
};

pub const DEFAULT_LAYOUT_ID: &str = "win_en-US";
const LAYOUT_WIN_EN_GB_ID: &str = "win_en-GB";
const LAYOUT_WIN_PT_BR_ID: &str = "win_pt-BR";
const LAYOUT_WIN_DE_DE_ID: &str = "win_de-DE";
const LAYOUT_MAC_EN_GB_ID: &str = "mac_en-GB";
const LAYOUT_MAC_PT_BR_ID: &str = "mac_pt-BR";
const LAYOUT_MAC_DE_DE_ID: &str = "mac_de-DE";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutParseError {
    Unknown,
    NotEnabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutId {
    Us,
    #[cfg(feature = "layout_win_en_gb")]
    WinEnGb,
    #[cfg(feature = "layout_win_pt_br")]
    WinPtBr,
    #[cfg(feature = "layout_win_de_de")]
    WinDeDe,
    #[cfg(feature = "layout_mac_en_gb")]
    MacEnGb,
    #[cfg(feature = "layout_mac_pt_br")]
    MacPtBr,
    #[cfg(feature = "layout_mac_de_de")]
    MacDeDe,
}

impl Default for LayoutId {
    fn default() -> Self {
        LayoutId::Us
    }
}

impl LayoutId {
    pub fn name(self) -> &'static str {
        match self {
            LayoutId::Us => DEFAULT_LAYOUT_ID,
            #[cfg(feature = "layout_win_en_gb")]
            LayoutId::WinEnGb => LAYOUT_WIN_EN_GB_ID,
            #[cfg(feature = "layout_win_pt_br")]
            LayoutId::WinPtBr => LAYOUT_WIN_PT_BR_ID,
            #[cfg(feature = "layout_win_de_de")]
            LayoutId::WinDeDe => LAYOUT_WIN_DE_DE_ID,
            #[cfg(feature = "layout_mac_en_gb")]
            LayoutId::MacEnGb => LAYOUT_MAC_EN_GB_ID,
            #[cfg(feature = "layout_mac_pt_br")]
            LayoutId::MacPtBr => LAYOUT_MAC_PT_BR_ID,
            #[cfg(feature = "layout_mac_de_de")]
            LayoutId::MacDeDe => LAYOUT_MAC_DE_DE_ID,
        }
    }

    pub fn map_char(self, c: char) -> Option<(u8, u8)> {
        match self {
            LayoutId::Us => char_to_key_us(c),
            #[cfg(feature = "layout_win_en_gb")]
            LayoutId::WinEnGb => map_with_overrides(c, WIN_EN_GB_OVERRIDES),
            #[cfg(feature = "layout_win_pt_br")]
            LayoutId::WinPtBr => map_with_overrides(c, WIN_PT_BR_OVERRIDES),
            #[cfg(feature = "layout_win_de_de")]
            LayoutId::WinDeDe => map_with_overrides(c, WIN_DE_DE_OVERRIDES),
            #[cfg(feature = "layout_mac_en_gb")]
            LayoutId::MacEnGb => map_with_overrides(c, MAC_EN_GB_OVERRIDES),
            #[cfg(feature = "layout_mac_pt_br")]
            LayoutId::MacPtBr => map_with_overrides(c, MAC_PT_BR_OVERRIDES),
            #[cfg(feature = "layout_mac_de_de")]
            LayoutId::MacDeDe => map_with_overrides(c, MAC_DE_DE_OVERRIDES),
        }
    }
}

impl FromStr for LayoutId {
    type Err = LayoutParseError;

    fn from_str(id: &str) -> Result<Self, Self::Err> {
        parse_layout_id(id)
    }
}

pub fn available_layouts() -> &'static [&'static str] {
    const LAYOUTS: &[&str] = &[
        DEFAULT_LAYOUT_ID,
        #[cfg(feature = "layout_win_en_gb")]
        LAYOUT_WIN_EN_GB_ID,
        #[cfg(feature = "layout_win_pt_br")]
        LAYOUT_WIN_PT_BR_ID,
        #[cfg(feature = "layout_win_de_de")]
        LAYOUT_WIN_DE_DE_ID,
        #[cfg(feature = "layout_mac_en_gb")]
        LAYOUT_MAC_EN_GB_ID,
        #[cfg(feature = "layout_mac_pt_br")]
        LAYOUT_MAC_PT_BR_ID,
        #[cfg(feature = "layout_mac_de_de")]
        LAYOUT_MAC_DE_DE_ID,
    ];
    LAYOUTS
}

pub fn parse_layout_id(id: &str) -> Result<LayoutId, LayoutParseError> {
    if id.eq_ignore_ascii_case(DEFAULT_LAYOUT_ID) || id.eq_ignore_ascii_case("us") {
        return Ok(LayoutId::Us);
    }
    if id.eq_ignore_ascii_case(LAYOUT_WIN_EN_GB_ID) {
        return parse_win_en_gb();
    }
    if id.eq_ignore_ascii_case(LAYOUT_WIN_PT_BR_ID) {
        return parse_win_pt_br();
    }
    if id.eq_ignore_ascii_case(LAYOUT_WIN_DE_DE_ID) {
        return parse_win_de_de();
    }
    if id.eq_ignore_ascii_case(LAYOUT_MAC_EN_GB_ID) {
        return parse_mac_en_gb();
    }
    if id.eq_ignore_ascii_case(LAYOUT_MAC_PT_BR_ID) {
        return parse_mac_pt_br();
    }
    if id.eq_ignore_ascii_case(LAYOUT_MAC_DE_DE_ID) {
        return parse_mac_de_de();
    }
    Err(LayoutParseError::Unknown)
}

fn parse_win_en_gb() -> Result<LayoutId, LayoutParseError> {
    #[cfg(feature = "layout_win_en_gb")]
    {
        Ok(LayoutId::WinEnGb)
    }
    #[cfg(not(feature = "layout_win_en_gb"))]
    {
        Err(LayoutParseError::NotEnabled)
    }
}

fn parse_win_pt_br() -> Result<LayoutId, LayoutParseError> {
    #[cfg(feature = "layout_win_pt_br")]
    {
        Ok(LayoutId::WinPtBr)
    }
    #[cfg(not(feature = "layout_win_pt_br"))]
    {
        Err(LayoutParseError::NotEnabled)
    }
}

fn parse_win_de_de() -> Result<LayoutId, LayoutParseError> {
    #[cfg(feature = "layout_win_de_de")]
    {
        Ok(LayoutId::WinDeDe)
    }
    #[cfg(not(feature = "layout_win_de_de"))]
    {
        Err(LayoutParseError::NotEnabled)
    }
}

fn parse_mac_en_gb() -> Result<LayoutId, LayoutParseError> {
    #[cfg(feature = "layout_mac_en_gb")]
    {
        Ok(LayoutId::MacEnGb)
    }
    #[cfg(not(feature = "layout_mac_en_gb"))]
    {
        Err(LayoutParseError::NotEnabled)
    }
}

fn parse_mac_pt_br() -> Result<LayoutId, LayoutParseError> {
    #[cfg(feature = "layout_mac_pt_br")]
    {
        Ok(LayoutId::MacPtBr)
    }
    #[cfg(not(feature = "layout_mac_pt_br"))]
    {
        Err(LayoutParseError::NotEnabled)
    }
}

fn parse_mac_de_de() -> Result<LayoutId, LayoutParseError> {
    #[cfg(feature = "layout_mac_de_de")]
    {
        Ok(LayoutId::MacDeDe)
    }
    #[cfg(not(feature = "layout_mac_de_de"))]
    {
        Err(LayoutParseError::NotEnabled)
    }
}

#[derive(Debug, Clone, Copy)]
struct LayoutOverride {
    ch: char,
    usage: u8,
    mods: u8,
}

fn map_with_overrides(c: char, overrides: &[LayoutOverride]) -> Option<(u8, u8)> {
    for ov in overrides {
        if ov.ch == c {
            return Some((ov.usage, ov.mods));
        }
    }
    char_to_key_us(c)
}

const KEY_Q: u8 = crate::KEY_A + (b'Q' - b'A');
const KEY_W: u8 = crate::KEY_A + (b'W' - b'A');
const KEY_Y: u8 = crate::KEY_A + (b'Y' - b'A');
const KEY_Z: u8 = crate::KEY_A + (b'Z' - b'A');

#[cfg(feature = "layout_win_en_gb")]
const WIN_EN_GB_OVERRIDES: &[LayoutOverride] = &[
    LayoutOverride {
        ch: '#',
        usage: KEY_BACKSLASH,
        mods: 0,
    },
    LayoutOverride {
        ch: '\\',
        usage: KEY_BACKSLASH,
        mods: MOD_RALT,
    },
    LayoutOverride {
        ch: '"',
        usage: KEY_2,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: '~',
        usage: KEY_BACKSLASH,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: '@',
        usage: KEY_APOSTROPHE,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: '|',
        usage: KEY_NON_US_BACKSLASH,
        mods: MOD_LSHIFT,
    },
];

#[cfg(feature = "layout_win_pt_br")]
const WIN_PT_BR_OVERRIDES: &[LayoutOverride] = &[
    LayoutOverride {
        ch: '^',
        usage: KEY_SPACE,
        mods: 0,
    },
    LayoutOverride {
        ch: '~',
        usage: KEY_SPACE,
        mods: 0,
    },
    LayoutOverride {
        ch: '?',
        usage: KEY_W,
        mods: MOD_RALT,
    },
    LayoutOverride {
        ch: ']',
        usage: KEY_BACKSLASH,
        mods: 0,
    },
    LayoutOverride {
        ch: '|',
        usage: KEY_NON_US_BACKSLASH,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: '{',
        usage: KEY_RIGHT_BRACKET,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: '`',
        usage: KEY_SPACE,
        mods: 0,
    },
    LayoutOverride {
        ch: '/',
        usage: KEY_Q,
        mods: MOD_RALT,
    },
    LayoutOverride {
        ch: '[',
        usage: KEY_RIGHT_BRACKET,
        mods: 0,
    },
    LayoutOverride {
        ch: '\'',
        usage: KEY_GRAVE,
        mods: 0,
    },
    LayoutOverride {
        ch: '"',
        usage: KEY_GRAVE,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: '\\',
        usage: KEY_NON_US_BACKSLASH,
        mods: 0,
    },
    LayoutOverride {
        ch: '}',
        usage: KEY_BACKSLASH,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: ';',
        usage: KEY_SLASH,
        mods: 0,
    },
    LayoutOverride {
        ch: ':',
        usage: KEY_SLASH,
        mods: MOD_LSHIFT,
    },
];

#[cfg(feature = "layout_win_de_de")]
const WIN_DE_DE_OVERRIDES: &[LayoutOverride] = &[
    LayoutOverride {
        ch: 'Y',
        usage: KEY_Z,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: 'Z',
        usage: KEY_Y,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: 'y',
        usage: KEY_Z,
        mods: 0,
    },
    LayoutOverride {
        ch: 'z',
        usage: KEY_Y,
        mods: 0,
    },
    LayoutOverride {
        ch: '*',
        usage: KEY_RIGHT_BRACKET,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: '(',
        usage: KEY_8,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: '"',
        usage: KEY_2,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: '\'',
        usage: KEY_BACKSLASH,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: ']',
        usage: KEY_9,
        mods: MOD_RALT,
    },
    LayoutOverride {
        ch: '-',
        usage: KEY_SLASH,
        mods: 0,
    },
    LayoutOverride {
        ch: '#',
        usage: KEY_BACKSLASH,
        mods: 0,
    },
    LayoutOverride {
        ch: ')',
        usage: KEY_9,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: '&',
        usage: KEY_6,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: '}',
        usage: KEY_0,
        mods: MOD_RALT,
    },
    LayoutOverride {
        ch: ';',
        usage: KEY_COMMA,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: '`',
        usage: KEY_SPACE,
        mods: 0,
    },
    LayoutOverride {
        ch: '?',
        usage: KEY_MINUS,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: '=',
        usage: KEY_0,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: '~',
        usage: KEY_RIGHT_BRACKET,
        mods: MOD_RALT,
    },
    LayoutOverride {
        ch: '{',
        usage: KEY_7,
        mods: MOD_RALT,
    },
    LayoutOverride {
        ch: '[',
        usage: KEY_8,
        mods: MOD_RALT,
    },
    LayoutOverride {
        ch: '/',
        usage: KEY_7,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: '+',
        usage: KEY_RIGHT_BRACKET,
        mods: 0,
    },
    LayoutOverride {
        ch: '|',
        usage: KEY_NON_US_BACKSLASH,
        mods: MOD_RALT,
    },
    LayoutOverride {
        ch: '^',
        usage: KEY_SPACE,
        mods: 0,
    },
    LayoutOverride {
        ch: '@',
        usage: KEY_Q,
        mods: MOD_RALT,
    },
    LayoutOverride {
        ch: '\\',
        usage: KEY_MINUS,
        mods: MOD_RALT,
    },
    LayoutOverride {
        ch: ':',
        usage: KEY_DOT,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: '_',
        usage: KEY_SLASH,
        mods: MOD_LSHIFT,
    },
];

#[cfg(feature = "layout_mac_en_gb")]
const MAC_EN_GB_OVERRIDES: &[LayoutOverride] = &[
    LayoutOverride {
        ch: '"',
        usage: KEY_2,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: '#',
        usage: KEY_BACKSLASH,
        mods: 0,
    },
    LayoutOverride {
        ch: '@',
        usage: KEY_APOSTROPHE,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: '\\',
        usage: KEY_NON_US_BACKSLASH,
        mods: 0,
    },
    LayoutOverride {
        ch: '|',
        usage: KEY_NON_US_BACKSLASH,
        mods: MOD_LSHIFT,
    },
];

#[cfg(feature = "layout_mac_de_de")]
const MAC_DE_DE_OVERRIDES: &[LayoutOverride] = &[
    LayoutOverride {
        ch: '"',
        usage: KEY_2,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: '#',
        usage: KEY_BACKSLASH,
        mods: 0,
    },
    LayoutOverride {
        ch: '&',
        usage: KEY_6,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: '\'',
        usage: KEY_BACKSLASH,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: '(',
        usage: KEY_8,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: ')',
        usage: KEY_9,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: '*',
        usage: KEY_RIGHT_BRACKET,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: '+',
        usage: KEY_RIGHT_BRACKET,
        mods: 0,
    },
    LayoutOverride {
        ch: '-',
        usage: KEY_SLASH,
        mods: 0,
    },
    LayoutOverride {
        ch: '/',
        usage: KEY_7,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: ':',
        usage: KEY_DOT,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: ';',
        usage: KEY_COMMA,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: '<',
        usage: KEY_NON_US_BACKSLASH,
        mods: 0,
    },
    LayoutOverride {
        ch: '=',
        usage: KEY_0,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: '>',
        usage: KEY_NON_US_BACKSLASH,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: '?',
        usage: KEY_MINUS,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: '@',
        usage: KEY_Q,
        mods: MOD_LALT,
    },
    LayoutOverride {
        ch: 'Y',
        usage: KEY_Z,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: 'Z',
        usage: KEY_Y,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: '[',
        usage: KEY_5,
        mods: MOD_LALT,
    },
    LayoutOverride {
        ch: '\\',
        usage: KEY_MINUS,
        mods: MOD_LALT,
    },
    LayoutOverride {
        ch: ']',
        usage: KEY_6,
        mods: MOD_LALT,
    },
    LayoutOverride {
        ch: '_',
        usage: KEY_SLASH,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: 'y',
        usage: KEY_Z,
        mods: 0,
    },
    LayoutOverride {
        ch: 'z',
        usage: KEY_Y,
        mods: 0,
    },
    LayoutOverride {
        ch: '{',
        usage: KEY_7,
        mods: MOD_LALT,
    },
    LayoutOverride {
        ch: '|',
        usage: KEY_NON_US_BACKSLASH,
        mods: MOD_LALT,
    },
    LayoutOverride {
        ch: '}',
        usage: KEY_0,
        mods: MOD_LALT,
    },
];

#[cfg(feature = "layout_mac_pt_br")]
const MAC_PT_BR_OVERRIDES: &[LayoutOverride] = &[
    LayoutOverride {
        ch: '"',
        usage: KEY_NON_US_BACKSLASH,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: '\'',
        usage: KEY_NON_US_BACKSLASH,
        mods: 0,
    },
    LayoutOverride {
        ch: ':',
        usage: KEY_RIGHT_BRACKET,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: ';',
        usage: KEY_RIGHT_BRACKET,
        mods: 0,
    },
    LayoutOverride {
        ch: '[',
        usage: KEY_LEFT_BRACKET,
        mods: MOD_LALT,
    },
    LayoutOverride {
        ch: ']',
        usage: KEY_RIGHT_BRACKET,
        mods: MOD_LALT,
    },
    LayoutOverride {
        ch: '`',
        usage: KEY_NON_US_BACKSLASH,
        mods: MOD_LSHIFT | MOD_LALT,
    },
    LayoutOverride {
        ch: '{',
        usage: KEY_LEFT_BRACKET,
        mods: MOD_LSHIFT | MOD_LALT,
    },
    LayoutOverride {
        ch: '}',
        usage: KEY_RIGHT_BRACKET,
        mods: MOD_LSHIFT | MOD_LALT,
    },
];

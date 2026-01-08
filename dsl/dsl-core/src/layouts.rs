use core::str::FromStr;

use crate::char_to_key_us;
#[cfg(feature = "layout_win_en_gb")]
use crate::{
    KEY_2, KEY_APOSTROPHE, KEY_BACKSLASH, KEY_NON_US_BACKSLASH, MOD_LSHIFT, MOD_RALT,
};
#[cfg(feature = "layout_win_pt_br")]
use crate::{
    KEY_BACKSLASH, KEY_GRAVE, KEY_NON_US_BACKSLASH, KEY_RIGHT_BRACKET, KEY_SLASH, KEY_SPACE,
    MOD_LSHIFT, MOD_RALT,
};
#[cfg(feature = "layout_win_de_de")]
use crate::{
    KEY_0, KEY_2, KEY_6, KEY_7, KEY_8, KEY_9, KEY_BACKSLASH, KEY_COMMA, KEY_DOT, KEY_MINUS,
    KEY_NON_US_BACKSLASH, KEY_RIGHT_BRACKET, KEY_SLASH, KEY_SPACE, MOD_LSHIFT, MOD_RALT,
};
#[cfg(feature = "layout_mac_en_gb")]
use crate::{KEY_2, KEY_APOSTROPHE, KEY_BACKSLASH, KEY_NON_US_BACKSLASH, MOD_LSHIFT};
#[cfg(feature = "layout_mac_de_de")]
use crate::{
    KEY_0, KEY_2, KEY_5, KEY_6, KEY_7, KEY_8, KEY_9, KEY_BACKSLASH, KEY_COMMA, KEY_DOT,
    KEY_MINUS, KEY_NON_US_BACKSLASH, KEY_RIGHT_BRACKET, KEY_SLASH, MOD_LALT, MOD_LSHIFT,
};
#[cfg(feature = "layout_mac_pt_br")]
use crate::{KEY_LEFT_BRACKET, KEY_NON_US_BACKSLASH, KEY_RIGHT_BRACKET, MOD_LALT, MOD_LSHIFT};

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

#[cfg(any(
    feature = "layout_win_en_gb",
    feature = "layout_win_pt_br",
    feature = "layout_win_de_de",
    feature = "layout_mac_en_gb",
    feature = "layout_mac_pt_br",
    feature = "layout_mac_de_de"
))]
#[derive(Debug, Clone, Copy)]
struct LayoutOverride {
    ch: char,
    usage: u8,
    mods: u8,
}

#[cfg(any(
    feature = "layout_win_en_gb",
    feature = "layout_win_pt_br",
    feature = "layout_win_de_de",
    feature = "layout_mac_en_gb",
    feature = "layout_mac_pt_br",
    feature = "layout_mac_de_de"
))]
fn map_with_overrides(c: char, overrides: &[LayoutOverride]) -> Option<(u8, u8)> {
    for ov in overrides {
        if ov.ch == c {
            return Some((ov.usage, ov.mods));
        }
    }
    char_to_key_us(c)
}

#[cfg(any(
    feature = "layout_win_pt_br",
    feature = "layout_win_de_de",
    feature = "layout_mac_de_de"
))]
const KEY_Q: u8 = crate::KEY_A + (b'Q' - b'A');
#[cfg(feature = "layout_win_pt_br")]
const KEY_W: u8 = crate::KEY_A + (b'W' - b'A');
#[cfg(any(
    feature = "layout_win_pt_br",
    feature = "layout_win_de_de",
    feature = "layout_mac_de_de"
))]
const KEY_Y: u8 = crate::KEY_A + (b'Y' - b'A');
#[cfg(any(
    feature = "layout_win_pt_br",
    feature = "layout_win_de_de",
    feature = "layout_mac_de_de"
))]
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

#[cfg(all(test, feature = "std", feature = "layout_win_de_de"))]
mod tests_win_de_de {
    use crate::{
        compile_and_link, lower_to_flat_with_layout, preprocess, FlatOp, LayoutId,
        PreprocessOptions, KEY_A,
    };

    fn empty_provider<'a>(_: &'a str) -> Option<&'a str> {
        None
    }

    #[test]
    fn layout_switches_text_lowering() {
        let entry = "text(\"y\", 0)\nlayout(\"win_de-DE\")\ntext(\"y\", 0)";
        let _ = preprocess(entry, &PreprocessOptions::default())
            .unwrap_or_else(|e| panic!("preprocess: {:?}", e));
        let owned = compile_and_link(entry, &empty_provider)
            .unwrap_or_else(|e| panic!("compile_and_link: {:?}", e));
        let flat =
            lower_to_flat_with_layout(&owned, LayoutId::Us).expect("lower_to_flat_with_layout");
        let taps: Vec<u8> = flat
            .ops
            .iter()
            .filter_map(|op| match op {
                FlatOp::Tap { usage, .. } => Some(*usage),
                _ => None,
            })
            .collect();
        assert_eq!(taps.len(), 2);
        let key_y = KEY_A + (b'Y' - b'A');
        let key_z = KEY_A + (b'Z' - b'A');
        assert_eq!(taps[0], key_y);
        assert_eq!(taps[1], key_z);
    }
}

#[cfg(all(test, feature = "std", feature = "layout_mac_de_de"))]
mod tests_mac_de_de {
    use crate::{
        compile_and_link, lower_to_flat_with_layout, preprocess, FlatOp, LayoutId,
        PreprocessOptions, KEY_A, MOD_LALT,
    };

    fn empty_provider<'a>(_: &'a str) -> Option<&'a str> {
        None
    }

    #[test]
    fn layout_switches_text_lowering_mac_de() {
        let entry = "layout(\"mac_de-DE\")\ntext(\"@\", 0)";
        let _ = preprocess(entry, &PreprocessOptions::default())
            .unwrap_or_else(|e| panic!("preprocess: {:?}", e));
        let owned = compile_and_link(entry, &empty_provider)
            .unwrap_or_else(|e| panic!("compile_and_link: {:?}", e));
        let flat =
            lower_to_flat_with_layout(&owned, LayoutId::Us).expect("lower_to_flat_with_layout");
        let tap = flat
            .ops
            .iter()
            .find_map(|op| match op {
                FlatOp::Tap { usage, mods } => Some((*usage, *mods)),
                _ => None,
            })
            .expect("expected tap");
        let key_q = KEY_A + (b'Q' - b'A');
        assert_eq!(tap.0, key_q);
        assert_eq!(tap.1, MOD_LALT);
    }
}

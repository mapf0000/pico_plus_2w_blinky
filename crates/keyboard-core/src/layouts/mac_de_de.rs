//! macOS German layout overrides.
use super::{CharMapping, LayoutOverride};
use crate::{
    KEY_0, KEY_2, KEY_5, KEY_6, KEY_7, KEY_8, KEY_9, KEY_BACKSLASH, KEY_COMMA, KEY_DOT, KEY_MINUS,
    KEY_NON_US_BACKSLASH, KEY_RIGHT_BRACKET, KEY_SLASH, KEY_SPACE, KeyTap, MOD_LALT, MOD_LSHIFT,
    Mods,
};

const KEY_Q: crate::Usage = crate::KEY_A.add(b'Q' - b'A');
const KEY_N: crate::Usage = crate::KEY_A.add(b'N' - b'A');
const KEY_Y: crate::Usage = crate::KEY_A.add(b'Y' - b'A');
const KEY_Z: crate::Usage = crate::KEY_A.add(b'Z' - b'A');

const TILDE_SEQ: &[KeyTap] = &[
    KeyTap {
        usage: KEY_N,
        mods: MOD_LALT,
    },
    KeyTap {
        usage: KEY_SPACE,
        mods: Mods::empty(),
    },
];

pub(super) const OVERRIDES: &[LayoutOverride] = &[
    LayoutOverride {
        ch: '"',
        usage: KEY_2,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: '#',
        usage: KEY_BACKSLASH,
        mods: Mods::empty(),
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
        mods: Mods::empty(),
    },
    LayoutOverride {
        ch: '-',
        usage: KEY_SLASH,
        mods: Mods::empty(),
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
        mods: Mods::empty(),
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
        mods: Mods::empty(),
    },
    LayoutOverride {
        ch: 'z',
        usage: KEY_Y,
        mods: Mods::empty(),
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

pub(super) fn map_char(c: char) -> Option<CharMapping> {
    if c == '~' {
        return Some(CharMapping::Seq(TILDE_SEQ));
    }
    super::map_with_overrides(c, OVERRIDES)
}

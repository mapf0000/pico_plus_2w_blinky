//! Windows German layout overrides.
use super::LayoutOverride;
use crate::{
    KEY_0, KEY_2, KEY_6, KEY_7, KEY_8, KEY_9, KEY_BACKSLASH, KEY_COMMA, KEY_DOT, KEY_MINUS,
    KEY_NON_US_BACKSLASH, KEY_RIGHT_BRACKET, KEY_SLASH, KEY_SPACE, MOD_LSHIFT, MOD_RALT, Mods,
};

const KEY_Q: crate::Usage = crate::KEY_A.add(b'Q' - b'A');
const KEY_Y: crate::Usage = crate::KEY_A.add(b'Y' - b'A');
const KEY_Z: crate::Usage = crate::KEY_A.add(b'Z' - b'A');

pub(super) const OVERRIDES: &[LayoutOverride] = &[
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
        mods: Mods::empty(),
    },
    LayoutOverride {
        ch: 'z',
        usage: KEY_Y,
        mods: Mods::empty(),
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
        mods: Mods::empty(),
    },
    LayoutOverride {
        ch: '#',
        usage: KEY_BACKSLASH,
        mods: Mods::empty(),
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
        mods: Mods::empty(),
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
        mods: Mods::empty(),
    },
    LayoutOverride {
        ch: '|',
        usage: KEY_NON_US_BACKSLASH,
        mods: MOD_RALT,
    },
    LayoutOverride {
        ch: '^',
        usage: KEY_SPACE,
        mods: Mods::empty(),
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

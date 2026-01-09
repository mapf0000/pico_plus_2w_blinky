use super::LayoutOverride;
use crate::{
    KEY_BACKSLASH, KEY_GRAVE, KEY_NON_US_BACKSLASH, KEY_RIGHT_BRACKET, KEY_SLASH, KEY_SPACE,
    MOD_LSHIFT, MOD_RALT, Mods,
};

const KEY_Q: crate::Usage = crate::KEY_A.add(b'Q' - b'A');
const KEY_W: crate::Usage = crate::KEY_A.add(b'W' - b'A');

pub(super) const OVERRIDES: &[LayoutOverride] = &[
    LayoutOverride {
        ch: '^',
        usage: KEY_SPACE,
        mods: Mods::empty(),
    },
    LayoutOverride {
        ch: '~',
        usage: KEY_SPACE,
        mods: Mods::empty(),
    },
    LayoutOverride {
        ch: '?',
        usage: KEY_W,
        mods: MOD_RALT,
    },
    LayoutOverride {
        ch: ']',
        usage: KEY_BACKSLASH,
        mods: Mods::empty(),
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
        mods: Mods::empty(),
    },
    LayoutOverride {
        ch: '/',
        usage: KEY_Q,
        mods: MOD_RALT,
    },
    LayoutOverride {
        ch: '[',
        usage: KEY_RIGHT_BRACKET,
        mods: Mods::empty(),
    },
    LayoutOverride {
        ch: '\'',
        usage: KEY_GRAVE,
        mods: Mods::empty(),
    },
    LayoutOverride {
        ch: '"',
        usage: KEY_GRAVE,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: '\\',
        usage: KEY_NON_US_BACKSLASH,
        mods: Mods::empty(),
    },
    LayoutOverride {
        ch: '}',
        usage: KEY_BACKSLASH,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: ';',
        usage: KEY_SLASH,
        mods: Mods::empty(),
    },
    LayoutOverride {
        ch: ':',
        usage: KEY_SLASH,
        mods: MOD_LSHIFT,
    },
];

use super::LayoutOverride;
use crate::{
    KEY_LEFT_BRACKET, KEY_NON_US_BACKSLASH, KEY_RIGHT_BRACKET, MOD_LALT, MOD_LSHIFT, Mods,
};

pub(super) const OVERRIDES: &[LayoutOverride] = &[
    LayoutOverride {
        ch: '"',
        usage: KEY_NON_US_BACKSLASH,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: '\'',
        usage: KEY_NON_US_BACKSLASH,
        mods: Mods::empty(),
    },
    LayoutOverride {
        ch: ':',
        usage: KEY_RIGHT_BRACKET,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: ';',
        usage: KEY_RIGHT_BRACKET,
        mods: Mods::empty(),
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
        mods: MOD_LSHIFT.or(MOD_LALT),
    },
    LayoutOverride {
        ch: '{',
        usage: KEY_LEFT_BRACKET,
        mods: MOD_LSHIFT.or(MOD_LALT),
    },
    LayoutOverride {
        ch: '}',
        usage: KEY_RIGHT_BRACKET,
        mods: MOD_LSHIFT.or(MOD_LALT),
    },
];

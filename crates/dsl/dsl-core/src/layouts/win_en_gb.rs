use super::LayoutOverride;
use crate::{
    KEY_2, KEY_APOSTROPHE, KEY_BACKSLASH, KEY_NON_US_BACKSLASH, MOD_LSHIFT, MOD_RALT, Mods,
};

pub(super) const OVERRIDES: &[LayoutOverride] = &[
    LayoutOverride {
        ch: '#',
        usage: KEY_BACKSLASH,
        mods: Mods::empty(),
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

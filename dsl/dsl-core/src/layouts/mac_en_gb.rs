use super::LayoutOverride;
use crate::{
    Mods, KEY_2, KEY_APOSTROPHE, KEY_BACKSLASH, KEY_NON_US_BACKSLASH, MOD_LSHIFT,
};

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
        ch: '@',
        usage: KEY_APOSTROPHE,
        mods: MOD_LSHIFT,
    },
    LayoutOverride {
        ch: '\\',
        usage: KEY_NON_US_BACKSLASH,
        mods: Mods::empty(),
    },
    LayoutOverride {
        ch: '|',
        usage: KEY_NON_US_BACKSLASH,
        mods: MOD_LSHIFT,
    },
];

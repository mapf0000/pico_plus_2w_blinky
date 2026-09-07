#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(all(feature = "alloc", not(feature = "std")))]
extern crate alloc;

#[cfg(feature = "alloc")]
mod errors;
#[cfg(feature = "alloc")]
mod ir;
mod keycodes;
mod layouts;
mod limits;
#[cfg(feature = "alloc")]
mod lower;

pub mod bytecode;

#[cfg(feature = "alloc")]
pub use errors::{CompileError, Severity, Span, message_for_code};
#[cfg(feature = "alloc")]
pub use ir::{FlatOp, FlatProgram, FlatProgramError, Op, OpOwned, Program, ProgramOwned};
pub use keycodes::{
    ArrayString, KEY_0, KEY_1, KEY_2, KEY_3, KEY_4, KEY_5, KEY_6, KEY_7, KEY_8, KEY_9, KEY_A,
    KEY_APOSTROPHE, KEY_BACKSLASH, KEY_BACKSPACE, KEY_CAPS_LOCK, KEY_COMMA, KEY_DELETE, KEY_DOT,
    KEY_DOWN, KEY_END, KEY_ENTER, KEY_EQUAL, KEY_ESC, KEY_F1, KEY_F2, KEY_F3, KEY_F4, KEY_F5,
    KEY_F6, KEY_F7, KEY_F8, KEY_F9, KEY_F10, KEY_F11, KEY_F12, KEY_F13, KEY_F14, KEY_F15, KEY_F16,
    KEY_F17, KEY_F18, KEY_F19, KEY_F20, KEY_F21, KEY_F22, KEY_F23, KEY_F24, KEY_GRAVE, KEY_HOME,
    KEY_INSERT, KEY_KP_0, KEY_KP_1, KEY_KP_2, KEY_KP_3, KEY_KP_4, KEY_KP_5, KEY_KP_6, KEY_KP_7,
    KEY_KP_8, KEY_KP_9, KEY_KP_ASTERISK, KEY_KP_DOT, KEY_KP_ENTER, KEY_KP_MINUS, KEY_KP_PLUS,
    KEY_KP_SLASH, KEY_LEFT, KEY_LEFT_BRACKET, KEY_MINUS, KEY_NON_US_BACKSLASH, KEY_NON_US_HASH,
    KEY_NUM_LOCK, KEY_PAGE_DOWN, KEY_PAGE_UP, KEY_PAUSE, KEY_PRINT_SCREEN, KEY_RIGHT,
    KEY_RIGHT_BRACKET, KEY_SCROLL_LOCK, KEY_SEMICOLON, KEY_SLASH, KEY_SPACE, KEY_TAB, KEY_UP,
    MOD_LALT, MOD_LCTRL, MOD_LGUI, MOD_LSHIFT, MOD_RALT, MOD_RCTRL, MOD_RGUI, MOD_RSHIFT, Mods,
    Usage, char_to_key_us, parse_key, parse_modtap,
};
pub use layouts::{CharMapping, DEFAULT_LAYOUT_ID, LayoutId, LayoutParseError, available_layouts};
pub use limits::{MAX_DEVICE_DELAY_MS, MAX_EFFECT_DELAY_TOTAL_MS, MAX_TOTAL_FLAT_OPS};
#[cfg(feature = "alloc")]
pub use lower::{lower_to_flat, lower_to_flat_us, lower_to_flat_with_layout};

/// One keyboard usage with its HID modifier bitmap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyTap {
    pub usage: Usage,
    pub mods: Mods,
}

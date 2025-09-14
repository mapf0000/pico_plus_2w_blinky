// This module defines a comprehensive set of HID key codes and helpers.
// Depending on features used, many items may be intentionally unused.
use embassy_time::Timer;
use embassy_usb::class::hid::HidWriter as UsbHidWriter;
use usbd_hid::descriptor::KeyboardReport;

// Maintainability: centralize default HID delays
const DELAY_MOD_DOWN_MS: u64 = 8;
const DELAY_TAP_HOLD_MS: u64 = 20;

/// USB HID Keyboard/Keypad usage codes (page 0x07)
///
/// These constants cover common ANSI keys. Values follow the HID
/// Usage Tables spec and match what `usbd-hid` expects in `KeyboardReport.keycodes`.
/// Letters/numbers are unshifted usage codes; use the `modifier` field to apply Shift.
// Letters
// Use `KEY_A + offset` for other letters (A=0x04).
// Derived letters not listed as constants:
// B(+1), C(+2), D(+3), E(+4), F(+5), G(+6), H(+7), I(+8), J(+9),
// K(+10), L(+11), M(+12), N(+13), O(+14), P(+15), Q(+16), R(+17),
// S(+18), T(+19), U(+20), V(+21), W(+22), X(+23), Y(+24), Z(+25)
pub const KEY_A: u8 = 0x04; // 'A'

// Number row (unshifted values)
pub const KEY_1: u8 = 0x1e; // '1'  (shift: '!')
pub const KEY_2: u8 = 0x1f; // '2'  (shift: '@')
pub const KEY_3: u8 = 0x20; // '3'  (shift: '#')
pub const KEY_4: u8 = 0x21; // '4'  (shift: '$')
pub const KEY_5: u8 = 0x22; // '5'  (shift: '%')
pub const KEY_6: u8 = 0x23; // '6'  (shift: '^')
pub const KEY_7: u8 = 0x24; // '7'  (shift: '&')
pub const KEY_8: u8 = 0x25; // '8'  (shift: '*')
pub const KEY_9: u8 = 0x26; // '9'  (shift: '(')
pub const KEY_0: u8 = 0x27; // '0'  (shift: ')')

// Control and punctuation cluster
pub const KEY_ENTER: u8 = 0x28; // Enter
pub const KEY_ESC: u8 = 0x29; // Escape
pub const KEY_BACKSPACE: u8 = 0x2a; // Backspace
pub const KEY_TAB: u8 = 0x2b; // Tab
pub const KEY_SPACE: u8 = 0x2c; // Space
pub const KEY_MINUS: u8 = 0x2d; // '-'  (shift: '_')
pub const KEY_EQUAL: u8 = 0x2e; // '='  (shift: '+')
pub const KEY_LEFT_BRACKET: u8 = 0x2f; // '[' (shift: '{')
pub const KEY_RIGHT_BRACKET: u8 = 0x30; // ']' (shift: '}')
pub const KEY_BACKSLASH: u8 = 0x31; // '\\' (shift: '|')
pub const KEY_NON_US_HASH: u8 = 0x32; // Non-US # ~ (ISO)
pub const KEY_SEMICOLON: u8 = 0x33; // ';' (shift: ':')
pub const KEY_APOSTROPHE: u8 = 0x34; // '\'' (shift: '"')
pub const KEY_GRAVE: u8 = 0x35; // '`' (shift: '~')
pub const KEY_COMMA: u8 = 0x36; // ',' (shift: '<')
pub const KEY_DOT: u8 = 0x37; // '.' (shift: '>')
pub const KEY_SLASH: u8 = 0x38; // '/' (shift: '?') — ANSI: left of Right Shift
pub const KEY_CAPS_LOCK: u8 = 0x39; // Caps Lock
pub const KEY_NON_US_BACKSLASH: u8 = 0x64; // Non-US '\\' and '|' (ISO)

// Function keys
pub const KEY_F1: u8 = 0x3a;
pub const KEY_F2: u8 = 0x3b;
pub const KEY_F3: u8 = 0x3c;
pub const KEY_F4: u8 = 0x3d;
pub const KEY_F5: u8 = 0x3e;
pub const KEY_F6: u8 = 0x3f;
pub const KEY_F7: u8 = 0x40;
pub const KEY_F8: u8 = 0x41;
pub const KEY_F9: u8 = 0x42;
pub const KEY_F10: u8 = 0x43;
pub const KEY_F11: u8 = 0x44;
pub const KEY_F12: u8 = 0x45;

// Navigation cluster
pub const KEY_PRINT_SCREEN: u8 = 0x46; // Print Screen
pub const KEY_SCROLL_LOCK: u8 = 0x47;  // Scroll Lock
pub const KEY_PAUSE: u8 = 0x48;        // Pause / Break
pub const KEY_INSERT: u8 = 0x49;       // Insert
pub const KEY_HOME: u8 = 0x4a;         // Home
pub const KEY_PAGE_UP: u8 = 0x4b;      // Page Up
pub const KEY_DELETE: u8 = 0x4c;       // Delete Forward
pub const KEY_END: u8 = 0x4d;          // End
pub const KEY_PAGE_DOWN: u8 = 0x4e;    // Page Down
pub const KEY_RIGHT: u8 = 0x4f;        // Right Arrow
pub const KEY_LEFT: u8 = 0x50;         // Left Arrow
pub const KEY_DOWN: u8 = 0x51;         // Down Arrow
pub const KEY_UP: u8 = 0x52;           // Up Arrow

// Keypad
pub const KEY_NUM_LOCK: u8 = 0x53;     // Num Lock / Clear
pub const KEY_KP_SLASH: u8 = 0x54;     // Keypad '/'
pub const KEY_KP_ASTERISK: u8 = 0x55;  // Keypad '*'
pub const KEY_KP_MINUS: u8 = 0x56;     // Keypad '-'
pub const KEY_KP_PLUS: u8 = 0x57;      // Keypad '+'
pub const KEY_KP_ENTER: u8 = 0x58;     // Keypad Enter
pub const KEY_KP_1: u8 = 0x59;         // Keypad '1' / End
pub const KEY_KP_2: u8 = 0x5a;         // Keypad '2' / Down
pub const KEY_KP_3: u8 = 0x5b;         // Keypad '3' / Page Down
pub const KEY_KP_4: u8 = 0x5c;         // Keypad '4' / Left
pub const KEY_KP_5: u8 = 0x5d;         // Keypad '5'
pub const KEY_KP_6: u8 = 0x5e;         // Keypad '6' / Right
pub const KEY_KP_7: u8 = 0x5f;         // Keypad '7' / Home
pub const KEY_KP_8: u8 = 0x60;         // Keypad '8' / Up
pub const KEY_KP_9: u8 = 0x61;         // Keypad '9' / Page Up
pub const KEY_KP_0: u8 = 0x62;         // Keypad '0' / Insert
pub const KEY_KP_DOT: u8 = 0x63;       // Keypad '.' / Delete

/// Modifier bit masks for `KeyboardReport.modifier`
pub const MOD_LCTRL: u8 = 0x01;
pub const MOD_LSHIFT: u8 = 0x02;
pub const MOD_LALT: u8 = 0x04;
pub const MOD_LGUI: u8 = 0x08;
pub const MOD_RCTRL: u8 = 0x10;
pub const MOD_RSHIFT: u8 = 0x20;
pub const MOD_RALT: u8 = 0x40;
pub const MOD_RGUI: u8 = 0x80;

/// Press and release a single usage with an optional modifier.
/// To maximize compatibility across hosts, when a modifier is present we:
/// - press the modifier first,
/// - press the key while holding the modifier,
/// - release the key while still holding the modifier,
/// - then release the modifier.
pub async fn tap_with_mod<'d, D>(w: &mut UsbHidWriter<'d, D, 8>, usage: u8, modifier: u8)
where
    D: embassy_usb::driver::Driver<'d>,
{
    if modifier != 0 {
        // 1) Modifier down
        let mod_down = KeyboardReport {
            keycodes: [0, 0, 0, 0, 0, 0],
            leds: 0,
            modifier,
            reserved: 0,
        };
        if let Err(e) = w.write_serialize(&mod_down).await {
            log::warn!("hid: write error (mod down): {:?}", e);
        }
        Timer::after_millis(DELAY_MOD_DOWN_MS).await;

        // 2) Modifier + key down
        let both_down = KeyboardReport {
            keycodes: [usage, 0, 0, 0, 0, 0],
            leds: 0,
            modifier,
            reserved: 0,
        };
        if let Err(e) = w.write_serialize(&both_down).await {
            log::warn!("hid: write error (mod+key down): {:?}", e);
        }
        Timer::after_millis(DELAY_TAP_HOLD_MS).await;

        // 3) Release key (keep modifier)
        let key_up = KeyboardReport {
            keycodes: [0, 0, 0, 0, 0, 0],
            leds: 0,
            modifier,
            reserved: 0,
        };
        if let Err(e) = w.write_serialize(&key_up).await {
            log::warn!("hid: write error (key up): {:?}", e);
        }
        Timer::after_millis(DELAY_MOD_DOWN_MS).await;

        // 4) Release modifier
        let mod_up = KeyboardReport {
            keycodes: [0, 0, 0, 0, 0, 0],
            leds: 0,
            modifier: 0,
            reserved: 0,
        };
        if let Err(e) = w.write_serialize(&mod_up).await {
            log::warn!("hid: write error (mod up): {:?}", e);
        }
    } else {
        // Simple tap with no modifier
        let press = KeyboardReport {
            keycodes: [usage, 0, 0, 0, 0, 0],
            leds: 0,
            modifier: 0,
            reserved: 0,
        };
        let release = KeyboardReport {
            keycodes: [0, 0, 0, 0, 0, 0],
            leds: 0,
            modifier: 0,
            reserved: 0,
        };
        if let Err(e) = w.write_serialize(&press).await {
            log::warn!("hid: write error (press): {:?}", e);
        }
        Timer::after_millis(DELAY_TAP_HOLD_MS).await;
        if let Err(e) = w.write_serialize(&release).await {
            log::warn!("hid: write error (release): {:?}", e);
        }
    }
}

/// Convert a char (US-ANSI) to `(usage, modifier)`.
/// Returns None if the character is not supported.
pub fn char_to_key(c: char) -> Option<(u8, u8)> {
    match c {
        // Whitespace and control
        '\n' | '\r' => Some((KEY_ENTER, 0)),
        '\t' => Some((KEY_TAB, 0)),
        ' ' => Some((KEY_SPACE, 0)),

        // Letters
        'a'..='z' => {
            let idx = (c as u8) - b'a';
            Some((KEY_A + idx, 0))
        }
        'A'..='Z' => {
            let idx = (c as u8) - b'A';
            Some((KEY_A + idx, MOD_LSHIFT))
        }

        // Number row and shifted symbols
        '1' => Some((KEY_1, 0)),
        '2' => Some((KEY_2, 0)),
        '3' => Some((KEY_3, 0)),
        '4' => Some((KEY_4, 0)),
        '5' => Some((KEY_5, 0)),
        '6' => Some((KEY_6, 0)),
        '7' => Some((KEY_7, 0)),
        '8' => Some((KEY_8, 0)),
        '9' => Some((KEY_9, 0)),
        '0' => Some((KEY_0, 0)),
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

        // Punctuation cluster
        '-' => Some((KEY_MINUS, 0)),
        '_' => Some((KEY_MINUS, MOD_LSHIFT)),
        '=' => Some((KEY_EQUAL, 0)),
        '+' => Some((KEY_EQUAL, MOD_LSHIFT)),
        '[' => Some((KEY_LEFT_BRACKET, 0)),
        '{' => Some((KEY_LEFT_BRACKET, MOD_LSHIFT)),
        ']' => Some((KEY_RIGHT_BRACKET, 0)),
        '}' => Some((KEY_RIGHT_BRACKET, MOD_LSHIFT)),
        '\\' => Some((KEY_BACKSLASH, 0)),
        '|' => Some((KEY_BACKSLASH, MOD_LSHIFT)),
        ';' => Some((KEY_SEMICOLON, 0)),
        ':' => Some((KEY_SEMICOLON, MOD_LSHIFT)),
        '\'' => Some((KEY_APOSTROPHE, 0)),
        '"' => Some((KEY_APOSTROPHE, MOD_LSHIFT)),
        '`' => Some((KEY_GRAVE, 0)),
        '~' => Some((KEY_GRAVE, MOD_LSHIFT)),
        ',' => Some((KEY_COMMA, 0)),
        '<' => Some((KEY_COMMA, MOD_LSHIFT)),
        '.' => Some((KEY_DOT, 0)),
        '>' => Some((KEY_DOT, MOD_LSHIFT)),
        '/' => Some((KEY_SLASH, 0)),
        '?' => Some((KEY_SLASH, MOD_LSHIFT)),

        _ => None,
    }
}

/// Type a string over HID (US-ANSI mapping).
/// - `delay_ms`: delay between key presses
pub async fn type_str<'d, D>(w: &mut UsbHidWriter<'d, D, 8>, s: &str, delay_ms: u64)
where
    D: embassy_usb::driver::Driver<'d>,
{
    for ch in s.chars() {
        if let Some((usage, modifier)) = char_to_key(ch) {
            tap_with_mod(w, usage, modifier).await;
            if delay_ms != 0 {
                Timer::after_millis(delay_ms).await;
            }
        } else {
            log::debug!("hid: skipping unsupported char: {:?}", ch);
        }
    }
}

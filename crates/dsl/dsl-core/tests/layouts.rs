#![cfg(feature = "layout_mac_de_de")]

use dsl_core::{
    FlatOp, KEY_0, KEY_5, KEY_6, KEY_7, KEY_A, KEY_MINUS, KEY_NON_US_BACKSLASH, KEY_SLASH,
    LayoutId, MOD_LALT, MOD_LSHIFT, Mods, compile_and_link, lower_to_flat_with_layout,
};

fn empty_provider(_: &str) -> Option<&str> {
    None
}

#[test]
fn mac_de_layout_override_mappings() {
    let entry = r#"layout("mac_de-DE")
text("@[]{}\\|<>yzYZ_", 0)"#;
    let owned = compile_and_link(entry, &empty_provider).expect("compile_and_link");
    let flat = lower_to_flat_with_layout(&owned, LayoutId::Us).expect("lower_to_flat_with_layout");

    let taps: Vec<(dsl_core::Usage, Mods)> = flat
        .ops
        .iter()
        .map(|op| match op {
            FlatOp::Tap { usage, mods } => (*usage, *mods),
            FlatOp::DelayMs(ms) => panic!("unexpected delay: {}", ms),
        })
        .collect();

    let key_q = KEY_A.add(b'Q' - b'A');
    let key_y = KEY_A.add(b'Y' - b'A');
    let key_z = KEY_A.add(b'Z' - b'A');

    let expected = vec![
        (key_q, MOD_LALT),                     // @
        (KEY_5, MOD_LALT),                     // [
        (KEY_6, MOD_LALT),                     // ]
        (KEY_7, MOD_LALT),                     // {
        (KEY_0, MOD_LALT),                     // }
        (KEY_MINUS, MOD_LALT),                 // \
        (KEY_NON_US_BACKSLASH, MOD_LALT),      // |
        (KEY_NON_US_BACKSLASH, Mods::empty()), // <
        (KEY_NON_US_BACKSLASH, MOD_LSHIFT),    // >
        (key_z, Mods::empty()),                // y
        (key_y, Mods::empty()),                // z
        (key_z, MOD_LSHIFT),                   // Y
        (key_y, MOD_LSHIFT),                   // Z
        (KEY_SLASH, MOD_LSHIFT),               // _
    ];

    assert_eq!(taps, expected);
}

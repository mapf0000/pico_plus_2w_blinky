use embassy_time::Timer;
use embassy_usb::class::hid::HidWriter as UsbHidWriter;

use crate::keyboard::{
    self,
    KEY_ENTER, KEY_N, KEY_SLASH, KEY_SPACE, KEY_Z,
    MOD_LGUI,
};

/// Open Terminal on macOS via Spotlight: Cmd+Space, type "Terminal", Enter.
pub async fn open_macos_terminal<'d, D>(w: &mut UsbHidWriter<'d, D, 8>)
where
    D: embassy_usb::driver::Driver<'d>,
{
    // Open Spotlight
    keyboard::tap_with_mod(w, KEY_SPACE, MOD_LGUI).await; // Cmd+Space
    Timer::after_millis(400).await;

    // Type the app name and confirm
    keyboard::type_str(w, "Terminal", 10).await;
    Timer::after_millis(200).await;
    keyboard::tap(w, KEY_ENTER).await;

    // Give the app time to launch, then ensure a window is open
    Timer::after_millis(1500).await;
    keyboard::tap_with_mod(w, KEY_N, MOD_LGUI).await; // Cmd+N (new window)
}

/// Run the macOS Keyboard Setup Assistant automation sequence.
///
/// Sequence:
///  - Wait ~1.2s, press Enter
///  - Wait ~0.8s, press 'Z' (right of left Shift, ANSI)
///  - Wait ~0.8s, press '/' (left of right Shift, ANSI)
///  - Wait ~0.8s, press Enter to finish
///
/// This future never returns; it idles after completion.
pub async fn run_mac_assistant<'d, D>(mut writer: UsbHidWriter<'d, D, 8>) -> !
where
    D: embassy_usb::driver::Driver<'d>,
{
    writer.ready().await;
    log::info!("usb: HID keyboard ready");

    // Give host time to show the assistant UI.
    Timer::after_millis(1200).await;
    log::info!("usb: sending Enter for keyboard assistant");
    keyboard::tap(&mut writer, KEY_ENTER).await;

    Timer::after_millis(800).await;
    log::info!("usb: sending 'Z' (key right of left Shift)");
    keyboard::tap(&mut writer, KEY_Z).await;

    // Second prompt: key immediately to the left of the right Shift -> '/' on ANSI.
    Timer::after_millis(800).await;
    log::info!("usb: sending '/' (key left of right Shift)");
    keyboard::tap(&mut writer, KEY_SLASH).await;

    // Some macOS versions show a final confirmation screen; hit Enter to finish.
    Timer::after_millis(800).await;
    log::info!("usb: sending Enter to finish assistant");
    keyboard::tap(&mut writer, KEY_ENTER).await;

    // Idle forever.
    loop {
        Timer::after_secs(3600).await;
    }
}

/// Bring the HID keyboard interface up and idle.
///
/// This is useful when you want USB to start without running the
/// macOS Keyboard Setup Assistant sequence, but still ensure the
/// HID writer is fully ready as part of USB initialization.
pub async fn hid_idle<'d, D>(mut writer: UsbHidWriter<'d, D, 8>) -> !
where
    D: embassy_usb::driver::Driver<'d>,
{
    writer.ready().await;
    log::info!("usb: HID keyboard ready (idle mode)");
    // Idle forever; keeps the task alive.
    loop {
        Timer::after_secs(3600).await;
    }
}

/// Perform the macOS Keyboard Setup Assistant sequence once, then return.
///
/// Unlike `run_mac_assistant`, this does not call `writer.ready()` and does not
/// loop indefinitely. Intended for use inside a long-lived HID worker task.
pub async fn mac_assistant_once<'d, D>(w: &mut UsbHidWriter<'d, D, 8>)
where
    D: embassy_usb::driver::Driver<'d>,
{
    // Give host time to show the assistant UI.
    Timer::after_millis(1200).await;
    log::info!("usb: sending Enter for keyboard assistant");
    keyboard::tap(w, KEY_ENTER).await;

    Timer::after_millis(800).await;
    log::info!("usb: sending 'Z' (key right of left Shift)");
    keyboard::tap(w, KEY_Z).await;

    // Second prompt: key immediately to the left of the right Shift -> '/' on ANSI.
    Timer::after_millis(800).await;
    log::info!("usb: sending '/' (key left of right Shift)");
    keyboard::tap(w, KEY_SLASH).await;

    // Some macOS versions show a final confirmation screen; hit Enter to finish.
    Timer::after_millis(800).await;
    log::info!("usb: sending Enter to finish assistant");
    keyboard::tap(w, KEY_ENTER).await;
}

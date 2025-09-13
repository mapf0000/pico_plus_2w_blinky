use embassy_time::Timer;
use embassy_usb::class::hid::HidWriter as UsbHidWriter;
use usbd_hid::descriptor::KeyboardReport;

/// HID key usage codes used by the macOS keyboard setup assistant.
const KEY_ENTER: u8 = 0x28; // Enter
const KEY_Z: u8 = 0x1d;     // 'Z' (ANSI: key right of left Shift)
const KEY_SLASH: u8 = 0x38; // '/' (ANSI: key left of right Shift)

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
    tap(&mut writer, KEY_ENTER).await;

    Timer::after_millis(800).await;
    log::info!("usb: sending 'Z' (key right of left Shift)");
    tap(&mut writer, KEY_Z).await;

    // Second prompt: key immediately to the left of the right Shift -> '/' on ANSI.
    Timer::after_millis(800).await;
    log::info!("usb: sending '/' (key left of right Shift)");
    tap(&mut writer, KEY_SLASH).await;

    // Some macOS versions show a final confirmation screen; hit Enter to finish.
    Timer::after_millis(800).await;
    log::info!("usb: sending Enter to finish assistant");
    tap(&mut writer, KEY_ENTER).await;

    // Idle forever.
    loop {
        Timer::after_secs(3600).await;
    }
}

async fn tap<'d, D>(w: &mut UsbHidWriter<'d, D, 8>, usage: u8)
where
    D: embassy_usb::driver::Driver<'d>,
{
    let press = KeyboardReport { keycodes: [usage, 0, 0, 0, 0, 0], leds: 0, modifier: 0, reserved: 0 };
    let release = KeyboardReport { keycodes: [0, 0, 0, 0, 0, 0], leds: 0, modifier: 0, reserved: 0 };
    let _ = w.write_serialize(&press).await;
    Timer::after_millis(20).await;
    let _ = w.write_serialize(&release).await;
}


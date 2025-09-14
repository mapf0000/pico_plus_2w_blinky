use core::sync::atomic::{AtomicBool, Ordering};

use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, channel::Channel};
use embassy_usb::class::hid::HidWriter as UsbHidWriter;
use heapless::String;

use crate::{automation, keyboard};

// Command channel and state
pub enum HidCommand {
    Type { text: String<256>, delay_ms: u64 },
    OpenMacTerminal,
    MacAssistant,
}

pub static HID_CHAN: Channel<ThreadModeRawMutex, HidCommand, 8> = Channel::new();
pub static USB_READY: AtomicBool = AtomicBool::new(false);

/// Long-lived HID worker: waits for ready, optionally runs mac assistant, then processes commands.
pub async fn run_hid<'d, D>(mut writer: UsbHidWriter<'d, D, 8>, run_mac_assistant_on_start: bool) -> !
where
    D: embassy_usb::driver::Driver<'d>,
{
    writer.ready().await;
    USB_READY.store(true, Ordering::SeqCst);
    log::info!("usb: HID keyboard ready");

    if run_mac_assistant_on_start {
        automation::mac_assistant_once(&mut writer).await;
    }

    loop {
        let cmd = HID_CHAN.receive().await;
        match cmd {
            HidCommand::Type { text, delay_ms } => {
                keyboard::type_str(&mut writer, &text, delay_ms).await;
            }
            HidCommand::OpenMacTerminal => {
                automation::open_macos_terminal(&mut writer).await;
            }
            HidCommand::MacAssistant => {
                automation::mac_assistant_once(&mut writer).await;
            }
        }
    }
}

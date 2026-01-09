use core::sync::atomic::{AtomicBool, Ordering};

pub use bytecode_constants::MAX_BYTECODE;
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, channel::Channel};
use embassy_usb::class::hid::HidWriter as UsbHidWriter;
use firmware_exec::{self, ExecError};
use heapless::Vec;

// Command channel and state
pub enum HidCommand {
    RunBytecode { program: Vec<u8, { MAX_BYTECODE }> },
}

pub static HID_CHAN: Channel<ThreadModeRawMutex, HidCommand, 8> = Channel::new();
pub static USB_READY: AtomicBool = AtomicBool::new(false);

/// Long-lived HID worker: waits for ready, optionally runs mac assistant, then processes commands.
pub async fn run_hid<'d, D>(
    mut writer: UsbHidWriter<'d, D, 8>,
    run_mac_assistant_on_start: bool,
) -> !
where
    D: embassy_usb::driver::Driver<'d>,
{
    writer.ready().await;
    USB_READY.store(true, Ordering::SeqCst);
    log::info!("usb: HID keyboard ready");

    if run_mac_assistant_on_start {
        log::info!("usb: mac assistant requested at startup; expect frontend to queue script");
    }

    loop {
        let cmd = HID_CHAN.receive().await;
        match cmd {
            HidCommand::RunBytecode { program } => {
                if let Err(e) = firmware_exec::exec_bytecode(&mut writer, program.as_slice()).await
                {
                    match e {
                        ExecError::Decode(de) => {
                            log::warn!("bytecode decode error: {:?}", de);
                        }
                        ExecError::TooLong => {
                            log::warn!("bytecode too long; aborting execution");
                        }
                    }
                }
            }
        }
    }
}

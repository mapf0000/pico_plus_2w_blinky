use core::sync::atomic::{AtomicBool, Ordering};

pub use bytecode_constants::MAX_BYTECODE;
use embassy_futures::select::{Either3, select3};
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, channel::Channel, signal::Signal};
use embassy_usb::class::hid::HidWriter as UsbHidWriter;
use heapless::Vec;

// Command channel and state
#[allow(
    clippy::large_enum_variant,
    reason = "the bounded no_std channel owns the single 4096-byte effect without a heap"
)]
pub enum HidCommand {
    RunEffect {
        request_id: u64,
        process_id: u64,
        effect_id: u64,
        program: Vec<u8, { MAX_BYTECODE }>,
    },
}

pub static HID_CHAN: Channel<ThreadModeRawMutex, HidCommand, 1> = Channel::new();
pub static HID_CANCEL: Signal<ThreadModeRawMutex, HidCancel> = Signal::new();
pub static HID_RESULT_CHAN: Channel<ThreadModeRawMutex, HidResult, 8> = Channel::new();
pub static USB_READY: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HidCancel {
    pub process_id: u64,
    pub effect_id: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HidResultStatus {
    Completed,
    Rejected,
    Cancelled,
    UsbUnavailable,
}

#[derive(Clone, Copy, Debug)]
pub struct HidResult {
    pub request_id: u64,
    pub process_id: u64,
    pub effect_id: u64,
    pub status: HidResultStatus,
}

fn send_result(request_id: u64, process_id: u64, effect_id: u64, status: HidResultStatus) {
    let _ = HID_RESULT_CHAN.try_send(HidResult {
        request_id,
        process_id,
        effect_id,
        status,
    });
}

/// Long-lived HID worker: waits for ready, then processes tracked effects.
pub async fn run_hid<'d, D>(mut writer: UsbHidWriter<'d, D, 8>) -> !
where
    D: embassy_usb::driver::Driver<'d>,
{
    writer.ready().await;
    USB_READY.store(true, Ordering::SeqCst);
    log::info!("usb: HID keyboard ready");
    firmware_exec::release_all(&mut writer).await;

    loop {
        let cmd = HID_CHAN.receive().await;
        match cmd {
            HidCommand::RunEffect {
                request_id,
                process_id,
                effect_id,
                program,
            } => {
                if let Some(cancel) = HID_CANCEL.try_take()
                    && cancel.process_id == process_id
                    && cancel.effect_id == effect_id
                {
                    send_result(
                        request_id,
                        process_id,
                        effect_id,
                        HidResultStatus::Cancelled,
                    );
                    continue;
                }
                if let Err(error) = firmware_exec::validate_bytecode(program.as_slice()) {
                    log::warn!("script effect rejected before execution: {:?}", error);
                    send_result(request_id, process_id, effect_id, HidResultStatus::Rejected);
                    continue;
                }

                let execution = firmware_exec::exec_bytecode(&mut writer, program.as_slice());
                match select3(execution, HID_CHAN.receive(), HID_CANCEL.wait()).await {
                    Either3::First(Ok(())) => send_result(
                        request_id,
                        process_id,
                        effect_id,
                        HidResultStatus::Completed,
                    ),
                    Either3::First(Err(error)) => {
                        log::warn!("validated script effect failed: {:?}", error);
                        firmware_exec::release_all(&mut writer).await;
                        let status = match error {
                            firmware_exec::ExecError::Usb(_) => {
                                USB_READY.store(false, Ordering::SeqCst);
                                HidResultStatus::UsbUnavailable
                            }
                            firmware_exec::ExecError::Decode(_) => HidResultStatus::Rejected,
                        };
                        send_result(request_id, process_id, effect_id, status);
                        if status == HidResultStatus::UsbUnavailable {
                            writer.ready().await;
                            firmware_exec::release_all(&mut writer).await;
                            USB_READY.store(true, Ordering::SeqCst);
                        }
                    }
                    Either3::Third(_cancel) => {
                        firmware_exec::release_all(&mut writer).await;
                        send_result(
                            request_id,
                            process_id,
                            effect_id,
                            HidResultStatus::Cancelled,
                        );
                    }
                    Either3::Second(other) => {
                        // Only one effect is supported. Receiving another command while an
                        // effect is active cancels the old effect rather than leaving keys down.
                        firmware_exec::release_all(&mut writer).await;
                        send_result(
                            request_id,
                            process_id,
                            effect_id,
                            HidResultStatus::Cancelled,
                        );
                        let HidCommand::RunEffect {
                            request_id,
                            process_id,
                            effect_id,
                            ..
                        } = other;
                        send_result(request_id, process_id, effect_id, HidResultStatus::Rejected);
                    }
                }
            }
        }
    }
}

use core::ptr::NonNull;
use core::sync::atomic::Ordering;

use embassy_futures::join::join5;
use embassy_futures::select::{Either, select};
use embassy_rp::clocks::RoscRng;
use embassy_rp::peripherals::USB;
use embassy_rp::usb::Driver as UsbDriver;
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::Timer;
use embassy_usb::class::cdc_acm::{CdcAcmClass as UsbCdcAcmClass, State as UsbCdcState};
use embassy_usb::class::hid::{HidWriter as UsbHidWriter, State as UsbHidState};
use embassy_usb::{Builder as UsbBuilder, Config as UsbConfig};
use log::Record;
use static_cell::ConstStaticCell;
use usbd_hid::descriptor::{KeyboardReport, SerializedDescriptor};

// ===== USB-local constants =====

const USB_CFG_MAX_POWER_MA: u16 = 100; // UsbConfig::max_power expects u16 (mA)
const USB_CTRL_BUF_LEN: usize = 128;
const USB_DESC_BUF_LEN: usize = 1024;
const USB_MAX_PACKET_SIZE_0: u8 = 64;
const HID_POLL_MS: u8 = 10;
const USB_LOGGER_BUF: usize = 1024;

// Force macOS Keyboard Setup Assistant on every boot by varying PID/serial
const USB_FORCE_ASSISTANT_EACH_BOOT: bool = false;

struct IdentityStrings {
    manufacturer: heapless::String<{ crate::device_config::MANUFACTURER_MAX }>,
    product: heapless::String<{ crate::device_config::PRODUCT_MAX }>,
}

impl IdentityStrings {
    const fn new() -> Self {
        Self {
            manufacturer: heapless::String::new(),
            product: heapless::String::new(),
        }
    }
}

static IDENTITY_STORAGE: ConstStaticCell<IdentityStrings> =
    ConstStaticCell::new(IdentityStrings::new());
static mut IDENTITY_PTR: Option<NonNull<IdentityStrings>> = None;

fn identity_strings() -> &'static mut IdentityStrings {
    unsafe {
        if let Some(ptr) = IDENTITY_PTR {
            let mut ptr = ptr;
            return ptr.as_mut();
        }
        let reference = IDENTITY_STORAGE.take();
        let mut ptr = NonNull::from(reference);
        IDENTITY_PTR = Some(ptr);
        ptr.as_mut()
    }
}

#[allow(dead_code)]
static SERIAL_STORAGE: ConstStaticCell<heapless::String<16>> =
    ConstStaticCell::new(heapless::String::new());
#[allow(dead_code)]
static mut SERIAL_PTR: Option<NonNull<heapless::String<16>>> = None;

#[allow(dead_code)]
fn serial_buffer() -> &'static mut heapless::String<16> {
    unsafe {
        if let Some(ptr) = SERIAL_PTR {
            let mut ptr = ptr;
            return ptr.as_mut();
        }
        let reference = SERIAL_STORAGE.take();
        let mut ptr = NonNull::from(reference);
        SERIAL_PTR = Some(ptr);
        ptr.as_mut()
    }
}

#[embassy_executor::task]
pub async fn usb_task(
    cancel: &'static Signal<ThreadModeRawMutex, u64>,
    _start_req: &'static Signal<ThreadModeRawMutex, bool>,
) -> ! {
    loop {
        // Auto-start USB immediately (was waiting for HTTP/WebSocket trigger).
        let run_mac_assistant = false;

        if !crate::usb::usb_supervisor::USB_ENABLED.load(Ordering::SeqCst) {
            // Request was cancelled before bring-up completed.
            crate::usb::hid::USB_READY.store(false, Ordering::SeqCst);
            continue;
        }

        crate::usb::usb_supervisor::notify_started();
        crate::usb::hid::USB_READY.store(false, Ordering::SeqCst);
        crate::usb::ctrl::CTRL_READY.store(false, Ordering::SeqCst);

        let mut rng = RoscRng;
        let cfg = build_usb_config(&mut rng).await;

        let driver = UsbDriver::new(unsafe { USB::steal() }, crate::Irqs);

        // Descriptor/control buffers
        let mut config_descriptor = [0u8; USB_DESC_BUF_LEN];
        let mut bos_descriptor = [0u8; USB_DESC_BUF_LEN];
        let mut msos_descriptor = [0u8; USB_DESC_BUF_LEN];
        let mut control_buf = [0u8; USB_CTRL_BUF_LEN];

        // Class states (logger CDC + control CDC + HID + MSC)
        let mut log_cdc_state = UsbCdcState::new();
        let mut ctrl_cdc_state = UsbCdcState::new();
        let mut hid_state = UsbHidState::new();
        let mut msc_state = crate::usb::msc::State::new();

        // Build USB device + classes
        let mut builder = UsbBuilder::new(
            driver,
            cfg,
            &mut config_descriptor,
            &mut bos_descriptor,
            &mut msos_descriptor,
            &mut control_buf,
        );

        // CDC-ACM class used by embassy-usb-logger
        let logger_class = UsbCdcAcmClass::new(
            &mut builder,
            &mut log_cdc_state,
            embassy_usb_logger::MAX_PACKET_SIZE as u16,
        );
        let ctrl_class = UsbCdcAcmClass::new(
            &mut builder,
            &mut ctrl_cdc_state,
            embassy_usb_logger::MAX_PACKET_SIZE as u16,
        );

        // HID keyboard (IN only)
        let hid_cfg = embassy_usb::class::hid::Config {
            report_descriptor: KeyboardReport::desc(),
            request_handler: None,
            poll_ms: HID_POLL_MS,
            max_packet_size: 64,
        };
        let hid_writer: UsbHidWriter<'_, _, 8> =
            UsbHidWriter::new(&mut builder, &mut hid_state, hid_cfg);

        let mut msc_class = crate::usb::msc::MscClass::new(&mut builder, &mut msc_state, 64);

        // Finalize device
        let mut usb = builder.build();

        // Futures
        let usb_fut = usb.run();
        let log_fut = embassy_usb_logger::with_custom_style!(
            USB_LOGGER_BUF,
            log::LevelFilter::Info,
            logger_class,
            usb_log_style
        );
        let hid_fut = crate::usb::hid::run_hid(hid_writer, run_mac_assistant);
        let ctrl_fut = crate::usb::ctrl::run_ctrl(ctrl_class);
        let msc_fut = msc_class.run(crate::usb::msc::image());

        // Run device, logger, HID, control CDC, and MSC concurrently, but exit early on cancel.
        let quartet = join5(usb_fut, log_fut, hid_fut, ctrl_fut, msc_fut);
        let mut detach_delay_ms: Option<u64> = None;
        match select(cancel.wait(), quartet).await {
            Either::First(delay) => {
                if delay > 0 {
                    log::info!("usb: cancel received; delaying detach by {} ms", delay);
                    detach_delay_ms = Some(delay);
                } else {
                    log::info!("usb: cancel received; stopping device");
                }
            }
            Either::Second(_) => {
                log::warn!("usb: unexpected completion of task group");
            }
        }

        if let Some(delay) = detach_delay_ms {
            Timer::after_millis(delay).await;
        }
        usb.disable().await;
        crate::usb::hid::USB_READY.store(false, Ordering::SeqCst);
        crate::usb::ctrl::CTRL_READY.store(false, Ordering::SeqCst);
        crate::usb::usb_supervisor::notify_stopped();
    }
}

fn usb_log_style(record: &Record, writer: &mut embassy_usb_logger::Writer<'_, USB_LOGGER_BUF>) {
    use core::fmt::Write as _;
    crate::log_buffer::push_record(record);
    let _ = write!(writer, "{}\r\n", record.args());
}

async fn build_usb_config(rng: &mut RoscRng) -> UsbConfig<'static> {
    let pid = select_pid(rng);
    let mut cfg = UsbConfig::new(0x1209, pid); // pid.codes style VID/PID (dummy)

    // Read current device identity from runtime config
    let dev_cfg = crate::device_config::get().await;
    let identity = identity_strings();
    identity.manufacturer.clear();
    let _ = identity
        .manufacturer
        .push_str(dev_cfg.usb_manufacturer.as_str());
    identity.product.clear();
    let _ = identity.product.push_str(dev_cfg.usb_product.as_str());
    cfg.manufacturer = Some(identity.manufacturer.as_str());
    cfg.product = Some(identity.product.as_str());

    if USB_FORCE_ASSISTANT_EACH_BOOT {
        use core::fmt::Write as _;
        let serial = serial_buffer();
        serial.clear();
        let r = rng.next_u32();
        let _ = write!(serial, "{:08X}", r);
        cfg.serial_number = Some(serial.as_str());
    } else {
        cfg.serial_number = None;
    }

    cfg.max_power = USB_CFG_MAX_POWER_MA;
    cfg.max_packet_size_0 = USB_MAX_PACKET_SIZE_0;
    cfg
}

fn select_pid(rng: &mut RoscRng) -> u16 {
    if USB_FORCE_ASSISTANT_EACH_BOOT {
        if (rng.next_u32() & 1) == 0 {
            0x0001
        } else {
            0x0002
        }
    } else {
        0x0001
    }
}

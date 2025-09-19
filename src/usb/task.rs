use embassy_futures::join::join3;
use embassy_futures::select::{select, Either};
use embassy_rp::clocks::RoscRng;
use embassy_rp::peripherals::USB;
use embassy_rp::usb::Driver as UsbDriver;
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_usb::class::cdc_acm::{CdcAcmClass as UsbCdcAcmClass, State as UsbCdcState};
use embassy_usb::class::hid::{HidWriter as UsbHidWriter, State as UsbHidState};
use embassy_usb::{Builder as UsbBuilder, Config as UsbConfig};
use static_cell::StaticCell;
use usbd_hid::descriptor::{KeyboardReport, SerializedDescriptor};

// ===== USB-local constants =====

const USB_CFG_MAX_POWER_MA: u16 = 100; // UsbConfig::max_power expects u16 (mA)
const USB_CTRL_BUF_LEN: usize = 64;
const USB_DESC_BUF_LEN: usize = 256;
const USB_MAX_PACKET_SIZE_0: u8 = 64;
const HID_POLL_MS: u8 = 10;

// Force macOS Keyboard Setup Assistant on every boot by varying PID/serial
const USB_FORCE_ASSISTANT_EACH_BOOT: bool = false;

/// USB task: composite device with CDC logger + HID keyboard
#[embassy_executor::task]
pub async fn usb_task(
    driver: UsbDriver<'static, USB>,
    cancel: &'static embassy_sync::signal::Signal<ThreadModeRawMutex, ()>,
    start_req: &'static embassy_sync::signal::Signal<ThreadModeRawMutex, bool>,
    _unused: bool,
) {
    // Wait for start request from supervisor before bringing up USB
    let run_mac_assistant = start_req.wait().await;
    // --- Device configuration ---
    let mut rng = RoscRng;

    // Choose a PID depending on whether we want to force macOS assistant each boot.
    let pid = if USB_FORCE_ASSISTANT_EACH_BOOT {
        if (rng.next_u32() & 1) == 0 {
            0x0001
        } else {
            0x0002
        }
    } else {
        0x0001
    };

    let mut cfg = UsbConfig::new(0x1209, pid); // pid.codes style VID/PID (dummy)

    // Read current device identity from runtime config
    let dev_cfg = crate::device_config::get().await;
    static MANUF: StaticCell<heapless::String<{ crate::device_config::MANUFACTURER_MAX }>> =
        StaticCell::new();
    static PROD: StaticCell<heapless::String<{ crate::device_config::PRODUCT_MAX }>> =
        StaticCell::new();
    let mref = MANUF.init(dev_cfg.usb_manufacturer);
    let pref = PROD.init(dev_cfg.usb_product);
    cfg.manufacturer = Some(mref.as_str());
    cfg.product = Some(pref.as_str());

    if USB_FORCE_ASSISTANT_EACH_BOOT {
        use core::fmt::Write as _;
        static SN: StaticCell<heapless::String<16>> = StaticCell::new();
        let s = SN.init(heapless::String::new());
        let r = rng.next_u32();
        let _ = write!(s, "{:08X}", r);
        cfg.serial_number = Some(s.as_str());
    } else {
        cfg.serial_number = None;
    }

    cfg.max_power = USB_CFG_MAX_POWER_MA;
    cfg.max_packet_size_0 = USB_MAX_PACKET_SIZE_0;

    // Descriptor/control buffers
    let mut config_descriptor = [0u8; USB_DESC_BUF_LEN];
    let mut bos_descriptor = [0u8; USB_DESC_BUF_LEN];
    let mut msos_descriptor = [0u8; USB_DESC_BUF_LEN];
    let mut control_buf = [0u8; USB_CTRL_BUF_LEN];

    // Class states (logger CDC + HID)
    let mut log_cdc_state = UsbCdcState::new();
    let mut hid_state = UsbHidState::new();

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

    // HID keyboard (IN only)
    let hid_cfg = embassy_usb::class::hid::Config {
        report_descriptor: KeyboardReport::desc(),
        request_handler: None,
        poll_ms: HID_POLL_MS,
        max_packet_size: 64,
    };
    let hid_writer: UsbHidWriter<'_, _, 8> =
        UsbHidWriter::new(&mut builder, &mut hid_state, hid_cfg);

    // Finalize device
    let mut usb = builder.build();

    // Futures
    let usb_fut = usb.run();
    let log_fut = embassy_usb_logger::with_class!(1024, log::LevelFilter::Info, logger_class);
    let hid_fut = crate::usb::hid::run_hid(hid_writer, run_mac_assistant);

    // Run device, logger and HID concurrently, but exit early on cancel.
    let trio = join3(usb_fut, log_fut, hid_fut);
    match select(cancel.wait(), trio).await {
        Either::First(_) => {
            log::info!("usb: cancel received; stopping device");
        }
        Either::Second(_) => {
            log::warn!("usb: unexpected completion of trio");
        }
    }
    // Clear flags and notify supervisor
    crate::usb::usb_supervisor::USB_ENABLED.store(false, core::sync::atomic::Ordering::SeqCst);
    crate::usb::usb_supervisor::notify_stopped();
}


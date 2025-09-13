#![no_std]
#![no_main]

use cyw43_pio::{PioSpi, RM2_CLOCK_DIVIDER}; // RM2 divider recommended on Pico Plus 2 W
use embassy_executor::Spawner;
use embassy_net::{self as net, Config, StackResources};
use embassy_rp::bind_interrupts;
use embassy_rp::clocks::RoscRng;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH0, PIO0, USB};
use embassy_rp::pio::Pio;
use embassy_rp::usb::Driver as UsbDriver;
use embassy_usb::{Builder as UsbBuilder, Config as UsbConfig};
use embassy_usb::class::cdc_acm::{CdcAcmClass as UsbCdcAcmClass, State as UsbCdcState};
use embassy_usb::class::hid::{HidWriter as UsbHidWriter, State as UsbHidState};
use embassy_futures::join::join3;
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, signal::Signal};
use core::sync::atomic::{AtomicBool, Ordering};
use usbd_hid::descriptor::{KeyboardReport, SerializedDescriptor};
use embassy_time::Timer;
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct Irqs {
    // USB controller interrupt for the logger
    USBCTRL_IRQ => embassy_rp::usb::InterruptHandler<USB>;
    // PIO interrupt for CYW43 PIO-SPI
    PIO0_IRQ_0  => embassy_rp::pio::InterruptHandler<PIO0>;
});

// Global signal to trigger on-demand USB bring-up from HTTP handler
pub static USB_START: Signal<ThreadModeRawMutex, ()> = Signal::new();
pub static USB_ENABLED: AtomicBool = AtomicBool::new(false);

/// USB task: composite device with CDC logger + HID keyboard
#[embassy_executor::task]
async fn usb_task(driver: UsbDriver<'static, USB>) {
    // --- Device configuration ---
    // Force macOS to show Keyboard Setup Assistant on every boot by presenting
    // a different Product ID and random serial number. Disable by setting the
    // constant to false.
    const FORCE_ASSISTANT_EACH_BOOT: bool = true;
    let mut rng = RoscRng;
    let pid = if FORCE_ASSISTANT_EACH_BOOT {
        if (rng.next_u32() & 1) == 0 { 0x0001 } else { 0x0002 }
    } else {
        0x0001
    };
    let mut cfg = UsbConfig::new(0x1209, pid); // pid.codes style VID/PID (dummy)
    cfg.manufacturer = Some("Pico 2W");
    cfg.product = Some("Logger + Keyboard");
    if FORCE_ASSISTANT_EACH_BOOT {
        use core::fmt::Write as _;
        static SN: StaticCell<heapless::String<16>> = StaticCell::new();
        let s = SN.init(heapless::String::new());
        let r = rng.next_u32();
        let _ = write!(s, "{:08X}", r);
        cfg.serial_number = Some(s.as_str());
    } else {
        cfg.serial_number = None;
    }
    cfg.max_power = 100;
    cfg.max_packet_size_0 = 64;

    // Descriptor and control buffers
    let mut config_descriptor = [0u8; 256];
    let mut bos_descriptor = [0u8; 256];
    let mut msos_descriptor = [0u8; 256];
    let mut control_buf = [0u8; 64];

    // Class states
    let mut cdc_state = UsbCdcState::new();
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
    let logger_class = UsbCdcAcmClass::new(&mut builder, &mut cdc_state, embassy_usb_logger::MAX_PACKET_SIZE as u16);

    // HID keyboard (IN only)
    let hid_cfg = embassy_usb::class::hid::Config {
        report_descriptor: KeyboardReport::desc(),
        request_handler: None,
        poll_ms: 10,
        max_packet_size: 64,
    };
    let hid_writer: UsbHidWriter<'_, _, 8> = UsbHidWriter::new(&mut builder, &mut hid_state, hid_cfg);

    // Finalize device
    let mut usb = builder.build();

    // Futures
    let usb_fut = usb.run();
    let log_fut = embassy_usb_logger::with_class!(1024, log::LevelFilter::Info, logger_class);
    let hid_fut = crate::keyboard::run_mac_assistant(hid_writer);

    // Run device, logger and HID concurrently.
    join3(usb_fut, log_fut, hid_fut).await;
}

/// Drives low-level CYW43 events
#[embassy_executor::task]
async fn cyw43_task(
    runner: cyw43::Runner<'static, Output<'static>, PioSpi<'static, PIO0, 0, DMA_CH0>>,
) -> ! {
    runner.run().await
}

/// Drives the network stack
#[embassy_executor::task]
async fn net_task(mut runner: net::Runner<'static, cyw43::NetDriver<'static>>) -> ! {
    runner.run().await
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    // Do not start USB at boot. It will be started on POST /usb/register

    let fw = include_bytes!("../cyw43-firmware/43439A0.bin");
    let clm = include_bytes!("../cyw43-firmware/43439A0_clm.bin");

    let pwr = Output::new(p.PIN_23, Level::Low);
    let cs = Output::new(p.PIN_25, Level::High);
    let mut pio = Pio::new(p.PIO0, Irqs);
    log::info!("pio: initializing CYW43 PIO-SPI interface");
    let spi = PioSpi::new(
        &mut pio.common,
        pio.sm0,
        RM2_CLOCK_DIVIDER, // important for RM2-based Pico Plus 2 W
        pio.irq0,
        cs,
        p.PIN_24, // MOSI
        p.PIN_29, // MISO
        p.DMA_CH0,
    );

    static STATE: StaticCell<cyw43::State> = StaticCell::new();
    let state = STATE.init(cyw43::State::new());
    log::info!("cyw43: loading firmware and bringing up chip");
    let (net_device, mut control, cyw_runner) = cyw43::new(state, pwr, spi, fw).await;
    log::info!("cyw43: init complete; spawning runner");
    spawner.spawn(cyw43_task(cyw_runner)).unwrap();

    log::info!("cyw43: applying CLM/regulatory data");
    control.init(clm).await;
    control
        .set_power_management(cyw43::PowerManagementMode::PowerSave)
        .await;
    log::info!("cyw43: power management set to PowerSave");

    // --- Embassy net stack (Static IPv4 for AP mode) ---
    static NET_RES: StaticCell<StackResources<3>> = StaticCell::new();
    let mut rng = RoscRng;
    let seed = rng.next_u64();
    log::info!("net: rng seeded with 0x{:08x}{:08x}", (seed >> 32) as u32, seed as u32);

    // Configure a static IP for the AP interface, e.g. 192.168.4.1/24
    let cfg = Config::ipv4_static(embassy_net::StaticConfigV4 {
        address: embassy_net::Ipv4Cidr::new(embassy_net::Ipv4Address::new(192, 168, 4, 1), 24),
        gateway: None,
        dns_servers: Default::default(),
    });
    let (stack_val, net_runner) =
        net::new(net_device, cfg, NET_RES.init(StackResources::new()), seed);
    static NET_STACK: StaticCell<net::Stack<'static>> = StaticCell::new();
    let stack = NET_STACK.init(stack_val);
    spawner.spawn(net_task(net_runner)).unwrap();
    log::info!("net: stack runner spawned (static IPv4)");

    // --- Bring up a WPA2-protected Access Point ---
    const AP_SSID: &str = "PicoEndpoint";
    const AP_PASS: &str = "pico12345"; // 8+ chars per WPA2 requirements
    const AP_CHANNEL: u8 = 6;
    log::info!("wifi: starting AP '{}' on channel {}", AP_SSID, AP_CHANNEL);
    control.start_ap_wpa2(AP_SSID, AP_PASS, AP_CHANNEL).await;
    log::info!("wifi: AP started; clients can connect to '{}'", AP_SSID);

    // Wait for DHCP/stack to be usable
    log::info!("net: waiting for stack config");
    stack.wait_config_up().await;
    log::info!("net: up: {:?}", stack.config_v4());

    // Start a tiny HTTP server to receive the USB trigger
    spawner.spawn(http::server_task(stack)).unwrap();
    log::info!("http: server task spawned (port 80)");

    // Wait for POST /usb/register to arrive, then bring up USB.
    USB_START.wait().await;
    if !USB_ENABLED.swap(true, Ordering::SeqCst) {
        log::info!("usb: starting composite (logger + keyboard) after HTTP trigger");
        let usb_driver = UsbDriver::new(p.USB, Irqs);
        spawner.spawn(usb_task(usb_driver)).unwrap();
    }

    // Main can park; tasks run forever.
    core::future::pending::<()>().await;
}

mod http;
mod keyboard;

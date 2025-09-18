#![no_std]
#![no_main]

// ===== Imports =====

use core::sync::atomic::{AtomicBool, Ordering};

use cyw43_pio::PioSpi; // RM2 divider is used directly via RM2_CLOCK_DIVIDER
use cyw43_pio::RM2_CLOCK_DIVIDER;
use embassy_executor::Spawner;
use embassy_futures::join::join3;
use embassy_net::{self as net, Config, StackResources};
use embassy_rp::bind_interrupts;
use embassy_rp::clocks::RoscRng;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH0, PIO0, USB};
use embassy_rp::pio::Pio;
use embassy_rp::usb::Driver as UsbDriver;
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, signal::Signal};
use embassy_usb::class::cdc_acm::{CdcAcmClass as UsbCdcAcmClass, State as UsbCdcState};
use embassy_usb::class::hid::{HidWriter as UsbHidWriter, State as UsbHidState};
use embassy_usb::{Builder as UsbBuilder, Config as UsbConfig};
use static_cell::StaticCell;
use usbd_hid::descriptor::{KeyboardReport, SerializedDescriptor};

use crate::http::spawn_http_server_pool;

use {defmt_rtt as _, panic_probe as _};

// ===== Interrupt bindings =====

bind_interrupts!(struct Irqs {
    // USB controller interrupt for the logger
    USBCTRL_IRQ => embassy_rp::usb::InterruptHandler<USB>;
    // PIO interrupt for CYW43 PIO-SPI
    PIO0_IRQ_0  => embassy_rp::pio::InterruptHandler<PIO0>;
});

// ===== Globals =====

// Global signal to trigger on-demand USB bring-up from HTTP handler
pub static USB_START: Signal<ThreadModeRawMutex, bool> = Signal::new();
pub static USB_ENABLED: AtomicBool = AtomicBool::new(false);

// ===== Module-local constants (small & auditable) =====

const USB_CFG_MAX_POWER_MA: u16 = 100; // UsbConfig::max_power expects u16 (mA)
const USB_CTRL_BUF_LEN: usize = 64;
const USB_DESC_BUF_LEN: usize = 256;
const USB_MAX_PACKET_SIZE_0: u8 = 64;
const HID_POLL_MS: u8 = 10;

const AP_SSID: &str = "PicoEndpoint";
const AP_PASS: &str = "pico12345"; // WPA2: 8+ chars
const AP_CHANNEL: u8 = 6;

// Force macOS Keyboard Setup Assistant on every boot by varying PID/serial
const USB_FORCE_ASSISTANT_EACH_BOOT: bool = false;

// ===== Small utilities =====

#[inline]
fn log_spawn<T, E: core::fmt::Debug>(name: &str, res: Result<T, E>) -> bool {
    match res {
        Ok(_) => true,
        Err(e) => {
            log::error!("spawn {} failed: {:?}", name, e);
            false
        }
    }
}

/// Start early USB (CDC logger + HID) for immediate logging at boot.
fn early_usb_logging(spawner: &Spawner, driver: UsbDriver<'static, USB>) {
    log::info!("usb: early start at boot");
    if log_spawn("usb_task", spawner.spawn(usb_task(driver, false))) {
        USB_ENABLED.store(true, Ordering::SeqCst);
    }
}

/// Seed RNG and log the value; returns the seed.
fn seed_rng() -> u64 {
    let mut rng = RoscRng;
    let seed = rng.next_u64();
    log::info!(
        "net: rng seeded with 0x{:08x}{:08x}",
        (seed >> 32) as u32,
        seed as u32
    );
    seed
}

/// Initialize the network stack with a static IPv4 config for AP mode.
fn init_net_stack(
    net_device: cyw43::NetDriver<'static>,
    seed: u64,
) -> (
    &'static net::Stack<'static>,
    net::Runner<'static, cyw43::NetDriver<'static>>,
) {
    // Increase socket pool: accommodate DHCP server + HTTP listener + multiple connections.
    // "3" can starve the HTTP server; 16 leaves headroom without being excessive.
    static NET_RES: StaticCell<StackResources<16>> = StaticCell::new();
    static NET_STACK: StaticCell<net::Stack<'static>> = StaticCell::new();

    // Static AP gateway: 192.168.4.1/24
    let cfg = Config::ipv4_static(embassy_net::StaticConfigV4 {
        address: embassy_net::Ipv4Cidr::new(embassy_net::Ipv4Address::new(192, 168, 4, 1), 24),
        gateway: None,
        dns_servers: Default::default(),
    });

    let (stack_val, runner) = net::new(net_device, cfg, NET_RES.init(StackResources::new()), seed);
    let stack = NET_STACK.init(stack_val);
    log::info!("net: stack runner prepared (static IPv4)");
    (stack, runner)
}

/// Start WPA2 AP with constants above.
async fn start_access_point(control: &mut cyw43::Control<'static>) {
    log::info!("wifi: starting AP '{}' on channel {}", AP_SSID, AP_CHANNEL);
    control.start_ap_wpa2(AP_SSID, AP_PASS, AP_CHANNEL).await;
    log::info!("wifi: AP started; clients can connect to '{}'", AP_SSID);
}

/// Spawn DHCP server.
fn spawn_dhcp(spawner: &Spawner, stack: &'static net::Stack<'static>) -> bool {
    let ok = log_spawn("dhcp::server_task", spawner.spawn(dhcp::server_task(stack)));
    if ok {
        log::info!("dhcp: server task spawned (port 67)");
    }
    ok
}

/// Spawn tiny HTTP server for the USB trigger endpoint.
fn spawn_http(spawner: &Spawner, stack: &'static net::Stack<'static>) -> bool {
    // Original function takes &Spawner and Stack by value (Copy), preserve call style.
    spawn_http_server_pool(spawner, *stack);
    log::info!("http: server task spawned (port 80)");
    true
}

// ===== Tasks =====

/// USB task: composite device with CDC logger + HID keyboard
#[embassy_executor::task]
async fn usb_task(driver: UsbDriver<'static, USB>, run_mac_assistant: bool) {
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
    let hid_fut = crate::hid::run_hid(hid_writer, run_mac_assistant);

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

// ===== Orchestration =====

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    // 0) Peripherals
    let p = embassy_rp::init(Default::default());

    // 1) Early USB for logs (CDC) + HID (kept running after)
    let usb_driver = UsbDriver::new(p.USB, Irqs);
    early_usb_logging(&spawner, usb_driver);

    // 2) Runtime config + (optional) PSRAM + flash persistence
    crate::device_config::init().await;

    #[cfg(feature = "psram")]
    {
        psram_pool::init(&p).await;
    }

    let flash_drv = embassy_rp::flash::Flash::<
        _,
        embassy_rp::flash::Blocking,
        { crate::device_config::FLASH_CAPACITY },
    >::new_blocking(p.FLASH);
    crate::device_config::set_flash_driver(flash_drv).await;

    // --- CYW43 bring-up via PIO-SPI ---
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
    let _ = log_spawn("cyw43_task", spawner.spawn(cyw43_task(cyw_runner)));

    log::info!("cyw43: applying CLM/regulatory data");
    control.init(clm).await;
    control
        .set_power_management(cyw43::PowerManagementMode::None)
        .await;
    log::info!("cyw43: power management set to None (AP mode)");

    // --- Embassy net stack (Static IPv4 for AP mode) ---
    let seed = seed_rng();
    let (stack, net_runner) = init_net_stack(net_device, seed);
    let _ = log_spawn("net_task", spawner.spawn(net_task(net_runner)));
    log::info!("net: stack runner spawned (static IPv4)");

    // --- Bring up a WPA2-protected Access Point ---
    start_access_point(&mut control).await;

    // Wait for DHCP/stack to be usable
    log::info!("net: waiting for stack config");
    stack.wait_config_up().await;
    log::info!("net: up: {:?}", stack.config_v4());

    // Start DHCP server for AP clients
    let _ = spawn_dhcp(&spawner, stack);

    // Start a tiny HTTP server to receive the USB trigger (don’t block on config)
    let _ = spawn_http(&spawner, stack);

    // USB is already running; HTTP endpoint will report it as enabled.

    // Park forever; tasks run forever.
    core::future::pending::<()>().await;
}

// ===== Submodules (kept declared; implementations live in their own files) =====

mod device_config;
mod dhcp;
mod hid;
mod host;
mod http;
mod keyboard;
#[cfg(feature = "psram")]
mod psram_pool;
mod script_dsl;
mod scripts;
// mod usb_ctrl; // disabled: control CDC removed to keep only keyboard + logging

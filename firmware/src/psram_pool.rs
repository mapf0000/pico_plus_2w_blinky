#![cfg(feature = "psram")]

use embassy_rp::{
    Peri,
    psram::{Config, Psram},
    qmi_cs1::QmiCs1,
};
use portable_atomic::{AtomicBool, Ordering};
static READY: AtomicBool = AtomicBool::new(false);
pub async fn init(
    qmi_cs1: Peri<'static, embassy_rp::peripherals::QMI_CS1>,
    cs_pin: Peri<'static, embassy_rp::peripherals::PIN_47>,
) {
    // Board-specific APS6404L on GPIO47. Bluetooth needs no PSRAM buffers.
    match Psram::new(QmiCs1::new(qmi_cs1, cs_pin), Config::aps6404l()) {
        Ok(psram) => {
            READY.store(true, Ordering::Release);
            log::info!("psram: initialized ({} bytes)", psram.size());
        }
        Err(_) => log::warn!("psram: not detected"),
    }
}
pub fn is_available() -> bool {
    READY.load(Ordering::Acquire)
}

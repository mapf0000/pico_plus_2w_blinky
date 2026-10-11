//! Hardware adapters for bounded, verified USB identity persistence.
use crate::device_config_core::{self as core_config, Error, Storage};
pub use core_config::{DeviceConfig, FLASH_CAPACITY, MANUFACTURER_MAX, PRODUCT_MAX};
use embassy_rp::flash::{Blocking as FlashBlocking, Flash as RpFlash};
use embassy_rp::peripherals::FLASH as FlashPeriph;
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, mutex::Mutex};

type FlashDrv = RpFlash<'static, FlashPeriph, FlashBlocking, { FLASH_CAPACITY }>;
static CONFIG: Mutex<ThreadModeRawMutex, Option<DeviceConfig>> = Mutex::new(None);
static FLASH_DRV: Mutex<ThreadModeRawMutex, Option<FlashDrv>> = Mutex::new(None);

struct Adapter<'a>(&'a mut FlashDrv);
impl Storage for Adapter<'_> {
    fn read(&mut self, offset: usize, out: &mut [u8]) -> Result<(), Error> {
        self.0
            .blocking_read(offset as u32, out)
            .map_err(|_| Error::Io)
    }
    fn erase(&mut self, from: usize, to: usize) -> Result<(), Error> {
        self.0
            .blocking_erase(from as u32, to as u32)
            .map_err(|_| Error::Io)
    }
    fn write(&mut self, offset: usize, data: &[u8]) -> Result<(), Error> {
        self.0
            .blocking_write(offset as u32, data)
            .map_err(|_| Error::Io)
    }
}

pub async fn get() -> DeviceConfig {
    CONFIG
        .lock()
        .await
        .clone()
        .unwrap_or_else(DeviceConfig::defaults)
}
pub async fn init() {
    let mut config = CONFIG.lock().await;
    if config.is_none() {
        *config = Some(DeviceConfig::defaults());
    }
}
pub async fn set_flash_driver(driver: FlashDrv) {
    *FLASH_DRV.lock().await = Some(driver);
    let _ = reload().await;
}

/// Read back the winning flash record; unavailable/corrupt-empty slots use defaults.
pub async fn reload() -> Result<DeviceConfig, Error> {
    let mut flash = FLASH_DRV.lock().await;
    let driver = flash.as_mut().ok_or(Error::Io)?;
    let config = core_config::load(&mut Adapter(driver))?.unwrap_or_else(DeviceConfig::defaults);
    *CONFIG.lock().await = Some(config.clone());
    Ok(config)
}

/// Gate USB start and stop against persistence. Only an entirely stopped USB
/// session with no reserved HID/CDC work can enter the flash operation.
pub async fn save(
    identity: ble_protocol::identity::Identity<'_>,
) -> Result<(), ble_protocol::identity::Status> {
    use ble_protocol::identity::Status;
    let config = DeviceConfig::from_identity(identity).map_err(|_| Status::Invalid)?;
    let _lifecycle = crate::usb::usb_supervisor::LIFECYCLE.lock().await;
    if crate::usb::usb_supervisor::USB_ENABLED.load(core::sync::atomic::Ordering::SeqCst)
        || crate::usb::usb_supervisor::is_running()
    {
        return Err(Status::UsbActive);
    }
    if crate::usb::hid::active_job().is_some() || crate::usb::bootstrap::active() {
        return Err(Status::Busy);
    }
    let mut flash = FLASH_DRV.lock().await;
    let driver = flash.as_mut().ok_or(Status::FlashFailure)?;
    // Embassy's RP2350 driver invokes ROM/RAM helpers, pauses core 1, disables
    // interrupts and drains XiP DMA for each erase/program. USB is stopped; the
    // BLE runner resumes after the bounded transaction. No application flash code
    // or write buffer is executed/read from XiP while programming is active.
    if core_config::save(&mut Adapter(driver), &config).is_err() {
        // An I/O error after the commit may mean the new record is already valid.
        // Reconcile RAM when possible, but never acknowledge an unverified save.
        if let Ok(Some(committed)) = core_config::load(&mut Adapter(driver)) {
            *CONFIG.lock().await = Some(committed);
        }
        return Err(Status::FlashFailure);
    }
    *CONFIG.lock().await = Some(config);
    Ok(())
}

/// Provisioning is separate from identity. This adapter never writes the key region.
pub async fn control_profile() -> Option<ble_session::provisioning::Profile> {
    use ble_session::provisioning::{FLASH_OFFSET, Profile, RECORD_SIZE, SLOT_SIZE};
    let mut flash = FLASH_DRV.lock().await;
    let driver = flash.as_mut()?;
    let mut chosen: Option<(u32, Profile)> = None;
    for slot in 0..2 {
        let mut bytes = [0; RECORD_SIZE];
        let offset = FLASH_OFFSET + (slot * SLOT_SIZE) as u32;
        if driver.blocking_read(offset, &mut bytes).is_err() {
            ble_session::erase(&mut bytes);
            return None;
        }
        let decoded = Profile::decode(&bytes);
        ble_session::erase(&mut bytes);
        if let Some((sequence, profile)) = decoded
            && chosen
                .as_ref()
                .is_none_or(|(old, _)| core_config::seq_is_newer(sequence, *old))
        {
            chosen = Some((sequence, profile));
        }
    }
    chosen.map(|(_, profile)| profile)
}

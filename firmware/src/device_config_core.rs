//! Existing USB identity v1 flash format and bounded two-slot transaction.
use ble_protocol::identity::Identity;
pub use ble_protocol::identity::{MANUFACTURER_MAX, PRODUCT_MAX};
use heapless::String;

pub const FLASH_CAPACITY: usize = 16 * 1024 * 1024;
pub const SLOT_SIZE: usize = 4096;
pub const PERSIST_OFFSET: usize = FLASH_CAPACITY - 2 * SLOT_SIZE;
pub const PAGE_SIZE: usize = 256;
const HEADER_SIZE: usize = 18;
const MAGIC: u32 = 0x50494346;
const VERSION: u8 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceConfig {
    pub usb_manufacturer: String<MANUFACTURER_MAX>,
    pub usb_product: String<PRODUCT_MAX>,
}
impl DeviceConfig {
    pub fn defaults() -> Self {
        // These literals are statically within their capacities and ASCII.
        Self::from_identity(Identity {
            manufacturer: "Pico 2W",
            product: "Logger + Keyboard",
        })
        .expect("bounded default USB identity")
    }
    pub fn from_identity(identity: Identity<'_>) -> Result<Self, Error> {
        if !identity.valid() {
            return Err(Error::Invalid);
        }
        let mut cfg = Self {
            usb_manufacturer: String::new(),
            usb_product: String::new(),
        };
        cfg.usb_manufacturer
            .push_str(identity.manufacturer)
            .map_err(|_| Error::Invalid)?;
        cfg.usb_product
            .push_str(identity.product)
            .map_err(|_| Error::Invalid)?;
        Ok(cfg)
    }
    pub fn identity(&self) -> Identity<'_> {
        Identity {
            manufacturer: &self.usb_manufacturer,
            product: &self.usb_product,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid,
    Io,
    Verify,
}

/// Flash adapters use offsets, never absolute XiP addresses. Writes only clear bits.
pub trait Storage {
    fn read(&mut self, offset: usize, out: &mut [u8]) -> Result<(), Error>;
    fn erase(&mut self, from: usize, to: usize) -> Result<(), Error>;
    fn write(&mut self, offset: usize, data: &[u8]) -> Result<(), Error>;
}

pub fn seq_is_newer(a: u32, b: u32) -> bool {
    let diff = a.wrapping_sub(b);
    diff != 0 && diff < 0x8000_0000
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}

fn decode(bytes: &[u8; PAGE_SIZE]) -> Option<(u32, DeviceConfig)> {
    if bytes[..4] != MAGIC.to_le_bytes() || bytes[4] != VERSION || bytes[5..8] != [0; 3] {
        return None;
    }
    let seq = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    let length = usize::from(u16::from_le_bytes([bytes[12], bytes[13]]));
    if !(2..=ble_protocol::identity::PAYLOAD_MAX).contains(&length) {
        return None;
    }
    let stored_crc = u32::from_le_bytes([bytes[14], bytes[15], bytes[16], bytes[17]]);
    let payload = &bytes[HEADER_SIZE..HEADER_SIZE + length];
    if crc32(payload) != stored_crc {
        return None;
    }
    let config = DeviceConfig::from_identity(Identity::decode(payload).ok()?).ok()?;
    Some((seq, config))
}

fn encode(config: &DeviceConfig, seq: u32) -> Result<[u8; PAGE_SIZE], Error> {
    let mut page = [0xff; PAGE_SIZE];
    let length = config
        .identity()
        .encode(&mut page[HEADER_SIZE..])
        .map_err(|_| Error::Invalid)?;
    page[..4].copy_from_slice(&MAGIC.to_le_bytes());
    page[4] = VERSION;
    page[5..8].fill(0);
    page[8..12].copy_from_slice(&seq.to_le_bytes());
    page[12..14].copy_from_slice(&(length as u16).to_le_bytes());
    let crc = crc32(&page[HEADER_SIZE..HEADER_SIZE + length]);
    page[14..18].copy_from_slice(&crc.to_le_bytes());
    Ok(page)
}

fn current(storage: &mut impl Storage) -> Result<Option<(usize, u32, DeviceConfig)>, Error> {
    let mut chosen: Option<(usize, u32, DeviceConfig)> = None;
    let mut page = [0; PAGE_SIZE];
    for slot in 0..2 {
        storage.read(PERSIST_OFFSET + slot * SLOT_SIZE, &mut page)?;
        if let Some((seq, cfg)) = decode(&page)
            && chosen
                .as_ref()
                .is_none_or(|(_, old, _)| seq_is_newer(seq, *old) || seq == *old)
        {
            chosen = Some((slot, seq, cfg));
        }
    }
    Ok(chosen)
}

pub fn load(storage: &mut impl Storage) -> Result<Option<DeviceConfig>, Error> {
    Ok(current(storage)?.map(|(_, _, cfg)| cfg))
}

/// Erase the inactive slot, stage/verify a complete page without its magic,
/// then program the same page with magic and verify exact bytes. Only the last
/// write makes the record boot-loadable. Never erases the last valid record.
pub fn save(storage: &mut impl Storage, config: &DeviceConfig) -> Result<(), Error> {
    let (slot, seq) = match current(storage)? {
        Some((old_slot, old_seq, old)) => {
            if old == *config {
                return Ok(());
            } // Avoid wear on unchanged saves.
            (1 - old_slot, old_seq.wrapping_add(1))
        }
        None => (0, 0),
    };
    let page = encode(config, seq)?;
    let mut staged = page;
    staged[..4].fill(0xff);
    let offset = PERSIST_OFFSET + slot * SLOT_SIZE;
    storage.erase(offset, offset + SLOT_SIZE)?;
    storage.write(offset, &staged)?;
    let mut readback = [0; PAGE_SIZE];
    storage.read(offset, &mut readback)?;
    if readback != staged {
        return Err(Error::Verify);
    }
    storage.write(offset, &page)?;
    storage.read(offset, &mut readback)?;
    if readback != page || decode(&readback) != Some((seq, config.clone())) {
        return Err(Error::Verify);
    }
    Ok(())
}

#[cfg(test)]
mod tests;

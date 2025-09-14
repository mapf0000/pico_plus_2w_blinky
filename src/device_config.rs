use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::mutex::Mutex;
use heapless::String;
use embassy_rp::flash::{Blocking as FlashBlocking, Flash as RpFlash};
use embassy_rp::peripherals::FLASH as FlashPeriph;

// Limits for identity strings
pub const MANUFACTURER_MAX: usize = 32;
pub const PRODUCT_MAX: usize = 48;

// Pimoroni Pico Plus 2 W: 16 MiB external flash
pub const FLASH_CAPACITY: usize = 16 * 1024 * 1024;
const PERSIST_SLOTS: usize = 2;
const SLOT_SIZE: usize = 4096; // one erase block
const PERSIST_TOTAL: usize = SLOT_SIZE * PERSIST_SLOTS; // 8K total
const PERSIST_OFFSET: usize = FLASH_CAPACITY - PERSIST_TOTAL;
const MAGIC: u32 = 0x50494346; // 'P','I','C','F'
const VERSION: u8 = 1;

type FlashDrv = RpFlash<'static, FlashPeriph, FlashBlocking, { FLASH_CAPACITY }>;

#[derive(Clone)]
pub struct DeviceConfig {
    pub usb_manufacturer: String<MANUFACTURER_MAX>,
    pub usb_product: String<PRODUCT_MAX>,
}

impl DeviceConfig {
    pub fn defaults() -> Self {
        let mut man: String<MANUFACTURER_MAX> = String::new();
        let _ = man.push_str("Pico 2W");
        let mut prod: String<PRODUCT_MAX> = String::new();
        let _ = prod.push_str("Logger + Keyboard");
        Self { usb_manufacturer: man, usb_product: prod }
    }
}

enum DeviceConfigSlot {
    Uninit,
    Ready(DeviceConfig),
}

static CONFIG: Mutex<ThreadModeRawMutex, DeviceConfigSlot> = Mutex::new(DeviceConfigSlot::Uninit);
static FLASH_DRV: Mutex<ThreadModeRawMutex, Option<FlashDrv>> = Mutex::new(None);

pub fn is_ascii_printable(s: &str) -> bool {
    s.bytes().all(|b| matches!(b, 32..=126))
}

pub async fn get() -> DeviceConfig {
    match &*CONFIG.lock().await {
        DeviceConfigSlot::Ready(cfg) => cfg.clone(),
        DeviceConfigSlot::Uninit => DeviceConfig::defaults(),
    }
}

pub async fn set_partial(manufacturer: Option<&str>, product: Option<&str>) -> Result<(), SetError> {
    let mut guard = CONFIG.lock().await;
    let mut current = match &*guard {
        DeviceConfigSlot::Ready(cfg) => cfg.clone(),
        DeviceConfigSlot::Uninit => DeviceConfig::defaults(),
    };
    if let Some(m) = manufacturer {
        if m.len() > MANUFACTURER_MAX { return Err(SetError::TooLongManufacturer); }
        if !is_ascii_printable(m) { return Err(SetError::InvalidChars); }
        current.usb_manufacturer.clear();
        let _ = current.usb_manufacturer.push_str(m);
    }
    if let Some(p) = product {
        if p.len() > PRODUCT_MAX { return Err(SetError::TooLongProduct); }
        if !is_ascii_printable(p) { return Err(SetError::InvalidChars); }
        current.usb_product.clear();
        let _ = current.usb_product.push_str(p);
    }
    *guard = DeviceConfigSlot::Ready(current);
    Ok(())
}

#[derive(Debug, Copy, Clone)]
pub enum SetError {
    TooLongManufacturer,
    TooLongProduct,
    InvalidChars,
}

// Persistence hooks (stubbed for now; can be wired to flash later)
pub async fn init() {
    // Initialize defaults; real values loaded when flash driver is set.
    let mut g = CONFIG.lock().await;
    if let DeviceConfigSlot::Uninit = &*g { *g = DeviceConfigSlot::Ready(DeviceConfig::defaults()); }
}

pub async fn set_flash_driver(driver: FlashDrv) {
    *FLASH_DRV.lock().await = Some(driver);
    // Attempt to load persisted config
    if let Some(cfg) = load_from_flash().await.ok().flatten() {
        let mut g = CONFIG.lock().await;
        *g = DeviceConfigSlot::Ready(cfg);
    }
}

pub async fn save() -> Result<(), ()> { persist_to_flash().await }

// ------- Persistence implementation -------

// Note: previously used a "Header" struct for persistence; the implementation
// now encodes/decodes raw bytes directly for lower overhead.

fn crc32_ieee(mut crc: u32, data: &[u8]) -> u32 {
    crc ^= 0xFFFF_FFFF;
    for &b in data {
        let mut x = (crc ^ (b as u32)) & 0xFF;
        for _ in 0..8 {
            let mask = (x & 1).wrapping_neg();
            x = (x >> 1) ^ (0xEDB8_8320u32 & mask);
        }
        crc = (crc >> 8) ^ x;
    }
    crc ^ 0xFFFF_FFFF
}

fn encode_payload(cfg: &DeviceConfig, out: &mut [u8]) -> usize {
    let m = cfg.usb_manufacturer.as_str().as_bytes();
    let p = cfg.usb_product.as_str().as_bytes();
    let mut i = 0;
    out[i] = m.len() as u8; i += 1;
    out[i..i + m.len()].copy_from_slice(m); i += m.len();
    out[i] = p.len() as u8; i += 1;
    out[i..i + p.len()].copy_from_slice(p); i += p.len();
    i
}

fn decode_payload(buf: &[u8]) -> Option<DeviceConfig> {
    let mut i = 0;
    if i >= buf.len() { return None; }
    let mlen = buf[i] as usize; i += 1;
    if i + mlen > buf.len() { return None; }
    let man_bytes = &buf[i..i + mlen]; i += mlen;
    if i >= buf.len() { return None; }
    let plen = buf[i] as usize; i += 1;
    if i + plen > buf.len() { return None; }
    let prod_bytes = &buf[i..i + plen];

    if mlen > MANUFACTURER_MAX || plen > PRODUCT_MAX { return None; }
    let man = core::str::from_utf8(man_bytes).ok()?;
    let prod = core::str::from_utf8(prod_bytes).ok()?;
    if !is_ascii_printable(man) || !is_ascii_printable(prod) { return None; }
    let mut cfg = DeviceConfig::defaults();
    cfg.usb_manufacturer.clear();
    let _ = cfg.usb_manufacturer.push_str(man);
    cfg.usb_product.clear();
    let _ = cfg.usb_product.push_str(prod);
    Some(cfg)
}

async fn with_flash<R>(f: impl FnOnce(&mut FlashDrv) -> R) -> Result<R, ()> {
    let mut guard = FLASH_DRV.lock().await;
    let Some(ref mut drv) = *guard else { return Err(()); };
    Ok(f(drv))
}

async fn read_slot(idx: usize) -> Result<Option<(u32 /*seq*/, DeviceConfig)>, ()> {
    if idx >= PERSIST_SLOTS { return Err(()); }
    let mut hdr_buf = [0u8; 16];
    let slot_off = PERSIST_OFFSET + idx * SLOT_SIZE;
    let _ = with_flash(|f| f.blocking_read(slot_off as u32, &mut hdr_buf)).await.map_err(|_| ())?;
    let magic = u32::from_le_bytes([hdr_buf[0], hdr_buf[1], hdr_buf[2], hdr_buf[3]]);
    if magic != MAGIC { return Ok(None); }
    let version = hdr_buf[4];
    if version != VERSION { return Ok(None); }
    let seq = u32::from_le_bytes([hdr_buf[8], hdr_buf[9], hdr_buf[10], hdr_buf[11]]);
    let len = u16::from_le_bytes([hdr_buf[12], hdr_buf[13]]) as usize;
    // Read remaining 2 bytes of CRC tail + payload
    let mut tail_and_payload = [0u8; 2 + 1 + MANUFACTURER_MAX + 1 + PRODUCT_MAX];
    let need = 2 + len;
    // Safety: need <= buffer size by construction
    let buf_slice = &mut tail_and_payload[..need];
    let _ = with_flash(|f| f.blocking_read((slot_off as u32) + 16, buf_slice)).await;
    let crc_full = u32::from_le_bytes([hdr_buf[14], hdr_buf[15], buf_slice[0], buf_slice[1]]);
    let payload = &buf_slice[2..];
    let calc = crc32_ieee(0, payload);
    if calc != crc_full { return Ok(None); }
    let cfg = match decode_payload(payload) { Some(c) => c, None => return Ok(None) };
    Ok(Some((seq, cfg)))
}

async fn load_from_flash() -> Result<Option<DeviceConfig>, ()> {
    // Check both slots, choose highest seq
    let a = read_slot(0).await?;
    let b = read_slot(1).await?;
    let chosen = match (a, b) {
        (Some((sa, ca)), Some((sb, cb))) => if sa >= sb { Some(ca) } else { Some(cb) },
        (Some((_, ca)), None) => Some(ca),
        (None, Some((_, cb))) => Some(cb),
        (None, None) => None,
    };
    Ok(chosen)
}

async fn persist_to_flash() -> Result<(), ()> {
    // Determine next seq and next slot
    let cur = get().await;
    let mut seq_a = 0u32; let mut seq_b = 0u32;
    if let Some((s, _)) = read_slot(0).await? { seq_a = s; }
    if let Some((s, _)) = read_slot(1).await? { seq_b = s; }
    let (target_idx, next_seq) = if seq_a <= seq_b { (0usize, seq_b.wrapping_add(1)) } else { (1usize, seq_a.wrapping_add(1)) };

    let mut payload = [0u8; 1 + MANUFACTURER_MAX + 1 + PRODUCT_MAX];
    let plen = encode_payload(&cur, &mut payload);
    let crc = crc32_ieee(0, &payload[..plen]);
    // Build header: magic(4), version(1), pad(3), seq(4), len(2), crc32(4)
    let mut header = [0u8; 16 + 2]; // first 16 bytes + last 2 bytes of crc later aligned
    header[0..4].copy_from_slice(&MAGIC.to_le_bytes());
    header[4] = VERSION; header[5] = 0; header[6] = 0; header[7] = 0;
    header[8..12].copy_from_slice(&next_seq.to_le_bytes());
    header[12..14].copy_from_slice(&(plen as u16).to_le_bytes());
    let crc_le = crc.to_le_bytes();
    header[14..16].copy_from_slice(&crc_le[0..2]);
    let mut tail = [0u8; 2];
    tail.copy_from_slice(&crc_le[2..4]);

    let slot_off = PERSIST_OFFSET + target_idx * SLOT_SIZE;
    // Erase target slot
    let _ = with_flash(|f| f.blocking_erase(slot_off as u32, (slot_off + SLOT_SIZE) as u32)).await.map_err(|_| ())?;
    // Program header first 16 bytes
    let _ = with_flash(|f| f.blocking_write(slot_off as u32, &header[..16])).await.map_err(|_| ())?;
    // Program rest: last 2 bytes of header (crc), then payload
    let _ = with_flash(|f| f.blocking_write((slot_off as u32) + 16, &tail)).await.map_err(|_| ())?;
    let _ = with_flash(|f| f.blocking_write((slot_off as u32) + 18, &payload[..plen])).await.map_err(|_| ())?;
    Ok(())
}

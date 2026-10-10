//! Dedicated 8-KiB provisioning region before the unchanged internal agent image.
use noise_protocol::U8Array;
use noise_rust_crypto::sensitive::Sensitive;
use zeroize::Zeroize;

pub const FLASH_OFFSET: u32 = 12272 * 1024;
pub const FLASH_ADDRESS: u32 = 0x10000000 + FLASH_OFFSET;
pub const SLOT_SIZE: usize = 4096;
pub const REGION_SIZE: usize = SLOT_SIZE * 2;
pub const RECORD_SIZE: usize = 64;
pub struct Profile {
    pub device_id: [u8; 16],
    key: Sensitive<[u8; 32]>,
}
impl Profile {
    pub fn new(device_id: [u8; 16], mut key: [u8; 32]) -> Option<Self> {
        if device_id == [0; 16] || key == [0; 32] {
            key.zeroize();
            return None;
        }
        let profile = Self {
            device_id,
            key: Sensitive::from_slice(&key),
        };
        key.zeroize();
        Some(profile)
    }
    pub fn key(&self) -> &[u8; 32] {
        &self.key
    }
    pub fn encode(&self, sequence: u32) -> [u8; RECORD_SIZE] {
        let mut out = [0; RECORD_SIZE];
        out[..4].copy_from_slice(b"PKEY");
        out[4] = 1;
        out[8..12].copy_from_slice(&sequence.to_le_bytes());
        out[16..32].copy_from_slice(&self.device_id);
        out[32..].copy_from_slice(self.key());
        let crc = crc32(&[&out[..12], &out[16..]]);
        out[12..16].copy_from_slice(&crc.to_le_bytes());
        out
    }
    pub fn decode(bytes: &[u8]) -> Option<(u32, Self)> {
        if bytes.len() != RECORD_SIZE
            || &bytes[..4] != b"PKEY"
            || bytes[4] != 1
            || bytes[5..8] != [0; 3]
        {
            return None;
        }
        let sequence = u32::from_le_bytes(bytes[8..12].try_into().ok()?);
        if sequence == 0
            || crc32(&[&bytes[..12], &bytes[16..]])
                != u32::from_le_bytes(bytes[12..16].try_into().ok()?)
        {
            return None;
        }
        let profile = Self::new(
            bytes[16..32].try_into().ok()?,
            bytes[32..64].try_into().ok()?,
        )?;
        Some((sequence, profile))
    }
}
fn crc32(parts: &[&[u8]]) -> u32 {
    let mut crc = u32::MAX;
    for byte in parts.iter().flat_map(|part| part.iter()) {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}

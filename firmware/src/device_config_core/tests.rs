use super::*;
use std::vec;
use std::vec::Vec;

#[derive(Clone)]
struct Flash {
    bytes: Vec<u8>,
    writes: usize,
    erases: usize,
    cut_write: Option<(usize, usize)>,
    corrupt_read: bool,
    reads: usize,
    corrupt_at_read: Option<usize>,
    cut_erase: Option<usize>,
}
impl Flash {
    fn new() -> Self {
        Self {
            bytes: vec![0xff; 2 * SLOT_SIZE],
            writes: 0,
            erases: 0,
            cut_write: None,
            corrupt_read: false,
            reads: 0,
            corrupt_at_read: None,
            cut_erase: None,
        }
    }
    fn range(offset: usize, length: usize) -> core::ops::Range<usize> {
        assert!(offset >= PERSIST_OFFSET && offset + length <= FLASH_CAPACITY);
        offset - PERSIST_OFFSET..offset - PERSIST_OFFSET + length
    }
}
impl Storage for Flash {
    fn read(&mut self, offset: usize, out: &mut [u8]) -> Result<(), Error> {
        self.reads += 1;
        out.copy_from_slice(&self.bytes[Self::range(offset, out.len())]);
        if (self.corrupt_read && self.writes != 0) || self.corrupt_at_read == Some(self.reads) {
            out[19] ^= 1;
        }
        Ok(())
    }
    fn erase(&mut self, from: usize, to: usize) -> Result<(), Error> {
        self.erases += 1;
        self.bytes[Self::range(from, self.cut_erase.unwrap_or(to - from))].fill(0xff);
        if self.cut_erase.is_some() {
            Err(Error::Io)
        } else {
            Ok(())
        }
    }
    fn write(&mut self, offset: usize, data: &[u8]) -> Result<(), Error> {
        self.writes += 1;
        let cut = self
            .cut_write
            .filter(|(call, _)| *call == self.writes)
            .map(|(_, length)| length);
        let length = cut.unwrap_or(data.len()).min(data.len());
        for (out, byte) in self.bytes[Self::range(offset, length)].iter_mut().zip(data) {
            *out &= byte;
        }
        if cut.is_some() {
            Err(Error::Io)
        } else {
            Ok(())
        }
    }
}
fn alternate() -> DeviceConfig {
    DeviceConfig::from_identity(Identity {
        manufacturer: "Test vendor",
        product: "Test keyboard",
    })
    .unwrap()
}

#[test]
fn preserves_legacy_schema_crc_and_both_slot_locations() {
    assert_eq!(PERSIST_OFFSET, 0xffe000);
    let old = DeviceConfig::defaults();
    let mut flash = Flash::new();
    // Independent layout fixture of the previous version-1 format.
    let payload = b"\x07Pico 2W\x11Logger + Keyboard";
    let mut legacy = [0xff; PAGE_SIZE];
    legacy[..4].copy_from_slice(&0x50494346u32.to_le_bytes());
    legacy[4..8].copy_from_slice(&[1, 0, 0, 0]);
    legacy[8..12].copy_from_slice(&15u32.to_le_bytes());
    legacy[12..14].copy_from_slice(&(payload.len() as u16).to_le_bytes());
    // Standard IEEE CRC implementation must match a known reference vector.
    assert_eq!(crc32(b"123456789"), 0xcbf43926);
    legacy[14..18].copy_from_slice(&crc32(payload).to_le_bytes());
    legacy[18..18 + payload.len()].copy_from_slice(payload);
    flash.bytes[..PAGE_SIZE].copy_from_slice(&legacy);
    assert_eq!(load(&mut flash), Ok(Some(old.clone())));
    save(&mut flash, &alternate()).unwrap();
    assert_eq!(&flash.bytes[..PAGE_SIZE], &legacy);
    assert_eq!(load(&mut flash), Ok(Some(alternate())));
    save(&mut flash, &old).unwrap();
    assert_eq!(load(&mut flash), Ok(Some(old)));
}

#[test]
fn maximum_empty_identity_unchanged_save_and_sequence_wrap() {
    let max = DeviceConfig::from_identity(Identity {
        manufacturer: &"M".repeat(32),
        product: &"P".repeat(48),
    })
    .unwrap();
    let mut flash = Flash::new();
    let old = encode(&DeviceConfig::defaults(), u32::MAX).unwrap();
    flash.bytes[SLOT_SIZE..SLOT_SIZE + PAGE_SIZE].copy_from_slice(&old);
    save(&mut flash, &max).unwrap();
    assert_eq!(load(&mut flash), Ok(Some(max.clone())));
    assert_eq!(
        decode(flash.bytes[..PAGE_SIZE].try_into().unwrap())
            .unwrap()
            .0,
        0
    );
    let counts = (flash.writes, flash.erases);
    save(&mut flash, &max).unwrap();
    assert_eq!((flash.writes, flash.erases), counts);
    let empty = DeviceConfig::from_identity(Identity {
        manufacturer: "",
        product: "",
    })
    .unwrap();
    save(&mut flash, &empty).unwrap();
    assert_eq!(load(&mut flash), Ok(Some(empty)));
}

#[test]
fn power_loss_during_every_staged_or_commit_byte_keeps_old_or_complete_new() {
    let old = DeviceConfig::defaults();
    let new = alternate();
    let mut baseline = Flash::new();
    save(&mut baseline, &old).unwrap();
    for call in 1..=2 {
        for cut in 0..=PAGE_SIZE {
            let mut flash = baseline.clone();
            flash.writes = 0;
            flash.cut_write = Some((call, cut));
            assert_eq!(save(&mut flash, &new), Err(Error::Io));
            flash.cut_write = None;
            let loaded = load(&mut flash).unwrap().unwrap();
            assert!(loaded == old || loaded == new);
            if call == 1 {
                assert_eq!(loaded, old);
            }
            // Neither erase nor torn programming touched the surviving old slot.
            assert_eq!(&flash.bytes[..SLOT_SIZE], &baseline.bytes[..SLOT_SIZE]);
        }
    }
}

#[test]
fn corrupt_records_and_failed_verification_do_not_report_success() {
    let valid = encode(&alternate(), 10).unwrap();
    for index in [0, 4, 5, 6, 7, 12, 13, 14, 15, 16, 17, 18, 19] {
        let mut bad = valid;
        bad[index] ^= 0xff;
        assert!(decode(&bad).is_none(), "byte {index}");
    }
    let mut flash = Flash::new();
    save(&mut flash, &DeviceConfig::defaults()).unwrap();
    flash.writes = 0;
    flash.corrupt_read = true;
    assert_eq!(save(&mut flash, &alternate()), Err(Error::Verify));
    flash.corrupt_read = false;
    assert_eq!(load(&mut flash), Ok(Some(DeviceConfig::defaults())));
    assert!(
        DeviceConfig::from_identity(Identity {
            manufacturer: "not\nASCII",
            product: ""
        })
        .is_err()
    );
}

#[test]
fn interrupted_erase_preserves_latest_record_and_commit_read_failure_never_acknowledges() {
    let mut baseline = Flash::new();
    save(&mut baseline, &DeviceConfig::defaults()).unwrap();
    save(&mut baseline, &alternate()).unwrap();
    for cut in 0..=SLOT_SIZE {
        let mut flash = baseline.clone();
        flash.cut_erase = Some(cut);
        assert_eq!(save(&mut flash, &DeviceConfig::defaults()), Err(Error::Io));
        flash.cut_erase = None;
        assert_eq!(load(&mut flash), Ok(Some(alternate())));
        assert_eq!(&flash.bytes[SLOT_SIZE..], &baseline.bytes[SLOT_SIZE..]);
    }
    let mut flash = baseline;
    flash.reads = 0;
    flash.corrupt_at_read = Some(4); // Final read after commit, not the staging read.
    assert_eq!(
        save(&mut flash, &DeviceConfig::defaults()),
        Err(Error::Verify)
    );
    flash.corrupt_at_read = None;
    // Commit may have succeeded despite a failed verification acknowledgement.
    assert_eq!(load(&mut flash), Ok(Some(DeviceConfig::defaults())));
}

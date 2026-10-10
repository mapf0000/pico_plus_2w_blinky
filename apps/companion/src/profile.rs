//! Private factory provisioning files. Never expose the key in diagnostics.
use ble_session::provisioning::{Profile, REGION_SIZE};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};
use zeroize::{Zeroize, Zeroizing};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u8,
    device_id: String,
    key: String,
}
impl Drop for Document {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}

pub fn load(path: &Path) -> io::Result<Profile> {
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(invalid());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Provisioning profile must be private (chmod 600 PATH)",
            ));
        }
    }
    let mut bytes = Zeroizing::new(Vec::new());
    file.take(4097).read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(invalid());
    }
    let document: Document = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if document.version != 1 {
        return Err(invalid());
    }
    let id = decode_hex::<16>(&document.device_id).ok_or_else(invalid)?;
    let key = Zeroizing::new(decode_hex::<32>(&document.key).ok_or_else(invalid)?);
    Profile::new(id, *key).ok_or_else(invalid)
}

/// Create, never overwrite, a profile and a matching flash provisioning region.
pub fn create(path: &Path) -> io::Result<PathBuf> {
    let mut id = [0; 16];
    let mut key = Zeroizing::new([0; 32]);
    getrandom::fill(&mut id).map_err(|_| io::Error::other("OS randomness unavailable"))?;
    getrandom::fill(&mut *key).map_err(|_| io::Error::other("OS randomness unavailable"))?;
    let profile = Profile::new(id, *key).ok_or_else(invalid)?;
    let document = Document {
        version: 1,
        device_id: hex(&id),
        key: hex(profile.key()),
    };
    let bytes = Zeroizing::new(serde_json::to_vec_pretty(&document).map_err(|_| invalid())?);
    let mut region = Zeroizing::new([0xff; REGION_SIZE]);
    let mut record = Zeroizing::new(profile.encode(1));
    region[..record.len()].copy_from_slice(&*record);
    record.zeroize();
    let flash_path = path.with_extension("flash.bin");
    if flash_path == path {
        return Err(invalid());
    }
    write_private(path, &bytes)?;
    if let Err(error) = write_private(&flash_path, &*region) {
        let _ = std::fs::remove_file(path);
        return Err(error);
    }
    Ok(flash_path)
}

fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(error);
    }
    Ok(())
}
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid provisioning profile")
}
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|b| {
            [
                DIGITS[(b >> 4) as usize] as char,
                DIGITS[(b & 15) as usize] as char,
            ]
        })
        .collect()
}
fn decode_hex<const N: usize>(value: &str) -> Option<[u8; N]> {
    if value.len() != N * 2 {
        return None;
    }
    let mut out = [0; N];
    for (index, pair) in value.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        let digit = |b: u8| match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            _ => None,
        };
        out[index] = digit(pair[0])? * 16 + digit(pair[1])?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hex_is_strict_and_round_trips() {
        assert_eq!(decode_hex::<2>(&hex(&[0, 255])), Some([0, 255]));
        assert!(decode_hex::<2>("ffff00").is_none());
        assert!(decode_hex::<2>("FFff").is_none());
        assert!(decode_hex::<2>("é00").is_none());
    }
    #[test]
    fn provisioning_is_private_round_trips_and_never_overwrites() {
        let mut random = [0; 16];
        getrandom::fill(&mut random).unwrap();
        let directory = std::env::temp_dir().join(format!("pico-profile-{}", hex(&random)));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("pico.json");
        let flash = create(&path).unwrap();
        let profile = load(&path).unwrap();
        let region = Zeroizing::new(std::fs::read(&flash).unwrap());
        assert_eq!(region.len(), REGION_SIZE);
        let (sequence, stored) = Profile::decode(&region[..64]).unwrap();
        assert_eq!(sequence, 1);
        assert_eq!(stored.device_id, profile.device_id);
        assert!(stored.key() == profile.key());
        assert!(region[64..].iter().all(|b| *b == 255));
        assert_eq!(
            create(&path).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert_eq!(
                load(&path).err().unwrap().kind(),
                io::ErrorKind::PermissionDenied
            );
        }
        std::fs::remove_dir_all(directory).unwrap();
    }
}

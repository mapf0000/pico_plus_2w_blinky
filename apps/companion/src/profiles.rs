//! Bounded non-secret catalog and native credential storage. Never serialize keys here.
use ble_session::provisioning::Profile;
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

const MAX_PROFILES: usize = 16;
const CATALOG_MAX: usize = 8192;
#[cfg(target_os = "macos")]
const SERVICE: &str = "org.pico-rust.companion.ble.v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Catalog,
    Storage,
    Missing,
    Conflict,
    Full,
    Busy,
    InvalidProfile,
    PrivateProfile,
    Unsupported,
}
impl Error {
    pub fn message(self) -> &'static str {
        match self {
            Self::Catalog => {
                "Cannot read or save the profile catalog; existing credentials were retained"
            }
            Self::Storage => "Credential store unavailable or access denied; unlock it and retry",
            Self::Missing => "Saved key is missing; import its private provisioning profile again",
            Self::Conflict => {
                "A different key is already stored for this device; existing key retained"
            }
            Self::Full => "The saved profile limit of 16 has been reached",
            Self::Busy => "Another companion is updating profiles; try again",
            Self::InvalidProfile => "Invalid or oversized provisioning profile",
            Self::PrivateProfile => "Provisioning profile must be private (chmod 600 PATH)",
            Self::Unsupported => {
                "Saved profiles currently require macOS Keychain; use --profile PATH on this platform"
            }
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Summary {
    pub id: String,
    pub name: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    version: u8,
    pub profiles: Vec<Summary>,
    pub selected: Option<String>,
}
impl Default for Catalog {
    fn default() -> Self {
        Self {
            version: 1,
            profiles: Vec::new(),
            selected: None,
        }
    }
}
impl Catalog {
    fn valid(&self) -> bool {
        self.version == 1
            && self.profiles.len() <= MAX_PROFILES
            && self.profiles.iter().enumerate().all(|(i, p)| {
                valid_id(&p.id)
                    && !p.name.is_empty()
                    && p.name.len() <= 64
                    && p.name.chars().all(visible)
                    && !self.profiles[..i].iter().any(|old| old.id == p.id)
            })
            && self
                .selected
                .as_ref()
                .is_none_or(|id| self.profiles.iter().any(|p| &p.id == id))
    }
}
fn visible(c: char) -> bool {
    !c.is_control()
        && !matches!(c as u32, 0x061c | 0x200e | 0x200f | 0x202a..=0x202e | 0x2066..=0x2069)
}
fn valid_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && id.bytes().any(|b| b != b'0')
}
pub fn id(profile: &Profile) -> String {
    profile
        .device_id
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub(crate) trait Credentials {
    fn read(&self, id: &str) -> Result<Option<Profile>, Error>;
    /// Must never replace an existing credential, including across processes.
    fn insert(&mut self, id: &str, profile: &Profile) -> Result<(), Error>;
    fn remove(&mut self, id: &str) -> Result<(), Error>;
}
pub(crate) struct Native;
#[cfg(target_os = "macos")]
impl Credentials for Native {
    fn read(&self, id: &str) -> Result<Option<Profile>, Error> {
        use security_framework::passwords::{PasswordOptions, generic_password};
        let bytes = match generic_password(PasswordOptions::new_generic_password(SERVICE, id)) {
            Ok(bytes) => Zeroizing::new(bytes),
            Err(e) if e.code() == -25300 => return Ok(None),
            Err(_) => return Err(Error::Storage),
        };
        let (sequence, profile) = Profile::decode(&bytes).ok_or(Error::Storage)?;
        if sequence != 1 || self::id(&profile) != id {
            return Err(Error::Storage);
        }
        Ok(Some(profile))
    }
    fn insert(&mut self, id: &str, profile: &Profile) -> Result<(), Error> {
        // Add-only API: a second process cannot overwrite a key after our read.
        use security_framework::os::macos::keychain::SecKeychain;
        let bytes = Zeroizing::new(profile.encode(1));
        SecKeychain::default()
            .map_err(|_| Error::Storage)?
            .add_generic_password(SERVICE, id, &*bytes)
            .map_err(|e| {
                if e.code() == -25299 {
                    Error::Conflict
                } else {
                    Error::Storage
                }
            })
    }
    fn remove(&mut self, id: &str) -> Result<(), Error> {
        security_framework::passwords::delete_generic_password(SERVICE, id)
            .map_err(|_| Error::Storage)
    }
}
#[cfg(not(target_os = "macos"))]
impl Credentials for Native {
    fn read(&self, _: &str) -> Result<Option<Profile>, Error> {
        Err(Error::Unsupported)
    }
    fn insert(&mut self, _: &str, _: &Profile) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    fn remove(&mut self, _: &str) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
}

pub(crate) struct Manager<S = Native> {
    path: PathBuf,
    store: S,
}
impl Manager {
    pub fn open() -> Result<Self, Error> {
        #[cfg(not(target_os = "windows"))]
        let home = home::home_dir().ok_or(Error::Catalog)?;
        #[cfg(target_os = "macos")]
        let directory = home.join("Library/Application Support/Pico Companion");
        #[cfg(target_os = "windows")]
        let directory = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .ok_or(Error::Catalog)?
            .join("Pico Companion");
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let directory = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".config"))
            .join("pico-companion");
        Ok(Self {
            path: directory.join("profiles-v1.json"),
            store: Native,
        })
    }
}
impl<S: Credentials> Manager<S> {
    fn catalog(&self) -> Result<Catalog, Error> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Catalog::default()),
            Err(_) => return Err(Error::Catalog),
        };
        if !file.metadata().map_err(|_| Error::Catalog)?.is_file() {
            return Err(Error::Catalog);
        }
        let mut bytes = Vec::new();
        file.take(CATALOG_MAX as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Catalog)?;
        if bytes.len() > CATALOG_MAX {
            return Err(Error::Catalog);
        }
        let catalog: Catalog = serde_json::from_slice(&bytes).map_err(|_| Error::Catalog)?;
        if !catalog.valid() {
            return Err(Error::Catalog);
        }
        Ok(catalog)
    }
    fn lock(&self) -> Result<File, Error> {
        let parent = self.path.parent().ok_or(Error::Catalog)?;
        let mut directory = std::fs::DirBuilder::new();
        directory.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            directory.mode(0o700);
        }
        directory.create(parent).map_err(|_| Error::Catalog)?;
        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options
            .open(self.path.with_extension("lock"))
            .map_err(|_| Error::Catalog)?;
        lock.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => Error::Busy,
            std::fs::TryLockError::Error(_) => Error::Catalog,
        })?;
        Ok(lock)
    }
    fn save(&self, catalog: &Catalog) -> Result<(), Error> {
        if !catalog.valid() {
            return Err(Error::Catalog);
        }
        let bytes = serde_json::to_vec(catalog).map_err(|_| Error::Catalog)?;
        if bytes.len() > CATALOG_MAX {
            return Err(Error::Catalog);
        }
        let mut file = tempfile::NamedTempFile::new_in(self.path.parent().ok_or(Error::Catalog)?)
            .map_err(|_| Error::Catalog)?;
        file.write_all(&bytes)
            .and_then(|()| file.as_file().sync_all())
            .map_err(|_| Error::Catalog)?;
        file.persist(&self.path).map_err(|_| Error::Catalog)?;
        Ok(())
    }
    pub fn load(&self, restore: bool) -> Result<(Catalog, Option<Profile>), Error> {
        let catalog = self.catalog()?;
        let profile = if restore && let Some(id) = &catalog.selected {
            Some(self.store.read(id)?.ok_or(Error::Missing)?)
        } else {
            None
        };
        Ok((catalog, profile))
    }
    pub fn select(&mut self, id: &str) -> Result<(Catalog, Profile), Error> {
        let _lock = self.lock()?;
        let mut catalog = self.catalog()?;
        if !catalog.profiles.iter().any(|p| p.id == id) {
            return Err(Error::Missing);
        }
        let profile = self.store.read(id)?.ok_or(Error::Missing)?;
        catalog.selected = Some(id.into());
        self.save(&catalog)?;
        Ok((catalog, profile))
    }
    pub fn import(&mut self, path: &Path) -> Result<(Catalog, Profile), Error> {
        let profile = crate::profile::load(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::PermissionDenied => Error::PrivateProfile,
            _ => Error::InvalidProfile,
        })?;
        let _lock = self.lock()?;
        let mut catalog = self.catalog()?;
        let id = id(&profile);
        let listed = catalog.profiles.iter().any(|p| p.id == id);
        if !listed && catalog.profiles.len() == MAX_PROFILES {
            return Err(Error::Full);
        }
        let existing = self.store.read(&id)?;
        if existing
            .as_ref()
            .is_some_and(|old| old.key() != profile.key())
        {
            return Err(Error::Conflict);
        }
        let inserted = existing.is_none();
        if inserted {
            self.store.insert(&id, &profile)?;
        }
        if !listed {
            let mut name = String::new();
            for c in path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Pico")
                .chars()
                .filter(|c| visible(*c))
            {
                if name.len() + c.len_utf8() > 64 {
                    break;
                }
                name.push(c);
            }
            if name.is_empty() {
                name = "Pico".into();
            }
            catalog.profiles.push(Summary {
                id: id.clone(),
                name,
            });
        }
        catalog.selected = Some(id.clone());
        if let Err(error) = self.save(&catalog) {
            // Only remove a credential this transaction inserted; never a backup.
            if inserted {
                let _ = self.store.remove(&id);
            }
            return Err(error);
        }
        Ok((catalog, profile))
    }
}

/// Explicit opt-in acceptance; uses one random disposable credential, never a real profile.
pub fn self_test() -> Result<(), Box<dyn std::error::Error>> {
    let mut key = Zeroizing::new([0; 32]);
    let mut device = [0; 16];
    getrandom::fill(&mut *key)?;
    getrandom::fill(&mut device)?;
    let profile = Profile::new(device, *key).ok_or("Random profile unavailable")?;
    let id = id(&profile);
    let mut store = Native;
    if store.read(&id).map_err(Error::message)?.is_some() {
        return Err("Credential collision".into());
    }
    store.insert(&id, &profile).map_err(Error::message)?;
    let result = (|| {
        let loaded = store
            .read(&id)
            .map_err(Error::message)?
            .ok_or("Credential missing")?;
        if loaded.key() != profile.key() || loaded.device_id != profile.device_id {
            return Err("Credential round trip failed");
        }
        if store.insert(&id, &profile) != Err(Error::Conflict) {
            return Err("Credential overwrite was allowed");
        }
        Ok(())
    })();
    store.remove(&id).map_err(Error::message)?;
    result?;
    if store.read(&id).map_err(Error::message)?.is_some() {
        return Err("Credential cleanup failed".into());
    }
    println!("Native credential store: round trip, no-overwrite and cleanup passed.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    #[derive(Default)]
    struct Fake {
        entries: BTreeMap<String, Zeroizing<[u8; 64]>>,
        denied: bool,
        sabotage_save: Option<PathBuf>,
        sabotage_read: Option<PathBuf>,
    }
    impl Credentials for Fake {
        fn read(&self, id: &str) -> Result<Option<Profile>, Error> {
            if self.denied {
                return Err(Error::Storage);
            }
            if let Some(path) = &self.sabotage_read {
                std::fs::create_dir(path).unwrap();
            }
            self.entries
                .get(id)
                .map(|bytes| {
                    Profile::decode(&bytes[..])
                        .map(|(_, p)| p)
                        .ok_or(Error::Storage)
                })
                .transpose()
        }
        fn insert(&mut self, id: &str, profile: &Profile) -> Result<(), Error> {
            if self.denied {
                return Err(Error::Storage);
            }
            if self.entries.contains_key(id) {
                return Err(Error::Conflict);
            }
            self.entries
                .insert(id.into(), Zeroizing::new(profile.encode(1)));
            if let Some(path) = &self.sabotage_save {
                std::fs::create_dir(path).unwrap();
            }
            Ok(())
        }
        fn remove(&mut self, id: &str) -> Result<(), Error> {
            self.entries.remove(id);
            Ok(())
        }
    }
    fn fixture() -> (tempfile::TempDir, Manager<Fake>, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("test-pico.json");
        crate::profile::create(&source).unwrap();
        let manager = Manager {
            path: directory.path().join("profiles.json"),
            store: Fake::default(),
        };
        (directory, manager, source)
    }
    #[test]
    fn imported_key_is_separate_from_catalog_and_reloads_without_source_file() {
        let (_directory, mut manager, source) = fixture();
        let (catalog, imported) = manager.import(&source).unwrap();
        assert_eq!(catalog.profiles.len(), 1);
        assert_eq!(catalog.selected.as_deref(), Some(id(&imported).as_str()));
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manager.path).unwrap()).unwrap();
        assert_eq!(json["profiles"][0].as_object().unwrap().len(), 2);
        assert!(json["profiles"][0].get("key").is_none());
        std::fs::remove_file(source).unwrap();
        let (_, loaded) = manager.load(true).unwrap();
        let loaded = loaded.unwrap();
        assert!(loaded.key() == imported.key());
        assert_eq!(loaded.device_id, imported.device_id);
        assert!(manager.load(false).unwrap().1.is_none());
    }
    #[test]
    fn reimport_is_idempotent_and_conflicting_key_never_replaces_existing_one() {
        let (_directory, mut manager, source) = fixture();
        let (_, imported) = manager.import(&source).unwrap();
        manager.import(&source).unwrap();
        assert_eq!(manager.store.entries.len(), 1);
        assert_eq!(manager.catalog().unwrap().profiles.len(), 1);
        let id = id(&imported);
        let other = Profile::new(imported.device_id, [19; 32]).unwrap();
        manager
            .store
            .entries
            .insert(id.clone(), Zeroizing::new(other.encode(1)));
        assert!(matches!(manager.import(&source), Err(Error::Conflict)));
        assert!(manager.store.read(&id).unwrap().unwrap().key() == other.key());
        assert_eq!(manager.catalog().unwrap().selected, Some(id));
    }
    #[test]
    fn storage_denial_missing_keys_and_unknown_selection_preserve_catalog() {
        let (directory, mut manager, source) = fixture();
        let (_, first) = manager.import(&source).unwrap();
        let second = directory.path().join("second.json");
        crate::profile::create(&second).unwrap();
        let (_, second_profile) = manager.import(&second).unwrap();
        manager.select(&id(&first)).unwrap();
        let before = std::fs::read(&manager.path).unwrap();
        manager.store.denied = true;
        assert!(matches!(
            manager.select(&id(&second_profile)),
            Err(Error::Storage)
        ));
        assert!(matches!(manager.load(true), Err(Error::Storage)));
        assert_eq!(std::fs::read(&manager.path).unwrap(), before);
        manager.store.denied = false;
        manager.store.entries.remove(&id(&first));
        assert!(matches!(manager.load(true), Err(Error::Missing)));
        assert!(matches!(
            manager.select(&"f".repeat(32)),
            Err(Error::Missing)
        ));
        assert_eq!(std::fs::read(&manager.path).unwrap(), before);
    }
    #[test]
    fn failed_catalog_commit_rolls_back_only_the_new_credential() {
        let (_directory, mut manager, source) = fixture();
        manager.store.sabotage_save = Some(manager.path.clone());
        assert!(matches!(manager.import(&source), Err(Error::Catalog)));
        assert!(manager.store.entries.is_empty());
    }
    #[test]
    fn failed_catalog_commit_never_deletes_a_preexisting_credential() {
        let (_directory, mut manager, source) = fixture();
        let profile = crate::profile::load(&source).unwrap();
        let id = id(&profile);
        manager
            .store
            .entries
            .insert(id.clone(), Zeroizing::new(profile.encode(1)));
        manager.store.sabotage_read = Some(manager.path.clone());
        assert!(matches!(manager.import(&source), Err(Error::Catalog)));
        assert!(manager.store.entries.contains_key(&id));
    }
    #[test]
    fn malformed_oversized_duplicate_and_unknown_catalogs_fail_closed() {
        let (_directory, manager, _source) = fixture();
        for bytes in [
            b"{}".to_vec(),
            vec![b' '; CATALOG_MAX + 1],
            br#"{"version":2,"profiles":[],"selected":null}"#.to_vec(),
            br#"{"version":1,"profiles":[],"selected":null,"key":"not-a-key"}"#.to_vec(),
            br#"{"version":1,"profiles":[],"selected":"unknown"}"#.to_vec(),
        ] {
            std::fs::write(&manager.path, bytes).unwrap();
            assert!(matches!(manager.load(true), Err(Error::Catalog)));
        }
        let summary = Summary {
            id: "1".repeat(32),
            name: "Pico".into(),
        };
        assert!(
            !Catalog {
                version: 1,
                profiles: vec![summary.clone(), summary],
                selected: None
            }
            .valid()
        );
        assert!(
            !Catalog {
                version: 1,
                profiles: vec![Summary {
                    id: "é".repeat(16),
                    name: "Pico".into()
                }],
                selected: None
            }
            .valid()
        );
        assert!(
            !Catalog {
                version: 1,
                profiles: vec![Summary {
                    id: "1".repeat(32),
                    name: "spoof\u{202e}".into()
                }],
                selected: None
            }
            .valid()
        );
    }
    #[test]
    fn catalog_lock_is_exclusive_and_released_without_a_stale_lockfile() {
        let (_directory, mut manager, source) = fixture();
        let lock = manager.lock().unwrap();
        assert!(matches!(manager.import(&source), Err(Error::Busy)));
        assert!(manager.store.entries.is_empty());
        drop(lock);
        manager.import(&source).unwrap();
        assert_eq!(manager.store.entries.len(), 1);
    }
    #[test]
    fn full_catalog_and_insecure_source_are_rejected_before_storing_any_key() {
        let (_directory, mut manager, source) = fixture();
        let catalog = Catalog {
            version: 1,
            selected: None,
            profiles: (1..=MAX_PROFILES)
                .map(|i| Summary {
                    id: format!("{i:032x}"),
                    name: "Pico".into(),
                })
                .collect(),
        };
        manager.save(&catalog).unwrap();
        assert!(matches!(manager.import(&source), Err(Error::Full)));
        assert!(manager.store.entries.is_empty());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(matches!(
                manager.import(&source),
                Err(Error::PrivateProfile)
            ));
        }
    }
}

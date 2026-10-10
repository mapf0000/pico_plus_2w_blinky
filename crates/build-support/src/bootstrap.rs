//! Build-time CDC installer and flash extent metadata. Executable bytes stay in the FAT image.
use super::*;
use sha2::{Digest, Sha256};
use std::fmt::Write as _;

pub(super) const COMMAND: &str = "(p=(/dev/cu.usbmodemP*3);(($#p==1))&&exec 3<>$p&&stty raw -echo<&3&&echo B1>&3&&exec /bin/sh<&3)";
pub(super) const INSTALLER: &str = include_str!("cdc_installer.sh");

#[derive(Debug)]
pub(super) struct Artifact {
    pub offset: usize,
    pub size: usize,
    pub digest: String,
}

pub(super) fn metadata(offset: usize, data: &[u8]) -> Result<Artifact> {
    if data.is_empty()
        || offset
            .checked_add(data.len())
            .is_none_or(|end| end > 4 * 1024 * 1024)
    {
        bail!("invalid CDC artifact extent");
    }
    let mut digest = String::new();
    for byte in Sha256::digest(data) {
        write!(digest, "{byte:02x}")?;
    }
    Ok(Artifact {
        offset,
        size: data.len(),
        digest,
    })
}

pub(super) fn validate_serial(serial: &str) -> Result<()> {
    if !(2..=8).contains(&serial.len())
        || !serial.starts_with('P')
        || !serial.bytes().all(|b| b.is_ascii_alphanumeric())
    {
        bail!(
            "PICO_USB_SERIAL must be 2..=8 ASCII alphanumeric characters starting with P; provision a unique value per board"
        );
    }
    Ok(())
}

pub(super) fn prepare(cfg: &Config, artifact: Option<&Artifact>) -> Result<()> {
    if !INSTALLER.starts_with("{\n") || !INSTALLER.ends_with("}\n") || INSTALLER.len() > 8192 {
        bail!("CDC installer must be one bounded compound command");
    }
    let mut generated = String::from("// Generated CDC artifact metadata; do not edit.\n");
    if let Some(a) = artifact {
        writeln!(
            generated,
            "pub static ARTIFACT: Option<crate::bootstrap_core::Artifact> = Some(crate::bootstrap_core::Artifact {{ size: {}, digest: {:?}, extents: &[crate::bootstrap_core::Extent {{ offset: {}, length: {} }}] }});",
            a.size, a.digest, a.offset, a.size
        )?;
    } else {
        generated
            .push_str("pub static ARTIFACT: Option<crate::bootstrap_core::Artifact> = None;\n");
    }
    fs::write(cfg.out_dir.join("cdc_bootstrap.rs"), generated)?;
    fs::write(cfg.out_dir.join("cdc_installer.sh"), INSTALLER)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn short_selector_and_serial_contract() {
        assert_eq!(COMMAND.len(), 96);
        assert!(COMMAND.is_ascii());
        for good in ["P1", "P1234567", "PabcDEF"] {
            validate_serial(good).unwrap();
        }
        for bad in ["", "P", "X1234567", "P12345678", "P123*", "P123é"] {
            assert!(validate_serial(bad).is_err());
        }
    }
    #[test]
    fn installer_parses_as_one_shell_compound_command() {
        let mut child = std::process::Command::new("/bin/sh")
            .arg("-n")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        std::io::Write::write_all(child.stdin.take().as_mut().unwrap(), INSTALLER.as_bytes())
            .unwrap();
        assert!(child.wait().unwrap().success());
        assert!(INSTALLER.len() < 8192);
    }
}

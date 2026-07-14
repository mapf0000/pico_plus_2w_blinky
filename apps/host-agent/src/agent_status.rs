use bytes::Bytes;

const AGENT_STATUS_MAGIC: &[u8] = b"PICOAGENT\0";

pub(crate) fn production_payload() -> Bytes {
    let hostname = hostname::get().unwrap_or_else(|_| "unknown".into());
    payload_for(&hostname.to_string_lossy())
}

pub(crate) fn payload_for(hostname: &str) -> Bytes {
    let version = env!("CARGO_PKG_VERSION");
    let mut payload =
        Vec::with_capacity(AGENT_STATUS_MAGIC.len() + version.len() + 1 + hostname.len());
    payload.extend_from_slice(AGENT_STATUS_MAGIC);
    payload.extend_from_slice(version.as_bytes());
    payload.push(0);
    payload.extend_from_slice(hostname.as_bytes());
    Bytes::from(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_payload_contains_magic_version_and_hostname() {
        let payload = production_payload();
        assert!(payload.starts_with(AGENT_STATUS_MAGIC));
        let fields = &payload[AGENT_STATUS_MAGIC.len()..];
        let split = fields
            .iter()
            .position(|byte| *byte == 0)
            .expect("version and hostname separator");
        assert_eq!(&fields[..split], env!("CARGO_PKG_VERSION").as_bytes());
        assert!(!fields[split + 1..].is_empty());
    }

    #[test]
    fn fixed_identity_payload_is_versioned_and_exact() {
        let payload = payload_for("device-self-test");
        let prefix = format!("PICOAGENT\0{}\0", env!("CARGO_PKG_VERSION"));
        assert!(payload.starts_with(prefix.as_bytes()));
        assert!(payload.ends_with(b"device-self-test"));
        assert_eq!(payload.len(), prefix.len() + "device-self-test".len());
    }
}

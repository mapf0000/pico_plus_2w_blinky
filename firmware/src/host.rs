use core::sync::atomic::{AtomicU8, Ordering};

/// Host operating system selection for automations.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum HostOs {
    Unknown = 0,
    Mac = 1,
    Windows = 2,
}

static HOST_OS: AtomicU8 = AtomicU8::new(HostOs::Unknown as u8);

pub fn set_host_os(os: HostOs) {
    HOST_OS.store(os as u8, Ordering::SeqCst);
}

pub fn get_host_os() -> HostOs {
    match HOST_OS.load(Ordering::SeqCst) {
        1 => HostOs::Mac,
        2 => HostOs::Windows,
        _ => HostOs::Unknown,
    }
}

pub fn host_os_str() -> &'static str {
    match get_host_os() {
        HostOs::Unknown => "unknown",
        HostOs::Mac => "mac",
        HostOs::Windows => "windows",
    }
}

#![no_std]

mod common;
#[cfg(feature = "macos")]
mod macos;
#[cfg(feature = "windows")]
mod windows;

pub struct BuiltinScript {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub dsl: &'static str,
}

const BUILTIN_SCRIPTS: &[BuiltinScript] = &[
    common::HELLO_WORLD,
    #[cfg(feature = "macos")]
    macos::OPEN_TERMINAL,
    #[cfg(feature = "macos")]
    macos::MACOS_HOST_AGENT,
    #[cfg(feature = "macos")]
    macos::MACOS_HOST_AGENT_DEBUG,
    #[cfg(feature = "macos")]
    macos::ASSISTANT_US,
    #[cfg(feature = "macos")]
    macos::ASSISTANT_DE,
    #[cfg(feature = "macos")]
    macos::DEMO_CALL,
];

pub fn all() -> &'static [BuiltinScript] {
    BUILTIN_SCRIPTS
}

pub fn lookup(id: &str) -> Option<&'static str> {
    BUILTIN_SCRIPTS.iter().find(|s| s.id == id).map(|s| s.dsl)
}

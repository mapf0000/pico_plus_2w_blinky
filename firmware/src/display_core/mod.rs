//! Allocation-free display models and rendering, shared by firmware and host tests.

pub mod input;
pub mod model;
pub mod renderer;
mod scene;
pub mod tile;

pub use model::{Controller, PageId, Preset, SystemSnapshot};
pub use renderer::{DisplayConfig, Renderer};
pub use scene::MAX_LOG_ROWS;

#[cfg(test)]
mod tests;

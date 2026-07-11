use wasm_bindgen::prelude::*;
use yew::prelude::*;

use dsl_core::MAX_DSL_LINES;

mod api;
mod app;
pub mod codec;
pub mod dsl;
mod filesystem;
pub mod scripts;
mod transfer;
mod ui;

use app::App;
pub(crate) use app::{BrowseRequest, ConfigState, ConnectionState, StatusState};
use ui::{
    DownloadManagerCard, FileBrowserCard, IdentityCard, LogCard, ScriptingCard, StatusCard,
    ToastBar, TransferStartCard, UsbCard,
};

#[wasm_bindgen(start)]
pub fn main_js() {
    yew::Renderer::<App>::new().render();
}

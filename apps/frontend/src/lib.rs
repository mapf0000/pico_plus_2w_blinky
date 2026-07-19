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
    DownloadManagerCard, FileBrowserCard, IdentityCard, LogCard, ScriptingCard, SecureSessionCard,
    StatusCard, ToastBar, TransferStartCard, UsbCard,
};

#[cfg(test)]
mod tests {
    #[test]
    fn indexed_db_request_adapter_does_not_own_database_cleanup() {
        let source = include_str!("../ui/idb.js");
        let (_, after_helper_start) = source
            .split_once("function requestToPromise(request)")
            .expect("requestToPromise helper");
        let (helper, _) = after_helper_start
            .split_once("function transactionDone(transaction)")
            .expect("transactionDone helper after requestToPromise");

        assert!(helper.contains("resolve(request.result)"));
        assert!(!helper.contains(".transaction("));
        assert!(!helper.contains(".close("));
    }
}

#[wasm_bindgen(start)]
pub fn main_js() {
    yew::Renderer::<App>::new().render();
}

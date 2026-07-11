#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsValue, prelude::*};

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(raw_module = "/ui/idb.js")]
extern "C" {
    #[wasm_bindgen(js_name = queueChunkPersist)]
    fn js_queue_chunk_persist(transfer_id: u64, chunk_index: u32, payload: &[u8]);

    #[wasm_bindgen(js_name = queueClearTransferChunks)]
    fn js_queue_clear_transfer_chunks(transfer_id: u64);

    #[wasm_bindgen(catch, js_name = waitForChunkPersistQueue)]
    async fn js_wait_for_chunk_persist_queue() -> Result<JsValue, JsValue>;

    #[wasm_bindgen(catch, js_name = downloadTransferFromDb)]
    async fn js_download_transfer_from_db(
        transfer_id: u64,
        chunk_count: u32,
        file_name: &str,
        max_blob_bytes: f64,
    ) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(catch, js_name = clearTransferChunks)]
    async fn js_clear_transfer_chunks(transfer_id: u64) -> Result<JsValue, JsValue>;
}

#[cfg(target_arch = "wasm32")]
pub(super) fn storage_queue_chunk_persist(transfer_id: u64, chunk_index: u32, payload: &[u8]) {
    js_queue_chunk_persist(transfer_id, chunk_index, payload);
}

#[cfg(target_arch = "wasm32")]
pub(super) fn storage_queue_clear_transfer_chunks(transfer_id: u64) {
    js_queue_clear_transfer_chunks(transfer_id);
}

#[cfg(target_arch = "wasm32")]
pub(super) async fn storage_wait_for_chunk_persist_queue() -> Result<(), String> {
    js_wait_for_chunk_persist_queue()
        .await
        .map(|_| ())
        .map_err(js_error_to_string)
}

#[cfg(target_arch = "wasm32")]
pub(super) async fn storage_download_transfer_from_db(
    transfer_id: u64,
    chunk_count: u32,
    file_name: &str,
    max_blob_bytes: f64,
) -> Result<(), String> {
    js_download_transfer_from_db(transfer_id, chunk_count, file_name, max_blob_bytes)
        .await
        .map(|_| ())
        .map_err(js_error_to_string)
}

#[cfg(target_arch = "wasm32")]
pub(super) async fn storage_clear_transfer_chunks(transfer_id: u64) -> Result<(), String> {
    js_clear_transfer_chunks(transfer_id)
        .await
        .map(|_| ())
        .map_err(js_error_to_string)
}

#[cfg(not(target_arch = "wasm32"))]
mod native_storage {
    use std::collections::HashMap;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    type ChunkKey = (u64, u32);
    type ChunkMap = HashMap<ChunkKey, Vec<u8>>;

    static CHUNKS: OnceLock<Mutex<ChunkMap>> = OnceLock::new();

    fn chunks() -> &'static Mutex<ChunkMap> {
        CHUNKS.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn lock_chunks() -> MutexGuard<'static, ChunkMap> {
        match chunks().lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub fn queue_chunk_persist(transfer_id: u64, chunk_index: u32, payload: &[u8]) {
        let mut db = lock_chunks();
        db.insert((transfer_id, chunk_index), payload.to_vec());
    }

    pub fn queue_clear_transfer_chunks(transfer_id: u64) {
        let mut db = lock_chunks();
        db.retain(|(existing_transfer_id, _), _| *existing_transfer_id != transfer_id);
    }

    pub async fn wait_for_chunk_persist_queue() -> Result<(), String> {
        Ok(())
    }

    pub async fn download_transfer_from_db(
        transfer_id: u64,
        chunk_count: u32,
        _file_name: &str,
        _max_blob_bytes: f64,
    ) -> Result<(), String> {
        let db = lock_chunks();
        for chunk_index in 0..chunk_count {
            if !db.contains_key(&(transfer_id, chunk_index)) {
                return Err(format!("Missing chunk {chunk_index}"));
            }
        }
        Ok(())
    }

    pub async fn clear_transfer_chunks(transfer_id: u64) -> Result<(), String> {
        queue_clear_transfer_chunks(transfer_id);
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn storage_queue_chunk_persist(transfer_id: u64, chunk_index: u32, payload: &[u8]) {
    native_storage::queue_chunk_persist(transfer_id, chunk_index, payload);
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn storage_queue_clear_transfer_chunks(transfer_id: u64) {
    native_storage::queue_clear_transfer_chunks(transfer_id);
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) async fn storage_wait_for_chunk_persist_queue() -> Result<(), String> {
    native_storage::wait_for_chunk_persist_queue().await
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) async fn storage_download_transfer_from_db(
    transfer_id: u64,
    chunk_count: u32,
    file_name: &str,
    max_blob_bytes: f64,
) -> Result<(), String> {
    native_storage::download_transfer_from_db(transfer_id, chunk_count, file_name, max_blob_bytes)
        .await
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) async fn storage_clear_transfer_chunks(transfer_id: u64) -> Result<(), String> {
    native_storage::clear_transfer_chunks(transfer_id).await
}

#[cfg(target_arch = "wasm32")]
fn js_error_to_string(value: JsValue) -> String {
    value
        .as_string()
        .or_else(|| {
            js_sys::JSON::stringify(&value)
                .ok()
                .and_then(|v| v.as_string())
        })
        .unwrap_or_else(|| "javascript error".to_string())
}

use super::storage::{
    storage_clear_transfer_chunks, storage_download_transfer_from_db,
    storage_wait_for_chunk_persist_queue,
};
use super::{FinalizeError, FinalizePlan};

const MAX_BLOB_FALLBACK_BYTES: f64 = 128.0 * 1024.0 * 1024.0;

pub async fn finalize_and_download(plan: &FinalizePlan) -> Result<(), FinalizeError> {
    storage_wait_for_chunk_persist_queue()
        .await
        .map_err(FinalizeError::download)?;

    verify_finalize_plan(plan).map_err(FinalizeError::verification)?;

    storage_download_transfer_from_db(
        plan.transfer_id,
        plan.chunk_count,
        &plan.file_name,
        MAX_BLOB_FALLBACK_BYTES,
    )
    .await
    .map_err(FinalizeError::download)?;

    storage_clear_transfer_chunks(plan.transfer_id)
        .await
        .map_err(FinalizeError::download)?;

    Ok(())
}

pub(super) fn verify_finalize_plan(plan: &FinalizePlan) -> Result<(), String> {
    if !plan.expected_sha256_hex.is_empty()
        && !plan
            .expected_sha256_hex
            .eq_ignore_ascii_case(&plan.computed_sha256_hex)
    {
        return Err("sha256 mismatch".to_string());
    }
    Ok(())
}

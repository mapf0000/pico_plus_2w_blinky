mod download;
mod model;
mod protocol;
mod secure;
mod storage;
mod store;

pub use download::finalize_and_download;
pub use model::{
    ChunkCounters, ChunkState, FinalizeError, FinalizePlan, TransferState, TransferStore,
    TransferView,
};
pub use secure::{SecureSession, SecureSessionView, secure_transfer_id};

#[cfg(test)]
use download::verify_finalize_plan;
#[cfg(test)]
use sha2::{Digest, Sha256};
#[cfg(test)]
use store::hex_lower;

#[cfg(test)]
mod tests {
    use super::*;

    fn open(
        transfer_id: u64,
        total_size: u64,
        chunk_size: u16,
        chunk_count: u32,
    ) -> secure::SecureOpenData {
        secure::SecureOpenData {
            transfer_id,
            file_name: "x.bin".into(),
            total_size,
            chunk_size,
            chunk_count,
        }
    }

    #[test]
    fn plaintext_open_is_rejected() {
        let mut store = TransferStore::new();
        let error = store
            .apply_text_event(
                r#"{"event_type":"transfer/open","version":2,"transfer_id":1,"file_name":"a.bin","total_size":10,"chunk_size":5,"chunk_count":2,"sha256":""}"#,
                1000.0,
            )
            .unwrap_err();
        assert!(error.contains("unauthenticated"));
    }

    #[test]
    fn authenticated_open_and_progress_update_state() {
        let mut store = TransferStore::new();
        store.apply_secure_open(open(1, 10, 5, 2), 1000.0).unwrap();
        store
            .apply_text_event(
                r#"{"event_type":"transfer/progress","version":2,"transfer_id":1,"received_size":5,"total_size":10,"finished_chunks":1,"chunk_count":2}"#,
                1500.0,
            )
            .unwrap();

        let rows = store.snapshots();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].received_size, 5);
        assert_eq!(rows[0].finished_chunks, 1);
        assert_eq!(rows[0].status, TransferState::Downloading);
    }

    #[test]
    fn ewma_and_eta_are_computed() {
        let mut store = TransferStore::new();
        store.apply_secure_open(open(5, 100, 10, 10), 0.0).unwrap();
        let payload = vec![1u8; 10];
        store.apply_secure_chunk(5, 0, &payload, 1000.0).unwrap();
        let rows = store.snapshots();
        assert!(rows[0].smoothed_rate_bps > 0.0);
        assert!(rows[0].eta_total_secs.is_some());
    }

    #[test]
    fn rejects_out_of_order_authenticated_chunks() {
        let mut store = TransferStore::new();
        store.apply_secure_open(open(9, 20, 10, 2), 0.0).unwrap();
        let payload = vec![1u8; 10];
        let err = store
            .apply_secure_chunk(9, 1, &payload, 1000.0)
            .unwrap_err();
        assert!(err.contains("out-of-order chunk"));
    }

    #[test]
    fn finalize_plan_uses_streamed_sha256() {
        let payload = b"hello-world";
        let expected_sha256: [u8; 32] = {
            let mut h = Sha256::new();
            h.update(payload);
            h.finalize().into()
        };

        let mut store = TransferStore::new();
        store
            .apply_secure_open(open(11, payload.len() as u64, payload.len() as u16, 1), 0.0)
            .unwrap();
        store.apply_secure_chunk(11, 0, payload, 1000.0).unwrap();
        store
            .apply_secure_close(
                secure::SecureCloseData {
                    transfer_id: 11,
                    total_size: payload.len() as u64,
                    chunk_count: 1,
                    sha256: expected_sha256,
                },
                1100.0,
            )
            .unwrap();

        let plan = store.prepare_finalize(11).unwrap();
        assert_eq!(plan.computed_sha256_hex, hex_lower(&expected_sha256));
        verify_finalize_plan(&plan).unwrap();
    }

    #[test]
    fn download_errors_keep_completed_transfer_retryable() {
        let mut store = TransferStore::new();
        store.apply_secure_open(open(21, 0, 4, 0), 0.0).unwrap();
        let empty_digest: [u8; 32] = Sha256::digest([]).into();
        store
            .apply_secure_close(
                secure::SecureCloseData {
                    transfer_id: 21,
                    total_size: 0,
                    chunk_count: 0,
                    sha256: empty_digest,
                },
                10.0,
            )
            .unwrap();

        store.finish_finalize(21, &Err(FinalizeError::download("picker canceled")));

        let rows = store.snapshots();
        assert_eq!(rows[0].status, TransferState::Finished);
    }

    #[test]
    fn verification_errors_mark_transfer_failed() {
        let mut store = TransferStore::new();
        store.apply_secure_open(open(22, 4, 4, 1), 0.0).unwrap();

        store.finish_finalize(22, &Err(FinalizeError::verification("sha256 mismatch")));

        let rows = store.snapshots();
        assert_eq!(rows[0].status, TransferState::Failed);
        assert_eq!(rows[0].failed_chunks, 1);
    }
}

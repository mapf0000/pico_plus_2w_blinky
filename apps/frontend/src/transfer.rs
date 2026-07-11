mod download;
mod model;
mod protocol;
mod storage;
mod store;

pub use download::finalize_and_download;
pub use model::{
    ChunkCounters, ChunkState, FinalizeError, FinalizePlan, TransferState, TransferStore,
    TransferView,
};

#[cfg(test)]
use crc32fast::Hasher as Crc32Hasher;
#[cfg(test)]
use download::verify_finalize_plan;
#[cfg(test)]
use protocol::decode_chunk_envelope;
#[cfg(test)]
use sha2::{Digest, Sha256};
#[cfg(test)]
use store::hex_lower;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_binary_chunk_roundtrip() {
        let payload = b"hello";
        let mut crc32 = Crc32Hasher::new();
        crc32.update(payload);
        let crc = crc32.finalize();

        let mut frame = Vec::new();
        frame.extend_from_slice(&7u64.to_le_bytes());
        frame.extend_from_slice(&3u32.to_le_bytes());
        frame.extend_from_slice(&42u64.to_le_bytes());
        frame.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        frame.extend_from_slice(&crc.to_le_bytes());
        frame.extend_from_slice(payload);

        let chunk = decode_chunk_envelope(&frame).expect("decode chunk envelope");
        assert_eq!(chunk.transfer_id, 7);
        assert_eq!(chunk.chunk_index, 3);
        assert_eq!(chunk.offset, 42);
        assert_eq!(chunk.payload, payload);
        assert_eq!(chunk.crc32, crc);
    }

    #[test]
    fn open_and_progress_updates_state() {
        let mut store = TransferStore::new();
        store
            .apply_text_event(
                r#"{"event_type":"transfer/open","version":1,"transfer_id":1,"file_name":"a.bin","total_size":10,"chunk_size":5,"chunk_count":2,"sha256":""}"#,
                1000.0,
            )
            .unwrap();
        store
            .apply_text_event(
                r#"{"event_type":"transfer/progress","version":1,"transfer_id":1,"received_size":5,"total_size":10,"finished_chunks":1,"chunk_count":2}"#,
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
        store
            .apply_text_event(
                r#"{"event_type":"transfer/open","version":1,"transfer_id":5,"file_name":"a.bin","total_size":100,"chunk_size":10,"chunk_count":10,"sha256":""}"#,
                0.0,
            )
            .unwrap();

        let payload = vec![1u8; 10];
        let mut crc = Crc32Hasher::new();
        crc.update(&payload);
        let crc = crc.finalize();

        let mut frame = Vec::new();
        frame.extend_from_slice(&5u64.to_le_bytes());
        frame.extend_from_slice(&0u32.to_le_bytes());
        frame.extend_from_slice(&0u64.to_le_bytes());
        frame.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        frame.extend_from_slice(&crc.to_le_bytes());
        frame.extend_from_slice(&payload);

        store.apply_binary_chunk(&frame, 1000.0).unwrap();
        let rows = store.snapshots();
        assert!(rows[0].smoothed_rate_bps > 0.0);
        assert!(rows[0].eta_total_secs.is_some());
    }

    #[test]
    fn rejects_out_of_order_binary_chunks() {
        let mut store = TransferStore::new();
        store
            .apply_text_event(
                r#"{"event_type":"transfer/open","version":1,"transfer_id":9,"file_name":"x.bin","total_size":20,"chunk_size":10,"chunk_count":2,"sha256":""}"#,
                0.0,
            )
            .unwrap();

        let payload = vec![1u8; 10];
        let mut crc = Crc32Hasher::new();
        crc.update(&payload);
        let crc = crc.finalize();

        let mut frame = Vec::new();
        frame.extend_from_slice(&9u64.to_le_bytes());
        frame.extend_from_slice(&1u32.to_le_bytes());
        frame.extend_from_slice(&10u64.to_le_bytes());
        frame.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        frame.extend_from_slice(&crc.to_le_bytes());
        frame.extend_from_slice(&payload);

        let err = store.apply_binary_chunk(&frame, 1000.0).unwrap_err();
        assert!(err.contains("out-of-order chunk"));
    }

    #[test]
    fn finalize_plan_uses_streamed_sha256() {
        let payload = b"hello-world";
        let expected_sha256 = {
            let mut h = Sha256::new();
            h.update(payload);
            hex_lower(&h.finalize())
        };

        let mut store = TransferStore::new();
        store
            .apply_text_event(
                &format!(
                    "{{\"event_type\":\"transfer/open\",\"version\":1,\"transfer_id\":11,\"file_name\":\"x.bin\",\"total_size\":{},\"chunk_size\":{},\"chunk_count\":1,\"sha256\":\"{}\"}}",
                    payload.len(),
                    payload.len(),
                    expected_sha256
                ),
                0.0,
            )
            .unwrap();

        let mut crc = Crc32Hasher::new();
        crc.update(payload);
        let crc = crc.finalize();

        let mut frame = Vec::new();
        frame.extend_from_slice(&11u64.to_le_bytes());
        frame.extend_from_slice(&0u32.to_le_bytes());
        frame.extend_from_slice(&0u64.to_le_bytes());
        frame.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        frame.extend_from_slice(&crc.to_le_bytes());
        frame.extend_from_slice(payload);
        store.apply_binary_chunk(&frame, 1000.0).unwrap();

        let plan = store.prepare_finalize(11).unwrap();
        assert_eq!(plan.computed_sha256_hex, expected_sha256);
        verify_finalize_plan(&plan).unwrap();
    }

    #[test]
    fn download_errors_keep_completed_transfer_retryable() {
        let mut store = TransferStore::new();
        store
            .apply_text_event(
                r#"{"event_type":"transfer/open","version":1,"transfer_id":21,"file_name":"x.bin","total_size":4,"chunk_size":4,"chunk_count":1,"sha256":""}"#,
                0.0,
            )
            .unwrap();
        store
            .apply_text_event(
                r#"{"event_type":"transfer/finished","version":1,"transfer_id":21}"#,
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
        store
            .apply_text_event(
                r#"{"event_type":"transfer/open","version":1,"transfer_id":22,"file_name":"x.bin","total_size":4,"chunk_size":4,"chunk_count":1,"sha256":""}"#,
                0.0,
            )
            .unwrap();

        store.finish_finalize(22, &Err(FinalizeError::verification("sha256 mismatch")));

        let rows = store.snapshots();
        assert_eq!(rows[0].status, TransferState::Failed);
        assert_eq!(rows[0].failed_chunks, 1);
    }
}

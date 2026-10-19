//! One disk receiver; each record is owned by a retained async worker task.
use crate::download_crypto::SecureSession;
use ble_protocol::download as wire;
use companion_core::download::{self, Command, Item, Session, State, View};
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    path::PathBuf,
    time::{Duration, Instant},
};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
struct Request {
    path: Option<String>,
    destination: PathBuf,
}
struct Active {
    id: u64,
    chunk_size: u16,
    chunk_count: u32,
    next: u32,
    bytes: u64,
    total: u64,
    hash: Sha256,
    file: tokio::fs::File,
    started: Instant,
    last_digest: Option<[u8; 32]>,
}
pub struct Receiver {
    crypto: SecureSession,
    pub view: View,
    queue: VecDeque<Request>,
    requested: Option<Request>,
    active: Option<Active>,
    deadline: Option<Instant>,
}
impl Default for Receiver {
    fn default() -> Self {
        Self::new()
    }
}
impl Receiver {
    pub fn new() -> Self {
        Self {
            crypto: SecureSession::new(),
            view: View::default(),
            queue: VecDeque::new(),
            requested: None,
            active: None,
            deadline: None,
        }
    }
    pub fn expired(&self) -> bool {
        self.deadline.is_some_and(|d| Instant::now() >= d)
    }
    pub fn failed_view(mut view: View, message: &str) -> View {
        let preserve = view.session == Session::Failed && view.error.is_some();
        view.session = Session::Failed;
        if !preserve {
            view.error = Some(message.into());
        }
        for i in &mut view.items {
            if matches!(
                i.state,
                State::Queued | State::Requested | State::Receiving | State::Verifying
            ) {
                i.state = State::Failed;
                i.error = Some(message.into());
            }
        }
        view
    }
    pub fn fail(&mut self, message: &str) {
        self.view = Self::failed_view(self.view.clone(), message);
        if let Some(active) = &self.active {
            self.crypto.discard_file(active.id);
        }
        self.crypto.reset();
        self.active = None;
        self.queue.clear();
        self.requested = None;
        self.deadline = None;
    }
    pub async fn command(&mut self, command: Command) -> Result<Vec<Vec<u8>>, String> {
        if !command.valid() {
            return Err("Invalid path or destination; use an absolute local filename".into());
        }
        match command {
            Command::ConnectSession => {
                if self.view.active() {
                    return Err("Cancel downloads before replacing the session".into());
                }
                self.crypto.reset();
                let frame = self.crypto.request_session()?;
                self.view.session = Session::Negotiating;
                self.view.error = None;
                self.deadline = Some(Instant::now() + Duration::from_secs(60));
                Ok(vec![frame])
            }
            Command::SetDefault(path) => Ok(vec![self.crypto.seal_default_path(&path)?]),
            Command::Queue { path, destination } => self.enqueue(Some(path), destination).await,
            Command::QueueDefault { destination } => self.enqueue(None, destination).await,
            Command::Cancel => {
                self.fail("Downloads cancelled");
                for i in &mut self.view.items {
                    if i.state == State::Failed && i.error.as_deref() == Some("Downloads cancelled")
                    {
                        i.state = State::Aborted;
                    }
                }
                Ok(Vec::new())
            }
        }
    }
    async fn enqueue(
        &mut self,
        path: Option<String>,
        destination: PathBuf,
    ) -> Result<Vec<Vec<u8>>, String> {
        if self.view.session != Session::Ready {
            return Err("Connect the encrypted host session first".into());
        }
        if self.queue.len()
            + usize::from(self.requested.is_some())
            + usize::from(self.active.is_some())
            >= download::MAX_QUEUE
        {
            return Err("Download queue is full".into());
        }
        if self.view.items.iter().any(|i| {
            i.destination == destination
                && matches!(
                    i.state,
                    State::Queued | State::Requested | State::Receiving | State::Verifying
                )
        }) {
            return Err("That destination is already queued".into());
        }
        if tokio::fs::symlink_metadata(&destination).await.is_ok() {
            return Err("Destination exists; choose a new filename".into());
        }
        while self.view.items.len() >= download::MAX_HISTORY {
            let Some(index) = self
                .view
                .items
                .iter()
                .position(|i| matches!(i.state, State::Saved | State::Failed | State::Aborted))
            else {
                return Err("Download history is full".into());
            };
            self.view.items.remove(index);
        }
        self.view.items.push_back(Item {
            id: 0,
            name: String::new(),
            destination: destination.clone(),
            state: State::Queued,
            received: 0,
            total: 0,
            bytes_per_second: 0.0,
            retries: 0,
            error: None,
        });
        self.queue.push_back(Request { path, destination });
        self.start_next()
    }
    fn start_next(&mut self) -> Result<Vec<Vec<u8>>, String> {
        if self.requested.is_some() || self.active.is_some() {
            return Ok(Vec::new());
        }
        let Some(request) = self.queue.pop_front() else {
            self.deadline = None;
            return Ok(Vec::new());
        };
        let frame = if let Some(path) = &request.path {
            self.crypto.seal_start(path)?
        } else {
            self.crypto.seal_start_default()?
        };
        self.view
            .items
            .iter_mut()
            .find(|i| i.destination == request.destination && i.state == State::Queued)
            .ok_or("Missing queued download")?
            .state = State::Requested;
        self.requested = Some(request);
        self.deadline = Some(Instant::now() + Duration::from_secs(60));
        Ok(vec![frame])
    }
    pub async fn record(&mut self, record: &[u8]) -> Result<Vec<Vec<u8>>, String> {
        let (tag, body) = wire::record(record).map_err(|_| "Invalid transfer record")?;
        let previous_deadline = self.deadline;
        self.deadline = Some(Instant::now() + Duration::from_secs(60));
        match tag {
            wire::SESSION => {
                let envelope = transfer_protocol::decode_session_envelope(body)
                    .map_err(|_| "Invalid session envelope")?;
                if envelope.kind == transfer_protocol::SESSION_KIND_ERROR {
                    if self.crypto.session_id() == Some(envelope.session_id) {
                        self.fail("Host could not start or complete the transfer");
                    } else {
                        self.deadline = previous_deadline;
                    }
                    return Ok(Vec::new());
                }
                let response = self.crypto.handle_session(body)?;
                if self.crypto.view().established() {
                    self.view.session = Session::Ready;
                    self.deadline = None;
                }
                Ok(response.into_iter().collect())
            }
            wire::OPEN => {
                if self.active.is_some() {
                    return Err("Duplicate or unsolicited file manifest".into());
                }
                let request = self.requested.take().ok_or("Unsolicited file manifest")?;
                let open = self.crypto.decrypt_open(body)?;
                if open.total_size > download::MAX_FILE_BYTES
                    || open.chunk_size == 0
                    || u64::from(open.chunk_count)
                        != open.total_size.div_ceil(u64::from(open.chunk_size))
                {
                    return Err(
                        "File exceeds the 1 MiB limit or has inconsistent chunk geometry".into(),
                    );
                }
                let parent = request
                    .destination
                    .parent()
                    .ok_or("Missing destination directory")?
                    .to_owned();
                // Anonymous staging is removed by the OS even after an abrupt process exit.
                let handle = tokio::task::spawn_blocking(move || tempfile::tempfile_in(parent))
                    .await
                    .map_err(|_| "Storage worker failed")?
                    .map_err(|_| "Cannot create private staging file")?;
                let item = self
                    .view
                    .items
                    .iter_mut()
                    .find(|i| i.destination == request.destination && i.state == State::Requested)
                    .ok_or("Missing requested download")?;
                item.id = open.transfer_id;
                item.name = open.file_name.chars().map(|c| {
                    if c.is_control() || matches!(c as u32,0x061c|0x200e|0x200f|0x202a..=0x202e|0x2066..=0x2069) {'�'}else{c}
                }).collect();
                item.total = open.total_size;
                item.state = State::Receiving;
                self.active = Some(Active {
                    id: open.transfer_id,
                    chunk_size: open.chunk_size,
                    chunk_count: open.chunk_count,
                    next: 0,
                    bytes: 0,
                    total: open.total_size,
                    hash: Sha256::new(),
                    file: tokio::fs::File::from_std(handle),
                    started: Instant::now(),
                    last_digest: None,
                });
                Ok(Vec::new())
            }
            wire::CHUNK => {
                let (id, index, plain) = self.crypto.decrypt_chunk(body)?;
                let a = self.active.as_mut().ok_or("Chunk before manifest")?;
                if id != a.id {
                    return Err("Chunk belongs to another transfer".into());
                }
                let digest: [u8; 32] = Sha256::digest(body).into();
                let item = self
                    .view
                    .items
                    .iter_mut()
                    .find(|i| i.id == id && i.state == State::Receiving)
                    .ok_or("Missing active download")?;
                if a.next > 0 && index == a.next - 1 {
                    if a.last_digest != Some(digest) {
                        return Err("Retry changed its ciphertext".into());
                    }
                    item.retries = item.retries.saturating_add(1);
                    return Ok(Vec::new());
                }
                if index != a.next || index >= a.chunk_count {
                    return Err("Chunk ordering mismatch".into());
                }
                let expected = if index + 1 < a.chunk_count {
                    u64::from(a.chunk_size)
                } else {
                    a.total.saturating_sub(a.bytes)
                };
                if index != a.next || index >= a.chunk_count || plain.len() as u64 != expected {
                    return Err("Chunk ordering or length mismatch".into());
                }
                a.file
                    .write_all(&plain)
                    .await
                    .map_err(|_| "Disk write failed")?;
                a.file.flush().await.map_err(|_| "Disk flush failed")?;
                a.hash.update(plain.as_slice());
                a.bytes += plain.len() as u64;
                a.next += 1;
                a.last_digest = Some(digest);
                item.received = a.bytes;
                item.bytes_per_second =
                    a.bytes as f64 / a.started.elapsed().as_secs_f64().max(0.001);
                Ok(Vec::new())
            }
            wire::CLOSE => {
                let close = self.crypto.decrypt_close(body)?;
                let mut a = self.active.take().ok_or("Close before manifest")?;
                let hash: [u8; 32] = a.hash.finalize().into();
                if close.transfer_id != a.id
                    || close.total_size != a.total
                    || a.bytes != a.total
                    || close.chunk_count != a.chunk_count
                    || a.next != a.chunk_count
                    || close.sha256 != hash
                {
                    return Err("File length, chunk count or SHA-256 verification failed".into());
                }
                let item = self
                    .view
                    .items
                    .iter_mut()
                    .find(|i| i.id == a.id && i.state == State::Receiving)
                    .ok_or("Missing active download")?;
                item.state = State::Verifying;
                a.file.flush().await.map_err(|_| "Disk flush failed")?;
                a.file.sync_all().await.map_err(|_| "Disk sync failed")?;
                a.file
                    .seek(std::io::SeekFrom::Start(0))
                    .await
                    .map_err(|_| "Cannot rewind verified staging file")?;
                let destination = item.destination.clone();
                let parent = destination
                    .parent()
                    .ok_or("Missing destination directory")?
                    .to_owned();
                let stage = tokio::task::spawn_blocking(move || {
                    tempfile::Builder::new()
                        .prefix(".pico-download-")
                        .tempfile_in(parent)
                })
                .await
                .map_err(|_| "Save worker failed")?
                .map_err(|_| "Cannot create save file")?;
                let mut output =
                    tokio::fs::File::from_std(stage.reopen().map_err(|_| "Cannot open save file")?);
                let mut copy_hash = Sha256::new();
                let mut copied = 0u64;
                let mut buffer = zeroize::Zeroizing::new([0u8; 8192]);
                loop {
                    let n = a
                        .file
                        .read(&mut buffer[..])
                        .await
                        .map_err(|_| "Verified staging read failed")?;
                    if n == 0 {
                        break;
                    }
                    copied = copied.checked_add(n as u64).ok_or("Save size overflow")?;
                    if copied > a.total {
                        return Err("Verified staging size changed".into());
                    }
                    output
                        .write_all(&buffer[..n])
                        .await
                        .map_err(|_| "Verified file copy failed")?;
                    copy_hash.update(&buffer[..n]);
                }
                let copied_hash: [u8; 32] = copy_hash.finalize().into();
                if copied != a.total || copied_hash != hash {
                    return Err("Verified staging size or SHA-256 changed".into());
                }
                output.flush().await.map_err(|_| "Save flush failed")?;
                output.sync_all().await.map_err(|_| "Save sync failed")?;
                drop(output);
                drop(a.file);
                tokio::task::spawn_blocking(move || stage.persist_noclobber(destination))
                    .await
                    .map_err(|_| "Save worker failed")?
                    .map_err(|_| "Cannot save file; destination exists or storage failed")?;
                item.state = State::Saved;
                let receipt = self.crypto.seal_receipt(close.transfer_id, &hash)?;
                let mut out = vec![receipt];
                out.extend(self.start_next()?);
                Ok(out)
            }
            wire::ABORT => {
                self.fail("Host agent aborted the transfer");
                Ok(Vec::new())
            }
            _ => Err("Unsupported transfer record".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use transfer_crypto::{
        FileCipher, HostHandshake, derive_session_psk, encode_close, encode_manifest,
    };
    use transfer_protocol as p;
    fn record(tag: u8, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![tag];
        out.extend_from_slice(payload);
        out
    }
    fn envelope(kind: u8, id: &[u8; 16], body: &[u8]) -> Vec<u8> {
        let mut out = vec![0; p::MAX_SECURE_SESSION_FRAME];
        let n = p::encode_session_envelope(&mut out, kind, id, body).unwrap();
        out.truncate(n);
        record(wire::SESSION, &out)
    }
    async fn session() -> (Receiver, transfer_crypto::HostTransport, [u8; 32], [u8; 16]) {
        let mut r = Receiver::new();
        r.command(Command::ConnectSession).await.unwrap();
        let id = [7; 16];
        let code = "00112233445566778899aabbccddeeff";
        let first = r
            .record(&envelope(p::SESSION_KIND_READY, &id, code.as_bytes()))
            .await
            .unwrap()
            .remove(0);
        let e = p::decode_session_envelope(&first).unwrap();
        let psk = derive_session_psk(code, &id).unwrap();
        let (mut host, response) = HostHandshake::new(&psk).unwrap().respond(e.body).unwrap();
        let master_frame = r
            .record(&envelope(p::SESSION_KIND_HANDSHAKE, &id, &response))
            .await
            .unwrap()
            .remove(0);
        let e = p::decode_session_envelope(&master_frame).unwrap();
        let master: [u8; 32] = host.open(e.body).unwrap().try_into().unwrap();
        let ready = host.seal(b"pico-transfer-ready-v1").unwrap();
        r.record(&envelope(p::SESSION_KIND_TRANSPORT, &id, &ready))
            .await
            .unwrap();
        assert_eq!(r.view.session, Session::Ready);
        (r, host, master, id)
    }
    fn open(id: &[u8; 16], master: &[u8; 32], size: u64, name: &str) -> (FileCipher, Vec<u8>, u32) {
        let count = size.div_ceil(p::MAX_PLAINTEXT_CHUNK as u64) as u32;
        let cipher = FileCipher::new(
            master,
            id,
            42,
            &[9; 32],
            p::MAX_PLAINTEXT_CHUNK as u16,
            count,
        )
        .unwrap();
        let manifest = cipher
            .seal(p::RECORD_MANIFEST, 0, &encode_manifest(name, size).unwrap())
            .unwrap();
        let mut out = vec![0; p::TLV_MAX_PAYLOAD];
        let n = p::encode_secure_open(
            &mut out,
            id,
            42,
            &[9; 32],
            p::MAX_PLAINTEXT_CHUNK as u16,
            count,
            &manifest,
        )
        .unwrap();
        (cipher, record(wire::OPEN, &out[..n]), count)
    }
    fn chunk(id: &[u8; 16], cipher: &FileCipher, index: u32, data: &[u8]) -> Vec<u8> {
        let encrypted = cipher.seal_chunk(index, data).unwrap();
        let mut out = vec![0; p::TLV_MAX_PAYLOAD];
        let n = p::encode_secure_chunk(&mut out, id, 42, index, &encrypted).unwrap();
        record(wire::CHUNK, &out[..n])
    }
    fn close(
        id: &[u8; 16],
        cipher: &FileCipher,
        size: u64,
        count: u32,
        hash: &[u8; 32],
    ) -> Vec<u8> {
        let encrypted = cipher
            .seal(p::RECORD_CLOSE, 0, &encode_close(size, count, hash))
            .unwrap();
        let mut out = vec![0; p::TLV_MAX_PAYLOAD];
        let n = p::encode_secure_close(&mut out, id, 42, &encrypted).unwrap();
        record(wire::CLOSE, &out[..n])
    }
    #[tokio::test]
    async fn empty_and_multichunk_files_save_only_after_verification_and_receipt() {
        for size in [0, 3 * p::MAX_PLAINTEXT_CHUNK + 17] {
            let dir = tempfile::tempdir().unwrap();
            let dest = dir.path().join("chosen.bin");
            let (mut r, mut host, master, id) = session().await;
            let start = r
                .command(Command::Queue {
                    path: "/host/source".into(),
                    destination: dest.clone(),
                })
                .await
                .unwrap()
                .remove(0);
            let e = p::decode_session_envelope(&start).unwrap();
            assert_eq!(host.open(e.body).unwrap(), b"\x01/host/source");
            let data: Vec<u8> = (0..size).map(|i| i as u8).collect();
            let (cipher, manifest, count) = open(&id, &master, size as u64, "../untrusted.bin");
            r.record(&manifest).await.unwrap();
            assert!(!dest.exists());
            for (index, data) in data.chunks(p::MAX_PLAINTEXT_CHUNK).enumerate() {
                r.record(&chunk(&id, &cipher, index as u32, data))
                    .await
                    .unwrap();
            }
            assert!(!dest.exists());
            let hash: [u8; 32] = Sha256::digest(&data).into();
            let receipt = r
                .record(&close(&id, &cipher, size as u64, count, &hash))
                .await
                .unwrap()
                .remove(0);
            let e = p::decode_session_envelope(&receipt).unwrap();
            let plain = host.open(e.body).unwrap();
            assert_eq!(plain[0], p::CONTROL_TRANSFER_RECEIPT);
            assert_eq!(&plain[9..], &hash);
            assert_eq!(std::fs::read(&dest).unwrap(), data);
            assert_eq!(r.view.items[0].state, State::Saved);
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        }
    }
    #[tokio::test]
    async fn corrupt_order_length_and_hash_never_publish_a_file() {
        for mode in 0..5 {
            let dir = tempfile::tempdir().unwrap();
            let dest = dir.path().join("chosen.bin");
            let (mut r, _, master, id) = session().await;
            r.command(Command::Queue {
                path: "/host/source".into(),
                destination: dest.clone(),
            })
            .await
            .unwrap();
            let (cipher, manifest, count) = open(&id, &master, 3, "a.bin");
            r.record(&manifest).await.unwrap();
            let result = match mode {
                0 => {
                    let mut b = chunk(&id, &cipher, 0, b"abc");
                    *b.last_mut().unwrap() ^= 1;
                    r.record(&b).await
                }
                1 => r.record(&chunk(&id, &cipher, 1, b"abc")).await,
                2 => r.record(&chunk(&id, &cipher, 0, b"ab")).await,
                4 => r.record(&chunk(&id, &cipher, u32::MAX, b"abc")).await,
                _ => {
                    r.record(&chunk(&id, &cipher, 0, b"abc")).await.unwrap();
                    r.record(&close(&id, &cipher, 3, count, &[0; 32])).await
                }
            };
            assert!(result.is_err());
            r.fail("test failure");
            assert!(!dest.exists());
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        }
    }
    #[tokio::test]
    async fn exact_ciphertext_retry_is_idempotent_and_cancellation_cleans_staging() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("chosen.bin");
        let (mut r, _, master, id) = session().await;
        r.command(Command::Queue {
            path: "/source".into(),
            destination: dest.clone(),
        })
        .await
        .unwrap();
        let (cipher, manifest, _) = open(&id, &master, 3, "file.bin");
        r.record(&manifest).await.unwrap();
        let b = chunk(&id, &cipher, 0, b"abc");
        r.record(&b).await.unwrap();
        r.record(&b).await.unwrap();
        assert_eq!(r.view.items[0].received, 3);
        assert_eq!(r.view.items[0].retries, 1);
        r.command(Command::Cancel).await.unwrap();
        assert!(!dest.exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        assert_eq!(r.view.items[0].state, State::Aborted);
    }
    #[tokio::test]
    async fn destination_race_preserves_existing_file_and_discards_staging() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("chosen.bin");
        let (mut r, _, master, id) = session().await;
        r.command(Command::Queue {
            path: "/source".into(),
            destination: dest.clone(),
        })
        .await
        .unwrap();
        let (cipher, manifest, count) = open(&id, &master, 0, "file.bin");
        r.record(&manifest).await.unwrap();
        std::fs::write(&dest, b"existing").unwrap();
        let hash: [u8; 32] = Sha256::digest([]).into();
        assert!(
            r.record(&close(&id, &cipher, 0, count, &hash))
                .await
                .is_err()
        );
        r.fail("save failed");
        assert_eq!(std::fs::read(&dest).unwrap(), b"existing");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    #[tokio::test]
    async fn queue_default_controls_bounds_and_unsolicited_manifests() {
        let dir = tempfile::tempdir().unwrap();
        let (mut r, mut host, master, id) = session().await;
        let frame = r
            .command(Command::SetDefault("/source".into()))
            .await
            .unwrap()
            .remove(0);
        assert_eq!(
            host.open(p::decode_session_envelope(&frame).unwrap().body)
                .unwrap(),
            b"\x02/source"
        );
        let frame = r
            .command(Command::QueueDefault {
                destination: dir.path().join("a"),
            })
            .await
            .unwrap()
            .remove(0);
        assert_eq!(
            host.open(p::decode_session_envelope(&frame).unwrap().body)
                .unwrap(),
            [p::CONTROL_START_DEFAULT_TRANSFER]
        );
        for n in 1..download::MAX_QUEUE {
            assert!(
                r.command(Command::QueueDefault {
                    destination: dir.path().join(n.to_string())
                })
                .await
                .unwrap()
                .is_empty()
            );
        }
        assert!(
            r.command(Command::QueueDefault {
                destination: dir.path().join("full")
            })
            .await
            .is_err()
        );
        let (_, manifest, _) = open(&id, &master, download::MAX_FILE_BYTES + 1, "file.bin");
        assert!(r.record(&manifest).await.is_err());
        r.fail("too large");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        assert!(r.record(&manifest).await.is_err());
    }
    #[tokio::test]
    async fn write_failure_never_advances_credit_and_releases_staging() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("result.bin");
        let (mut r, _, master, id) = session().await;
        r.command(Command::Queue {
            path: "/source".into(),
            destination: destination.clone(),
        })
        .await
        .unwrap();
        let (cipher, manifest, _) = open(&id, &master, 3, "file.bin");
        r.record(&manifest).await.unwrap();
        let read_only = dir.path().join("read-only.bin");
        std::fs::write(&read_only, []).unwrap();
        r.active.as_mut().unwrap().file = tokio::fs::File::open(&read_only).await.unwrap();
        assert!(r.record(&chunk(&id, &cipher, 0, b"abc")).await.is_err());
        assert_eq!(r.view.items[0].received, 0);
        r.fail("write failed");
        assert!(!destination.exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    #[tokio::test]
    async fn verified_receipt_precedes_the_next_queued_request() {
        let dir = tempfile::tempdir().unwrap();
        let (mut r, mut host, master, id) = session().await;
        let first = r
            .command(Command::Queue {
                path: "/first".into(),
                destination: dir.path().join("first"),
            })
            .await
            .unwrap()
            .remove(0);
        host.open(
            transfer_protocol::decode_session_envelope(&first)
                .unwrap()
                .body,
        )
        .unwrap();
        assert!(
            r.command(Command::Queue {
                path: "/second".into(),
                destination: dir.path().join("second")
            })
            .await
            .unwrap()
            .is_empty()
        );
        let (cipher, manifest, count) = open(&id, &master, 0, "first");
        r.record(&manifest).await.unwrap();
        let hash: [u8; 32] = Sha256::digest([]).into();
        let out = r
            .record(&close(&id, &cipher, 0, count, &hash))
            .await
            .unwrap();
        assert_eq!(out.len(), 2);
        let receipt = host
            .open(
                transfer_protocol::decode_session_envelope(&out[0])
                    .unwrap()
                    .body,
            )
            .unwrap();
        assert_eq!(receipt[0], transfer_protocol::CONTROL_TRANSFER_RECEIPT);
        let next = host
            .open(
                transfer_protocol::decode_session_envelope(&out[1])
                    .unwrap()
                    .body,
            )
            .unwrap();
        assert_eq!(next, b"\x01/second");
        assert_eq!(r.view.items[0].state, State::Saved);
        assert_eq!(r.view.items[1].state, State::Requested);
    }
    #[tokio::test]
    async fn stale_source_errors_do_not_invalidate_a_fresh_session() {
        let (mut r, _, _, id) = session().await;
        r.record(&envelope(
            transfer_protocol::SESSION_KIND_ERROR,
            &[9; 16],
            b"transfer failed",
        ))
        .await
        .unwrap();
        assert_eq!(r.view.session, Session::Ready);
        assert!(r.deadline.is_none());
        r.record(&envelope(
            transfer_protocol::SESSION_KIND_ERROR,
            &id,
            b"transfer failed",
        ))
        .await
        .unwrap();
        assert_eq!(r.view.session, Session::Failed);
    }
    #[tokio::test]
    async fn staging_corruption_is_detected_during_the_verified_save_copy() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("result.bin");
        let (mut r, _, master, id) = session().await;
        r.command(Command::Queue {
            path: "/source".into(),
            destination: destination.clone(),
        })
        .await
        .unwrap();
        let (cipher, manifest, count) = open(&id, &master, 3, "file.bin");
        r.record(&manifest).await.unwrap();
        r.record(&chunk(&id, &cipher, 0, b"abc")).await.unwrap();
        let active = r.active.as_mut().unwrap();
        active.file.seek(std::io::SeekFrom::Start(0)).await.unwrap();
        active.file.write_all(b"xyz").await.unwrap();
        active.file.flush().await.unwrap();
        let hash: [u8; 32] = Sha256::digest(b"abc").into();
        assert!(
            r.record(&close(&id, &cipher, 3, count, &hash))
                .await
                .is_err()
        );
        r.fail("staging changed");
        assert!(!destination.exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
    #[tokio::test]
    async fn restarted_agents_may_reuse_ids_without_changing_old_history_or_destinations() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old");
        let new = dir.path().join("new");
        let (mut first, _, master, id) = session().await;
        first
            .command(Command::Queue {
                path: "/old".into(),
                destination: old.clone(),
            })
            .await
            .unwrap();
        let (cipher, manifest, count) = open(&id, &master, 0, "old");
        first.record(&manifest).await.unwrap();
        let hash: [u8; 32] = Sha256::digest([]).into();
        first
            .record(&close(&id, &cipher, 0, count, &hash))
            .await
            .unwrap();
        let (mut next, _, master, id) = session().await;
        next.view.items = first.view.items;
        next.command(Command::Queue {
            path: "/new".into(),
            destination: new.clone(),
        })
        .await
        .unwrap();
        let (cipher, manifest, count) = open(&id, &master, 3, "new");
        next.record(&manifest).await.unwrap();
        next.record(&chunk(&id, &cipher, 0, b"new")).await.unwrap();
        assert_eq!(next.view.items[0].state, State::Saved);
        assert_eq!(next.view.items[0].received, 0);
        assert_eq!(next.view.items[1].received, 3);
        let hash: [u8; 32] = Sha256::digest(b"new").into();
        next.record(&close(&id, &cipher, 3, count, &hash))
            .await
            .unwrap();
        assert_eq!(std::fs::read(old).unwrap(), b"");
        assert_eq!(std::fs::read(new).unwrap(), b"new");
        assert_eq!(next.view.items[1].state, State::Saved);
    }
}

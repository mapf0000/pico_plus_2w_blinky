use anyhow::{Context, Result, bail};
use bytes::{BufMut, Bytes, BytesMut};
use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::UNIX_EPOCH;
use tokio::sync::mpsc;

use crate::tlv::{self, Frame};

pub const TAG_FS_LIST_REQUEST: u8 = 29;
pub const TAG_FS_LIST_PAGE: u8 = 30;
pub const TAG_FS_LIST_CANCEL: u8 = 31;

pub const PROTOCOL_VERSION: u16 = 1;
pub const FLAG_SHOW_HIDDEN: u8 = 1 << 0;
pub const PAGE_FLAG_HAS_MORE: u8 = 1 << 0;

pub const STATUS_OK: u8 = 0;
pub const STATUS_NOT_FOUND: u8 = 1;
pub const STATUS_PERMISSION_DENIED: u8 = 2;
pub const STATUS_NOT_DIRECTORY: u8 = 3;
pub const STATUS_INVALID_PATH: u8 = 4;
pub const STATUS_INTERNAL_ERROR: u8 = 5;

pub const ENTRY_FILE: u8 = 0;
pub const ENTRY_DIRECTORY: u8 = 1;
pub const ENTRY_SYMLINK_FILE: u8 = 2;
pub const ENTRY_SYMLINK_DIRECTORY: u8 = 3;
pub const ENTRY_OTHER: u8 = 4;

pub const ENTRY_FLAG_HIDDEN: u8 = 1 << 0;
pub const ENTRY_FLAG_READABLE: u8 = 1 << 1;

const DEFAULT_ENTRY_LIMIT: usize = 64;
const MAX_ENTRY_LIMIT: usize = 128;
const MAX_REQUEST_PATH_LEN: usize = 1024;
#[cfg(test)]
const REQUEST_FIXED_LEN: usize = 2 + 8 + 4 + 2 + 1 + 2;
const PAGE_FIXED_LEN: usize = 2 + 8 + 1 + 1 + 4 + 2 + 2 + 2;
const ENTRY_FIXED_LEN: usize = 1 + 1 + 8 + 8 + 2;
const UNKNOWN_MODIFIED_SECS: u64 = u64::MAX;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListRequest {
    pub request_id: u64,
    pub cursor: u32,
    pub entry_limit: u16,
    pub flags: u8,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListEntry {
    pub kind: u8,
    pub flags: u8,
    pub size: u64,
    pub modified_secs: u64,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListPage {
    pub request_id: u64,
    pub status: u8,
    pub flags: u8,
    pub next_cursor: u32,
    pub directory: String,
    pub error: String,
    pub entries: Vec<ListEntry>,
}

#[derive(Debug, Default)]
struct CancellationState {
    active: HashSet<u64>,
    canceled: HashSet<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct CancellationRegistry {
    state: Arc<Mutex<CancellationState>>,
}

impl CancellationRegistry {
    pub fn begin(&self, request_id: u64) {
        let mut state = self.lock();
        state.canceled.remove(&request_id);
        state.active.insert(request_id);
    }

    pub fn cancel(&self, request_id: u64) -> bool {
        let mut state = self.lock();
        if !state.active.contains(&request_id) {
            return false;
        }
        state.canceled.insert(request_id);
        true
    }

    fn finish(&self, request_id: u64) -> bool {
        let mut state = self.lock();
        state.active.remove(&request_id);
        state.canceled.remove(&request_id)
    }

    fn lock(&self) -> MutexGuard<'_, CancellationState> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

pub fn decode_list_request(payload: &[u8]) -> Result<ListRequest> {
    let mut cursor = payload;
    let version = take_u16(&mut cursor)?;
    if version != PROTOCOL_VERSION {
        bail!("unsupported filesystem protocol version {version}");
    }
    let request_id = take_u64(&mut cursor)?;
    let page_cursor = take_u32(&mut cursor)?;
    let entry_limit = take_u16(&mut cursor)?;
    let flags = take_u8(&mut cursor)?;
    let path_len = take_u16(&mut cursor)? as usize;
    if path_len > MAX_REQUEST_PATH_LEN {
        bail!("filesystem path exceeds {MAX_REQUEST_PATH_LEN} bytes");
    }
    let path = std::str::from_utf8(take_bytes(&mut cursor, path_len)?)
        .context("filesystem path is not UTF-8")?
        .to_string();
    if !cursor.is_empty() {
        bail!("trailing bytes in filesystem list request");
    }

    Ok(ListRequest {
        request_id,
        cursor: page_cursor,
        entry_limit,
        flags,
        path,
    })
}

pub fn decode_cancel_request(payload: &[u8]) -> Result<u64> {
    if payload.len() != 8 {
        bail!("filesystem cancel payload must be 8 bytes");
    }
    Ok(u64::from_le_bytes(
        payload.try_into().expect("length checked"),
    ))
}

pub fn encode_list_page(page: &ListPage) -> Result<Bytes> {
    let directory = page.directory.as_bytes();
    let error = page.error.as_bytes();
    let directory_len: u16 = directory
        .len()
        .try_into()
        .context("directory path is too long")?;
    let error_len: u16 = error
        .len()
        .try_into()
        .context("error message is too long")?;
    let entry_count: u16 = page
        .entries
        .len()
        .try_into()
        .context("too many filesystem entries")?;

    let mut encoded_len = PAGE_FIXED_LEN + directory.len() + error.len();
    for entry in &page.entries {
        encoded_len = encoded_len
            .checked_add(ENTRY_FIXED_LEN + entry.name.len())
            .context("filesystem page length overflow")?;
    }
    if encoded_len > tlv::MAX_PAYLOAD_LEN {
        bail!(
            "filesystem page length {encoded_len} exceeds {}",
            tlv::MAX_PAYLOAD_LEN
        );
    }

    let mut out = BytesMut::with_capacity(encoded_len);
    out.put_u16_le(PROTOCOL_VERSION);
    out.put_u64_le(page.request_id);
    out.put_u8(page.status);
    out.put_u8(page.flags);
    out.put_u32_le(page.next_cursor);
    out.put_u16_le(directory_len);
    out.extend_from_slice(directory);
    out.put_u16_le(error_len);
    out.extend_from_slice(error);
    out.put_u16_le(entry_count);
    for entry in &page.entries {
        let name = entry.name.as_bytes();
        let name_len: u16 = name.len().try_into().context("entry name is too long")?;
        out.put_u8(entry.kind);
        out.put_u8(entry.flags);
        out.put_u64_le(entry.size);
        out.put_u64_le(entry.modified_secs);
        out.put_u16_le(name_len);
        out.extend_from_slice(name);
    }
    Ok(out.freeze())
}

#[cfg(test)]
pub fn decode_list_page(payload: &[u8]) -> Result<ListPage> {
    let mut cursor = payload;
    let version = take_u16(&mut cursor)?;
    if version != PROTOCOL_VERSION {
        bail!("unsupported filesystem protocol version {version}");
    }
    let request_id = take_u64(&mut cursor)?;
    let status = take_u8(&mut cursor)?;
    let flags = take_u8(&mut cursor)?;
    let next_cursor = take_u32(&mut cursor)?;
    let directory_len = take_u16(&mut cursor)? as usize;
    let directory = std::str::from_utf8(take_bytes(&mut cursor, directory_len)?)?.to_string();
    let error_len = take_u16(&mut cursor)? as usize;
    let error = std::str::from_utf8(take_bytes(&mut cursor, error_len)?)?.to_string();
    let entry_count = take_u16(&mut cursor)? as usize;
    let mut entries = Vec::with_capacity(entry_count);
    for _ in 0..entry_count {
        let kind = take_u8(&mut cursor)?;
        let flags = take_u8(&mut cursor)?;
        let size = take_u64(&mut cursor)?;
        let modified_secs = take_u64(&mut cursor)?;
        let name_len = take_u16(&mut cursor)? as usize;
        let name = std::str::from_utf8(take_bytes(&mut cursor, name_len)?)?.to_string();
        entries.push(ListEntry {
            kind,
            flags,
            size,
            modified_secs,
            name,
        });
    }
    if !cursor.is_empty() {
        bail!("trailing bytes in filesystem list page");
    }
    Ok(ListPage {
        request_id,
        status,
        flags,
        next_cursor,
        directory,
        error,
        entries,
    })
}

pub async fn send_list_page(
    outbound: mpsc::Sender<Frame>,
    request: ListRequest,
    cancellations: CancellationRegistry,
) {
    let request_id = request.request_id;
    let result = tokio::task::spawn_blocking(move || build_list_page(&request)).await;
    if cancellations.finish(request_id) {
        return;
    }
    let page = match result {
        Ok(Ok(page)) => page,
        Ok(Err(err)) => error_page(request_id, STATUS_INTERNAL_ERROR, err.to_string()),
        Err(err) => error_page(
            request_id,
            STATUS_INTERNAL_ERROR,
            format!("filesystem task failed: {err}"),
        ),
    };

    match encode_list_page(&page) {
        Ok(payload) => {
            let _ = outbound.send(Frame::new(TAG_FS_LIST_PAGE, payload)).await;
        }
        Err(err) => {
            let fallback = error_page(request_id, STATUS_INTERNAL_ERROR, err.to_string());
            if let Ok(payload) = encode_list_page(&fallback) {
                let _ = outbound.send(Frame::new(TAG_FS_LIST_PAGE, payload)).await;
            }
        }
    }
}

fn build_list_page(request: &ListRequest) -> Result<ListPage> {
    let requested_path = resolve_requested_path(&request.path)?;
    let canonical = match std::fs::canonicalize(&requested_path) {
        Ok(path) => path,
        Err(err) => {
            return Ok(error_page(
                request.request_id,
                status_for_io_error(&err),
                err.to_string(),
            ));
        }
    };
    let directory = match canonical.to_str() {
        Some(path) => path.to_string(),
        None => {
            return Ok(error_page(
                request.request_id,
                STATUS_INVALID_PATH,
                "directory path is not valid UTF-8",
            ));
        }
    };

    if !canonical.is_dir() {
        return Ok(ListPage {
            request_id: request.request_id,
            status: STATUS_NOT_DIRECTORY,
            flags: 0,
            next_cursor: request.cursor,
            directory,
            error: "path is not a directory".to_string(),
            entries: Vec::new(),
        });
    }

    let show_hidden = request.flags & FLAG_SHOW_HIDDEN != 0;
    let mut entries = Vec::new();
    let read_dir = match std::fs::read_dir(&canonical) {
        Ok(read_dir) => read_dir,
        Err(err) => {
            return Ok(ListPage {
                request_id: request.request_id,
                status: status_for_io_error(&err),
                flags: 0,
                next_cursor: request.cursor,
                directory,
                error: err.to_string(),
                entries: Vec::new(),
            });
        }
    };

    for item in read_dir {
        let Ok(item) = item else {
            continue;
        };
        let Some(name) = item.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let hidden = name.starts_with('.');
        if hidden && !show_hidden {
            continue;
        }
        entries.push(build_entry(item.path(), name, hidden));
    }

    entries.sort_by(compare_entries);

    let start = usize::try_from(request.cursor)
        .unwrap_or(usize::MAX)
        .min(entries.len());
    let requested_limit = if request.entry_limit == 0 {
        DEFAULT_ENTRY_LIMIT
    } else {
        usize::from(request.entry_limit).min(MAX_ENTRY_LIMIT)
    };
    let mut selected = Vec::new();
    let base_len = PAGE_FIXED_LEN + directory.len();

    for entry in entries.iter().skip(start).take(requested_limit) {
        let candidate_len = ENTRY_FIXED_LEN + entry.name.len();
        let used_len = base_len
            + selected
                .iter()
                .map(|existing: &ListEntry| ENTRY_FIXED_LEN + existing.name.len())
                .sum::<usize>();
        if used_len + candidate_len > tlv::MAX_PAYLOAD_LEN {
            break;
        }
        selected.push(entry.clone());
    }

    let next = start.saturating_add(selected.len());
    let has_more = next < entries.len();
    let next_cursor = u32::try_from(next).unwrap_or(u32::MAX);

    Ok(ListPage {
        request_id: request.request_id,
        status: STATUS_OK,
        flags: if has_more { PAGE_FLAG_HAS_MORE } else { 0 },
        next_cursor,
        directory,
        error: String::new(),
        entries: selected,
    })
}

fn resolve_requested_path(path: &str) -> Result<PathBuf> {
    if !path.is_empty() {
        return Ok(PathBuf::from(path));
    }
    if let Some(home) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(home));
    }
    std::env::current_dir().context("resolve current directory")
}

fn build_entry(path: PathBuf, name: String, hidden: bool) -> ListEntry {
    let symlink_metadata = std::fs::symlink_metadata(&path);
    let (kind, metadata) = match symlink_metadata {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let target = std::fs::metadata(&path).ok();
            let kind = match target.as_ref() {
                Some(target) if target.is_dir() => ENTRY_SYMLINK_DIRECTORY,
                Some(target) if target.is_file() => ENTRY_SYMLINK_FILE,
                _ => ENTRY_OTHER,
            };
            (kind, target)
        }
        Ok(metadata) if metadata.is_dir() => (ENTRY_DIRECTORY, Some(metadata)),
        Ok(metadata) if metadata.is_file() => (ENTRY_FILE, Some(metadata)),
        Ok(metadata) => (ENTRY_OTHER, Some(metadata)),
        Err(_) => (ENTRY_OTHER, None),
    };

    let mut flags = if hidden { ENTRY_FLAG_HIDDEN } else { 0 };
    if metadata.is_some() {
        flags |= ENTRY_FLAG_READABLE;
    }
    let size = metadata
        .as_ref()
        .filter(|value| value.is_file())
        .map_or(0, std::fs::Metadata::len);
    let modified_secs = metadata
        .and_then(|value| value.modified().ok())
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map_or(UNKNOWN_MODIFIED_SECS, |value| value.as_secs());

    ListEntry {
        kind,
        flags,
        size,
        modified_secs,
        name,
    }
}

fn compare_entries(left: &ListEntry, right: &ListEntry) -> Ordering {
    entry_group(left.kind)
        .cmp(&entry_group(right.kind))
        .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
        .then_with(|| left.name.cmp(&right.name))
}

fn entry_group(kind: u8) -> u8 {
    match kind {
        ENTRY_DIRECTORY | ENTRY_SYMLINK_DIRECTORY => 0,
        ENTRY_FILE | ENTRY_SYMLINK_FILE => 1,
        _ => 2,
    }
}

fn status_for_io_error(error: &std::io::Error) -> u8 {
    match error.kind() {
        std::io::ErrorKind::NotFound => STATUS_NOT_FOUND,
        std::io::ErrorKind::PermissionDenied => STATUS_PERMISSION_DENIED,
        _ => STATUS_INTERNAL_ERROR,
    }
}

fn error_page(request_id: u64, status: u8, error: impl Into<String>) -> ListPage {
    let mut error = error.into();
    truncate_utf8(&mut error, 512);
    ListPage {
        request_id,
        status,
        flags: 0,
        next_cursor: 0,
        directory: String::new(),
        error,
        entries: Vec::new(),
    }
}

fn truncate_utf8(value: &mut String, max_bytes: usize) {
    if value.len() <= max_bytes {
        return;
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
}

fn take_bytes<'a>(cursor: &mut &'a [u8], len: usize) -> Result<&'a [u8]> {
    if cursor.len() < len {
        bail!("unexpected end of filesystem payload");
    }
    let (head, tail) = cursor.split_at(len);
    *cursor = tail;
    Ok(head)
}

fn take_u8(cursor: &mut &[u8]) -> Result<u8> {
    Ok(take_bytes(cursor, 1)?[0])
}

fn take_u16(cursor: &mut &[u8]) -> Result<u16> {
    Ok(u16::from_le_bytes(take_bytes(cursor, 2)?.try_into()?))
}

fn take_u32(cursor: &mut &[u8]) -> Result<u32> {
    Ok(u32::from_le_bytes(take_bytes(cursor, 4)?.try_into()?))
}

fn take_u64(cursor: &mut &[u8]) -> Result<u64> {
    Ok(u64::from_le_bytes(take_bytes(cursor, 8)?.try_into()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn encode_request(request: &ListRequest) -> Bytes {
        let mut out = BytesMut::with_capacity(REQUEST_FIXED_LEN + request.path.len());
        out.put_u16_le(PROTOCOL_VERSION);
        out.put_u64_le(request.request_id);
        out.put_u32_le(request.cursor);
        out.put_u16_le(request.entry_limit);
        out.put_u8(request.flags);
        out.put_u16_le(request.path.len() as u16);
        out.extend_from_slice(request.path.as_bytes());
        out.freeze()
    }

    #[test]
    fn request_roundtrip() {
        let request = ListRequest {
            request_id: 42,
            cursor: 7,
            entry_limit: 25,
            flags: FLAG_SHOW_HIDDEN,
            path: "/Users/alice".to_string(),
        };
        assert_eq!(
            decode_list_request(&encode_request(&request)).unwrap(),
            request
        );
    }

    #[test]
    fn page_roundtrip() {
        let page = ListPage {
            request_id: 9,
            status: STATUS_OK,
            flags: PAGE_FLAG_HAS_MORE,
            next_cursor: 2,
            directory: "/tmp".to_string(),
            error: String::new(),
            entries: vec![
                ListEntry {
                    kind: ENTRY_DIRECTORY,
                    flags: ENTRY_FLAG_READABLE,
                    size: 0,
                    modified_secs: 123,
                    name: "folder".to_string(),
                },
                ListEntry {
                    kind: ENTRY_FILE,
                    flags: ENTRY_FLAG_READABLE,
                    size: 99,
                    modified_secs: 456,
                    name: "file.bin".to_string(),
                },
            ],
        };
        let encoded = encode_list_page(&page).unwrap();
        assert_eq!(decode_list_page(&encoded).unwrap(), page);
    }

    #[test]
    fn listing_sorts_directories_and_filters_hidden() {
        let root = temp_directory("sort");
        fs::create_dir(root.join("z-folder")).unwrap();
        fs::write(root.join("a-file"), b"a").unwrap();
        fs::write(root.join(".hidden"), b"h").unwrap();

        let request = ListRequest {
            request_id: 1,
            cursor: 0,
            entry_limit: 64,
            flags: 0,
            path: root.to_string_lossy().into_owned(),
        };
        let page = build_list_page(&request).unwrap();
        assert_eq!(page.status, STATUS_OK);
        assert_eq!(
            page.entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["z-folder", "a-file"]
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn listing_paginates() {
        let root = temp_directory("page");
        for name in ["a", "b", "c"] {
            fs::write(root.join(name), name.as_bytes()).unwrap();
        }

        let first = build_list_page(&ListRequest {
            request_id: 2,
            cursor: 0,
            entry_limit: 2,
            flags: 0,
            path: root.to_string_lossy().into_owned(),
        })
        .unwrap();
        assert_eq!(first.entries.len(), 2);
        assert_ne!(first.flags & PAGE_FLAG_HAS_MORE, 0);
        assert_eq!(first.next_cursor, 2);

        let second = build_list_page(&ListRequest {
            request_id: 3,
            cursor: first.next_cursor,
            entry_limit: 2,
            flags: 0,
            path: root.to_string_lossy().into_owned(),
        })
        .unwrap();
        assert_eq!(second.entries.len(), 1);
        assert_eq!(second.flags & PAGE_FLAG_HAS_MORE, 0);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn file_path_reports_not_directory() {
        let root = temp_directory("file");
        let file = root.join("data");
        fs::write(&file, b"x").unwrap();
        let page = build_list_page(&ListRequest {
            request_id: 4,
            cursor: 0,
            entry_limit: 64,
            flags: 0,
            path: file.to_string_lossy().into_owned(),
        })
        .unwrap();
        assert_eq!(page.status, STATUS_NOT_DIRECTORY);
        fs::remove_dir_all(root).unwrap();
    }

    fn temp_directory(label: &str) -> PathBuf {
        let unique = format!(
            "pico-fs-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = std::env::temp_dir().join(unique);
        fs::create_dir_all(&path).unwrap();
        path
    }
}

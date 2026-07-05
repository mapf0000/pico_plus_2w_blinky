use std::collections::HashSet;

pub const PROTOCOL_VERSION: u16 = 1;
pub const STATUS_OK: u8 = 0;
pub const PAGE_FLAG_HAS_MORE: u8 = 1 << 0;
pub const ENTRY_FLAG_HIDDEN: u8 = 1 << 0;
pub const ENTRY_FLAG_READABLE: u8 = 1 << 1;
pub const UNKNOWN_MODIFIED_SECS: u64 = u64::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Directory,
    SymlinkFile,
    SymlinkDirectory,
    Other,
}

impl EntryKind {
    pub fn is_directory(self) -> bool {
        matches!(self, Self::Directory | Self::SymlinkDirectory)
    }

    pub fn is_file(self) -> bool {
        matches!(self, Self::File | Self::SymlinkFile)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryEntry {
    pub kind: EntryKind,
    pub hidden: bool,
    pub readable: bool,
    pub size: u64,
    pub modified_secs: Option<u64>,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryPage {
    pub request_id: u64,
    pub status: u8,
    pub has_more: bool,
    pub next_cursor: u32,
    pub directory: String,
    pub error: String,
    pub entries: Vec<DirectoryEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BrowserView {
    pub directory: String,
    pub entries: Vec<DirectoryEntry>,
    pub next_cursor: u32,
    pub has_more: bool,
    pub loading: bool,
    pub error: Option<String>,
    pub show_hidden: bool,
}

#[derive(Debug, Clone)]
struct PendingRequest {
    request_id: u64,
    append: bool,
}

#[derive(Debug, Default)]
pub struct BrowserStore {
    view: BrowserView,
    pending: Option<PendingRequest>,
}

impl BrowserStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn begin_request(
        &mut self,
        request_id: u64,
        append: bool,
        show_hidden: bool,
    ) -> Option<u64> {
        let previous = self.pending.replace(PendingRequest { request_id, append });
        self.view.loading = true;
        self.view.error = None;
        self.view.show_hidden = show_hidden;
        if !append {
            self.view.has_more = false;
            self.view.next_cursor = 0;
        }
        previous.map(|pending| pending.request_id)
    }

    pub fn fail_request(&mut self, request_id: u64, error: String) {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.request_id == request_id)
        {
            self.pending = None;
            self.view.loading = false;
            self.view.error = Some(error);
        }
    }

    pub fn apply_page(&mut self, page: DirectoryPage) -> bool {
        let Some(pending) = self.pending.take() else {
            return false;
        };
        if pending.request_id != page.request_id {
            self.pending = Some(pending);
            return false;
        }

        self.view.loading = false;
        if page.status != STATUS_OK {
            self.view.error = Some(if page.error.is_empty() {
                format!("filesystem request failed with status {}", page.status)
            } else {
                page.error
            });
            return true;
        }

        self.view.error = None;
        if pending.append && self.view.directory == page.directory {
            let mut names = self
                .view
                .entries
                .iter()
                .map(|entry| entry.name.clone())
                .collect::<HashSet<_>>();
            self.view.entries.extend(
                page.entries
                    .into_iter()
                    .filter(|entry| names.insert(entry.name.clone())),
            );
        } else {
            self.view.directory = page.directory;
            self.view.entries = page.entries;
        }
        self.view.next_cursor = page.next_cursor;
        self.view.has_more = page.has_more;
        true
    }

    pub fn snapshot(&self) -> BrowserView {
        self.view.clone()
    }
}

pub fn decode_list_page(payload: &[u8]) -> Result<DirectoryPage, String> {
    let mut cursor = payload;
    let version = take_u16(&mut cursor)?;
    if version != PROTOCOL_VERSION {
        return Err(format!("unsupported filesystem protocol version {version}"));
    }
    let request_id = take_u64(&mut cursor)?;
    let status = take_u8(&mut cursor)?;
    let flags = take_u8(&mut cursor)?;
    let next_cursor = take_u32(&mut cursor)?;
    let directory_len = take_u16(&mut cursor)? as usize;
    let directory = take_utf8(&mut cursor, directory_len, "directory")?;
    let error_len = take_u16(&mut cursor)? as usize;
    let error = take_utf8(&mut cursor, error_len, "error")?;
    let entry_count = take_u16(&mut cursor)? as usize;
    let mut entries = Vec::with_capacity(entry_count);

    for _ in 0..entry_count {
        let kind = match take_u8(&mut cursor)? {
            0 => EntryKind::File,
            1 => EntryKind::Directory,
            2 => EntryKind::SymlinkFile,
            3 => EntryKind::SymlinkDirectory,
            _ => EntryKind::Other,
        };
        let entry_flags = take_u8(&mut cursor)?;
        let size = take_u64(&mut cursor)?;
        let modified = take_u64(&mut cursor)?;
        let name_len = take_u16(&mut cursor)? as usize;
        let name = take_utf8(&mut cursor, name_len, "entry name")?;
        entries.push(DirectoryEntry {
            kind,
            hidden: entry_flags & ENTRY_FLAG_HIDDEN != 0,
            readable: entry_flags & ENTRY_FLAG_READABLE != 0,
            size,
            modified_secs: (modified != UNKNOWN_MODIFIED_SECS).then_some(modified),
            name,
        });
    }

    if !cursor.is_empty() {
        return Err("trailing bytes in filesystem page".to_string());
    }

    Ok(DirectoryPage {
        request_id,
        status,
        has_more: flags & PAGE_FLAG_HAS_MORE != 0,
        next_cursor,
        directory,
        error,
        entries,
    })
}

pub fn join_path(directory: &str, name: &str) -> String {
    if directory == "/" {
        format!("/{name}")
    } else if directory.ends_with('/') {
        format!("{directory}{name}")
    } else {
        format!("{directory}/{name}")
    }
}

pub fn parent_path(path: &str) -> String {
    if path.is_empty() || path == "/" {
        return "/".to_string();
    }
    let trimmed = path.trim_end_matches('/');
    match trimmed.rsplit_once('/') {
        Some(("", _)) | None => "/".to_string(),
        Some((parent, _)) => parent.to_string(),
    }
}

pub fn breadcrumbs(path: &str) -> Vec<(String, String)> {
    if path.is_empty() {
        return Vec::new();
    }
    let mut result = vec![("/".to_string(), "/".to_string())];
    let mut current = String::new();
    for part in path.split('/').filter(|part| !part.is_empty()) {
        current.push('/');
        current.push_str(part);
        result.push((part.to_string(), current.clone()));
    }
    result
}

fn take_bytes<'a>(cursor: &mut &'a [u8], len: usize) -> Result<&'a [u8], String> {
    if cursor.len() < len {
        return Err("unexpected end of filesystem page".to_string());
    }
    let (head, tail) = cursor.split_at(len);
    *cursor = tail;
    Ok(head)
}

fn take_utf8(cursor: &mut &[u8], len: usize, field: &str) -> Result<String, String> {
    std::str::from_utf8(take_bytes(cursor, len)?)
        .map(str::to_string)
        .map_err(|_| format!("{field} is not UTF-8"))
}

fn take_u8(cursor: &mut &[u8]) -> Result<u8, String> {
    Ok(take_bytes(cursor, 1)?[0])
}

fn take_u16(cursor: &mut &[u8]) -> Result<u16, String> {
    Ok(u16::from_le_bytes(
        take_bytes(cursor, 2)?.try_into().expect("length checked"),
    ))
}

fn take_u32(cursor: &mut &[u8]) -> Result<u32, String> {
    Ok(u32::from_le_bytes(
        take_bytes(cursor, 4)?.try_into().expect("length checked"),
    ))
}

fn take_u64(cursor: &mut &[u8]) -> Result<u64, String> {
    Ok(u64::from_le_bytes(
        take_bytes(cursor, 8)?.try_into().expect("length checked"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_page(request_id: u64) -> DirectoryPage {
        DirectoryPage {
            request_id,
            status: STATUS_OK,
            has_more: true,
            next_cursor: 2,
            directory: "/Users/alice".to_string(),
            error: String::new(),
            entries: vec![
                DirectoryEntry {
                    kind: EntryKind::Directory,
                    hidden: false,
                    readable: true,
                    size: 0,
                    modified_secs: Some(100),
                    name: "Documents".to_string(),
                },
                DirectoryEntry {
                    kind: EntryKind::File,
                    hidden: false,
                    readable: true,
                    size: 42,
                    modified_secs: None,
                    name: "file.bin".to_string(),
                },
            ],
        }
    }

    fn encode_page(page: &DirectoryPage) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&PROTOCOL_VERSION.to_le_bytes());
        out.extend_from_slice(&page.request_id.to_le_bytes());
        out.push(page.status);
        out.push(if page.has_more { PAGE_FLAG_HAS_MORE } else { 0 });
        out.extend_from_slice(&page.next_cursor.to_le_bytes());
        out.extend_from_slice(&(page.directory.len() as u16).to_le_bytes());
        out.extend_from_slice(page.directory.as_bytes());
        out.extend_from_slice(&(page.error.len() as u16).to_le_bytes());
        out.extend_from_slice(page.error.as_bytes());
        out.extend_from_slice(&(page.entries.len() as u16).to_le_bytes());
        for entry in &page.entries {
            out.push(match entry.kind {
                EntryKind::File => 0,
                EntryKind::Directory => 1,
                EntryKind::SymlinkFile => 2,
                EntryKind::SymlinkDirectory => 3,
                EntryKind::Other => 4,
            });
            let flags = (if entry.hidden { ENTRY_FLAG_HIDDEN } else { 0 })
                | (if entry.readable {
                    ENTRY_FLAG_READABLE
                } else {
                    0
                });
            out.push(flags);
            out.extend_from_slice(&entry.size.to_le_bytes());
            out.extend_from_slice(
                &entry
                    .modified_secs
                    .unwrap_or(UNKNOWN_MODIFIED_SECS)
                    .to_le_bytes(),
            );
            out.extend_from_slice(&(entry.name.len() as u16).to_le_bytes());
            out.extend_from_slice(entry.name.as_bytes());
        }
        out
    }

    #[test]
    fn page_roundtrip() {
        let page = sample_page(7);
        assert_eq!(decode_list_page(&encode_page(&page)).unwrap(), page);
    }

    #[test]
    fn stale_pages_are_ignored() {
        let mut store = BrowserStore::new();
        store.begin_request(2, false, false);
        assert!(!store.apply_page(sample_page(1)));
        assert!(store.snapshot().loading);
        assert!(store.apply_page(sample_page(2)));
        assert_eq!(store.snapshot().directory, "/Users/alice");
    }

    #[test]
    fn continuation_pages_append_without_duplicates() {
        let mut store = BrowserStore::new();
        store.begin_request(1, false, false);
        store.apply_page(sample_page(1));

        let mut continuation = sample_page(2);
        continuation.has_more = false;
        continuation.next_cursor = 3;
        continuation.entries = vec![
            continuation.entries[1].clone(),
            DirectoryEntry {
                kind: EntryKind::File,
                hidden: false,
                readable: true,
                size: 1,
                modified_secs: None,
                name: "other".to_string(),
            },
        ];
        store.begin_request(2, true, false);
        assert!(store.apply_page(continuation));
        assert_eq!(store.snapshot().entries.len(), 3);
    }

    #[test]
    fn path_helpers_handle_root() {
        assert_eq!(join_path("/", "tmp"), "/tmp");
        assert_eq!(join_path("/Users", "alice"), "/Users/alice");
        assert_eq!(parent_path("/Users/alice"), "/Users");
        assert_eq!(parent_path("/"), "/");
        assert_eq!(
            breadcrumbs("/Users/alice"),
            vec![
                ("/".to_string(), "/".to_string()),
                ("Users".to_string(), "/Users".to_string()),
                ("alice".to_string(), "/Users/alice".to_string())
            ]
        );
    }
}

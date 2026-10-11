//! Bounded native browser state and target-platform path navigation.
use crate::Failure;
use ble_protocol::filesystem::{PATH_MAX, Page};
pub const MAX_ENTRIES: usize = 512;
#[derive(Clone, Debug)]
pub struct Entry {
    pub kind: u8,
    pub flags: u8,
    pub size: u64,
    pub modified: u64,
    pub name: String,
}
impl Entry {
    pub fn directory(&self) -> bool {
        matches!(self.kind, 1 | 3)
    }
    pub fn kind_label(&self) -> &'static str {
        match self.kind {
            0 => "File",
            1 => "Folder",
            2 => "File link",
            3 => "Folder link",
            _ => "Other",
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct Browser {
    pub directory: String,
    pub entries: Vec<Entry>,
    pub cursor: u32,
    pub more: bool,
    pub hidden: bool,
    pub loading: bool,
    pub error: Option<String>,
    pub selected: Option<String>,
}
impl Browser {
    pub fn apply(
        &mut self,
        bytes: &[u8],
        id: u64,
        cursor: u32,
        path: &str,
        hidden: bool,
    ) -> Result<(), Failure> {
        let page = Page::decode(bytes).map_err(|_| Failure::InvalidData)?;
        if page.id != id {
            return Err(Failure::InvalidData);
        }
        self.loading = false;
        if page.status != 0 {
            self.error = Some(if page.error.is_empty() {
                match page.status {
                    1 => "Path not found",
                    2 => "Permission denied",
                    3 => "Path is not a directory",
                    4 => "Invalid path",
                    _ => "Host filesystem error",
                }
                .into()
            } else {
                page.error.into()
            });
            return Ok(());
        }
        if page.next_cursor
            != cursor
                .checked_add(page.count as u32)
                .ok_or(Failure::InvalidData)?
            || (cursor != 0
                && (path != self.directory
                    || hidden != self.hidden
                    || page.directory != self.directory
                    || cursor != self.cursor))
        {
            return Err(Failure::InvalidData);
        }
        let existing = if cursor == 0 { 0 } else { self.entries.len() };
        if existing + page.count > MAX_ENTRIES {
            self.error =
                Some("Directory display limit reached (512 entries); browse a subfolder".into());
            self.more = false;
            return Ok(());
        }
        let mut entries = Vec::with_capacity(page.count);
        for e in page.entries() {
            if (cursor != 0 && self.entries.iter().any(|old| old.name == e.name))
                || entries.iter().any(|old: &Entry| old.name == e.name)
            {
                return Err(Failure::InvalidData);
            }
            entries.push(Entry {
                kind: e.kind,
                flags: e.flags,
                size: e.size,
                modified: e.modified,
                name: e.name.into(),
            });
        }
        if cursor == 0 {
            self.entries.clear();
            self.selected = None;
        }
        self.entries.extend(entries);
        self.directory = page.directory.into();
        self.hidden = hidden;
        self.cursor = page.next_cursor;
        self.more = page.more;
        self.error = None;
        Ok(())
    }
}
pub fn valid_path(path: &str) -> bool {
    path.len() <= PATH_MAX && !path.contains('\0')
}
fn windows(path: &str) -> bool {
    !path.starts_with('/') && (path.as_bytes().get(1) == Some(&b':') || path.starts_with("\\\\"))
}
pub fn root(path: &str) -> String {
    if path.as_bytes().get(1) == Some(&b':') {
        format!("{}\\", &path[..2])
    } else if let Some(rest) = path.strip_prefix("\\\\?\\UNC\\") {
        let mut parts = rest.split('\\').filter(|v| !v.is_empty());
        format!(
            "\\\\?\\UNC\\{}\\{}\\",
            parts.next().unwrap_or(""),
            parts.next().unwrap_or("")
        )
    } else if let Some(rest) = path.strip_prefix("\\\\") {
        let mut parts = rest.split('\\').filter(|v| !v.is_empty());
        format!(
            "\\\\{}\\{}\\",
            parts.next().unwrap_or(""),
            parts.next().unwrap_or("")
        )
    } else {
        "/".into()
    }
}
pub fn parent(path: &str) -> String {
    let r = root(path);
    let sep = if windows(path) { '\\' } else { '/' };
    let trimmed = path.trim_end_matches(sep);
    if trimmed.len() <= r.trim_end_matches(sep).len() {
        return r;
    }
    match trimmed.rfind(sep) {
        Some(i) if i < r.len() => r,
        Some(i) => trimmed[..i].into(),
        None => r,
    }
}
pub fn join(path: &str, name: &str) -> Option<String> {
    let sep = if windows(path) { '\\' } else { '/' };
    if name.is_empty()
        || matches!(name, "." | "..")
        || name.contains(['/', '\0'])
        || (sep == '\\' && name.contains('\\'))
    {
        return None;
    }
    let value = format!("{}{sep}{name}", path.trim_end_matches(sep));
    valid_path(&value).then_some(value)
}
pub fn breadcrumbs(path: &str) -> Vec<(String, String)> {
    let r = root(path);
    let sep = if windows(path) { '\\' } else { '/' };
    let mut result = vec![(r.clone(), r.clone())];
    let rest = path.get(r.len()..).unwrap_or("");
    let mut current = r;
    for part in rest.split(sep).filter(|v| !v.is_empty()) {
        if let Some(next) = join(&current, part) {
            current = next;
            result.push((part.into(), current.clone()));
        }
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths_follow_target_not_control_pc() {
        assert_eq!(parent("/a/b/"), "/a");
        assert_eq!(parent("/a"), "/");
        assert_eq!(parent("C:\\a"), "C:\\");
        assert_eq!(root("C:\\a"), "C:\\");
        assert_eq!(join("C:\\a", "b").unwrap(), "C:\\a\\b");
        assert_eq!(parent("\\\\server\\share\\a"), "\\\\server\\share\\");
        assert!(join("/a", "../b").is_none());
        assert_eq!(breadcrumbs("/a/b").len(), 3);
    }
}

#[cfg(test)]
mod state_tests {
    use super::*;
    use crate::mock::mock_directory;
    #[test]
    fn pagination_errors_duplicates_and_capacity_do_not_mix_directories() {
        let mut b = Browser::default();
        let page = mock_directory(1, "/a", 0, false);
        b.apply(&page, 1, 0, "", false).unwrap();
        assert_eq!(b.entries.len(), 2);
        b.more = true;
        b.cursor = 1;
        let duplicate = mock_directory(2, "/a", 1, false);
        assert_eq!(
            b.apply(&duplicate, 2, 1, "/a", false),
            Err(Failure::InvalidData)
        );
        assert_eq!(b.entries.len(), 2);
        assert_eq!(b.apply(&page, 9, 0, "", false), Err(Failure::InvalidData));
        b.entries = (0..512)
            .map(|i| Entry {
                kind: 0,
                flags: 2,
                size: 0,
                modified: u64::MAX,
                name: format!("old-{i}"),
            })
            .collect();
        b.cursor = 1;
        b.more = true;
        b.apply(&duplicate, 2, 1, "/a", false).unwrap();
        assert_eq!(b.entries.len(), 512);
        assert!(!b.more);
        assert!(b.error.is_some());
        let mut error = Vec::new();
        error.extend_from_slice(&1u16.to_le_bytes());
        error.extend_from_slice(&3u64.to_le_bytes());
        error.extend_from_slice(&[2, 0]);
        error.extend_from_slice(&0u32.to_le_bytes());
        error.extend_from_slice(&[0; 6]);
        b.apply(&error, 3, 0, "", false).unwrap();
        assert_eq!(b.error.as_deref(), Some("Permission denied"));
        assert_eq!(b.directory, "/a");
    }
    #[test]
    fn target_paths_include_extended_windows_roots_and_unix_backslashes() {
        assert_eq!(root(r"\\?\UNC\server\share\a"), r"\\?\UNC\server\share\");
        assert_eq!(parent(r"\\?\C:\a"), r"\\?\C:\");
        assert_eq!(join("/tmp", r"a\b").unwrap(), r"/tmp/a\b");
        assert_eq!(parent(r"/tmp/a\b"), "/tmp");
        assert_eq!(breadcrumbs(r"/tmp/a\b").last().unwrap().1, r"/tmp/a\b");
        assert!(join("/tmp", &"a".repeat(513)).is_none());
    }
}

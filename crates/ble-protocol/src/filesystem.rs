//! Borrowed, strict filesystem-v1 codec. USB payloads remain unchanged.
use crate::Error;
pub const VERSION: u16 = 1;
pub const PAGE_MAX: usize = 2048;
pub const PATH_MAX: usize = 512; // firmware command channel capacity
pub const ENTRY_MAX: usize = 128;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request<'a> {
    pub id: u64,
    pub cursor: u32,
    pub hidden: bool,
    pub path: &'a str,
}
impl Request<'_> {
    pub fn encode(self, out: &mut [u8]) -> Result<usize, Error> {
        if self.id == 0 || self.path.len() > PATH_MAX || self.path.contains('\0') {
            return Err(Error::Invalid);
        }
        let n = 19 + self.path.len();
        if out.len() < n {
            return Err(Error::Length);
        }
        out[..2].copy_from_slice(&VERSION.to_le_bytes());
        out[2..10].copy_from_slice(&self.id.to_le_bytes());
        out[10..14].copy_from_slice(&self.cursor.to_le_bytes());
        out[14..16].copy_from_slice(&64u16.to_le_bytes());
        out[16] = u8::from(self.hidden);
        out[17..19].copy_from_slice(&(self.path.len() as u16).to_le_bytes());
        out[19..n].copy_from_slice(self.path.as_bytes());
        Ok(n)
    }
    pub fn decode(bytes: &[u8]) -> Result<Request<'_>, Error> {
        let mut c = Cursor(bytes);
        if c.u16()? != VERSION {
            return Err(Error::Version);
        }
        let id = c.u64()?;
        let cursor = c.u32()?;
        let limit = c.u16()?;
        let flags = c.u8()?;
        let path = c.string(PATH_MAX)?;
        if id == 0 || limit != 64 || flags > 1 || !c.0.is_empty() {
            return Err(Error::Invalid);
        }
        Ok(Request {
            id,
            cursor,
            hidden: flags == 1,
            path,
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry<'a> {
    pub kind: u8,
    pub flags: u8,
    pub size: u64,
    pub modified: u64,
    pub name: &'a str,
}
#[derive(Clone, Copy, Debug)]
pub struct Page<'a> {
    pub id: u64,
    pub status: u8,
    pub more: bool,
    pub next_cursor: u32,
    pub directory: &'a str,
    pub error: &'a str,
    pub count: usize,
    entries: &'a [u8],
}
impl<'a> Page<'a> {
    pub fn decode(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() > PAGE_MAX {
            return Err(Error::Length);
        }
        let mut c = Cursor(bytes);
        if c.u16()? != VERSION {
            return Err(Error::Version);
        }
        let id = c.u64()?;
        let status = c.u8()?;
        let flags = c.u8()?;
        let next_cursor = c.u32()?;
        let directory = c.string(PATH_MAX)?;
        let error = c.string(512)?;
        let count = usize::from(c.u16()?);
        if id == 0
            || status > 5
            || flags > 1
            || count > ENTRY_MAX
            || (status != 0 && (count != 0 || flags != 0))
            || (status == 0 && (directory.is_empty() || !error.is_empty()))
            || (flags != 0 && count == 0)
        {
            return Err(Error::Invalid);
        }
        let entries = c.0;
        for _ in 0..count {
            parse_entry(&mut c, !directory.starts_with('/'))?;
        }
        if !c.0.is_empty() {
            return Err(Error::Invalid);
        }
        Ok(Self {
            id,
            status,
            more: flags == 1,
            next_cursor,
            directory,
            error,
            count,
            entries,
        })
    }
    pub fn entries(self) -> impl Iterator<Item = Entry<'a>> {
        let mut c = Cursor(self.entries);
        (0..self.count)
            .filter_map(move |_| parse_entry(&mut c, !self.directory.starts_with('/')).ok())
    }
}
fn parse_entry<'a>(c: &mut Cursor<'a>, windows: bool) -> Result<Entry<'a>, Error> {
    let kind = c.u8()?;
    let flags = c.u8()?;
    let size = c.u64()?;
    let modified = c.u64()?;
    let name = c.string(255)?;
    if kind > 4
        || flags & !3 != 0
        || name.is_empty()
        || matches!(name, "." | "..")
        || name.contains('/')
        || (windows && name.contains('\\'))
    {
        return Err(Error::Invalid);
    }
    Ok(Entry {
        kind,
        flags,
        size,
        modified,
        name,
    })
}
struct Cursor<'a>(&'a [u8]);
impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        if self.0.len() < n {
            return Err(Error::Length);
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, Error> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().map_err(|_| Error::Length)?,
        ))
    }
    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().map_err(|_| Error::Length)?,
        ))
    }
    fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().map_err(|_| Error::Length)?,
        ))
    }
    fn string(&mut self, max: usize) -> Result<&'a str, Error> {
        let n = usize::from(self.u16()?);
        if n > max {
            return Err(Error::Length);
        }
        let v = core::str::from_utf8(self.take(n)?).map_err(|_| Error::Invalid)?;
        if v.contains('\0') {
            return Err(Error::Invalid);
        }
        Ok(v)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn request_bounds_and_trailing() {
        let mut b = [0; 531];
        let r = Request {
            id: 1,
            cursor: 2,
            hidden: true,
            path: "/tmp",
        };
        let n = r.encode(&mut b).unwrap();
        assert_eq!(Request::decode(&b[..n]), Ok(r));
        for k in 0..n {
            assert!(Request::decode(&b[..k]).is_err());
        }
        assert!(Request::decode(&b[..n + 1]).is_err());
    }
    #[test]
    fn empty_page_validates_all_bytes() {
        let mut b = [0; 23];
        b[..2].copy_from_slice(&1u16.to_le_bytes());
        b[2] = 1;
        b[16] = 1;
        b[18] = b'/';
        assert_eq!(Page::decode(&b).unwrap().count, 0);
        for n in 0..23 {
            assert!(Page::decode(&b[..n]).is_err());
        }
        b[11] = 1;
        assert!(Page::decode(&b).is_err());
    }
}

#[cfg(test)]
mod boundary_tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;
    fn max_page() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&7u64.to_le_bytes());
        b.extend_from_slice(&[0, 1]);
        b.extend_from_slice(&51u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.push(b'/');
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&51u16.to_le_bytes());
        for i in 0..51 {
            let name = if i == 50 {
                "final".into()
            } else {
                std::format!("{i:020}")
            };
            b.extend_from_slice(&[0, 2]);
            b.extend_from_slice(&u64::MAX.to_le_bytes());
            b.extend_from_slice(&u64::MAX.to_le_bytes());
            b.extend_from_slice(&(name.len() as u16).to_le_bytes());
            b.extend_from_slice(name.as_bytes());
        }
        b
    }
    #[test]
    fn maximum_page_exact_fields_truncation_and_hostile_metadata() {
        let b = max_page();
        assert_eq!(b.len(), 2048);
        let p = Page::decode(&b).unwrap();
        assert_eq!(p.entries().count(), 51);
        assert!(p.more);
        for n in 0..b.len() {
            assert!(Page::decode(&b[..n]).is_err(), "prefix {n}");
        }
        for (i, value) in [
            (0, 2),
            (10, 6),
            (11, 2),
            (21, 129),
            (23, 5),
            (24, 4),
            (43, 0),
            (43, b'/'),
        ] {
            let mut bad = b.clone();
            bad[i] = value;
            assert!(Page::decode(&bad).is_err(), "field {i}");
        }
        let mut bad = b;
        bad.push(0);
        assert!(Page::decode(&bad).is_err());
    }
}

//! Allocation-free CDC bootstrap v1 parsing and sequential transfer validation.

pub const BLOCK_BYTES: usize = 16_384;
pub const LINE_BYTES: usize = 64;
pub const MAX_EXTENTS: usize = 8;
pub const IMAGE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub struct Extent {
    pub offset: usize,
    pub length: usize,
}

pub struct Artifact {
    pub size: usize,
    pub digest: &'static str,
    pub extents: &'static [Extent],
}

impl Artifact {
    pub fn valid(&self, image: &[u8]) -> bool {
        self.size > 0
            && self.size <= IMAGE_BYTES
            && self.digest.len() == 64
            && self
                .digest
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            && !self.extents.is_empty()
            && self.extents.len() <= MAX_EXTENTS
            && self.extents.iter().all(|e| {
                e.length > 0
                    && e.offset
                        .checked_add(e.length)
                        .is_some_and(|end| end <= image.len())
            })
            && self
                .extents
                .iter()
                .try_fold(0usize, |n, e| n.checked_add(e.length))
                == Some(self.size)
    }

    /// Largest contiguous flash slice at a logical file offset, bounded by `limit`.
    pub fn slice<'a>(&self, image: &'a [u8], mut offset: usize, limit: usize) -> Option<&'a [u8]> {
        for extent in self.extents {
            if offset < extent.length {
                let start = extent.offset.checked_add(offset)?;
                let length = limit.min(extent.length - offset);
                return image.get(start..start.checked_add(length)?);
            }
            offset = offset.checked_sub(extent.length)?;
        }
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    Begin,
    Manifest,
    Block(usize),
    Verified,
    Abort,
}

pub fn parse(line: &[u8]) -> Option<Request> {
    match line {
        b"B1" => Some(Request::Begin),
        b"M1 arm64" => Some(Request::Manifest),
        b"V1" => Some(Request::Verified),
        b"X1" => Some(Request::Abort),
        _ => {
            let digits = line.strip_prefix(b"G1 ")?;
            if digits.is_empty() || (digits.len() > 1 && digits[0] == b'0') {
                return None;
            }
            let number = digits.iter().try_fold(0usize, |n, b| {
                if !b.is_ascii_digit() {
                    return None;
                }
                n.checked_mul(10)?.checked_add(usize::from(*b - b'0'))
            })?;
            Some(Request::Block(number))
        }
    }
}

pub struct RequestDecoder {
    line: [u8; LINE_BYTES],
    length: usize,
}

impl Default for RequestDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl RequestDecoder {
    pub const fn new() -> Self {
        Self {
            line: [0; LINE_BYTES],
            length: 0,
        }
    }
    pub fn packet(&mut self, bytes: &[u8]) -> Result<Option<Request>, DecodeError> {
        for (index, byte) in bytes.iter().enumerate() {
            if *byte == b'\n' {
                if index + 1 != bytes.len() {
                    return Err(DecodeError);
                }
                let request = parse(&self.line[..self.length]).ok_or(DecodeError)?;
                self.length = 0;
                return Ok(Some(request));
            }
            let slot = self.line.get_mut(self.length).ok_or(DecodeError)?;
            *slot = *byte;
            self.length += 1;
        }
        Ok(None)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct DecodeError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Response {
    Installer,
    Manifest,
    Block { offset: usize, length: usize },
    Complete,
}

pub struct Session {
    phase: u8,
    size: usize,
    offset: usize,
    index: usize,
}

impl Session {
    pub const fn new(size: usize) -> Self {
        Self {
            phase: 0,
            size,
            offset: 0,
            index: 0,
        }
    }

    pub fn accept(&mut self, request: Request) -> Option<Response> {
        match (self.phase, request) {
            (0, Request::Begin) => {
                self.phase = 1;
                Some(Response::Installer)
            }
            (1, Request::Manifest) if self.size > 0 => {
                self.phase = 2;
                Some(Response::Manifest)
            }
            (2, Request::Block(index)) if index == self.index && self.offset < self.size => {
                let length = BLOCK_BYTES.min(self.size - self.offset);
                let offset = self.offset;
                self.offset += length;
                self.index += 1;
                Some(Response::Block { offset, length })
            }
            (2, Request::Verified) if self.offset == self.size => {
                self.phase = 3;
                Some(Response::Complete)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragmented_lines_are_bounded_and_speculative_coalesced_requests_are_rejected() {
        let mut d = RequestDecoder::new();
        assert_eq!(d.packet(b"G1 12"), Ok(None));
        assert_eq!(d.packet(b"3\n"), Ok(Some(Request::Block(123))));
        assert_eq!(d.packet(b"G1 0\nV1\n"), Err(DecodeError));
        let mut d = RequestDecoder::new();
        assert_eq!(d.packet(&[b'a'; LINE_BYTES]), Ok(None));
        assert_eq!(d.packet(b"a"), Err(DecodeError));
    }

    #[test]
    fn strict_requests_reject_overflow_trailing_data_and_unknown_versions() {
        for line in [
            b"G1 ".as_slice(),
            b"G1 -1",
            b"G1 01",
            b"G1 1 ",
            b"G1 999999999999999999999999",
            b"B2",
            b"M1 x86_64",
            b"V1\0",
        ] {
            assert_eq!(parse(line), None);
        }
        assert_eq!(parse(b"G1 0"), Some(Request::Block(0)));
    }

    #[test]
    fn requests_must_be_ordered_and_cover_the_exact_size_before_verification() {
        let mut s = Session::new(BLOCK_BYTES + 1);
        assert_eq!(s.accept(Request::Manifest), None);
        assert_eq!(s.accept(Request::Begin), Some(Response::Installer));
        assert_eq!(s.accept(Request::Manifest), Some(Response::Manifest));
        assert_eq!(s.accept(Request::Verified), None);
        assert_eq!(s.accept(Request::Block(1)), None);
        assert_eq!(
            s.accept(Request::Block(0)),
            Some(Response::Block {
                offset: 0,
                length: BLOCK_BYTES
            })
        );
        assert_eq!(s.accept(Request::Block(0)), None);
        assert_eq!(
            s.accept(Request::Block(1)),
            Some(Response::Block {
                offset: BLOCK_BYTES,
                length: 1
            })
        );
        assert_eq!(s.accept(Request::Block(2)), None);
        assert_eq!(s.accept(Request::Verified), Some(Response::Complete));
        assert_eq!(s.accept(Request::Verified), None);
    }

    #[test]
    fn logical_slices_reconstruct_fragmented_files_and_reject_bad_metadata() {
        let image = b"abc___defgh";
        let a = Artifact {
            size: 8,
            digest: "0000000000000000000000000000000000000000000000000000000000000000",
            extents: &[
                Extent {
                    offset: 0,
                    length: 3,
                },
                Extent {
                    offset: 6,
                    length: 5,
                },
            ],
        };
        assert!(a.valid(image));
        assert_eq!(a.slice(image, 2, 64), Some(b"c".as_slice()));
        assert_eq!(a.slice(image, 3, 2), Some(b"de".as_slice()));
        assert_eq!(a.slice(image, 8, 64), None);
        let bad = Artifact {
            size: 8,
            digest: a.digest,
            extents: &[Extent {
                offset: usize::MAX,
                length: 8,
            }],
        };
        assert!(!bad.valid(image));
        assert_eq!(bad.slice(image, 0, 64), None);
    }
}

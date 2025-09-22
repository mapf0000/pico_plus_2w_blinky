use picoserve::response::Content;

/// Static bytes with explicit content type (avoid duplicate header code).
pub(crate) struct BytesWithType<'a> {
    pub(crate) ty: &'static str,
    pub(crate) data: &'a [u8],
}

impl<'a> Content for BytesWithType<'a> {
    fn content_type(&self) -> &'static str {
        self.ty
    }
    fn content_length(&self) -> usize {
        self.data.len()
    }
    async fn write_content<W: picoserve::io::Write>(self, mut writer: W) -> Result<(), W::Error> {
        writer.write_all(self.data).await
    }
}

// ===== utilities (fixed for UTF-8 correctness) =====

pub(crate) fn escape_json_str(s: &str) -> heapless::String<512> {
    let mut out: heapless::String<512> = heapless::String::new();
    for ch in s.chars() {
        match ch {
            '\"' => {
                let _ = out.push_str("\\\"");
            }
            '\\' => {
                let _ = out.push_str("\\\\");
            }
            '\n' => {
                let _ = out.push_str("\\n");
            }
            '\r' => {
                let _ = out.push_str("\\r");
            }
            '\t' => {
                let _ = out.push_str("\\t");
            }
            c if (c as u32) < 0x20 => {
                let _ = core::fmt::write(&mut out, format_args!("\\u{:04X}", c as u32));
            }
            c => {
                let _ = out.push(c);
            }
        }
    }
    out
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

pub(crate) fn percent_decode_str<const N: usize>(s: &str) -> Option<heapless::String<N>> {
    let bytes = s.as_bytes();
    let mut out_bytes: heapless::Vec<u8, N> = heapless::Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hi = hex_val(bytes[i + 1])?;
                let lo = hex_val(bytes[i + 2])?;
                let b = (hi << 4) | lo;
                if out_bytes.push(b).is_err() {
                    return None;
                }
                i += 3;
            }
            b'+' => {
                if out_bytes.push(b' ').is_err() {
                    return None;
                }
                i += 1;
            }
            c => {
                if out_bytes.push(c).is_err() {
                    return None;
                }
                i += 1;
            }
        }
    }
    let s = core::str::from_utf8(&out_bytes).ok()?;
    let mut out: heapless::String<N> = heapless::String::new();
    out.push_str(s).ok()?;
    Some(out)
}

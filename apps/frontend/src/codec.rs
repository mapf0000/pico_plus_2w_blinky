use std::vec::Vec;

pub fn encode_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(nibble_to_hex(b >> 4));
        out.push(nibble_to_hex(b & 0x0f));
    }
    out
}

pub fn decode_hex(input: &str) -> Result<Vec<u8>, &'static str> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("empty");
    }
    let chars = trimmed.as_bytes();
    if !chars.len().is_multiple_of(2) {
        return Err("odd-length");
    }
    let mut out = Vec::with_capacity(chars.len() / 2);
    let mut i = 0;
    while i < chars.len() {
        let hi = decode_nibble(chars[i]).ok_or("bad-hex")?;
        let lo = decode_nibble(chars[i + 1]).ok_or("bad-hex")?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Ok(out)
}

fn nibble_to_hex(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        10..=15 => (b'A' + (n - 10)) as char,
        _ => unreachable!(),
    }
}

fn decode_nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(10 + c - b'a'),
        b'A'..=b'F' => Some(10 + c - b'A'),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        let bytes = [0x00, 0xAB, 0x7F, 0x10];
        let encoded = encode_hex(&bytes);
        assert_eq!(encoded, "00AB7F10");
        let decoded = decode_hex(&encoded).expect("hex decode");
        assert_eq!(decoded, bytes);
    }
}

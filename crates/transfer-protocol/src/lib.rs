#![no_std]

pub const TRANSFER_PROTOCOL_VERSION: u16 = 2;
pub const SESSION_PROTOCOL_VERSION: u16 = 2;
pub const WEBSOCKET_PORT: u16 = 81;
/// Application close code used when a newer browser connection takes ownership
/// of the singleton transfer session.
pub const WEBSOCKET_CLOSE_SESSION_REPLACED: u16 = 4001;

pub const TAG_TRANSFER_SESSION_TO_HOST: u8 = 32;
pub const TAG_TRANSFER_SESSION_TO_BROWSER: u8 = 33;
pub const TAG_USB_BENCHMARK_START: u8 = 240;
pub const TAG_USB_BENCHMARK_DATA: u8 = 241;
pub const TAG_USB_BENCHMARK_FINISH: u8 = 242;
pub const TAG_USB_BENCHMARK_RESULT: u8 = 243;
pub const TAG_USB_RAW_BENCHMARK_START: u8 = 244;
pub const TAG_USB_RAW_BENCHMARK_RESULT: u8 = 245;

pub const USB_BENCHMARK_VERSION: u16 = 1;
pub const USB_BENCHMARK_STATUS_STARTED: u8 = 0;
pub const USB_BENCHMARK_STATUS_PROGRESS: u8 = 1;
pub const USB_BENCHMARK_STATUS_COMPLETE: u8 = 2;
pub const USB_BENCHMARK_STATUS_ERROR: u8 = 3;
pub const USB_BENCHMARK_DATA_HEADER_LEN: usize = 8;
pub const USB_BENCHMARK_RESULT_LEN: usize = 35;
pub const USB_RAW_BENCHMARK_VERSION: u16 = 1;
pub const USB_RAW_BENCHMARK_START_LEN: usize = 14;
pub const USB_RAW_BENCHMARK_RESULT_LEN: usize = 35;
pub const USB_RAW_BENCHMARK_MAX_BYTES: u64 = 64 * 1024 * 1024;

pub const WS_BINARY_KIND_SESSION: u8 = 3;
pub const WS_BINARY_KIND_SECURE_OPEN: u8 = 4;
pub const WS_BINARY_KIND_SECURE_CHUNK: u8 = 5;
pub const WS_BINARY_KIND_SECURE_CLOSE: u8 = 6;
pub const WS_BINARY_KIND_SECURE_CHUNK_BATCH: u8 = 7;

pub const SESSION_ID_LEN: usize = 16;
pub const FILE_SALT_LEN: usize = 32;
pub const FILE_TAG_LEN: usize = 16;
pub const SHA256_LEN: usize = 32;
pub const TLV_MAX_PAYLOAD: usize = 2048;
pub const MAX_SECURE_SESSION_FRAME: usize = 768;
pub const NOISE_TAG_LEN: usize = 16;

pub const SESSION_KIND_REQUEST: u8 = 1;
pub const SESSION_KIND_READY: u8 = 2;
pub const SESSION_KIND_HANDSHAKE: u8 = 3;
pub const SESSION_KIND_TRANSPORT: u8 = 4;
pub const SESSION_KIND_ERROR: u8 = 5;

pub const CONTROL_START_TRANSFER: u8 = 1;
pub const CONTROL_SET_DEFAULT_PATH: u8 = 2;
pub const CONTROL_TRANSFER_RECEIPT: u8 = 3;
pub const CONTROL_START_DEFAULT_TRANSFER: u8 = 4;

pub const RECORD_MANIFEST: u8 = 1;
pub const RECORD_CHUNK: u8 = 2;
pub const RECORD_CLOSE: u8 = 3;

pub const SESSION_ENVELOPE_HEADER_LEN: usize = 2 + 1 + SESSION_ID_LEN + 2;
pub const FILE_OPEN_HEADER_LEN: usize = 2 + SESSION_ID_LEN + 8 + FILE_SALT_LEN + 2 + 4 + 2;
pub const FILE_CHUNK_HEADER_LEN: usize = SESSION_ID_LEN + 8 + 4 + 2;
pub const FILE_CLOSE_HEADER_LEN: usize = SESSION_ID_LEN + 8 + 2;
pub const MAX_PLAINTEXT_CHUNK: usize = TLV_MAX_PAYLOAD - FILE_CHUNK_HEADER_LEN - FILE_TAG_LEN;
pub const MAX_FILE_NAME_LEN: usize = 96;
pub const MAX_TRANSFER_PATH_LEN: usize = 512;
pub const MANIFEST_PLAINTEXT_BASE_LEN: usize = 1 + 8 + 2;
pub const CLOSE_PLAINTEXT_LEN: usize = 8 + 4 + SHA256_LEN;
pub const MAX_MANIFEST_CIPHERTEXT_LEN: usize =
    MANIFEST_PLAINTEXT_BASE_LEN + MAX_FILE_NAME_LEN + FILE_TAG_LEN;
pub const CLOSE_CIPHERTEXT_LEN: usize = CLOSE_PLAINTEXT_LEN + FILE_TAG_LEN;
pub const SECURE_CHUNK_BATCH_MAX_CHUNKS: usize = 8;
pub const SECURE_CHUNK_BATCH_HEADER_LEN: usize = 1;
pub const SECURE_CHUNK_BATCH_ENTRY_HEADER_LEN: usize = 2;
pub const MAX_SECURE_CHUNK_BATCH_LEN: usize = SECURE_CHUNK_BATCH_HEADER_LEN
    + SECURE_CHUNK_BATCH_MAX_CHUNKS * (SECURE_CHUNK_BATCH_ENTRY_HEADER_LEN + TLV_MAX_PAYLOAD);
const _: () = assert!(
    SESSION_ENVELOPE_HEADER_LEN + 1 + MAX_TRANSFER_PATH_LEN + NOISE_TAG_LEN
        <= MAX_SECURE_SESSION_FRAME
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionEnvelope<'a> {
    pub kind: u8,
    pub session_id: [u8; SESSION_ID_LEN],
    pub body: &'a [u8],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SecureOpen<'a> {
    pub session_id: [u8; SESSION_ID_LEN],
    pub transfer_id: u64,
    pub file_salt: [u8; FILE_SALT_LEN],
    pub chunk_size: u16,
    pub chunk_count: u32,
    pub ciphertext: &'a [u8],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SecureChunk<'a> {
    pub session_id: [u8; SESSION_ID_LEN],
    pub transfer_id: u64,
    pub chunk_index: u32,
    pub ciphertext: &'a [u8],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SecureClose<'a> {
    pub session_id: [u8; SESSION_ID_LEN],
    pub transfer_id: u64,
    pub ciphertext: &'a [u8],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SecureChunkBatch<'a> {
    chunk_count: u8,
    entries: &'a [u8],
}

/// Incrementally encodes a bounded secure-chunk batch without allocation.
///
/// The output contains the batch payload beginning with `chunk_count`; callers
/// are responsible for prepending the WebSocket binary-kind byte.
pub struct SecureChunkBatchEncoder<'a> {
    output: &'a mut [u8],
    len: usize,
    chunk_count: u8,
}

impl<'a> SecureChunkBatchEncoder<'a> {
    pub fn new(output: &'a mut [u8]) -> Result<Self, DecodeError> {
        let Some(first) = output.first_mut() else {
            return Err(DecodeError::Truncated);
        };
        *first = 0;
        Ok(Self {
            output,
            len: SECURE_CHUNK_BATCH_HEADER_LEN,
            chunk_count: 0,
        })
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<(), DecodeError> {
        if self.chunk_count as usize >= SECURE_CHUNK_BATCH_MAX_CHUNKS {
            return Err(DecodeError::InvalidField);
        }
        if chunk.is_empty() || chunk.len() > TLV_MAX_PAYLOAD {
            return Err(DecodeError::InvalidLength);
        }
        decode_secure_chunk(chunk)?;

        let needed = self
            .len
            .checked_add(SECURE_CHUNK_BATCH_ENTRY_HEADER_LEN)
            .and_then(|len| len.checked_add(chunk.len()))
            .ok_or(DecodeError::InvalidLength)?;
        if needed > self.output.len() {
            return Err(DecodeError::Truncated);
        }

        let chunk_len = u16::try_from(chunk.len()).map_err(|_| DecodeError::InvalidLength)?;
        self.output[self.len..self.len + 2].copy_from_slice(&chunk_len.to_le_bytes());
        self.len += SECURE_CHUNK_BATCH_ENTRY_HEADER_LEN;
        self.output[self.len..self.len + chunk.len()].copy_from_slice(chunk);
        self.len += chunk.len();
        self.chunk_count += 1;
        self.output[0] = self.chunk_count;
        Ok(())
    }

    pub fn chunk_count(&self) -> usize {
        self.chunk_count as usize
    }

    pub fn finish(self) -> Result<usize, DecodeError> {
        if self.chunk_count == 0 {
            return Err(DecodeError::InvalidField);
        }
        Ok(self.len)
    }
}

impl<'a> SecureChunkBatch<'a> {
    pub fn chunk_count(&self) -> usize {
        self.chunk_count as usize
    }

    pub fn chunks(&self) -> SecureChunkBatchIter<'a> {
        SecureChunkBatchIter {
            remaining: self.chunk_count,
            entries: self.entries,
        }
    }
}

pub struct SecureChunkBatchIter<'a> {
    remaining: u8,
    entries: &'a [u8],
}

impl<'a> Iterator for SecureChunkBatchIter<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let chunk_len = read_u16(self.entries.get(..2)?) as usize;
        let chunk = self.entries.get(2..2 + chunk_len)?;
        self.entries = self.entries.get(2 + chunk_len..)?;
        self.remaining -= 1;
        Some(chunk)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.remaining as usize;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for SecureChunkBatchIter<'_> {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    Truncated,
    InvalidVersion,
    InvalidLength,
    InvalidField,
    TrailingData,
}

pub fn encode_session_envelope(
    out: &mut [u8],
    kind: u8,
    session_id: &[u8; SESSION_ID_LEN],
    body: &[u8],
) -> Result<usize, DecodeError> {
    let body_len = u16::try_from(body.len()).map_err(|_| DecodeError::InvalidLength)?;
    let needed = SESSION_ENVELOPE_HEADER_LEN + body.len();
    if out.len() < needed {
        return Err(DecodeError::Truncated);
    }
    out[0..2].copy_from_slice(&SESSION_PROTOCOL_VERSION.to_le_bytes());
    out[2] = kind;
    out[3..3 + SESSION_ID_LEN].copy_from_slice(session_id);
    let len_at = 3 + SESSION_ID_LEN;
    out[len_at..len_at + 2].copy_from_slice(&body_len.to_le_bytes());
    out[SESSION_ENVELOPE_HEADER_LEN..needed].copy_from_slice(body);
    Ok(needed)
}

pub fn decode_session_envelope(input: &[u8]) -> Result<SessionEnvelope<'_>, DecodeError> {
    if input.len() < SESSION_ENVELOPE_HEADER_LEN {
        return Err(DecodeError::Truncated);
    }
    if read_u16(&input[0..2]) != SESSION_PROTOCOL_VERSION {
        return Err(DecodeError::InvalidVersion);
    }
    let kind = input[2];
    let session_id = read_array::<SESSION_ID_LEN>(&input[3..3 + SESSION_ID_LEN]);
    let len_at = 3 + SESSION_ID_LEN;
    let body_len = read_u16(&input[len_at..len_at + 2]) as usize;
    if input.len() != SESSION_ENVELOPE_HEADER_LEN + body_len {
        return Err(DecodeError::InvalidLength);
    }
    Ok(SessionEnvelope {
        kind,
        session_id,
        body: &input[SESSION_ENVELOPE_HEADER_LEN..],
    })
}

pub fn encode_secure_open(
    out: &mut [u8],
    session_id: &[u8; SESSION_ID_LEN],
    transfer_id: u64,
    file_salt: &[u8; FILE_SALT_LEN],
    chunk_size: u16,
    chunk_count: u32,
    ciphertext: &[u8],
) -> Result<usize, DecodeError> {
    if chunk_size == 0 || chunk_size as usize > MAX_PLAINTEXT_CHUNK {
        return Err(DecodeError::InvalidField);
    }
    if !(MANIFEST_PLAINTEXT_BASE_LEN + FILE_TAG_LEN..=MAX_MANIFEST_CIPHERTEXT_LEN)
        .contains(&ciphertext.len())
    {
        return Err(DecodeError::InvalidLength);
    }
    let ciphertext_len = ciphertext.len() as u16;
    let needed = FILE_OPEN_HEADER_LEN + ciphertext.len();
    if out.len() < needed || needed > TLV_MAX_PAYLOAD {
        return Err(DecodeError::Truncated);
    }
    let mut at = 0;
    put(out, &mut at, &TRANSFER_PROTOCOL_VERSION.to_le_bytes());
    put(out, &mut at, session_id);
    put(out, &mut at, &transfer_id.to_le_bytes());
    put(out, &mut at, file_salt);
    put(out, &mut at, &chunk_size.to_le_bytes());
    put(out, &mut at, &chunk_count.to_le_bytes());
    put(out, &mut at, &ciphertext_len.to_le_bytes());
    put(out, &mut at, ciphertext);
    Ok(at)
}

pub fn decode_secure_open(input: &[u8]) -> Result<SecureOpen<'_>, DecodeError> {
    if input.len() < FILE_OPEN_HEADER_LEN {
        return Err(DecodeError::Truncated);
    }
    let mut at = 0;
    let version = take_u16(input, &mut at)?;
    if version != TRANSFER_PROTOCOL_VERSION {
        return Err(DecodeError::InvalidVersion);
    }
    let session_id = take_array::<SESSION_ID_LEN>(input, &mut at)?;
    let transfer_id = take_u64(input, &mut at)?;
    let file_salt = take_array::<FILE_SALT_LEN>(input, &mut at)?;
    let chunk_size = take_u16(input, &mut at)?;
    let chunk_count = take_u32(input, &mut at)?;
    let ciphertext_len = take_u16(input, &mut at)? as usize;
    if chunk_size == 0 || chunk_size as usize > MAX_PLAINTEXT_CHUNK {
        return Err(DecodeError::InvalidField);
    }
    if !(MANIFEST_PLAINTEXT_BASE_LEN + FILE_TAG_LEN..=MAX_MANIFEST_CIPHERTEXT_LEN)
        .contains(&ciphertext_len)
    {
        return Err(DecodeError::InvalidLength);
    }
    let ciphertext = take(input, &mut at, ciphertext_len)?;
    if at != input.len() {
        return Err(DecodeError::TrailingData);
    }
    Ok(SecureOpen {
        session_id,
        transfer_id,
        file_salt,
        chunk_size,
        chunk_count,
        ciphertext,
    })
}

pub fn encode_secure_chunk(
    out: &mut [u8],
    session_id: &[u8; SESSION_ID_LEN],
    transfer_id: u64,
    chunk_index: u32,
    ciphertext: &[u8],
) -> Result<usize, DecodeError> {
    if ciphertext.len() < FILE_TAG_LEN || ciphertext.len() > MAX_PLAINTEXT_CHUNK + FILE_TAG_LEN {
        return Err(DecodeError::InvalidLength);
    }
    let ciphertext_len = ciphertext.len() as u16;
    let needed = FILE_CHUNK_HEADER_LEN + ciphertext.len();
    if out.len() < needed || needed > TLV_MAX_PAYLOAD {
        return Err(DecodeError::Truncated);
    }
    let mut at = 0;
    put(out, &mut at, session_id);
    put(out, &mut at, &transfer_id.to_le_bytes());
    put(out, &mut at, &chunk_index.to_le_bytes());
    put(out, &mut at, &ciphertext_len.to_le_bytes());
    put(out, &mut at, ciphertext);
    Ok(at)
}

pub fn decode_secure_chunk(input: &[u8]) -> Result<SecureChunk<'_>, DecodeError> {
    if input.len() < FILE_CHUNK_HEADER_LEN {
        return Err(DecodeError::Truncated);
    }
    let mut at = 0;
    let session_id = take_array::<SESSION_ID_LEN>(input, &mut at)?;
    let transfer_id = take_u64(input, &mut at)?;
    let chunk_index = take_u32(input, &mut at)?;
    let ciphertext_len = take_u16(input, &mut at)? as usize;
    if !(FILE_TAG_LEN..=MAX_PLAINTEXT_CHUNK + FILE_TAG_LEN).contains(&ciphertext_len) {
        return Err(DecodeError::InvalidLength);
    }
    let ciphertext = take(input, &mut at, ciphertext_len)?;
    if at != input.len() {
        return Err(DecodeError::TrailingData);
    }
    Ok(SecureChunk {
        session_id,
        transfer_id,
        chunk_index,
        ciphertext,
    })
}

pub fn decode_secure_chunk_batch(input: &[u8]) -> Result<SecureChunkBatch<'_>, DecodeError> {
    let (&chunk_count, entries) = input.split_first().ok_or(DecodeError::Truncated)?;
    if chunk_count == 0 || chunk_count as usize > SECURE_CHUNK_BATCH_MAX_CHUNKS {
        return Err(DecodeError::InvalidField);
    }

    let mut at = 0;
    for _ in 0..chunk_count {
        let chunk_len = take_u16(entries, &mut at)? as usize;
        if chunk_len == 0 || chunk_len > TLV_MAX_PAYLOAD {
            return Err(DecodeError::InvalidLength);
        }
        decode_secure_chunk(take(entries, &mut at, chunk_len)?)?;
    }
    if at != entries.len() {
        return Err(DecodeError::TrailingData);
    }

    Ok(SecureChunkBatch {
        chunk_count,
        entries,
    })
}

pub fn encode_secure_close(
    out: &mut [u8],
    session_id: &[u8; SESSION_ID_LEN],
    transfer_id: u64,
    ciphertext: &[u8],
) -> Result<usize, DecodeError> {
    if ciphertext.len() != CLOSE_CIPHERTEXT_LEN {
        return Err(DecodeError::InvalidLength);
    }
    let ciphertext_len = ciphertext.len() as u16;
    let needed = FILE_CLOSE_HEADER_LEN + ciphertext.len();
    if out.len() < needed || needed > TLV_MAX_PAYLOAD {
        return Err(DecodeError::Truncated);
    }
    let mut at = 0;
    put(out, &mut at, session_id);
    put(out, &mut at, &transfer_id.to_le_bytes());
    put(out, &mut at, &ciphertext_len.to_le_bytes());
    put(out, &mut at, ciphertext);
    Ok(at)
}

pub fn decode_secure_close(input: &[u8]) -> Result<SecureClose<'_>, DecodeError> {
    if input.len() < FILE_CLOSE_HEADER_LEN {
        return Err(DecodeError::Truncated);
    }
    let mut at = 0;
    let session_id = take_array::<SESSION_ID_LEN>(input, &mut at)?;
    let transfer_id = take_u64(input, &mut at)?;
    let ciphertext_len = take_u16(input, &mut at)? as usize;
    if ciphertext_len != CLOSE_CIPHERTEXT_LEN {
        return Err(DecodeError::InvalidLength);
    }
    let ciphertext = take(input, &mut at, ciphertext_len)?;
    if at != input.len() {
        return Err(DecodeError::TrailingData);
    }
    Ok(SecureClose {
        session_id,
        transfer_id,
        ciphertext,
    })
}

fn put(out: &mut [u8], at: &mut usize, bytes: &[u8]) {
    out[*at..*at + bytes.len()].copy_from_slice(bytes);
    *at += bytes.len();
}

fn take<'a>(input: &'a [u8], at: &mut usize, len: usize) -> Result<&'a [u8], DecodeError> {
    let end = at.checked_add(len).ok_or(DecodeError::InvalidLength)?;
    let bytes = input.get(*at..end).ok_or(DecodeError::Truncated)?;
    *at = end;
    Ok(bytes)
}

fn take_u16(input: &[u8], at: &mut usize) -> Result<u16, DecodeError> {
    Ok(read_u16(take(input, at, 2)?))
}

fn take_u32(input: &[u8], at: &mut usize) -> Result<u32, DecodeError> {
    let bytes = take(input, at, 4)?;
    Ok(u32::from_le_bytes(read_array(bytes)))
}

fn take_u64(input: &[u8], at: &mut usize) -> Result<u64, DecodeError> {
    let bytes = take(input, at, 8)?;
    Ok(u64::from_le_bytes(read_array(bytes)))
}

fn take_array<const N: usize>(input: &[u8], at: &mut usize) -> Result<[u8; N], DecodeError> {
    Ok(read_array(take(input, at, N)?))
}

fn read_u16(input: &[u8]) -> u16 {
    u16::from_le_bytes(read_array(input))
}

fn read_array<const N: usize>(input: &[u8]) -> [u8; N] {
    let mut out = [0; N];
    out.copy_from_slice(input);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_roundtrip_rejects_trailing_data() {
        let mut out = [0; 64];
        let len =
            encode_session_envelope(&mut out, SESSION_KIND_HANDSHAKE, &[7; 16], b"abc").unwrap();
        let decoded = decode_session_envelope(&out[..len]).unwrap();
        assert_eq!(decoded.kind, SESSION_KIND_HANDSHAKE);
        assert_eq!(decoded.session_id, [7; 16]);
        assert_eq!(decoded.body, b"abc");
        assert_eq!(
            decode_session_envelope(&out[..len + 1]),
            Err(DecodeError::InvalidLength)
        );
    }

    #[test]
    fn maximum_chunk_exactly_fits_tlv() {
        let ciphertext = [0x5a; MAX_PLAINTEXT_CHUNK + FILE_TAG_LEN];
        let mut out = [0; TLV_MAX_PAYLOAD];
        let len = encode_secure_chunk(&mut out, &[1; 16], 9, 3, &ciphertext).unwrap();
        assert_eq!(len, TLV_MAX_PAYLOAD);
        let decoded = decode_secure_chunk(&out).unwrap();
        assert_eq!(decoded.ciphertext, ciphertext);
    }

    #[test]
    fn open_roundtrip() {
        let mut out = [0; 256];
        let ciphertext = [0x55; MANIFEST_PLAINTEXT_BASE_LEN + FILE_TAG_LEN];
        let len =
            encode_secure_open(&mut out, &[1; 16], 42, &[2; 32], 2002, 9, &ciphertext).unwrap();
        let open = decode_secure_open(&out[..len]).unwrap();
        assert_eq!(open.transfer_id, 42);
        assert_eq!(open.chunk_count, 9);
        assert_eq!(open.ciphertext, ciphertext);
    }

    #[test]
    fn secure_chunk_batch_validates_and_iterates_entries() {
        let ciphertext = [0x5a; FILE_TAG_LEN + 5];
        let mut first = [0; 128];
        let first_len = encode_secure_chunk(&mut first, &[1; 16], 9, 3, &ciphertext).unwrap();
        let mut second = [0; 128];
        let second_len = encode_secure_chunk(&mut second, &[1; 16], 9, 4, &ciphertext).unwrap();

        let mut batch = [0; 512];
        let mut encoder = SecureChunkBatchEncoder::new(&mut batch).unwrap();
        encoder.push(&first[..first_len]).unwrap();
        encoder.push(&second[..second_len]).unwrap();
        assert_eq!(encoder.chunk_count(), 2);
        let at = encoder.finish().unwrap();

        let decoded = decode_secure_chunk_batch(&batch[..at]).unwrap();
        assert_eq!(decoded.chunk_count(), 2);
        let mut chunks = decoded.chunks();
        assert_eq!(
            decode_secure_chunk(chunks.next().unwrap())
                .unwrap()
                .chunk_index,
            3
        );
        assert_eq!(
            decode_secure_chunk(chunks.next().unwrap())
                .unwrap()
                .chunk_index,
            4
        );
        assert!(chunks.next().is_none());
    }

    #[test]
    fn secure_chunk_batch_encoder_enforces_bounds() {
        assert!(matches!(
            SecureChunkBatchEncoder::new(&mut []),
            Err(DecodeError::Truncated)
        ));

        let ciphertext = [0x5a; FILE_TAG_LEN];
        let mut chunk = [0; 128];
        let chunk_len = encode_secure_chunk(&mut chunk, &[1; 16], 9, 0, &ciphertext).unwrap();

        let mut too_small = [0; 2];
        let mut encoder = SecureChunkBatchEncoder::new(&mut too_small).unwrap();
        assert_eq!(
            encoder.push(&chunk[..chunk_len]),
            Err(DecodeError::Truncated)
        );
        assert_eq!(encoder.finish(), Err(DecodeError::InvalidField));

        let mut batch = [0; MAX_SECURE_CHUNK_BATCH_LEN];
        let mut encoder = SecureChunkBatchEncoder::new(&mut batch).unwrap();
        for _ in 0..SECURE_CHUNK_BATCH_MAX_CHUNKS {
            encoder.push(&chunk[..chunk_len]).unwrap();
        }
        assert_eq!(
            encoder.push(&chunk[..chunk_len]),
            Err(DecodeError::InvalidField)
        );
    }

    #[test]
    fn secure_chunk_batch_rejects_invalid_counts_and_trailing_data() {
        assert_eq!(decode_secure_chunk_batch(&[]), Err(DecodeError::Truncated));
        assert_eq!(
            decode_secure_chunk_batch(&[0]),
            Err(DecodeError::InvalidField)
        );
        assert_eq!(
            decode_secure_chunk_batch(&[SECURE_CHUNK_BATCH_MAX_CHUNKS as u8 + 1]),
            Err(DecodeError::InvalidField)
        );

        let ciphertext = [0x5a; FILE_TAG_LEN];
        let mut chunk = [0; 128];
        let chunk_len = encode_secure_chunk(&mut chunk, &[1; 16], 9, 0, &ciphertext).unwrap();
        let mut batch = [0; 256];
        batch[0] = 1;
        batch[1..3].copy_from_slice(&(chunk_len as u16).to_le_bytes());
        batch[3..3 + chunk_len].copy_from_slice(&chunk[..chunk_len]);
        batch[3 + chunk_len] = 0xff;
        assert_eq!(
            decode_secure_chunk_batch(&batch[..4 + chunk_len]),
            Err(DecodeError::TrailingData)
        );
    }
}

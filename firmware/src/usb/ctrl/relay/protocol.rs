use super::*;

pub(super) fn parse_file_open(payload: &[u8]) -> Option<IncomingOpen<'_>> {
    let mut cursor = payload;
    let protocol_version = take_u16(&mut cursor)?;
    let transfer_id = take_u64(&mut cursor)?;
    let total_size = take_u64(&mut cursor)?;
    let chunk_size = take_u16(&mut cursor)?;
    let chunk_count = take_u32(&mut cursor)?;
    let sha256 = take_array_32(&mut cursor)?;
    let file_name_len = take_u16(&mut cursor)? as usize;
    let file_name_bytes = take_bytes(&mut cursor, file_name_len)?;
    let file_name = core::str::from_utf8(file_name_bytes).ok()?;

    Some(IncomingOpen {
        protocol_version,
        transfer_id,
        total_size,
        chunk_size,
        chunk_count,
        sha256,
        file_name,
    })
}

pub(super) fn parse_file_chunk(payload: &[u8]) -> Option<IncomingChunk<'_>> {
    let mut cursor = payload;
    let transfer_id = take_u64(&mut cursor)?;
    let chunk_index = take_u32(&mut cursor)?;
    let offset = take_u64(&mut cursor)?;
    let payload_len = take_u16(&mut cursor)? as usize;
    if payload_len > FILE_CHUNK_MAX_DATA {
        return None;
    }
    let payload = take_bytes(&mut cursor, payload_len)?;
    let chunk_crc32 = take_u32(&mut cursor)?;

    Some(IncomingChunk {
        transfer_id,
        chunk_index,
        offset,
        payload,
        chunk_crc32,
    })
}

pub(super) fn parse_file_close(payload: &[u8]) -> Option<IncomingClose> {
    let mut cursor = payload;
    Some(IncomingClose {
        transfer_id: take_u64(&mut cursor)?,
        sent_chunk_count: take_u32(&mut cursor)?,
        sent_total_size: take_u64(&mut cursor)?,
    })
}

pub(super) fn parse_file_abort(payload: &[u8]) -> Option<IncomingAbort<'_>> {
    let mut cursor = payload;
    let transfer_id = take_u64(&mut cursor)?;
    let reason_code = take_u8(&mut cursor)?;
    let detail_len = take_u16(&mut cursor)? as usize;
    let detail_bytes = take_bytes(&mut cursor, detail_len)?;
    let detail = core::str::from_utf8(detail_bytes).ok()?;

    Some(IncomingAbort {
        transfer_id,
        reason_code,
        detail,
    })
}

pub(super) fn crc32_ieee(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc ^= *byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn take_bytes<'a>(cursor: &mut &'a [u8], len: usize) -> Option<&'a [u8]> {
    if cursor.len() < len {
        return None;
    }
    let (head, tail) = cursor.split_at(len);
    *cursor = tail;
    Some(head)
}

fn take_u8(cursor: &mut &[u8]) -> Option<u8> {
    Some(take_bytes(cursor, 1)?[0])
}

fn take_u16(cursor: &mut &[u8]) -> Option<u16> {
    let bytes = take_bytes(cursor, 2)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn take_u32(cursor: &mut &[u8]) -> Option<u32> {
    let bytes = take_bytes(cursor, 4)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn take_u64(cursor: &mut &[u8]) -> Option<u64> {
    let bytes = take_bytes(cursor, 8)?;
    Some(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

fn take_array_32(cursor: &mut &[u8]) -> Option<[u8; 32]> {
    let bytes = take_bytes(cursor, 32)?;
    let mut out = [0u8; 32];
    out.copy_from_slice(bytes);
    Some(out)
}

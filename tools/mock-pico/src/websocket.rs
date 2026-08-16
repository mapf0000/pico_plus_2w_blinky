use anyhow::{Context, Result, bail, ensure};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

const MAX_HTTP_HEADER: usize = 8 * 1024;
const MAX_FRAME_PAYLOAD: usize = transfer_protocol::MAX_SECURE_CHUNK_BATCH_LEN + 1;
const WEBSOCKET_GUID: &[u8] = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

#[derive(Debug, PartialEq, Eq)]
pub enum Inbound {
    Text(String),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Close,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outbound {
    Text(String),
    Binary(Vec<u8>),
    Pong(Vec<u8>),
    Close { code: u16, reason: String },
}

pub async fn upgrade(mut stream: TcpStream) -> Result<(OwnedReadHalf, OwnedWriteHalf)> {
    let mut request = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];
    while request.len() < MAX_HTTP_HEADER {
        stream
            .read_exact(&mut byte)
            .await
            .context("read WebSocket upgrade request")?;
        request.push(byte[0]);
        if request.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    ensure!(
        request.ends_with(b"\r\n\r\n"),
        "WebSocket upgrade headers are too large"
    );
    let request = std::str::from_utf8(&request).context("upgrade request is not UTF-8")?;
    let mut lines = request.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut request_parts = request_line.split_ascii_whitespace();
    ensure!(
        request_parts.next() == Some("GET"),
        "expected WebSocket GET"
    );
    ensure!(
        request_parts.next() == Some("/ws"),
        "expected WebSocket path /ws"
    );

    let mut websocket_key = None;
    let mut upgrade = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("sec-websocket-key") {
            websocket_key = Some(value.trim());
        } else if name.eq_ignore_ascii_case("upgrade")
            && value.trim().eq_ignore_ascii_case("websocket")
        {
            upgrade = true;
        }
    }
    ensure!(upgrade, "missing WebSocket Upgrade header");
    let websocket_key = websocket_key.context("missing Sec-WebSocket-Key")?;
    let accept = websocket_accept(websocket_key);
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    stream
        .write_all(response.as_bytes())
        .await
        .context("write WebSocket upgrade response")?;
    Ok(stream.into_split())
}

pub async fn read_message(reader: &mut OwnedReadHalf) -> Result<Inbound> {
    read_message_from(reader).await
}

async fn read_message_from<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Inbound> {
    loop {
        let mut header = [0u8; 2];
        reader
            .read_exact(&mut header)
            .await
            .context("read WebSocket frame header")?;
        ensure!(
            header[0] & 0x70 == 0,
            "WebSocket RSV bits are not supported"
        );
        ensure!(
            header[0] & 0x80 != 0,
            "fragmented WebSocket messages are not supported"
        );
        ensure!(
            header[1] & 0x80 != 0,
            "client WebSocket frame is not masked"
        );

        let opcode = header[0] & 0x0f;
        let mut payload_len = u64::from(header[1] & 0x7f);
        if payload_len == 126 {
            let mut extended = [0u8; 2];
            reader.read_exact(&mut extended).await?;
            payload_len = u64::from(u16::from_be_bytes(extended));
        } else if payload_len == 127 {
            let mut extended = [0u8; 8];
            reader.read_exact(&mut extended).await?;
            payload_len = u64::from_be_bytes(extended);
        }
        ensure!(
            payload_len <= MAX_FRAME_PAYLOAD as u64,
            "WebSocket payload exceeds mock limit"
        );
        if opcode >= 0x8 {
            ensure!(payload_len <= 125, "oversized WebSocket control frame");
        }

        let mut mask = [0u8; 4];
        reader.read_exact(&mut mask).await?;
        let mut payload = vec![0u8; payload_len as usize];
        reader.read_exact(&mut payload).await?;
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[index % mask.len()];
        }

        match opcode {
            0x1 => {
                return Ok(Inbound::Text(
                    String::from_utf8(payload).context("WebSocket text is not UTF-8")?,
                ));
            }
            0x2 => return Ok(Inbound::Binary(payload)),
            0x8 => return Ok(Inbound::Close),
            0x9 => return Ok(Inbound::Ping(payload)),
            0xa => continue,
            _ => bail!("unsupported WebSocket opcode {opcode}"),
        }
    }
}

pub async fn write_message(writer: &mut OwnedWriteHalf, message: &Outbound) -> Result<()> {
    match message {
        Outbound::Text(payload) => write_frame(writer, 0x1, payload.as_bytes()).await,
        Outbound::Binary(payload) => write_frame(writer, 0x2, payload).await,
        Outbound::Pong(payload) => write_frame(writer, 0xa, payload).await,
        Outbound::Close { code, reason } => {
            let mut payload = Vec::with_capacity(2 + reason.len());
            payload.extend_from_slice(&code.to_be_bytes());
            payload.extend_from_slice(reason.as_bytes());
            write_frame(writer, 0x8, &payload).await
        }
    }
}

async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    opcode: u8,
    payload: &[u8],
) -> Result<()> {
    ensure!(
        payload.len() <= MAX_FRAME_PAYLOAD,
        "outbound payload is too large"
    );
    let mut header = [0u8; 10];
    header[0] = 0x80 | opcode;
    let header_len = if payload.len() <= 125 {
        header[1] = payload.len() as u8;
        2
    } else if payload.len() <= u16::MAX as usize {
        header[1] = 126;
        header[2..4].copy_from_slice(&(payload.len() as u16).to_be_bytes());
        4
    } else {
        header[1] = 127;
        header[2..10].copy_from_slice(&(payload.len() as u64).to_be_bytes());
        10
    };
    writer.write_all(&header[..header_len]).await?;
    writer.write_all(payload).await?;
    writer.flush().await?;
    Ok(())
}

fn websocket_accept(key: &str) -> String {
    let mut input = Vec::with_capacity(key.len() + WEBSOCKET_GUID.len());
    input.extend_from_slice(key.as_bytes());
    input.extend_from_slice(WEBSOCKET_GUID);
    base64_encode(&sha1(&input))
}

fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let a = chunk[0];
        let b = chunk.get(1).copied().unwrap_or(0);
        let c = chunk.get(2).copied().unwrap_or(0);
        output.push(TABLE[(a >> 2) as usize] as char);
        output.push(TABLE[(((a & 0x03) << 4) | (b >> 4)) as usize] as char);
        if chunk.len() > 1 {
            output.push(TABLE[(((b & 0x0f) << 2) | (c >> 6)) as usize] as char);
        } else {
            output.push('=');
        }
        if chunk.len() > 2 {
            output.push(TABLE[(c & 0x3f) as usize] as char);
        } else {
            output.push('=');
        }
    }
    output
}

fn sha1(input: &[u8]) -> [u8; 20] {
    let mut message = input.to_vec();
    let bit_len = (message.len() as u64) * 8;
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    let mut h0 = 0x6745_2301u32;
    let mut h1 = 0xefcd_ab89u32;
    let mut h2 = 0x98ba_dcfeu32;
    let mut h3 = 0x1032_5476u32;
    let mut h4 = 0xc3d2_e1f0u32;
    for chunk in message.chunks_exact(64) {
        let mut words = [0u32; 80];
        for (index, bytes) in chunk.chunks_exact(4).enumerate() {
            words[index] = u32::from_be_bytes(bytes.try_into().expect("four-byte chunk"));
        }
        for index in 16..80 {
            words[index] =
                (words[index - 3] ^ words[index - 8] ^ words[index - 14] ^ words[index - 16])
                    .rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h0, h1, h2, h3, h4);
        for (index, word) in words.iter().enumerate() {
            let (function, constant) = match index {
                0..=19 => ((b & c) | ((!b) & d), 0x5a82_7999),
                20..=39 => (b ^ c ^ d, 0x6ed9_eba1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1b_bcdc),
                _ => (b ^ c ^ d, 0xca62_c1d6),
            };
            let next = a
                .rotate_left(5)
                .wrapping_add(function)
                .wrapping_add(e)
                .wrapping_add(constant)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = next;
        }
        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }
    let mut digest = [0u8; 20];
    for (slot, value) in digest.chunks_exact_mut(4).zip([h0, h1, h2, h3, h4]) {
        slot.copy_from_slice(&value.to_be_bytes());
    }
    digest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_rfc_websocket_accept() {
        assert_eq!(
            websocket_accept("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn sha1_matches_known_vector() {
        assert_eq!(
            sha1(b"abc"),
            [
                0xa9, 0x99, 0x3e, 0x36, 0x47, 0x06, 0x81, 0x6a, 0xba, 0x3e, 0x25, 0x71, 0x78, 0x50,
                0xc2, 0x6c, 0x9c, 0xd0, 0xd8, 0x9d,
            ]
        );
    }
}

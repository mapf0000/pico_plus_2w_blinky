//! Versioned, bounded private parent/child IPC. No source or effects in argv.
use crate::StepResult;
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

pub const VERSION: u8 = 1;
// JSON escaping can expand a 32-KiB source sixfold.
pub const FRAME_MAX: usize = 256 * 1024;
pub const HEAP_LIMIT: usize = 128 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Input {
    Start { source: String },
    Resume { value: serde_json::Value },
    Raise { code: String, message: String },
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    pub version: u8,
    pub run: u64,
    pub step: u64,
    pub input: Input,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub version: u8,
    pub run: u64,
    pub step: u64,
    pub heap_peak: usize,
    pub result: StepResult,
}
pub fn encode<T: Serialize>(value: &T) -> io::Result<Vec<u8>> {
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    if bytes.is_empty() || bytes.len() > FRAME_MAX {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Python IPC frame exceeds its limit",
        ));
    }
    Ok(bytes)
}
pub fn read_frame(reader: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut header = [0; 4];
    reader.read_exact(&mut header)?;
    let n = u32::from_le_bytes(header) as usize;
    if n == 0 || n > FRAME_MAX {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid Python IPC length",
        ));
    }
    let mut bytes = vec![0; n];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}
pub fn write_frame(writer: &mut impl Write, value: &impl Serialize) -> io::Result<()> {
    let bytes = encode(value)?;
    writer.write_all(&(bytes.len() as u32).to_le_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_truncated_oversized_and_unknown_frames() {
        for n in [0, FRAME_MAX + 1, u32::MAX as usize] {
            assert!(read_frame(&mut &u32::try_from(n).unwrap().to_le_bytes()[..]).is_err());
        }
        let command = Command {
            version: 1,
            run: 1,
            step: 1,
            input: Input::Start {
                source: "é\n".into(),
            },
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &command).unwrap();
        for n in 0..bytes.len() {
            assert!(read_frame(&mut &bytes[..n]).is_err());
        }
        let parsed: Command =
            serde_json::from_slice(&read_frame(&mut &bytes[..]).unwrap()).unwrap();
        assert_eq!(parsed.run, 1);
        assert!(
            serde_json::from_str::<Command>(
                r#"{"version":1,"run":1,"step":1,"input":{"kind":"resume","value":null},"extra":0}"#
            )
            .is_err()
        );
    }
}

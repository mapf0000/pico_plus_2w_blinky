//! Bounded native download UI and command model; filenames are display metadata.
use std::{collections::VecDeque, path::PathBuf};
pub const MAX_QUEUE: usize = 4;
pub const MAX_HISTORY: usize = 8;
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
pub const COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
#[derive(Clone, Debug)]
pub enum Command {
    ConnectSession,
    Queue { path: String, destination: PathBuf },
    QueueDefault { destination: PathBuf },
    SetDefault(String),
    Cancel,
}
impl Command {
    pub fn valid(&self) -> bool {
        let path = |p: &str| !p.is_empty() && p.len() <= 512 && !p.contains('\0');
        let dest = |p: &PathBuf| {
            p.is_absolute()
                && p.as_os_str().len() <= 4096
                && p.file_name().is_some()
                && !p.as_os_str().as_encoded_bytes().contains(&0)
        };
        match self {
            Self::Queue {
                path: p,
                destination: d,
            } => path(p) && dest(d),
            Self::QueueDefault { destination: d } => dest(d),
            Self::SetDefault(p) => path(p),
            _ => true,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Session {
    #[default]
    Idle,
    Negotiating,
    Ready,
    Failed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Queued,
    Requested,
    Receiving,
    Verifying,
    Saved,
    Failed,
    Aborted,
}
#[derive(Clone, Debug)]
pub struct Item {
    pub id: u64,
    pub name: String,
    pub destination: PathBuf,
    pub state: State,
    pub received: u64,
    pub total: u64,
    pub bytes_per_second: f64,
    pub retries: u32,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Default)]
pub struct View {
    pub session: Session,
    pub items: VecDeque<Item>,
    pub error: Option<String>,
}
impl View {
    pub fn active(&self) -> bool {
        self.items.iter().any(|i| {
            matches!(
                i.state,
                State::Queued | State::Requested | State::Receiving | State::Verifying
            )
        })
    }
}

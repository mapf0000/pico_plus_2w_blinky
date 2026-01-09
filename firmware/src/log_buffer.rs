use core::cell::RefCell;
use core::fmt::Write as _;
use core::sync::atomic::{AtomicU32, Ordering};

use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use heapless::{Deque, String, Vec};
use log::Record;

pub const LOG_LINE_MAX: usize = 96;
pub const LOG_CAPACITY: usize = 32;

struct LogRing {
    lines: Deque<String<LOG_LINE_MAX>, LOG_CAPACITY>,
}

impl LogRing {
    const fn new() -> Self {
        Self {
            lines: Deque::new(),
        }
    }
}

static LOG_RING: Mutex<CriticalSectionRawMutex, RefCell<LogRing>> =
    Mutex::new(RefCell::new(LogRing::new()));
static LOG_GEN: AtomicU32 = AtomicU32::new(0);

fn push_line(line: String<LOG_LINE_MAX>) {
    LOG_RING.lock(|cell| {
        let mut ring = cell.borrow_mut();
        if ring.lines.is_full() {
            let _ = ring.lines.pop_front();
        }
        let _ = ring.lines.push_back(line);
    });
    let _ = LOG_GEN.fetch_add(1, Ordering::Relaxed);
}

pub fn push_record(record: &Record) {
    let mut line: String<LOG_LINE_MAX> = String::new();
    let _ = write!(line, "[{}] {}", record.level(), record.args());
    push_line(line);
}

pub fn generation() -> u32 {
    LOG_GEN.load(Ordering::Relaxed)
}

pub fn snapshot(out: &mut Vec<String<LOG_LINE_MAX>, LOG_CAPACITY>) {
    out.clear();
    LOG_RING.lock(|cell| {
        let ring = cell.borrow();
        for line in ring.lines.iter() {
            let _ = out.push(line.clone());
        }
    });
}

use core::sync::atomic::Ordering;

use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, channel::Channel};
use embassy_time::{Duration, Timer};
use heapless::{String, Vec};
use picoserve::{io::embedded_io_async, response::ws};
use portable_atomic::AtomicUsize;
#[cfg(not(feature = "psram"))]
use static_cell::StaticCell;
use transfer_protocol::{
    SECURE_CHUNK_BATCH_MAX_CHUNKS, SecureChunkBatchEncoder, WS_BINARY_KIND_SECURE_CHUNK,
    WS_BINARY_KIND_SECURE_CHUNK_BATCH,
};

pub const TRANSFER_TEXT_MAX: usize = 768;
pub const TRANSFER_BINARY_MAX: usize = 1 + transfer_protocol::TLV_MAX_PAYLOAD;
pub const WS_BINARY_KIND_FILESYSTEM: u8 = 2;

const INPUT_QUEUE_DEPTH: usize = 16;
const OUTPUT_QUEUE_DEPTH: usize = 1;
const BATCH_BINARY_MAX: usize = 1 + transfer_protocol::MAX_SECURE_CHUNK_BATCH_LEN;
const BATCH_COALESCE_DELAY: Duration = Duration::from_micros(500);

const _: () = assert!(TRANSFER_TEXT_MAX <= TRANSFER_BINARY_MAX);
const _: () = assert!(BATCH_BINARY_MAX <= u16::MAX as usize);
#[cfg(feature = "psram")]
const _: () = assert!(BATCH_BINARY_MAX == crate::psram_pool::TRANSFER_BATCH_SIZE);

type BatchBuffer = &'static mut [u8; BATCH_BINARY_MAX];

#[derive(Clone, Copy)]
enum EventKind {
    Text,
    Binary,
}

pub(super) struct TransferEvent {
    kind: EventKind,
    payload: Vec<u8, TRANSFER_BINARY_MAX>,
}

#[allow(
    clippy::large_enum_variant,
    reason = "the one-slot no_std queue owns either one bounded event or one batch-buffer lease; heap indirection is unavailable"
)]
pub(super) enum OutboundFrame {
    Event(TransferEvent),
    SecureChunkBatch { buffer: BatchBuffer, len: u16 },
}

static INPUT_EVENTS: Channel<ThreadModeRawMutex, TransferEvent, INPUT_QUEUE_DEPTH> = Channel::new();
static OUTPUT_FRAMES: Channel<ThreadModeRawMutex, OutboundFrame, OUTPUT_QUEUE_DEPTH> =
    Channel::new();
static RETURNED_BATCH_BUFFER: Channel<ThreadModeRawMutex, BatchBuffer, 1> = Channel::new();
static ACTIVE_WS_CLIENTS: AtomicUsize = AtomicUsize::new(0);

#[cfg(not(feature = "psram"))]
static SRAM_BATCH_BUFFER: StaticCell<[u8; BATCH_BINARY_MAX]> = StaticCell::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferQueueError {
    NoClient,
    Full,
}

pub fn spawn(spawner: &Spawner) -> bool {
    crate::log_spawn(spawner, "http::transfer_pump", transfer_pump_task())
}

pub fn has_active_client() -> bool {
    active_client_count() > 0
}

pub fn active_client_count() -> usize {
    ACTIVE_WS_CLIENTS.load(Ordering::Acquire)
}

pub fn queue_text(event: String<TRANSFER_TEXT_MAX>) -> Result<(), TransferQueueError> {
    let mut payload = Vec::new();
    payload
        .extend_from_slice(event.as_bytes())
        .expect("text capacity is bounded by the transfer-event payload capacity");
    try_enqueue(TransferEvent {
        kind: EventKind::Text,
        payload,
    })
}

pub async fn send_text(event: String<TRANSFER_TEXT_MAX>) -> Result<(), TransferQueueError> {
    let mut payload = Vec::new();
    payload
        .extend_from_slice(event.as_bytes())
        .expect("text capacity is bounded by the transfer-event payload capacity");
    enqueue(TransferEvent {
        kind: EventKind::Text,
        payload,
    })
    .await
}

pub fn queue_binary(event: Vec<u8, TRANSFER_BINARY_MAX>) -> Result<(), TransferQueueError> {
    try_enqueue(TransferEvent {
        kind: EventKind::Binary,
        payload: event,
    })
}

/// Enqueue transfer data without dropping it when USB temporarily outpaces Wi-Fi.
///
/// The USB control task awaits this function before acknowledging the chunk to
/// the host agent. The input channel, singleton batch slot, and one-frame output
/// channel form one bounded backpressure path through to the WebSocket writer.
pub async fn send_binary(event: Vec<u8, TRANSFER_BINARY_MAX>) -> Result<(), TransferQueueError> {
    enqueue(TransferEvent {
        kind: EventKind::Binary,
        payload: event,
    })
    .await
}

async fn enqueue(event: TransferEvent) -> Result<(), TransferQueueError> {
    if !has_active_client() {
        return Err(TransferQueueError::NoClient);
    }

    INPUT_EVENTS.send(event).await;

    // A disconnect drains the channel to wake blocked producers. Do not report
    // that wake-up as a successfully relayed chunk.
    if !has_active_client() {
        return Err(TransferQueueError::NoClient);
    }

    Ok(())
}

fn try_enqueue(event: TransferEvent) -> Result<(), TransferQueueError> {
    if !has_active_client() {
        return Err(TransferQueueError::NoClient);
    }

    INPUT_EVENTS
        .try_send(event)
        .map_err(|_| TransferQueueError::Full)
}

pub(super) struct SessionGuard;

impl Drop for SessionGuard {
    fn drop(&mut self) {
        end_session();
    }
}

pub(super) fn begin_session() -> SessionGuard {
    if ACTIVE_WS_CLIENTS.fetch_add(1, Ordering::AcqRel) == 0 {
        drain_queues();
    }
    SessionGuard
}

fn end_session() {
    let previous = ACTIVE_WS_CLIENTS.fetch_sub(1, Ordering::AcqRel);
    debug_assert!(previous > 0, "WebSocket session count underflow");
    if previous <= 1 {
        ACTIVE_WS_CLIENTS.store(0, Ordering::Release);
        drain_queues();
    }
}

fn drain_queues() {
    while INPUT_EVENTS.try_receive().is_ok() {}
    while let Ok(frame) = OUTPUT_FRAMES.try_receive() {
        if let OutboundFrame::SecureChunkBatch { buffer, .. } = frame {
            return_batch_buffer(buffer);
        }
    }
}

fn return_batch_buffer(buffer: BatchBuffer) {
    if RETURNED_BATCH_BUFFER.try_send(buffer).is_err() {
        // There is exactly one uniquely owned batch buffer. A full return
        // channel therefore indicates an internal ownership bug, not pressure
        // from external traffic.
        log::error!("transfer: duplicate batch-buffer return");
    }
}

struct BatchLease {
    buffer: Option<BatchBuffer>,
}

impl BatchLease {
    fn new(buffer: BatchBuffer) -> Self {
        Self {
            buffer: Some(buffer),
        }
    }

    fn bytes(&self, len: usize) -> &[u8] {
        &self
            .buffer
            .as_deref()
            .expect("batch lease owns its buffer until drop")[..len]
    }
}

impl Drop for BatchLease {
    fn drop(&mut self) {
        if let Some(buffer) = self.buffer.take() {
            return_batch_buffer(buffer);
        }
    }
}

pub(super) async fn receive_frame() -> OutboundFrame {
    OUTPUT_FRAMES.receive().await
}

pub(super) async fn send_frame<W: embedded_io_async::Write>(
    frame: OutboundFrame,
    tx: &mut ws::SocketTx<W>,
) -> Result<(), W::Error> {
    match frame {
        OutboundFrame::Event(event) => match event.kind {
            EventKind::Text => {
                let text = core::str::from_utf8(event.payload.as_slice())
                    .expect("text transfer events originate from UTF-8 strings");
                tx.send_text(text).await
            }
            EventKind::Binary => tx.send_binary(event.payload.as_slice()).await,
        },
        OutboundFrame::SecureChunkBatch { buffer, len } => {
            let lease = BatchLease::new(buffer);
            tx.send_binary(lease.bytes(usize::from(len))).await
        }
    }
}

#[embassy_executor::task]
async fn transfer_pump_task() -> ! {
    let mut pending = None;
    let mut batch_buffer = take_batch_buffer();

    loop {
        let event = match pending.take() {
            Some(event) => event,
            None => INPUT_EVENTS.receive().await,
        };

        if !has_active_client() {
            continue;
        }

        let Some(transfer_id) = secure_chunk_transfer_id(&event) else {
            forward_event(event).await;
            continue;
        };

        if batch_buffer.is_none() {
            forward_event(event).await;
            continue;
        }

        let next = match INPUT_EVENTS.try_receive() {
            Ok(next) => Some(next),
            Err(_) => {
                match select(INPUT_EVENTS.receive(), Timer::after(BATCH_COALESCE_DELAY)).await {
                    Either::First(next) => Some(next),
                    Either::Second(()) => None,
                }
            }
        };

        let Some(next) = next else {
            forward_event(event).await;
            continue;
        };

        if secure_chunk_transfer_id(&next) != Some(transfer_id) {
            forward_event(event).await;
            pending = Some(next);
            continue;
        }

        let buffer = batch_buffer
            .take()
            .expect("batching is attempted only while the singleton buffer is owned");
        let (returned_buffer, next_pending) =
            send_chunk_batch(buffer, event, next, transfer_id).await;
        batch_buffer = Some(returned_buffer);
        pending = next_pending;
    }
}

async fn forward_event(event: TransferEvent) {
    if !has_active_client() {
        return;
    }
    OUTPUT_FRAMES.send(OutboundFrame::Event(event)).await;
    if !has_active_client() {
        drain_queues();
    }
}

async fn send_chunk_batch(
    buffer: BatchBuffer,
    first: TransferEvent,
    second: TransferEvent,
    transfer_id: u64,
) -> (BatchBuffer, Option<TransferEvent>) {
    let (len, pending) = {
        buffer[0] = WS_BINARY_KIND_SECURE_CHUNK_BATCH;
        let mut encoder = SecureChunkBatchEncoder::new(&mut buffer[1..])
            .expect("the statically sized batch buffer always has a count byte");

        append_validated_chunk(&mut encoder, &first);
        append_validated_chunk(&mut encoder, &second);

        let mut pending = None;
        while encoder.chunk_count() < SECURE_CHUNK_BATCH_MAX_CHUNKS {
            let Ok(candidate) = INPUT_EVENTS.try_receive() else {
                break;
            };
            if secure_chunk_transfer_id(&candidate) != Some(transfer_id) {
                pending = Some(candidate);
                break;
            }
            append_validated_chunk(&mut encoder, &candidate);
        }

        let payload_len = encoder
            .finish()
            .expect("a secure chunk batch always contains at least two validated chunks");
        (1 + payload_len, pending)
    };

    if !has_active_client() {
        return (buffer, pending);
    }

    OUTPUT_FRAMES
        .send(OutboundFrame::SecureChunkBatch {
            buffer,
            len: len as u16,
        })
        .await;

    if !has_active_client() {
        drain_queues();
    }

    (RETURNED_BATCH_BUFFER.receive().await, pending)
}

#[cfg(feature = "psram")]
fn take_batch_buffer() -> Option<BatchBuffer> {
    let buffer = crate::psram_pool::take_transfer_batch_buffer();
    if buffer.is_some() {
        log::info!("transfer: singleton batch buffer ready in PSRAM");
    } else {
        log::warn!("transfer: PSRAM batch buffer unavailable; sending individual chunks");
    }
    buffer
}

#[cfg(not(feature = "psram"))]
fn take_batch_buffer() -> Option<BatchBuffer> {
    Some(SRAM_BATCH_BUFFER.init([0; BATCH_BINARY_MAX]))
}

fn append_validated_chunk(encoder: &mut SecureChunkBatchEncoder<'_>, event: &TransferEvent) {
    // `secure_chunk_transfer_id` validated the complete payload, and the batch
    // buffer is sized from the same shared protocol constants.
    encoder
        .push(&event.payload[1..])
        .expect("validated chunks always fit the statically bounded batch buffer");
}

fn secure_chunk_transfer_id(event: &TransferEvent) -> Option<u64> {
    if !matches!(event.kind, EventKind::Binary)
        || event.payload.first().copied() != Some(WS_BINARY_KIND_SECURE_CHUNK)
    {
        return None;
    }
    transfer_protocol::decode_secure_chunk(&event.payload[1..])
        .ok()
        .map(|chunk| chunk.transfer_id)
}

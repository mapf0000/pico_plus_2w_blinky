//! One owned transfer record, held through the native receiver's final ACK.
use ble_protocol::{download, stream};
use heapless::Vec;
pub struct Bridge {
    pub owner: u32,
    enabled: bool,
    next: u64,
    pending: Option<u64>,
    delivered: u64,
    bytes: Vec<u8, { download::RECORD_MAX }>,
    pub transfer_id: u64,
}
impl Default for Bridge {
    fn default() -> Self {
        Self::new()
    }
}
impl Bridge {
    pub const fn new() -> Self {
        Self {
            owner: 0,
            enabled: false,
            next: 0,
            pending: None,
            delivered: 0,
            bytes: Vec::new(),
            transfer_id: 0,
        }
    }
    pub fn activate(&mut self, owner: u32) {
        self.reset();
        self.owner = owner;
    }
    pub fn enable(&mut self, owner: u32) -> bool {
        if owner == 0 || owner != self.owner {
            return false;
        }
        self.enabled = true;
        true
    }
    pub fn reset(&mut self) -> u64 {
        let id = self.transfer_id;
        self.owner = 0;
        self.enabled = false;
        self.pending = None;
        self.bytes.clear();
        self.transfer_id = 0;
        id
    }
    pub fn cancel(&mut self, owner: u32) -> Option<u64> {
        if owner == 0 || owner != self.owner {
            return None;
        }
        let id = self.transfer_id;
        self.enabled = false;
        self.pending = None;
        self.bytes.clear();
        // No ticket: the old in-flight record's ACK must not acknowledge this barrier.
        let _ = self.bytes.push(download::CANCELLED);
        self.transfer_id = 0;
        Some(id)
    }
    pub fn active(&self) -> bool {
        self.owner != 0 && self.enabled
    }
    pub fn enqueue(&mut self, tag: u8, body: &[u8]) -> Option<(u32, u64)> {
        if !self.active() || self.pending.is_some() {
            return None;
        }
        self.bytes.push(tag).ok()?;
        if self.bytes.extend_from_slice(body).is_err() || download::record(&self.bytes).is_err() {
            self.bytes.clear();
            return None;
        }
        let Some(next) = self.next.checked_add(1) else {
            self.bytes.clear();
            return None;
        };
        self.next = next;
        self.pending = Some(next);
        if tag == download::OPEN {
            self.transfer_id = transfer_protocol::decode_secure_open(body)
                .ok()?
                .transfer_id;
        }
        Some((self.owner, next))
    }
    pub fn take(&mut self, out: &mut [u8; stream::PLAINTEXT_MAX]) -> Option<usize> {
        if self.bytes.is_empty() {
            return None;
        }
        let n = stream::encode(stream::DOWNLOAD, &self.bytes, out).ok()?;
        self.bytes.clear();
        Some(n)
    }
    pub fn ack(&mut self) {
        if let Some(ticket) = self.pending.take() {
            self.delivered = ticket;
        }
    }
    pub fn status(&self, owner: u32, ticket: u64) -> Option<bool> {
        if self.owner != owner || !self.enabled {
            Some(false)
        } else if self.delivered >= ticket {
            Some(true)
        } else if self.pending == Some(ticket) {
            None
        } else {
            Some(false)
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credit_is_held_until_ack_and_old_owners_fail_closed() {
        let mut b = Bridge::new();
        let mut frame = [0; 64];
        let n = transfer_protocol::encode_session_envelope(&mut frame, 1, &[0; 16], &[]).unwrap();
        assert!(b.enqueue(download::SESSION, &frame[..n]).is_none());
        b.activate(1);
        assert!(b.enable(1));
        let (owner, ticket) = b.enqueue(download::SESSION, &frame[..n]).unwrap();
        assert!(b.enqueue(download::SESSION, &frame[..n]).is_none());
        let mut out = [0; stream::PLAINTEXT_MAX];
        assert!(b.take(&mut out).is_some());
        assert_eq!(b.status(owner, ticket), None);
        assert!(b.enqueue(download::SESSION, &frame[..n]).is_none());
        b.ack();
        assert_eq!(b.status(owner, ticket), Some(true));
        let (_, next) = b.enqueue(download::SESSION, &frame[..n]).unwrap();
        assert!(next > ticket);
        b.cancel(owner);
        assert_eq!(b.status(owner, next), Some(false));
        b.ack(); // Late ACK for the cancelled record cannot consume the barrier.
        let n = b.take(&mut out).unwrap();
        assert_eq!(
            stream::decode(&out[..n]).unwrap(),
            (stream::DOWNLOAD, &[download::CANCELLED][..])
        );
        assert!(!b.active());
        b.activate(2);
        b.enable(2);
        assert_eq!(b.status(owner, next), Some(false));
    }
}

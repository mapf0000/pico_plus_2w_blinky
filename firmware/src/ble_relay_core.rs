//! Hardware-independent, bounded ownership and queue policy for BLE events.
use ble_protocol::{filesystem as fs, stream};
use heapless::{Deque, Vec};
pub struct Relay {
    owner: u32,
    high_water: [u8; 2],
    active: Option<(u64, u64)>,
    delivered: bool,
    page: Vec<u8, { fs::PAGE_MAX }>,
    buttons: Deque<u8, 8>,
}
impl Default for Relay {
    fn default() -> Self {
        Self::new()
    }
}
impl Relay {
    pub const fn new() -> Self {
        Self {
            owner: 0,
            high_water: [0; 2],
            active: None,
            delivered: false,
            page: Vec::new(),
            buttons: Deque::new(),
        }
    }
    pub fn activate(&mut self, owner: u32) {
        self.owner = owner;
    }
    pub fn reset(&mut self) -> Option<u64> {
        let old = self.active.take().map(|(id, _)| id);
        self.owner = 0;
        self.high_water = [0; 2];
        self.page.clear();
        self.buttons.clear();
        self.delivered = false;
        old
    }
    pub fn reserve(
        &mut self,
        owner: u32,
        token: u16,
        id: u64,
    ) -> Result<(u64, Option<u64>), ble_protocol::Error> {
        if owner == 0 || owner != self.owner || token == 0 || id == 0 {
            return Err(ble_protocol::Error::Invalid);
        }
        let usb_id = (u64::from(owner) << 32) | u64::from(token);
        let old = self.active.replace((usb_id, id)).map(|(id, _)| id);
        self.delivered = false;
        self.page.clear();
        Ok((usb_id, old))
    }
    pub fn cancel(&mut self, owner: u32, id: u64) -> Option<u64> {
        if self.owner != owner || !self.active.is_some_and(|(_, client_id)| client_id == id) {
            return None;
        }
        self.page.clear();
        self.delivered = false;
        self.active.take().map(|(id, _)| id)
    }
    pub fn page(&mut self, bytes: &[u8]) -> bool {
        let Ok(page) = fs::Page::decode(bytes) else {
            return false;
        };
        let Some((usb_id, client_id)) = self.active else {
            return false;
        };
        if self.owner == 0 || page.id != usb_id || self.delivered || !self.page.is_empty() {
            return false;
        }
        if self.page.extend_from_slice(bytes).is_err() {
            return false;
        }
        self.page[2..10].copy_from_slice(&client_id.to_le_bytes());
        self.delivered = true;
        self.high_water[1] = 1;
        true
    }
    pub fn button(&mut self, button: u8) {
        if self.owner != 0 && button < 4 {
            let _ = self.buttons.push_back(button);
            self.high_water[0] = self.high_water[0].max(self.buttons.len() as u8);
        }
    }
    pub fn high_water(&self) -> [u8; 2] {
        self.high_water
    }
    pub fn take(&mut self, out: &mut [u8; stream::PLAINTEXT_MAX]) -> Option<usize> {
        if !self.page.is_empty() {
            let n = stream::encode(stream::FILESYSTEM, &self.page, out).ok();
            self.page.clear();
            return n;
        }
        let button = self.buttons.pop_front()?;
        stream::encode(stream::BUTTON, &[button], out).ok()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn page(id: u64) -> [u8; 23] {
        let mut b = [0; 23];
        b[..2].copy_from_slice(&1u16.to_le_bytes());
        b[2..10].copy_from_slice(&id.to_le_bytes());
        b[16] = 1;
        b[18] = b'/';
        b
    }
    #[test]
    fn ownership_remap_supersede_and_reconnect_reject_stale_usb() {
        let mut r = Relay::new();
        let mut out = [0; stream::PLAINTEXT_MAX];
        r.button(0);
        assert!(r.take(&mut out).is_none());
        r.activate(1);
        let (a, _) = r.reserve(1, 1, 7).unwrap();
        let (b, old) = r.reserve(1, 2, 8).unwrap();
        assert_eq!(old, Some(a));
        assert!(!r.page(&page(a)));
        assert!(r.page(&page(b)));
        assert!(!r.page(&page(b)));
        let n = r.take(&mut out).unwrap();
        let (_, body) = stream::decode(&out[..n]).unwrap();
        assert_eq!(fs::Page::decode(body).unwrap().id, 8);
        assert!(!r.page(&page(b)));
        assert_eq!(r.reset(), Some(b));
        r.activate(2);
        let (c, _) = r.reserve(2, 1, 7).unwrap();
        assert_ne!(a, c);
        assert!(!r.page(&page(a)));
        assert!(r.page(&page(c)));
        assert_eq!(r.cancel(1, 7), None);
        assert_eq!(r.cancel(2, 7), Some(c));
        assert!(r.take(&mut out).is_none());
    }
    #[test]
    fn queues_saturate_without_overwriting_page_or_blocking_buttons() {
        let mut r = Relay::new();
        r.activate(1);
        let (id, _) = r.reserve(1, 1, 1).unwrap();
        for _ in 0..20 {
            r.button(1);
        }
        assert!(r.page(&page(id)));
        assert!(!r.page(&page(id)));
        let mut out = [0; stream::PLAINTEXT_MAX];
        let n = r.take(&mut out).unwrap();
        assert_eq!(stream::decode(&out[..n]).unwrap().0, stream::FILESYSTEM);
        for _ in 0..8 {
            let n = r.take(&mut out).unwrap();
            assert_eq!(
                stream::decode(&out[..n]).unwrap(),
                (stream::BUTTON, &[1][..])
            );
        }
        assert!(r.take(&mut out).is_none());
    }
}

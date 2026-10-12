//! Account Rust allocations from process startup. The limit is armed only in
//! --python-worker; the UI process retains the ordinary System allocator limit.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    ptr,
    sync::atomic::{AtomicUsize, Ordering},
};
pub struct BudgetAllocator;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static LIMIT: AtomicUsize = AtomicUsize::new(usize::MAX);
pub fn arm() {
    LIMIT.store(python_runtime::ipc::HEAP_LIMIT, Ordering::SeqCst);
}
pub fn peak() -> usize {
    PEAK.load(Ordering::Relaxed)
}
fn reserve(n: usize) -> bool {
    let result = LIVE.try_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
        live.checked_add(n)
            .filter(|next| *next <= LIMIT.load(Ordering::Relaxed))
    });
    if let Ok(old) = result {
        PEAK.fetch_max(old + n, Ordering::Relaxed);
        true
    } else {
        false
    }
}
fn release(n: usize) {
    LIVE.fetch_sub(n, Ordering::Relaxed);
}
// SAFETY: Every successful allocation is forwarded unchanged to System and
// accounted exactly once. Failure rolls back reservations; realloc failure
// retains its original allocation. These methods never allocate, lock or unwind.
unsafe impl GlobalAlloc for BudgetAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !reserve(layout.size()) {
            return ptr::null_mut();
        }
        // SAFETY: the caller supplies a valid nonzero Layout.
        let p = unsafe { System.alloc(layout) };
        if p.is_null() {
            release(layout.size());
        }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if !reserve(layout.size()) {
            return ptr::null_mut();
        }
        // SAFETY: same allocation contract as alloc.
        let p = unsafe { System.alloc_zeroed(layout) };
        if p.is_null() {
            release(layout.size());
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        // SAFETY: caller supplies the live allocation and its original Layout.
        unsafe { System.dealloc(p, layout) };
        release(layout.size());
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, n: usize) -> *mut u8 {
        let growth = n.saturating_sub(layout.size());
        if !reserve(growth) {
            return ptr::null_mut();
        }
        // SAFETY: caller supplies the live allocation and valid new size.
        let out = unsafe { System.realloc(p, layout, n) };
        if out.is_null() {
            release(growth);
        } else if n < layout.size() {
            release(layout.size() - n);
        }
        out
    }
}

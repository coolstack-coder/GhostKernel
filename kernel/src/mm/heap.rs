//! Kernel heap: a fixed run of physical memory right behind the kernel
//! image, handed to a linked-list allocator. Checkpoint images and the RAM
//! filesystem live here.

use linked_list_allocator::LockedHeap;

pub const HEAP_SIZE: usize = 40 * 1024 * 1024;

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

/// `start` must be identity mapped, page aligned and unused.
pub fn init(start: u64) -> u64 {
    unsafe { ALLOCATOR.lock().init(start as *mut u8, HEAP_SIZE) };
    start + HEAP_SIZE as u64
}

/// (used, total) bytes
pub fn stats() -> (usize, usize) {
    let h = ALLOCATOR.lock();
    (h.used(), h.size())
}

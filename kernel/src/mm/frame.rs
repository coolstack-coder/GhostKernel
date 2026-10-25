//! Physical page frames. Free frames form an intrusive singly linked list:
//! each free page stores the address of the next one in its first 8 bytes.
//! Everything below 1 GiB is identity mapped, so a frame's physical address
//! is also a pointer we can write through.

use super::PAGE_SIZE;
use spin::Mutex;

/// We only hand out frames we can reach through the identity map.
const MAX_PHYS: u64 = 1 << 30;

struct FrameList {
    head: u64,
    free: usize,
    total: usize,
}

static FRAMES: Mutex<FrameList> = Mutex::new(FrameList {
    head: 0,
    free: 0,
    total: 0,
});

pub fn init(info: u64, reserved_end: u64) {
    let mut list = FRAMES.lock();
    super::multiboot::for_each_usable(info, |r| {
        let start = super::page_ceil(r.base.max(reserved_end));
        let end = super::page_floor((r.base + r.len).min(MAX_PHYS));
        let mut f = start;
        while f + PAGE_SIZE <= end {
            unsafe { *(f as *mut u64) = list.head };
            list.head = f;
            list.free += 1;
            list.total += 1;
            f += PAGE_SIZE;
        }
    });
}

/// A zeroed 4 KiB frame, or None when RAM is gone.
pub fn alloc() -> Option<u64> {
    let mut list = FRAMES.lock();
    if list.head == 0 {
        return None;
    }
    let f = list.head;
    list.head = unsafe { *(f as *const u64) };
    list.free -= 1;
    drop(list);
    unsafe { core::ptr::write_bytes(f as *mut u8, 0, PAGE_SIZE as usize) };
    Some(f)
}

pub fn free(f: u64) {
    debug_assert!(f.is_multiple_of(PAGE_SIZE) && f < MAX_PHYS);
    let mut list = FRAMES.lock();
    unsafe { *(f as *mut u64) = list.head };
    list.head = f;
    list.free += 1;
}

/// (free, total) in frames
pub fn stats() -> (usize, usize) {
    let l = FRAMES.lock();
    (l.free, l.total)
}

pub fn as_slice(f: u64) -> &'static mut [u8] {
    unsafe { core::slice::from_raw_parts_mut(f as *mut u8, PAGE_SIZE as usize) }
}

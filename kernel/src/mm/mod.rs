pub mod frame;
pub mod heap;
pub mod multiboot;
pub mod paging;

pub const PAGE_SIZE: u64 = 4096;

pub const fn page_floor(a: u64) -> u64 {
    a & !(PAGE_SIZE - 1)
}

pub const fn page_ceil(a: u64) -> u64 {
    (a + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

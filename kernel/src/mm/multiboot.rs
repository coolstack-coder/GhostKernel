//! Multiboot v1 info: all we want from it is the physical memory map.

#[repr(C, packed)]
struct Info {
    flags: u32,
    mem_lower: u32,
    mem_upper: u32,
    boot_device: u32,
    cmdline: u32,
    mods_count: u32,
    mods_addr: u32,
    syms: [u32; 4],
    mmap_length: u32,
    mmap_addr: u32,
}

#[repr(C, packed)]
struct MmapEntry {
    size: u32,
    base: u64,
    len: u64,
    kind: u32,
}

pub const MAGIC: u32 = 0x2BADB002;

#[derive(Clone, Copy)]
pub struct Region {
    pub base: u64,
    pub len: u64,
}

/// Calls `f` for every usable RAM region.
pub fn for_each_usable(info_addr: u64, mut f: impl FnMut(Region)) {
    let info = unsafe { &*(info_addr as *const Info) };
    let flags = info.flags;
    assert!(flags & (1 << 6) != 0, "bootloader gave us no memory map");
    let mut p = info.mmap_addr as u64;
    let end = p + info.mmap_length as u64;
    while p < end {
        let e = unsafe { &*(p as *const MmapEntry) };
        if e.kind == 1 {
            f(Region {
                base: e.base,
                len: e.len,
            });
        }
        p += e.size as u64 + 4;
    }
}

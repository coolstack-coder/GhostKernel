//! Minimal ELF64 loader: PT_LOAD segments only, which is all a statically
//! linked user program has.

use crate::mm::paging::{AddressSpace, USER_BASE, USER_TOP};
use crate::mm::{PAGE_SIZE, frame, page_ceil, page_floor};
use alloc::collections::BTreeMap;

#[derive(Debug)]
pub enum ElfError {
    BadMagic,
    Unsupported,
    Truncated,
    OutOfRange,
    NoMemory,
}

fn u16_at(b: &[u8], o: usize) -> Result<u16, ElfError> {
    b.get(o..o + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or(ElfError::Truncated)
}
fn u32_at(b: &[u8], o: usize) -> Result<u32, ElfError> {
    b.get(o..o + 4)
        .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
        .ok_or(ElfError::Truncated)
}
fn u64_at(b: &[u8], o: usize) -> Result<u64, ElfError> {
    b.get(o..o + 8)
        .map(|s| u64::from_le_bytes(s.try_into().unwrap()))
        .ok_or(ElfError::Truncated)
}

struct Segment {
    offset: u64,
    vaddr: u64,
    filesz: u64,
    memsz: u64,
    writable: bool,
}

pub fn load(img: &[u8]) -> Result<(AddressSpace, u64), ElfError> {
    if img.get(0..4) != Some(b"\x7fELF") {
        return Err(ElfError::BadMagic);
    }
    // 64-bit, little endian, x86-64, executable
    if img[4] != 2 || img[5] != 1 || u16_at(img, 18)? != 0x3E || u16_at(img, 16)? != 2 {
        return Err(ElfError::Unsupported);
    }
    let entry = u64_at(img, 24)?;
    let phoff = u64_at(img, 32)? as usize;
    let phentsize = u16_at(img, 54)? as usize;
    let phnum = u16_at(img, 56)? as usize;

    let mut segs = alloc::vec::Vec::new();
    for i in 0..phnum {
        let ph = phoff + i * phentsize;
        if u32_at(img, ph)? != 1 {
            continue; // not PT_LOAD
        }
        let s = Segment {
            writable: u32_at(img, ph + 4)? & 2 != 0,
            offset: u64_at(img, ph + 8)?,
            vaddr: u64_at(img, ph + 16)?,
            filesz: u64_at(img, ph + 32)?,
            memsz: u64_at(img, ph + 40)?,
        };
        if s.vaddr < USER_BASE || s.vaddr + s.memsz > USER_TOP || s.filesz > s.memsz {
            return Err(ElfError::OutOfRange);
        }
        if (s.offset + s.filesz) as usize > img.len() {
            return Err(ElfError::Truncated);
        }
        segs.push(s);
    }
    if !(USER_BASE..USER_TOP).contains(&entry) {
        return Err(ElfError::OutOfRange);
    }

    // a page shared by two segments is writable if either one is
    let mut pages: BTreeMap<u64, bool> = BTreeMap::new();
    for s in &segs {
        let mut p = page_floor(s.vaddr);
        while p < page_ceil(s.vaddr + s.memsz) {
            *pages.entry(p).or_insert(false) |= s.writable;
            p += PAGE_SIZE;
        }
    }

    let mut space = AddressSpace::new().ok_or(ElfError::NoMemory)?;
    for (&va, &w) in &pages {
        space.map_new(va, w).ok_or(ElfError::NoMemory)?;
    }
    // copy file bytes through the identity map (frames are zeroed, so .bss is free)
    for s in &segs {
        let mut done = 0u64;
        while done < s.filesz {
            let va = s.vaddr + done;
            let (f, _) = space.translate(va).unwrap();
            let in_page = (va % PAGE_SIZE) as usize;
            let n = ((PAGE_SIZE as usize - in_page) as u64).min(s.filesz - done) as usize;
            let src = (s.offset + done) as usize;
            frame::as_slice(f)[in_page..in_page + n].copy_from_slice(&img[src..src + n]);
            done += n as u64;
        }
    }
    Ok((space, entry))
}

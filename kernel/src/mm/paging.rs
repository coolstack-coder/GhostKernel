//! Per-process address spaces.
//!
//! Every process gets its own PML4 + PDPT. Two PDPT slots are borrowed from
//! the boot tables (kernel identity map and the MMIO window) without the
//! USER bit, so ring 3 can't touch them. Slot 1 - virtual 1..2 GiB - is the
//! process's own. All user pages are 4 KiB so we can track their dirty bits
//! individually, which is what makes incremental checkpoints possible.

use super::{PAGE_SIZE, frame};
use crate::arch::cpu;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

pub const USER_BASE: u64 = 0x4000_0000;
pub const USER_TOP: u64 = 0x8000_0000;

pub const PTE_PRESENT: u64 = 1 << 0;
pub const PTE_WRITE: u64 = 1 << 1;
pub const PTE_USER: u64 = 1 << 2;
pub const PTE_DIRTY: u64 = 1 << 6;
const ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

unsafe extern "C" {
    static boot_pml4: u8;
    static boot_pdpt: u8;
}

pub fn kernel_pml4() -> u64 {
    (&raw const boot_pml4) as u64
}

fn table(phys: u64) -> &'static mut [u64; 512] {
    unsafe { &mut *(phys as *mut [u64; 512]) }
}

pub struct AddressSpace {
    pml4: u64,
    pdpt: u64,
    pd: u64,
    /// user virtual page -> backing frame
    pages: BTreeMap<u64, u64>,
}

impl AddressSpace {
    pub fn new() -> Option<Self> {
        let pml4 = frame::alloc()?;
        let pdpt = frame::alloc()?;
        let pd = frame::alloc()?;
        let boot = table((&raw const boot_pdpt) as u64);
        let t = table(pdpt);
        t[0] = boot[0];
        t[1] = pd | PTE_PRESENT | PTE_WRITE | PTE_USER;
        t[3] = boot[3];
        table(pml4)[0] = pdpt | PTE_PRESENT | PTE_WRITE | PTE_USER;
        Some(AddressSpace {
            pml4,
            pdpt,
            pd,
            pages: BTreeMap::new(),
        })
    }

    pub fn activate(&self) {
        if cpu::read_cr3() != self.pml4 {
            unsafe { cpu::write_cr3(self.pml4) };
        }
    }

    fn pte(&self, va: u64, create: bool) -> Option<&'static mut u64> {
        if !(USER_BASE..USER_TOP).contains(&va) {
            return None;
        }
        let pd = table(self.pd);
        let pdi = ((va >> 21) & 511) as usize;
        if pd[pdi] & PTE_PRESENT == 0 {
            if !create {
                return None;
            }
            let pt = frame::alloc()?;
            pd[pdi] = pt | PTE_PRESENT | PTE_WRITE | PTE_USER;
        }
        let pt = table(pd[pdi] & ADDR_MASK);
        Some(&mut pt[((va >> 12) & 511) as usize])
    }

    /// Map one page. `frame` must be owned by nobody else - the address
    /// space frees it on unmap/drop.
    pub fn map(&mut self, va: u64, frame: u64, writable: bool) -> Option<()> {
        let va = super::page_floor(va);
        let e = self.pte(va, true)?;
        assert!(*e & PTE_PRESENT == 0, "double map at {va:#x}");
        *e = frame | PTE_PRESENT | PTE_USER | if writable { PTE_WRITE } else { 0 };
        self.pages.insert(va, frame);
        Some(())
    }

    /// Allocate + map a zeroed page, returning the frame for the caller to fill.
    pub fn map_new(&mut self, va: u64, writable: bool) -> Option<u64> {
        let f = frame::alloc()?;
        if self.map(va, f, writable).is_none() {
            frame::free(f);
            return None;
        }
        Some(f)
    }

    pub fn translate(&self, va: u64) -> Option<(u64, u64)> {
        let e = self.pte(super::page_floor(va), false)?;
        (*e & PTE_PRESENT != 0).then_some((*e & ADDR_MASK, *e & 0xFFF))
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Every mapped page with its PTE flags, in address order.
    pub fn pages(&self) -> impl Iterator<Item = (u64, u64, u64)> + '_ {
        self.pages.iter().map(|(&va, &f)| {
            let flags = self.pte(va, false).map(|e| *e & 0xFFF).unwrap_or(0);
            (va, f, flags)
        })
    }

    /// Pages the CPU has written since the last call, and reset their dirty
    /// bits. The TLB caches the D bit, so it has to be flushed or the CPU
    /// would never set it again for pages that are already cached as dirty.
    pub fn take_dirty(&mut self) -> Vec<u64> {
        let mut dirty = Vec::new();
        for &va in self.pages.keys() {
            if let Some(e) = self.pte(va, false)
                && *e & PTE_DIRTY != 0
            {
                *e &= !PTE_DIRTY;
                dirty.push(va);
            }
        }
        if !dirty.is_empty() && cpu::read_cr3() == self.pml4 {
            unsafe { cpu::write_cr3(self.pml4) };
        }
        dirty
    }

    /// Just clear all dirty bits (after a full checkpoint).
    pub fn clear_dirty(&mut self) {
        let _ = self.take_dirty();
    }

    /// Is [va, va+len) entirely mapped (and writable, if asked)?
    pub fn check_range(&self, va: u64, len: u64, write: bool) -> bool {
        if len == 0 {
            return true;
        }
        let Some(end) = va.checked_add(len) else {
            return false;
        };
        if va < USER_BASE || end > USER_TOP {
            return false;
        }
        let mut p = super::page_floor(va);
        while p < end {
            match self.translate(p) {
                Some((_, fl)) if !write || fl & PTE_WRITE != 0 => {}
                _ => return false,
            }
            p += PAGE_SIZE;
        }
        true
    }
}

impl Drop for AddressSpace {
    fn drop(&mut self) {
        if cpu::read_cr3() == self.pml4 {
            unsafe { cpu::write_cr3(kernel_pml4()) };
        }
        for (_, f) in core::mem::take(&mut self.pages) {
            frame::free(f);
        }
        for e in table(self.pd).iter() {
            if e & PTE_PRESENT != 0 {
                frame::free(e & ADDR_MASK);
            }
        }
        frame::free(self.pd);
        frame::free(self.pdpt);
        frame::free(self.pml4);
    }
}

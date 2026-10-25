//! On-"disk" checkpoint format. Images live in the RAM filesystem under
//! /ckpt, one file per generation:
//!
//!   magic "GHOSTCK1"
//!   pid, seq, parent_seq, tick, flags            5 x u64
//!   n_mapped, n_data, n_fds                      3 x u64
//!   trap frame                                   176 bytes
//!   mapping:  n_mapped x (vaddr u64, pte_flags u64)
//!   pages:    n_data   x (vaddr u64, 4096 bytes)
//!   fds:      n_fds    x (slot, ino, offset, flags)  4 x u64
//!
//! A *full* image carries every page. A *delta* carries only the pages
//! whose dirty bit was set since the previous generation, but still lists
//! the complete mapping, so restore knows which pages exist and walks back
//! through older images for the rest.

use crate::arch::trap::TrapFrame;
use crate::mm::PAGE_SIZE;
use crate::proc::FileHandle;
use alloc::vec::Vec;

pub const MAGIC: &[u8; 8] = b"GHOSTCK1";
pub const FLAG_FULL: u64 = 1;
pub const FLAG_SYNC: u64 = 2;
const HEADER: usize = 8 + 8 * 8 + core::mem::size_of::<TrapFrame>();

pub struct Writer {
    pub buf: Vec<u8>,
}

impl Writer {
    pub fn new(cap: usize) -> Self {
        Writer {
            buf: Vec::with_capacity(cap),
        }
    }
    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }
}

pub struct Header {
    pub pid: u64,
    pub seq: u64,
    pub parent: u64,
    pub flags: u64,
    pub frame: TrapFrame,
}

/// A parsed view over an image's bytes; nothing is copied.
pub struct Image<'a> {
    pub header: Header,
    raw: &'a [u8],
    n_mapped: usize,
    n_data: usize,
    n_fds: usize,
}

fn rd(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

impl<'a> Image<'a> {
    pub fn parse(raw: &'a [u8]) -> Option<Self> {
        if raw.len() < HEADER || &raw[..8] != MAGIC {
            return None;
        }
        let f = |i: usize| rd(raw, 8 + i * 8);
        let header = Header {
            pid: f(0),
            seq: f(1),
            parent: f(2),
            flags: f(4),
            frame: TrapFrame::from_bytes(&raw[8 + 64..]),
        };
        let (n_mapped, n_data, n_fds) = (f(5) as usize, f(6) as usize, f(7) as usize);
        let need = HEADER + n_mapped * 16 + n_data * (8 + PAGE_SIZE as usize) + n_fds * 32;
        if raw.len() < need {
            return None;
        }
        Some(Image {
            header,
            raw,
            n_mapped,
            n_data,
            n_fds,
        })
    }

    pub fn mapping(&self) -> impl Iterator<Item = (u64, u64)> + '_ {
        (0..self.n_mapped).map(move |i| {
            let o = HEADER + i * 16;
            (rd(self.raw, o), rd(self.raw, o + 8))
        })
    }

    pub fn pages(&self) -> impl Iterator<Item = (u64, &'a [u8])> + '_ {
        let base = HEADER + self.n_mapped * 16;
        let raw = self.raw;
        (0..self.n_data).map(move |i| {
            let o = base + i * (8 + PAGE_SIZE as usize);
            (rd(raw, o), &raw[o + 8..o + 8 + PAGE_SIZE as usize])
        })
    }

    pub fn fds(&self) -> impl Iterator<Item = (usize, FileHandle)> + '_ {
        let base = HEADER + self.n_mapped * 16 + self.n_data * (8 + PAGE_SIZE as usize);
        (0..self.n_fds).map(move |i| {
            let o = base + i * 32;
            (
                rd(self.raw, o) as usize,
                FileHandle {
                    ino: rd(self.raw, o + 8) as usize,
                    offset: rd(self.raw, o + 16) as usize,
                    flags: rd(self.raw, o + 24) as u32,
                },
            )
        })
    }
}

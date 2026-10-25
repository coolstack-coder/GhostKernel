//! The file-side half of a checkpoint.
//!
//! Memory is easy to roll back - we just reload the pages. Files are not:
//! if `primes` appended 40 lines after its last checkpoint and then
//! crashed, restoring only its memory would make it append those 40 lines
//! a second time. So every write a protected process makes is recorded
//! here with the bytes it overwrote, and a restore replays the log
//! backwards. Each checkpoint starts a fresh log.

use super::ramfs::{Ino, RamFs};
use alloc::vec::Vec;

#[derive(Debug, Clone)]
pub enum UndoRecord {
    Write {
        ino: Ino,
        off: usize,
        old_bytes: Vec<u8>,
        old_size: usize,
    },
    Created {
        ino: Ino,
    },
}

#[derive(Default)]
pub struct UndoLog {
    records: Vec<UndoRecord>,
}

impl UndoLog {
    pub fn push(&mut self, r: UndoRecord) {
        self.records.push(r);
    }

    /// Undo everything since the last checkpoint. Returns bytes rolled back.
    pub fn rollback(&mut self, fs: &mut RamFs) -> usize {
        let mut bytes = 0;
        while let Some(r) = self.records.pop() {
            match r {
                UndoRecord::Write {
                    ino,
                    off,
                    old_bytes,
                    old_size,
                } => {
                    bytes += fs.size(ino).saturating_sub(old_size) + old_bytes.len();
                    fs.rollback_write(ino, off, &old_bytes, old_size);
                }
                UndoRecord::Created { ino } => {
                    let _ = fs.remove(ino);
                }
            }
        }
        bytes
    }
}

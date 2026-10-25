//! The ghost engine: checkpoint and resurrect user processes.
//!
//! * `checkpoint` freezes a process's registers, memory and open files
//!   into an image under /ckpt. After the first full image, only pages the
//!   CPU marked dirty are copied.
//! * `recover` runs when a protected process takes a fault in ring 3. It
//!   rebuilds a brand-new address space from the image chain, rolls its
//!   file writes back, and puts it back on the run queue - mid-computation.
//!
//! If the same restore point keeps crashing, we fall back one generation
//! at a time, and give up after a few tries rather than looping forever.

pub mod image;
pub mod inject;

use crate::arch::pit;
use crate::arch::trap::{EXCEPTION_NAMES, EXCEPTION_SHORT, TrapFrame};
use crate::fs::RamFs;
use crate::fs::undo::UndoLog;
use crate::mm::paging::{AddressSpace, PTE_WRITE};
use crate::mm::{PAGE_SIZE, frame};
use crate::proc::{MAX_FDS, Process};
use crate::time;
use crate::ui::state::{BannerKind, banner};
use crate::ui::{self, Ev};
use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use image::{FLAG_FULL, FLAG_SYNC, Image, Writer};

/// A delta chain longer than this gets a fresh full image.
const FULL_EVERY: usize = 6;
/// How many recent generations stay restorable.
const KEEP: usize = 3;
/// Consecutive failed restores before we declare the process lost.
const MAX_STREAK: u32 = 3;
/// More crashes than this inside CRASH_WINDOW ticks means a crash loop,
/// even if the process keeps managing to take fresh checkpoints in between.
const CRASH_BURST: usize = 4;
const CRASH_WINDOW: u64 = 10 * pit::HZ;
/// Bit set in `checkpoint()`'s return value when we came back from the dead.
pub const RESTORED_BIT: u64 = 1 << 62;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// the process asked for it (`checkpoint()` syscall)
    Sync,
    /// timer-driven, transparent to the process
    Auto,
    /// someone else asked (shell `ckpt <pid>`)
    Manual,
}

pub struct CkptMeta {
    pub seq: u64,
    pub path: String,
    pub full: bool,
    pub tick: u64,
    pub dirty: usize,
    pub bytes: usize,
    /// file writes made between the previous generation and this one
    pub undo_before: UndoLog,
}

#[derive(Default)]
pub struct Ghost {
    pub protected: bool,
    pub auto_every: Option<u64>,
    pub chain: Vec<CkptMeta>,
    pub seq: u64,
    pub taken: u64,
    pub restores: u32,
    pub last_auto: u64,
    pub last_restore_tick: Option<u64>,
    pub streak: u32,
    pub needs_full: bool,
    pub recent_crashes: VecDeque<u64>,
}

impl Ghost {
    pub fn auto_due(&self, now: u64) -> bool {
        self.protected && self.auto_every.is_some_and(|n| now >= self.last_auto + n)
    }

    pub fn last_seq(&self) -> u64 {
        self.chain.last().map(|c| c.seq).unwrap_or(0)
    }
}

pub struct Crash {
    pub vector: u8,
    pub rip: u64,
    pub rsp: u64,
    pub error: u64,
    pub cr2: u64,
}

impl Crash {
    pub fn summary(&self) -> String {
        let v = self.vector as usize;
        if v == 14 {
            format!(
                "{} rip={:#x} cr2={:#x} err={:#x}",
                EXCEPTION_SHORT[v], self.rip, self.cr2, self.error
            )
        } else {
            format!(
                "{} rip={:#x} err={:#x}",
                EXCEPTION_SHORT[v], self.rip, self.error
            )
        }
    }
}

fn label(p: &Process) -> String {
    format!("{}#{}", p.name, p.pid)
}

/// Freeze `p` (whose live registers are `frame`) into a new generation.
pub fn checkpoint(
    p: &mut Process,
    frame: &TrapFrame,
    fs: &mut RamFs,
    kind: Kind,
) -> Result<u64, i64> {
    let t0 = time::tsc();
    let now = pit::ticks();
    p.ghost.protected = true;
    let seq = p.ghost.seq + 1;
    let since_full = p.ghost.chain.iter().rev().take_while(|c| !c.full).count();
    let full = p.ghost.needs_full || p.ghost.chain.is_empty() || since_full + 1 >= FULL_EVERY;

    let space = p.aspace.as_mut().ok_or(-1i64)?;
    let dirty: BTreeSet<u64> = if full {
        space.clear_dirty();
        space.pages().map(|(va, _, _)| va).collect()
    } else {
        space.take_dirty().into_iter().collect()
    };
    let mapping: Vec<(u64, u64, u64)> = space.pages().collect();

    let mut regs = *frame;
    if kind == Kind::Sync {
        regs.rax = seq; // what the syscall returns on the normal path
    }
    let fds: Vec<(usize, crate::proc::FileHandle)> = p
        .fds
        .iter()
        .enumerate()
        .filter_map(|(i, f)| f.map(|f| (i, f)))
        .collect();

    let mut w = Writer::new(256 + mapping.len() * 16 + dirty.len() * (8 + PAGE_SIZE as usize));
    w.bytes(image::MAGIC);
    w.u64(p.pid as u64);
    w.u64(seq);
    w.u64(if full { 0 } else { p.ghost.last_seq() });
    w.u64(now);
    w.u64(if full { FLAG_FULL } else { 0 } | if kind == Kind::Sync { FLAG_SYNC } else { 0 });
    w.u64(mapping.len() as u64);
    w.u64(dirty.len() as u64);
    w.u64(fds.len() as u64);
    w.bytes(regs.as_bytes());
    for &(va, _, flags) in &mapping {
        w.u64(va);
        w.u64(flags);
    }
    for &(va, f, _) in mapping.iter().filter(|(va, _, _)| dirty.contains(va)) {
        w.u64(va);
        w.bytes(frame::as_slice(f));
    }
    for (slot, h) in &fds {
        w.u64(*slot as u64);
        w.u64(h.ino as u64);
        w.u64(h.offset as u64);
        w.u64(h.flags as u64);
    }

    let path = format!(
        "/ckpt/{}.{}.{:04}.{}",
        p.name,
        p.pid,
        seq,
        if full { "img" } else { "delta" }
    );
    let bytes = w.buf.len();
    fs.write_file(&path, &w.buf).map_err(|e| e.errno())?;

    let undo_before = core::mem::take(&mut p.undo);
    p.ghost.chain.push(CkptMeta {
        seq,
        path,
        full,
        tick: now,
        dirty: dirty.len(),
        bytes,
        undo_before,
    });
    p.ghost.seq = seq;
    p.ghost.taken += 1;
    p.ghost.needs_full = false;
    if kind == Kind::Auto {
        p.ghost.last_auto = now;
    }
    collect_garbage(&mut p.ghost, fs);

    let us = time::tsc_to_us(time::tsc() - t0);
    let how = match kind {
        Kind::Sync => "sync",
        Kind::Auto => "auto",
        Kind::Manual => "manual",
    };
    ui::event(
        Ev::Ckpt,
        &label(p),
        &format!(
            "#{seq:<3} {} {:>2}/{:<2} {:>4}K {:>6}us {how}",
            if full { "FULL" } else { "  Δ " },
            dirty.len(),
            mapping.len(),
            bytes.div_ceil(1024),
            us
        ),
    );
    ui::state::add_ckpt_bytes(bytes as u64);
    Ok(seq)
}

/// Keep just enough images to restore any of the last KEEP generations.
fn collect_garbage(g: &mut Ghost, fs: &mut RamFs) {
    let n = g.chain.len();
    let mut keep = vec![false; n];
    for target in n.saturating_sub(KEEP)..n {
        let mut i = target;
        loop {
            keep[i] = true;
            if g.chain[i].full || i == 0 {
                break;
            }
            i -= 1;
        }
    }
    let mut idx = 0;
    g.chain.retain(|c| {
        let k = keep[idx];
        idx += 1;
        if !k && let Ok(ino) = fs.lookup(&c.path) {
            let _ = fs.remove(ino);
        }
        k
    });
}

/// Rebuild `p` from generation `chain[idx]`. Returns (pages, images, undo bytes).
fn restore(
    p: &mut Process,
    fs: &mut RamFs,
    idx: usize,
) -> Result<(usize, usize, usize), &'static str> {
    let base = (0..=idx)
        .rev()
        .find(|&i| p.ghost.chain[i].full)
        .ok_or("no full image in chain")?;

    // Phase 1: read images and build the new address space.
    let (space, regs, sync, fds, pages, images) = {
        let mut raws: Vec<&[u8]> = Vec::new();
        for c in &p.ghost.chain[base..=idx] {
            let ino = fs.lookup(&c.path).map_err(|_| "checkpoint image missing")?;
            raws.push(&fs.inode(ino).unwrap().data);
        }
        let imgs: Vec<Image> = raws
            .iter()
            .map(|r| Image::parse(r))
            .collect::<Option<_>>()
            .ok_or("corrupt image")?;
        // the chain must be exactly what we recorded: right owner, right
        // generations, every delta pointing at its predecessor
        for (i, (img, meta)) in imgs.iter().zip(&p.ghost.chain[base..=idx]).enumerate() {
            let h = &img.header;
            let parent_ok = if i == 0 {
                h.parent == 0
            } else {
                h.parent == imgs[i - 1].header.seq
            };
            if h.pid != p.pid as u64 || h.seq != meta.seq || !parent_ok {
                return Err("checkpoint chain is inconsistent");
            }
        }
        let target = imgs.last().unwrap();

        // newest copy of each page wins
        let mut newest: BTreeMap<u64, &[u8]> = BTreeMap::new();
        for img in imgs.iter().rev() {
            for (va, data) in img.pages() {
                newest.entry(va).or_insert(data);
            }
        }

        let mut space = AddressSpace::new().ok_or("out of memory")?;
        let mut count = 0;
        for (va, flags) in target.mapping() {
            let f = space
                .map_new(va, flags & PTE_WRITE != 0)
                .ok_or("out of memory")?;
            let data = newest.get(&va).ok_or("page missing from chain")?;
            frame::as_slice(f).copy_from_slice(data);
            count += 1;
        }
        let mut fds = [None; MAX_FDS];
        for (slot, h) in target.fds() {
            if slot < MAX_FDS && fs.inode(h.ino).is_some() {
                fds[slot] = Some(h);
            }
        }
        let h = &target.header;
        (
            space,
            h.frame,
            h.flags & FLAG_SYNC != 0,
            fds,
            count,
            imgs.len(),
        )
    };

    // Phase 2: commit. Dropping the old address space frees its frames.
    p.aspace = Some(space);
    p.ctx = regs;
    let seq = p.ghost.chain[idx].seq;
    if sync {
        p.ctx.rax = seq | RESTORED_BIT;
    }
    p.fds = fds;

    // Undo file writes: first those since the last checkpoint, then the
    // ones recorded with every generation newer than the one we restore.
    let mut undone = p.undo.rollback(fs);
    while p.ghost.chain.len() > idx + 1 {
        let mut c = p.ghost.chain.pop().unwrap();
        undone += c.undo_before.rollback(fs);
        if let Ok(ino) = fs.lookup(&c.path) {
            let _ = fs.remove(ino);
        }
    }
    // the fresh pages were written by the kernel, so their dirty bits are
    // clear and the next delta is relative to exactly this generation
    Ok((pages, images, undone))
}

/// A protected process just faulted. Bring it back, or explain why not.
pub fn recover(p: &mut Process, fs: &mut RamFs, crash: &Crash) -> Result<(), String> {
    let now = pit::ticks();
    let who = label(p);
    ui::event(Ev::Crash, &who, &crash.summary());

    let v = crash.vector as usize;
    let mut crash_lines = vec![
        format!("{} - {}", EXCEPTION_SHORT[v], EXCEPTION_NAMES[v]),
        format!("rip {:#018x}   rsp {:#018x}", crash.rip, crash.rsp),
    ];
    if v == 14 {
        crash_lines.push(format!("cr2 {:#018x}   err {:#x}", crash.cr2, crash.error));
    }

    let give_up = |p: &mut Process, why: &str, mut lines: Vec<String>| -> Result<(), String> {
        let lost = (pit::ticks() - p.born_tick) * 1000 / pit::HZ;
        let lost = format!("{}.{} s", lost / 1000, (lost % 1000) / 100);
        ui::event(
            Ev::GiveUp,
            &label(p),
            &format!("{why}: {lost} of work gone"),
        );
        lines.push(String::new());
        lines.push(String::from(why));
        lines.push(format!("everything it did in {lost} is gone"));
        banner(
            BannerKind::Lost,
            format!("{} IS GONE", p.name.to_uppercase()),
            lines,
            0,
        );
        Err(String::from(why))
    };

    if !p.ghost.protected || p.ghost.chain.is_empty() {
        let why = if p.ghost.protected {
            "no checkpoint yet"
        } else {
            "not protected"
        };
        return give_up(p, why, crash_lines);
    }

    let recent = &mut p.ghost.recent_crashes;
    recent.push_back(now);
    while recent.front().is_some_and(|&t| t + CRASH_WINDOW < now) {
        recent.pop_front();
    }
    if recent.len() > CRASH_BURST {
        return give_up(p, "crash loop: 5 crashes in 10 s", crash_lines);
    }

    // Did the process make a fresh checkpoint since we last brought it back?
    let fresh = match p.ghost.last_restore_tick {
        None => true,
        Some(t) => p.ghost.chain.last().unwrap().tick > t,
    };
    if fresh {
        p.ghost.streak = 0;
    }
    p.ghost.streak += 1;
    if p.ghost.streak > MAX_STREAK {
        return give_up(p, "crash loop: every restore point failed", crash_lines);
    }
    // first attempt: newest generation. Each repeat: one generation older.
    let idx = if p.ghost.streak == 1 {
        p.ghost.chain.len() - 1
    } else {
        match p.ghost.chain.len().checked_sub(2) {
            Some(i) => i,
            None => return give_up(p, "crash loop: no older checkpoint", crash_lines),
        }
    };

    banner(
        BannerKind::Crash,
        format!("{} CRASHED", p.name.to_uppercase()),
        crash_lines.clone(),
        0,
    );

    let t0 = time::tsc();
    match restore(p, fs, idx) {
        Ok((pages, images, undone)) => {
            let us = time::tsc_to_us(time::tsc() - t0);
            let c = p.ghost.chain.last().unwrap();
            let age_ms = (now - c.tick) * 1000 / pit::HZ;
            p.ghost.restores += 1;
            p.ghost.last_restore_tick = Some(now);
            ui::event(
                Ev::Restore,
                &who,
                &format!(
                    "-> #{} {}.{}s back {images}img {us}us{}",
                    c.seq,
                    age_ms / 1000,
                    (age_ms % 1000) / 100,
                    if p.ghost.streak > 1 { " fallback" } else { "" }
                ),
            );
            banner(
                BannerKind::Restore,
                format!("{} RESTORED", p.name.to_uppercase()),
                vec![
                    format!(
                        "back to checkpoint #{}  ({} ms before the crash)",
                        c.seq, age_ms
                    ),
                    format!("{pages} pages rebuilt from {images} image(s) in {us} us"),
                    format!("{undone} bytes of file writes rolled back"),
                ],
                pages,
            );
            Ok(())
        }
        Err(e) => give_up(p, e, crash_lines),
    }
}

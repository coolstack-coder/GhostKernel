//! In-kernel tests (`make test`). They run after boot and before init,
//! against the real allocator, page tables and filesystem, then exit QEMU
//! with the verdict: 0x10 -> host sees 33 (pass), anything else fails.

use crate::arch::cpu;
use crate::fs::undo::{UndoLog, UndoRecord};
use crate::fs::{FS, Kind};
use crate::ghost::{self, Kind as CkptKind};
use crate::mm::paging::{AddressSpace, PTE_WRITE, USER_BASE};
use crate::mm::{frame, paging};
use crate::proc::{PROCS, elf};
use crate::serial_println;

type Test = (&'static str, fn());

const TESTS: &[Test] = &[
    ("frames: alloc, zeroed, free", frames),
    ("ramfs: create, write, read, remove", ramfs),
    ("undo log: rolls writes and creates back", undo),
    ("elf: loads /bin/primes with W^X pages", elf_load),
    ("paging: hardware dirty bits are tracked", dirty_bits),
    (
        "ghost: full + delta checkpoint, restore",
        checkpoint_roundtrip,
    ),
    (
        "ghost: garbage collection keeps chains whole",
        gc_keeps_bases,
    ),
];

pub fn run() -> ! {
    serial_println!("\nrunning {} kernel tests", TESTS.len());
    for (name, t) in TESTS {
        serial_println!("test {name} ...");
        t();
        serial_println!("  ok");
    }
    serial_println!("all {} tests passed", TESTS.len());
    cpu::qemu_exit(0x10)
}

fn frames() {
    let (free0, _) = frame::stats();
    let a = frame::alloc().unwrap();
    let b = frame::alloc().unwrap();
    assert_ne!(a, b);
    assert!(
        frame::as_slice(a).iter().all(|&x| x == 0),
        "frame not zeroed"
    );
    frame::as_slice(a)[0] = 0xAA;
    frame::free(a);
    frame::free(b);
    assert_eq!(frame::stats().0, free0);
    // the freed frame comes back zeroed
    let c = frame::alloc().unwrap();
    assert_eq!(frame::as_slice(c)[0], 0);
    frame::free(c);
}

fn ramfs() {
    let mut fs = FS.lock();
    fs.mkdir_p("/t/a/b");
    let ino = fs.write_file("/t/a/b/f", b"hello").unwrap();
    assert_eq!(fs.lookup("/t/a/b/f"), Ok(ino));
    assert_eq!(fs.lookup("/t/a/../a/b/./f"), Ok(ino));
    let mut buf = [0u8; 8];
    assert_eq!(fs.read_at(ino, 1, &mut buf), Ok(4));
    assert_eq!(&buf[..4], b"ello");
    assert!(fs.create("/t/a/b/f", Kind::File).is_err());
    let dir = fs.lookup("/t/a").unwrap();
    assert!(fs.remove(dir).is_err(), "removed a non-empty dir");
    fs.remove(ino).unwrap();
    assert!(fs.lookup("/t/a/b/f").is_err());
}

fn undo() {
    let mut fs = FS.lock();
    let ino = fs.write_file("/t/log", b"one\n").unwrap();
    let mut log = UndoLog::default();
    log.push(fs.write_at(ino, 4, b"two\n").unwrap());
    log.push(fs.write_at(ino, 0, b"ONE").unwrap());
    let created = fs.create("/t/new", Kind::File).unwrap();
    log.push(UndoRecord::Created { ino: created });
    assert_eq!(fs.inode(ino).unwrap().data, b"ONE\ntwo\n");
    log.rollback(&mut fs);
    assert_eq!(fs.inode(ino).unwrap().data, b"one\n");
    assert!(fs.lookup("/t/new").is_err());
}

fn elf_load() {
    let img = {
        let fs = FS.lock();
        fs.inode(fs.lookup("/bin/primes").unwrap())
            .unwrap()
            .data
            .clone()
    };
    let (space, entry) = elf::load(&img).unwrap();
    assert!(entry >= USER_BASE);
    let (_, flags) = space.translate(entry).unwrap();
    assert_eq!(flags & PTE_WRITE, 0, "code page is writable");
    assert!(
        space.pages().any(|(_, _, fl)| fl & PTE_WRITE != 0),
        "no writable data page"
    );
    assert!(elf::load(b"not an elf").is_err());
}

fn dirty_bits() {
    let mut space = AddressSpace::new().unwrap();
    let va = USER_BASE + 0x10_0000;
    space.map_new(va, true).unwrap();
    space.map_new(va + 0x1000, true).unwrap();
    space.activate();
    assert!(space.take_dirty().is_empty());
    // a ring-0 write through the user mapping sets D just like ring 3 would
    unsafe { core::ptr::write_volatile((va + 0x1000 + 8) as *mut u64, 42) };
    assert_eq!(space.take_dirty(), alloc::vec![va + 0x1000]);
    assert!(space.take_dirty().is_empty(), "dirty bit not cleared");
    unsafe { core::ptr::write_volatile((va + 0x1000) as *mut u8, 1) };
    assert_eq!(space.take_dirty().len(), 1, "TLB kept a stale dirty bit");
    unsafe { cpu::write_cr3(paging::kernel_pml4()) };
}

/// Write a marker into the process's stack page through the identity map.
fn poke(p: &mut crate::proc::Process, va: u64, val: u64) {
    p.space().activate();
    unsafe { core::ptr::write_volatile(va as *mut u64, val) };
    unsafe { cpu::write_cr3(paging::kernel_pml4()) };
}

fn peek(p: &crate::proc::Process, va: u64) -> u64 {
    let (f, _) = p.space().translate(va).unwrap();
    unsafe { *((f + (va & 0xFFF)) as *const u64) }
}

fn checkpoint_roundtrip() {
    let pid = {
        let fs = FS.lock();
        PROCS.lock().spawn(&fs, "/bin/counter", "").unwrap()
    };
    let mut t = PROCS.lock();
    t.queue.clear();
    let p = t.get(pid).unwrap();
    let marker = crate::proc::STACK_TOP - 0x800;
    poke(p, marker, 1111);

    let regs = p.ctx;
    let mut fs = FS.lock();
    let s1 = ghost::checkpoint(p, &regs, &mut fs, CkptKind::Manual).unwrap();
    assert!(p.ghost.chain[0].full);

    poke(p, marker, 2222);
    let s2 = ghost::checkpoint(p, &regs, &mut fs, CkptKind::Manual).unwrap();
    let delta = p.ghost.chain.last().unwrap();
    assert!(!delta.full);
    assert_eq!(
        delta.dirty, 1,
        "delta should hold exactly the page we wrote"
    );
    assert!(s2 > s1);

    poke(p, marker, 3333);
    // simulate a crash right now, restoring the newest generation
    let crash = ghost::Crash {
        vector: 14,
        rip: 0,
        rsp: 0,
        error: 0,
        cr2: 0,
    };
    ghost::recover(p, &mut fs, &crash).unwrap();
    assert_eq!(peek(p, marker), 2222, "restored memory is wrong");
    assert_eq!(p.ghost.restores, 1);

    // crash again with no new checkpoint: falls back one generation
    poke(p, marker, 4444);
    ghost::recover(p, &mut fs, &crash).unwrap();
    assert_eq!(peek(p, marker), 1111, "fallback generation is wrong");
    drop(fs);
    t.terminate(pid, crate::proc::Exit::Killed);
}

fn gc_keeps_bases() {
    let pid = {
        let fs = FS.lock();
        PROCS.lock().spawn(&fs, "/bin/counter", "").unwrap()
    };
    let mut t = PROCS.lock();
    t.queue.clear();
    let p = t.get(pid).unwrap();
    let regs = p.ctx;
    let mut fs = FS.lock();
    for i in 0..20u64 {
        poke(p, crate::proc::STACK_TOP - 0x100, i);
        ghost::checkpoint(p, &regs, &mut fs, CkptKind::Manual).unwrap();
        let chain = &p.ghost.chain;
        assert!(chain[0].full, "chain lost its base image");
        for c in chain {
            assert!(fs.lookup(&c.path).is_ok(), "image {} missing", c.path);
        }
    }
    let files = fs.list(fs.lookup("/ckpt").unwrap()).unwrap();
    let prefix = alloc::format!("counter.{pid}.");
    let mine = files
        .iter()
        .filter(|(n, _, _)| n.starts_with(&prefix))
        .count();
    assert_eq!(mine, p.ghost.chain.len(), "stale images left behind");
    drop(fs);
    t.terminate(pid, crate::proc::Exit::Killed);
}

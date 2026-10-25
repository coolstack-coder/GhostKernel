//! Raw syscalls. Numbers match kernel/src/syscall.rs.

use core::arch::asm;

#[inline(always)]
fn syscall(nr: u64, a0: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    let ret: i64;
    unsafe {
        asm!("int 0x80",
            inlateout("rax") nr as i64 => ret,
            in("rdi") a0, in("rsi") a1, in("rdx") a2, in("r10") a3,
            options(nostack));
    }
    ret
}

pub const O_READ: u32 = 1;
pub const O_WRITE: u32 = 2;
pub const O_CREATE: u32 = 4;
pub const O_TRUNC: u32 = 8;
pub const O_APPEND: u32 = 16;

pub const RESTORED_BIT: u64 = 1 << 62;

pub fn exit(code: i64) -> ! {
    syscall(0, code as u64, 0, 0, 0);
    unreachable!()
}

pub fn write(fd: i64, buf: &[u8]) -> i64 {
    syscall(1, fd as u64, buf.as_ptr() as u64, buf.len() as u64, 0)
}

pub fn read(fd: i64, buf: &mut [u8]) -> i64 {
    syscall(2, fd as u64, buf.as_mut_ptr() as u64, buf.len() as u64, 0)
}

pub fn open(path: &str, flags: u32) -> i64 {
    syscall(3, path.as_ptr() as u64, path.len() as u64, flags as u64, 0)
}

pub fn close(fd: i64) -> i64 {
    syscall(4, fd as u64, 0, 0, 0)
}

pub fn yield_now() {
    syscall(5, 0, 0, 0, 0);
}

pub fn getpid() -> u32 {
    syscall(6, 0, 0, 0, 0) as u32
}

pub fn sleep_ms(ms: u64) {
    syscall(7, ms, 0, 0, 0);
}

/// What `checkpoint()` tells you: either "saved as generation N", or -
/// the fun one - "you crashed, and this is generation N speaking".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ckpt {
    Saved(u64),
    Restored(u64),
    Failed(i64),
}

/// Like setjmp: returns once now, and again after every resurrection.
pub fn checkpoint() -> Ckpt {
    let r = syscall(8, 0, 0, 0, 0);
    if r < 0 {
        Ckpt::Failed(r)
    } else if r as u64 & RESTORED_BIT != 0 {
        Ckpt::Restored(r as u64 & !RESTORED_BIT)
    } else {
        Ckpt::Saved(r as u64)
    }
}

pub fn spawn(path: &str, arg: &str) -> i64 {
    syscall(
        9,
        path.as_ptr() as u64,
        path.len() as u64,
        arg.as_ptr() as u64,
        arg.len() as u64,
    )
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PsEntry {
    pub pid: u32,
    pub state: u8,
    pub protected: u8,
    pub auto: u8,
    pub cpu: u8,
    pub ckpts: u32,
    pub restores: u32,
    pub pages: u32,
    pub name: [u8; 16],
    pub cpu_ms: u32,
}

impl PsEntry {
    pub const EMPTY: PsEntry = PsEntry {
        pid: 0,
        state: 0,
        protected: 0,
        auto: 0,
        cpu: 0,
        ckpts: 0,
        restores: 0,
        pages: 0,
        name: [0; 16],
        cpu_ms: 0,
    };

    pub fn name(&self) -> &str {
        let n = self.name.iter().position(|&b| b == 0).unwrap_or(16);
        core::str::from_utf8(&self.name[..n]).unwrap_or("?")
    }
}

pub fn ps(out: &mut [PsEntry]) -> usize {
    let r = syscall(10, out.as_mut_ptr() as u64, out.len() as u64, 0, 0);
    r.max(0) as usize
}

pub fn kill(pid: u32) -> i64 {
    syscall(11, pid as u64, 0, 0, 0)
}

pub fn inject(pid: u32, kind: u8) -> i64 {
    syscall(12, pid as u64, kind as u64, 0, 0)
}

pub fn readdir(path: &str, out: &mut [u8]) -> i64 {
    syscall(
        13,
        path.as_ptr() as u64,
        path.len() as u64,
        out.as_mut_ptr() as u64,
        out.len() as u64,
    )
}

pub fn uptime_ms() -> u64 {
    syscall(14, 0, 0, 0, 0) as u64
}

/// Protect `pid` (0 = me). `auto_ms` > 0 adds transparent periodic checkpoints.
pub fn protect(pid: u32, auto_ms: u64) -> i64 {
    syscall(15, pid as u64, auto_ms, 0, 0)
}

pub fn unprotect(pid: u32) -> i64 {
    syscall(15, pid as u64, 0, 1, 0)
}

pub fn wait(pid: u32) -> i64 {
    syscall(16, pid as u64, 0, 0, 0)
}

/// Draw into the dashboard's shared window: one byte per cell, non-zero = lit.
pub fn draw(cells: &[u8], w: usize, h: usize, title: &str) -> i64 {
    syscall(
        17,
        cells.as_ptr() as u64,
        (w | (h << 16)) as u64,
        title.as_ptr() as u64,
        title.len() as u64,
    )
}

pub fn checkpoint_other(pid: u32) -> i64 {
    syscall(18, pid as u64, 0, 0, 0)
}

pub fn unlink(path: &str) -> i64 {
    syscall(19, path.as_ptr() as u64, path.len() as u64, 0, 0)
}

#[repr(C)]
#[derive(Default)]
pub struct SysInfo {
    pub uptime_ms: u64,
    pub frames_free: u64,
    pub frames_total: u64,
    pub heap_used: u64,
    pub heap_total: u64,
    pub checkpoints: u64,
    pub restores: u64,
    pub crashes: u64,
    pub ckpt_bytes: u64,
}

pub fn sysinfo() -> SysInfo {
    let mut s = SysInfo::default();
    syscall(20, &mut s as *mut SysInfo as u64, 0, 0, 0);
    s
}

pub fn seek(fd: i64, off: u64) -> i64 {
    syscall(21, fd as u64, off, 0, 0)
}

pub fn mkdir(path: &str) -> i64 {
    syscall(22, path.as_ptr() as u64, path.len() as u64, 0, 0)
}

pub fn set_tty() {
    syscall(23, 0, 0, 0, 0);
}

/// (restores, last checkpoint seq) for `pid` (0 = me)
pub fn ghost_info(pid: u32) -> (u32, u64) {
    let r = syscall(24, pid as u64, 0, 0, 0);
    if r < 0 {
        (0, 0)
    } else {
        ((r >> 32) as u32, (r & 0xFFFF_FFFF) as u64)
    }
}

/// Cheap randomness: the TSC. Good enough for a game of life seed.
pub fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe { asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack)) };
    ((hi as u64) << 32) | lo as u64
}

pub struct Rng(u64);

impl Rng {
    pub fn seeded() -> Self {
        Rng(rdtsc() | 1)
    }
    pub fn next_u64(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

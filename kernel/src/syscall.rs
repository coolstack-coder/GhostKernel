//! `int 0x80`. Number in rax; arguments in rdi, rsi, rdx, r10; result in
//! rax (negative = errno). Every user pointer is checked against the
//! caller's page tables before the kernel touches it.

use crate::arch::pit;
use crate::arch::trap::TrapFrame;
use crate::drivers::keyboard;
use crate::fs::undo::UndoRecord;
use crate::fs::{FS, Kind as FileKind};
use crate::ghost::{self, inject};
use crate::proc::{Exit, FileHandle, MAX_FDS, PROCS, Pid, ProcTable, Process, State, sched};
use crate::proc::{O_APPEND, O_CREATE, O_READ, O_TRUNC, O_WRITE};
use crate::ui::{self, Ev};
use alloc::format;
use alloc::string::String;
use core::fmt::Write;

pub mod nr {
    pub const EXIT: u64 = 0;
    pub const WRITE: u64 = 1;
    pub const READ: u64 = 2;
    pub const OPEN: u64 = 3;
    pub const CLOSE: u64 = 4;
    pub const YIELD: u64 = 5;
    pub const GETPID: u64 = 6;
    pub const SLEEP: u64 = 7;
    pub const CHECKPOINT: u64 = 8;
    pub const SPAWN: u64 = 9;
    pub const PS: u64 = 10;
    pub const KILL: u64 = 11;
    pub const INJECT: u64 = 12;
    pub const READDIR: u64 = 13;
    pub const UPTIME: u64 = 14;
    pub const PROTECT: u64 = 15;
    pub const WAIT: u64 = 16;
    pub const DRAW: u64 = 17;
    pub const CKPT_OTHER: u64 = 18;
    pub const UNLINK: u64 = 19;
    pub const SYSINFO: u64 = 20;
    pub const SEEK: u64 = 21;
    pub const MKDIR: u64 = 22;
    pub const SET_TTY: u64 = 23;
    pub const GHOST_INFO: u64 = 24;
}

const EINVAL: i64 = -22;
const EFAULT: i64 = -14;
const EBADF: i64 = -9;
const ESRCH: i64 = -3;
const EMFILE: i64 = -24;
const EPERM: i64 = -1;

enum Outcome {
    Ret(i64),
    /// park the caller and re-run this syscall when it's woken
    Block,
    Yield,
    Exited,
}

/// One row of `ps`, shared with user space.
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

#[repr(C)]
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

fn user_bytes<'a>(p: &Process, ptr: u64, len: u64) -> Result<&'a [u8], i64> {
    if len > 1 << 20 || !p.space().check_range(ptr, len, false) {
        return Err(EFAULT);
    }
    // the caller's address space is the active one during its syscall
    Ok(unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) })
}

fn user_bytes_mut<'a>(p: &Process, ptr: u64, len: u64) -> Result<&'a mut [u8], i64> {
    if len > 1 << 20 || !p.space().check_range(ptr, len, true) {
        return Err(EFAULT);
    }
    Ok(unsafe { core::slice::from_raw_parts_mut(ptr as *mut u8, len as usize) })
}

fn user_str<'a>(p: &Process, ptr: u64, len: u64) -> Result<&'a str, i64> {
    core::str::from_utf8(user_bytes(p, ptr, len.min(256))?).map_err(|_| EINVAL)
}

pub fn dispatch(frame: &mut TrapFrame) {
    let mut guard = PROCS.lock();
    let t = &mut *guard;
    let pid = t.current.expect("syscall without a current process");
    t.procs.get_mut(&pid).unwrap().syscalls += 1;

    let outcome = match handle(t, pid, frame) {
        Ok(o) => o,
        Err(e) => Outcome::Ret(e),
    };
    match outcome {
        Outcome::Ret(v) => frame.rax = v as u64,
        Outcome::Block => {
            frame.rip -= 2; // `int 0x80` is two bytes: run it again on wake-up
            sched::reschedule(t, frame);
        }
        Outcome::Yield => {
            frame.rax = 0;
            sched::reschedule(t, frame);
        }
        Outcome::Exited => sched::pick_next(t, frame),
    }
}

fn handle(t: &mut ProcTable, pid: Pid, f: &mut TrapFrame) -> Result<Outcome, i64> {
    let (a0, a1, a2, a3) = (f.rdi, f.rsi, f.rdx, f.r10);
    let ret = |v: i64| Ok(Outcome::Ret(v));
    match f.rax {
        nr::EXIT => {
            t.terminate(pid, Exit::Code(a0 as i64));
            Ok(Outcome::Exited)
        }
        nr::WRITE => {
            let p = t.get(pid).unwrap();
            let data = user_bytes(p, a1, a2)?;
            match a0 {
                1 | 2 => {
                    ui::console_write(pid, &p.name.clone(), data);
                    ret(data.len() as i64)
                }
                fd => {
                    let h = p.fds.get(fd as usize).copied().flatten().ok_or(EBADF)?;
                    if h.flags & O_WRITE == 0 {
                        return ret(EBADF);
                    }
                    let mut fs = FS.lock();
                    let off = if h.flags & O_APPEND != 0 {
                        fs.size(h.ino)
                    } else {
                        h.offset
                    };
                    let undo = fs.write_at(h.ino, off, data).map_err(|e| e.errno())?;
                    if p.ghost.protected {
                        p.undo.push(undo);
                    }
                    p.fds[fd as usize].as_mut().unwrap().offset = off + data.len();
                    ret(data.len() as i64)
                }
            }
        }
        nr::READ => {
            let p = t.get(pid).unwrap();
            let buf = user_bytes_mut(p, a1, a2)?;
            if a0 == 0 {
                if buf.is_empty() {
                    return ret(0);
                }
                let mut n = 0;
                while n < buf.len() {
                    match keyboard::pop() {
                        Some(c) => {
                            buf[n] = c;
                            n += 1;
                        }
                        None => break,
                    }
                }
                if n == 0 {
                    p.state = State::WaitKey;
                    return Ok(Outcome::Block);
                }
                return ret(n as i64);
            }
            let h = p.fds.get(a0 as usize).copied().flatten().ok_or(EBADF)?;
            if h.flags & O_READ == 0 {
                return ret(EBADF);
            }
            let n = FS
                .lock()
                .read_at(h.ino, h.offset, buf)
                .map_err(|e| e.errno())?;
            p.fds[a0 as usize].as_mut().unwrap().offset += n;
            ret(n as i64)
        }
        nr::OPEN => {
            let p = t.get(pid).unwrap();
            let path = user_str(p, a0, a1)?;
            let flags = a2 as u32;
            let slot = (3..MAX_FDS).find(|&i| p.fds[i].is_none()).ok_or(EMFILE)?;
            let mut fs = FS.lock();
            let ino = match fs.lookup(path) {
                Ok(i) => i,
                Err(crate::fs::FsError::NotFound) if flags & O_CREATE != 0 => {
                    let i = fs.create(path, FileKind::File).map_err(|e| e.errno())?;
                    if p.ghost.protected {
                        p.undo.push(UndoRecord::Created { ino: i });
                    }
                    i
                }
                Err(e) => return ret(e.errno()),
            };
            if fs.inode(ino).unwrap().kind == FileKind::Dir && flags & O_WRITE != 0 {
                return ret(crate::fs::FsError::IsDir.errno());
            }
            if flags & O_TRUNC != 0 && flags & O_WRITE != 0 {
                let undo = fs.truncate(ino, 0).map_err(|e| e.errno())?;
                if p.ghost.protected {
                    p.undo.push(undo);
                }
            }
            p.fds[slot] = Some(FileHandle {
                ino,
                offset: 0,
                flags,
            });
            ret(slot as i64)
        }
        nr::CLOSE => {
            let p = t.get(pid).unwrap();
            match p.fds.get_mut(a0 as usize) {
                Some(s @ Some(_)) => {
                    *s = None;
                    ret(0)
                }
                _ => ret(EBADF),
            }
        }
        nr::SEEK => {
            let p = t.get(pid).unwrap();
            let h = p
                .fds
                .get_mut(a0 as usize)
                .and_then(|h| h.as_mut())
                .ok_or(EBADF)?;
            h.offset = a1 as usize;
            ret(a1 as i64)
        }
        nr::YIELD => Ok(Outcome::Yield),
        nr::GETPID => ret(pid as i64),
        nr::UPTIME => ret(pit::uptime_ms() as i64),
        nr::SLEEP => {
            let p = t.get(pid).unwrap();
            let ticks = (a0 * pit::HZ).div_ceil(1000).max(1);
            p.state = State::Sleeping(pit::ticks() + ticks);
            Ok(Outcome::Yield)
        }
        nr::CHECKPOINT => {
            let p = t.get(pid).unwrap();
            let mut fs = FS.lock();
            let seq = ghost::checkpoint(p, f, &mut fs, ghost::Kind::Sync)?;
            ret(seq as i64)
        }
        nr::SPAWN => {
            let (path, arg) = {
                let p = t.get(pid).unwrap();
                (
                    String::from(user_str(p, a0, a1)?),
                    String::from(user_str(p, a2, a3)?),
                )
            };
            let fs = FS.lock();
            let child = t.spawn(&fs, &path, &arg)?;
            ret(child as i64)
        }
        nr::PS => {
            let max = a1 as usize;
            let entries: alloc::vec::Vec<PsEntry> = t
                .procs
                .values()
                .take(max)
                .map(|q| {
                    let mut name = [0u8; 16];
                    let n = q.name.len().min(15);
                    name[..n].copy_from_slice(&q.name.as_bytes()[..n]);
                    PsEntry {
                        pid: q.pid,
                        state: match (&q.state, &q.exit) {
                            (State::Running, _) => 0,
                            (State::Ready, _) => 1,
                            (State::Sleeping(_), _) => 2,
                            (State::WaitKey, _) | (State::WaitPid(_), _) => 3,
                            (State::Dead, Some(Exit::Crashed(_))) => 5,
                            (State::Dead, _) => 4,
                        },
                        protected: q.ghost.protected as u8,
                        auto: q.ghost.auto_every.is_some() as u8,
                        cpu: q.cpu_pct,
                        ckpts: q.ghost.taken as u32,
                        restores: q.ghost.restores,
                        pages: q
                            .aspace
                            .as_ref()
                            .map(|a| a.page_count() as u32)
                            .unwrap_or(0),
                        name,
                        cpu_ms: (q.cpu_ticks * 1000 / pit::HZ) as u32,
                    }
                })
                .collect();
            let p = t.get(pid).unwrap();
            let size = core::mem::size_of::<PsEntry>();
            let out = user_bytes_mut(p, a0, (entries.len() * size) as u64)?;
            for (i, e) in entries.iter().enumerate() {
                let src =
                    unsafe { core::slice::from_raw_parts(e as *const PsEntry as *const u8, size) };
                out[i * size..(i + 1) * size].copy_from_slice(src);
            }
            ret(entries.len() as i64)
        }
        nr::KILL => {
            let target = a0 as Pid;
            if !t.procs.get(&target).is_some_and(|p| p.alive()) {
                return ret(ESRCH);
            }
            t.terminate(target, Exit::Killed);
            if target == pid {
                return Ok(Outcome::Exited);
            }
            ret(0)
        }
        nr::INJECT => {
            let target = a0 as Pid;
            let kind = (a1 as u8).min(3);
            let q = t.get(target).filter(|q| q.alive()).ok_or(ESRCH)?;
            ui::event(
                Ev::Inject,
                &format!("{}#{}", q.name, q.pid),
                inject::describe(kind),
            );
            if target == pid {
                // corrupting ourselves: change the live frame instead
                q.ctx = *f;
                inject::apply(q, kind);
                *f = q.ctx;
                // yield so the corrupted frame gets parked and faults on its next run
                return Ok(Outcome::Yield);
            }
            q.inject = Some(kind);
            ret(0)
        }
        nr::PROTECT => {
            let target = if a0 == 0 { pid } else { a0 as Pid };
            let q = t.get(target).filter(|q| q.alive()).ok_or(ESRCH)?;
            q.ghost.protected = a2 == 0;
            q.ghost.auto_every = if a1 > 0 {
                Some((a1 * pit::HZ).div_ceil(1000))
            } else {
                None
            };
            q.ghost.last_auto = pit::ticks();
            let who = format!("{}#{}", q.name, q.pid);
            let what = match (a2, a1) {
                (0, 0) => String::from("protected (checkpoints on request)"),
                (0, ms) => format!("protected, auto-checkpoint every {ms} ms"),
                _ => String::from("protection removed"),
            };
            crate::serial_println!("protect {who}: {what}");
            ui::state::kprint(&[(crate::ui::theme::GHOST, &format!("{who} {what}"))]);
            ret(0)
        }
        nr::CKPT_OTHER => {
            let target = a0 as Pid;
            if target == pid {
                return ret(EINVAL);
            }
            let q = t.get(target).filter(|q| q.alive()).ok_or(ESRCH)?;
            let regs = q.ctx;
            let mut fs = FS.lock();
            let seq = ghost::checkpoint(q, &regs, &mut fs, ghost::Kind::Manual)?;
            ret(seq as i64)
        }
        nr::GHOST_INFO => {
            let target = if a0 == 0 { pid } else { a0 as Pid };
            let q = t.get(target).ok_or(ESRCH)?;
            ret(((q.ghost.restores as i64) << 32) | q.ghost.last_seq() as i64)
        }
        nr::WAIT => {
            let target = a0 as Pid;
            let done = match t.procs.get(&target) {
                None => true,
                Some(q) => !q.alive(),
            };
            if done {
                return ret(0);
            }
            t.get(pid).unwrap().state = State::WaitPid(target);
            Ok(Outcome::Block)
        }
        nr::READDIR => {
            let p = t.get(pid).unwrap();
            let path = user_str(p, a0, a1)?;
            let out = user_bytes_mut(p, a2, a3)?;
            let fs = FS.lock();
            let ino = fs.lookup(path).map_err(|e| e.errno())?;
            let mut s = String::new();
            for (name, kind, size) in fs.list(ino).map_err(|e| e.errno())? {
                let _ = writeln!(
                    s,
                    "{} {} {}",
                    if kind == FileKind::Dir { 'd' } else { 'f' },
                    size,
                    name
                );
            }
            let n = s.len().min(out.len());
            out[..n].copy_from_slice(&s.as_bytes()[..n]);
            ret(n as i64)
        }
        nr::UNLINK => {
            let p = t.get(pid).unwrap();
            let path = user_str(p, a0, a1)?;
            if path.starts_with("/ckpt") || path.starts_with("/bin") {
                return ret(EPERM);
            }
            let mut fs = FS.lock();
            let ino = fs.lookup(path).map_err(|e| e.errno())?;
            fs.remove(ino).map_err(|e| e.errno())?;
            ret(0)
        }
        nr::MKDIR => {
            let p = t.get(pid).unwrap();
            let path = user_str(p, a0, a1)?;
            FS.lock()
                .create(path, FileKind::Dir)
                .map_err(|e| e.errno())?;
            ret(0)
        }
        nr::DRAW => {
            let p = t.get(pid).unwrap();
            let (w, h) = ((a1 & 0xFFFF) as usize, (a1 >> 16) as usize);
            if w == 0 || h == 0 || w * h > 256 * 64 {
                return ret(EINVAL);
            }
            let cells = user_bytes(p, a0, (w * h) as u64)?;
            let title = user_str(p, a2, a3)?;
            ui::state::panel_draw(pid, title, w, h, cells);
            ret(0)
        }
        nr::SET_TTY => {
            ui::state::set_tty(pid);
            ret(0)
        }
        nr::SYSINFO => {
            let (free, total) = crate::mm::frame::stats();
            let (hu, ht) = crate::mm::heap::stats();
            let tot = &ui::state::UI.lock().totals;
            let info = SysInfo {
                uptime_ms: pit::uptime_ms(),
                frames_free: free as u64,
                frames_total: total as u64,
                heap_used: hu as u64,
                heap_total: ht as u64,
                checkpoints: tot.checkpoints,
                restores: tot.restores,
                crashes: tot.crashes,
                ckpt_bytes: tot.ckpt_bytes,
            };
            let p = t.get(pid).unwrap();
            let size = core::mem::size_of::<SysInfo>();
            let out = user_bytes_mut(p, a0, size as u64)?;
            out.copy_from_slice(unsafe {
                core::slice::from_raw_parts(&info as *const SysInfo as *const u8, size)
            });
            ret(0)
        }
        _ => ret(EINVAL),
    }
}

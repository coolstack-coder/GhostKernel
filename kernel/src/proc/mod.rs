//! Processes. A process is an address space, a saved trap frame, a file
//! descriptor table - and, if it's protected, a ghost: the chain of
//! checkpoints it can be brought back from.

pub mod elf;
pub mod fault;
pub mod sched;

use crate::arch::gdt;
use crate::arch::trap::TrapFrame;
use crate::fs::undo::UndoLog;
use crate::fs::{Ino, RamFs};
use crate::ghost::Ghost;
use crate::mm::paging::AddressSpace;
use crate::mm::{PAGE_SIZE, frame};
use crate::ui::{self, Ev};
use alloc::collections::{BTreeMap, VecDeque};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

pub type Pid = u32;

pub const MAX_FDS: usize = 16;
pub const STACK_TOP: u64 = 0x7FFF_0000;
pub const STACK_PAGES: u64 = 16;
const KEEP_DEAD: usize = 6;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Ready,
    Running,
    Sleeping(u64),
    WaitKey,
    WaitPid(Pid),
    Dead,
}

#[derive(Clone, Debug)]
pub enum Exit {
    Code(i64),
    Killed,
    Crashed(String),
}

#[derive(Clone, Copy, Debug)]
pub struct FileHandle {
    pub ino: Ino,
    pub offset: usize,
    pub flags: u32,
}

pub const O_READ: u32 = 1;
pub const O_WRITE: u32 = 2;
pub const O_CREATE: u32 = 4;
pub const O_TRUNC: u32 = 8;
pub const O_APPEND: u32 = 16;

pub struct Process {
    pub pid: Pid,
    pub name: String,
    pub state: State,
    pub ctx: TrapFrame,
    pub aspace: Option<AddressSpace>,
    pub fds: [Option<FileHandle>; MAX_FDS],
    pub ghost: Ghost,
    pub undo: UndoLog,
    pub exit: Option<Exit>,
    pub cpu_ticks: u64,
    pub cpu_window: u64,
    pub cpu_pct: u8,
    pub syscalls: u64,
    /// syscalls per 250 ms, newest last - drives the activity sparkline
    pub activity: VecDeque<u16>,
    activity_mark: u64,
    pub born_tick: u64,
    pub died_tick: u64,
    /// Kernel-requested fault, applied the next time this process runs.
    pub inject: Option<u8>,
}

impl Process {
    pub const ACTIVITY_LEN: usize = 12;

    /// Close the current activity window (called every 250 ms).
    pub fn sample_activity(&mut self) {
        let n = (self.syscalls - self.activity_mark).min(u16::MAX as u64) as u16;
        self.activity_mark = self.syscalls;
        self.activity.push_back(if self.alive() { n } else { 0 });
        if self.activity.len() > Self::ACTIVITY_LEN {
            self.activity.pop_front();
        }
    }

    pub fn space(&self) -> &AddressSpace {
        self.aspace.as_ref().expect("process has no address space")
    }

    pub fn alive(&self) -> bool {
        self.state != State::Dead
    }
}

pub struct ProcTable {
    pub procs: BTreeMap<Pid, Process>,
    pub queue: VecDeque<Pid>,
    pub current: Option<Pid>,
    pub next_pid: Pid,
    pub started: bool,
    pub quantum: u32,
    pub idle_ticks: u64,
    pub idle_window: u64,
    pub idle_pct: u8,
}

pub static PROCS: Mutex<ProcTable> = Mutex::new(ProcTable {
    procs: BTreeMap::new(),
    queue: VecDeque::new(),
    current: None,
    next_pid: 1,
    started: false,
    quantum: 0,
    idle_ticks: 0,
    idle_window: 0,
    idle_pct: 100,
});

impl ProcTable {
    pub fn get(&mut self, pid: Pid) -> Option<&mut Process> {
        self.procs.get_mut(&pid)
    }

    pub fn current_mut(&mut self) -> Option<&mut Process> {
        let pid = self.current?;
        self.procs.get_mut(&pid)
    }

    pub fn make_ready(&mut self, pid: Pid) {
        if let Some(p) = self.procs.get_mut(&pid) {
            p.state = State::Ready;
            if !self.queue.contains(&pid) {
                self.queue.push_back(pid);
            }
        }
    }

    /// Load `/bin/<name>`-style ELF from the filesystem and queue it.
    pub fn spawn(&mut self, fs: &RamFs, path: &str, arg: &str) -> Result<Pid, i64> {
        let ino = fs.lookup(path).map_err(|e| e.errno())?;
        let image = fs.inode(ino).ok_or(-2i64)?.data.clone();
        let (mut space, entry) = elf::load(&image).map_err(|_| -8i64)?;

        // stack, with the argument string copied to the very top
        let mut top_frame = 0;
        for i in 0..STACK_PAGES {
            let va = STACK_TOP - (i + 1) * PAGE_SIZE;
            let f = space.map_new(va, true).ok_or(-12i64)?;
            if i == 0 {
                top_frame = f;
            }
        }
        let arg_bytes = &arg.as_bytes()[..arg.len().min(200)];
        let arg_off = PAGE_SIZE as usize - 256;
        frame::as_slice(top_frame)[arg_off..arg_off + arg_bytes.len()].copy_from_slice(arg_bytes);
        let arg_va = STACK_TOP - 256;

        let pid = self.next_pid;
        self.next_pid += 1;
        let name = String::from(path.rsplit('/').next().unwrap_or(path));
        let ctx = TrapFrame {
            rip: entry,
            cs: gdt::USER_CS as u64,
            rflags: 0x202,
            rsp: arg_va - 64,
            ss: gdt::USER_DS as u64,
            rdi: arg_va,
            rsi: arg_bytes.len() as u64,
            ..Default::default()
        };
        let pages = space.page_count();
        let p = Process {
            pid,
            name: name.clone(),
            state: State::Ready,
            ctx,
            aspace: Some(space),
            fds: [None; MAX_FDS],
            ghost: Ghost::default(),
            undo: UndoLog::default(),
            exit: None,
            cpu_ticks: 0,
            cpu_window: 0,
            cpu_pct: 0,
            syscalls: 0,
            activity: VecDeque::new(),
            activity_mark: 0,
            born_tick: crate::arch::pit::ticks(),
            died_tick: 0,
            inject: None,
        };
        self.procs.insert(pid, p);
        self.queue.push_back(pid);
        ui::event(
            Ev::Spawn,
            &format!("{name}#{pid}"),
            &format!("{path} {arg}  ({pages} pages)"),
        );
        self.reap();
        Ok(pid)
    }

    /// Tear a process down. The table keeps a tombstone so the dashboard
    /// can show what happened to it.
    pub fn terminate(&mut self, pid: Pid, why: Exit) {
        let Some(p) = self.procs.get_mut(&pid) else {
            return;
        };
        if !p.alive() {
            return;
        }
        let label = format!("{}#{}", p.name, pid);
        match &why {
            Exit::Code(c) => ui::event(Ev::Exit, &label, &format!("exited with {c}")),
            Exit::Killed => ui::event(Ev::Kill, &label, "killed"),
            Exit::Crashed(_) => {}
        }
        // its ghost dies with it
        {
            let mut fs = crate::fs::FS.lock();
            for c in p.ghost.chain.drain(..) {
                if let Ok(ino) = fs.lookup(&c.path) {
                    let _ = fs.remove(ino);
                }
            }
        }
        p.state = State::Dead;
        p.exit = Some(why);
        p.aspace = None; // frees every frame it owned
        p.fds = [None; MAX_FDS];
        p.died_tick = crate::arch::pit::ticks();
        self.queue.retain(|&q| q != pid);
        if self.current == Some(pid) {
            self.current = None;
        }
        ui::state::panel_release(pid);
        // wake anyone waiting on it
        let waiters: Vec<Pid> = self
            .procs
            .values()
            .filter(|q| q.state == State::WaitPid(pid))
            .map(|q| q.pid)
            .collect();
        for w in waiters {
            self.make_ready(w);
        }
    }

    fn reap(&mut self) {
        let dead: Vec<Pid> = self
            .procs
            .values()
            .filter(|p| !p.alive())
            .map(|p| p.pid)
            .collect();
        if dead.len() > KEEP_DEAD {
            for pid in &dead[..dead.len() - KEEP_DEAD] {
                self.procs.remove(pid);
            }
        }
    }
}

/// What the dashboard needs to know about one process.
pub struct ProcRow {
    pub pid: Pid,
    pub name: String,
    pub state: State,
    pub exit: Option<Exit>,
    pub protected: bool,
    pub auto: bool,
    pub cpu_pct: u8,
    pub ckpts: u64,
    pub restores: u32,
    pub pages: usize,
    /// (full image?, pages in it) per generation
    pub chain: Vec<(bool, usize)>,
    pub image_bytes: usize,
    pub last_ckpt_ms: Option<u64>,
    pub activity: Vec<u16>,
    pub restored_ms: Option<u64>,
}

pub fn rows() -> (Vec<ProcRow>, u8) {
    let t = PROCS.lock();
    let rows = t
        .procs
        .values()
        .map(|p| ProcRow {
            pid: p.pid,
            name: p.name.clone(),
            state: p.state,
            exit: p.exit.clone(),
            protected: p.ghost.protected,
            auto: p.ghost.auto_every.is_some(),
            cpu_pct: p.cpu_pct,
            ckpts: p.ghost.taken,
            restores: p.ghost.restores,
            pages: p.aspace.as_ref().map(|a| a.page_count()).unwrap_or(0),
            chain: p.ghost.chain.iter().map(|c| (c.full, c.dirty)).collect(),
            image_bytes: p.ghost.chain.iter().map(|c| c.bytes).sum(),
            activity: p.activity.iter().copied().collect(),
            restored_ms: p
                .ghost
                .last_restore_tick
                .map(|t| t * 1000 / crate::arch::pit::HZ),
            last_ckpt_ms: p
                .ghost
                .chain
                .last()
                .map(|c| c.tick * 1000 / crate::arch::pit::HZ),
        })
        .collect();
    (rows, t.idle_pct)
}

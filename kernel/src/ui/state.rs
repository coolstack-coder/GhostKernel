//! Everything the dashboard shows that isn't process-table state: the
//! console scrollback, the ghost event log, the shared drawing panel and
//! the crash/restore banners.

use super::theme;
use crate::arch::pit;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

pub const CONSOLE_KEEP: usize = 300;
pub const EVENTS_KEEP: usize = 400;

pub struct Line {
    pub tag: Option<(String, u32)>,
    pub segs: Vec<(u32, String)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ev {
    Boot,
    Spawn,
    Exit,
    Ckpt,
    Crash,
    Restore,
    GiveUp,
    Kill,
    Inject,
}

impl Ev {
    pub fn label(self) -> &'static str {
        match self {
            Ev::Boot => "BOOT",
            Ev::Spawn => "SPAWN",
            Ev::Exit => "EXIT",
            Ev::Ckpt => "CKPT",
            Ev::Crash => "CRASH",
            Ev::Restore => "RESTORE",
            Ev::GiveUp => "LOST",
            Ev::Kill => "KILL",
            Ev::Inject => "INJECT",
        }
    }

    pub fn color(self) -> u32 {
        match self {
            Ev::Boot => theme::DIM,
            Ev::Spawn => theme::BLUE,
            Ev::Exit => theme::DIM,
            Ev::Ckpt => theme::GHOST,
            Ev::Crash => theme::RED,
            Ev::Restore => theme::GREEN,
            Ev::GiveUp => theme::ORANGE,
            Ev::Kill => theme::ORANGE,
            Ev::Inject => theme::PINK,
        }
    }

    pub fn icon(self) -> char {
        match self {
            Ev::Ckpt => '●',
            Ev::Crash => '✖',
            Ev::Restore => '↻',
            Ev::GiveUp => '✖',
            Ev::Inject => '⚡',
            Ev::Spawn => '▶',
            _ => '•',
        }
    }
}

pub struct Event {
    pub ms: u64,
    pub pid: u32,
    pub kind: Ev,
    pub who: String,
    pub text: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BannerKind {
    Crash,
    Restore,
    Lost,
}

pub struct Banner {
    pub kind: BannerKind,
    pub title: String,
    pub lines: Vec<String>,
    pub shown_at: Option<u64>,
    /// restore banners animate a "rebuilding N pages" bar
    pub pages: usize,
}

pub struct Panel {
    pub owner: u32,
    pub title: String,
    pub w: usize,
    pub h: usize,
    pub cells: Vec<u8>,
    pub frames: u64,
}

#[derive(Default)]
pub struct Totals {
    pub checkpoints: u64,
    pub restores: u64,
    pub crashes: u64,
    pub lost: u64,
    pub ckpt_bytes: u64,
}

pub struct UiState {
    pub lines: VecDeque<Line>,
    pub pending: BTreeMap<u32, (String, Vec<(u32, String)>, u32)>,
    pub tty: Option<u32>,
    pub events: VecDeque<Event>,
    pub banners: VecDeque<Banner>,
    pub panel: Option<Panel>,
    pub totals: Totals,
}

pub static UI: Mutex<UiState> = Mutex::new(UiState {
    lines: VecDeque::new(),
    pending: BTreeMap::new(),
    tty: None,
    events: VecDeque::new(),
    banners: VecDeque::new(),
    panel: None,
    totals: Totals {
        checkpoints: 0,
        restores: 0,
        crashes: 0,
        lost: 0,
        ckpt_bytes: 0,
    },
});

fn esc_color(c: u8) -> Option<u32> {
    Some(match c {
        b'r' => theme::RED,
        b'g' => theme::GREEN,
        b'y' => theme::YELLOW,
        b'b' => theme::BLUE,
        b'p' => theme::PURPLE,
        b'c' => theme::GHOST,
        b'o' => theme::ORANGE,
        b'k' => theme::PINK,
        b'w' => theme::WHITE,
        b'd' => theme::DIM,
        b'0' => theme::TEXT,
        _ => return None,
    })
}

fn stamp() -> (u64, u64) {
    let ms = pit::uptime_ms();
    (ms / 1000, ms % 1000)
}

impl UiState {
    fn push_line(&mut self, line: Line) {
        // mirror to serial as plain text
        let (s, ms) = stamp();
        let text: String = line.segs.iter().map(|(_, t)| t.as_str()).collect();
        match &line.tag {
            Some((name, _)) => crate::serial_println!("[{s:4}.{ms:03}] {name:>8} | {text}"),
            None => crate::serial_println!("[{s:4}.{ms:03}]          | {text}"),
        }
        self.lines.push_back(line);
        while self.lines.len() > CONSOLE_KEEP {
            self.lines.pop_front();
        }
    }
}

/// Bytes written by a process (or the kernel, pid 0) to the console.
/// `\x1b` followed by a letter switches colour, the shell uses it a lot.
pub fn console_write(pid: u32, name: &str, bytes: &[u8]) {
    let mut ui = UI.lock();
    let tty = ui.tty == Some(pid);
    let base = if pid == 0 { theme::DIM } else { theme::TEXT };
    let tag_color = if pid == 0 {
        theme::GHOST
    } else {
        theme::proc_color(pid)
    };
    let mut out: Vec<Line> = Vec::new();
    {
        let entry = ui
            .pending
            .entry(pid)
            .or_insert_with(|| (String::new(), Vec::new(), base));
        entry.0 = String::from(name);
        let mut esc = false;
        for ch in bytes.utf8_chunks().flat_map(|c| c.valid().chars()) {
            if esc {
                esc = false;
                if let Some(c) = esc_color(ch as u8) {
                    entry.2 = c;
                }
                continue;
            }
            match ch {
                '\n' => {
                    let segs = core::mem::take(&mut entry.1);
                    entry.2 = base;
                    let tag = if tty {
                        None
                    } else {
                        Some((String::from(name), tag_color))
                    };
                    out.push(Line { tag, segs });
                }
                '\x08' => {
                    if let Some((_, s)) = entry.1.last_mut() {
                        s.pop();
                        if s.is_empty() {
                            entry.1.pop();
                        }
                    }
                }
                '\x1b' => esc = true,
                '\t' => push_char(entry, ' '),
                c if !c.is_control() => push_char(entry, c),
                _ => {}
            }
        }
    }
    for l in out {
        ui.push_line(l);
    }
}

fn push_char(entry: &mut (String, Vec<(u32, String)>, u32), c: char) {
    let color = entry.2;
    match entry.1.last_mut() {
        Some((col, s)) if *col == color => s.push(c),
        _ => entry.1.push((color, String::from(c))),
    }
}

/// Kernel-side console line (boot log etc.).
pub fn kprint(segs: &[(u32, &str)]) {
    let mut ui = UI.lock();
    let segs = segs.iter().map(|(c, s)| (*c, String::from(*s))).collect();
    ui.push_line(Line {
        tag: Some((String::from("kernel"), theme::GHOST)),
        segs,
    });
}

pub fn event(kind: Ev, who: &str, text: &str) {
    let ms = pit::uptime_ms();
    crate::serial_println!(
        "[{:4}.{:03}] GHOST {:<7} {:<10} {}",
        ms / 1000,
        ms % 1000,
        kind.label(),
        who,
        text
    );
    // labels look like "primes#2"; the timeline wants the pid
    let pid = who
        .rsplit('#')
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);
    let mut ui = UI.lock();
    ui.events.push_back(Event {
        ms,
        pid,
        kind,
        who: String::from(who),
        text: String::from(text),
    });
    while ui.events.len() > EVENTS_KEEP {
        ui.events.pop_front();
    }
    match kind {
        Ev::Ckpt => ui.totals.checkpoints += 1,
        Ev::Restore => ui.totals.restores += 1,
        Ev::Crash => ui.totals.crashes += 1,
        Ev::GiveUp => ui.totals.lost += 1,
        _ => {}
    }
}

pub fn banner(kind: BannerKind, title: String, lines: Vec<String>, pages: usize) {
    let mut ui = UI.lock();
    // a new incident beats an old one that has already been on screen
    if kind == BannerKind::Crash || kind == BannerKind::Lost {
        while ui.banners.front().is_some_and(|b| b.shown_at.is_some()) {
            ui.banners.pop_front();
        }
    }
    while ui.banners.len() > 3 {
        ui.banners.pop_front();
    }
    ui.banners.push_back(Banner {
        kind,
        title,
        lines,
        shown_at: None,
        pages,
    });
}

pub fn set_tty(pid: u32) {
    UI.lock().tty = Some(pid);
}

pub fn add_ckpt_bytes(n: u64) {
    UI.lock().totals.ckpt_bytes += n;
}

pub fn panel_draw(pid: u32, title: &str, w: usize, h: usize, cells: &[u8]) {
    let mut ui = UI.lock();
    match &mut ui.panel {
        Some(p) if p.owner == pid || p.owner == 0 => {
            p.owner = pid;
            p.title = String::from(title);
            p.w = w;
            p.h = h;
            p.cells.clear();
            p.cells.extend_from_slice(cells);
            p.frames += 1;
        }
        Some(_) => {}
        None => {
            ui.panel = Some(Panel {
                owner: pid,
                title: String::from(title),
                w,
                h,
                cells: cells.to_vec(),
                frames: 1,
            })
        }
    }
}

/// Owner died for good: free the panel for someone else.
pub fn panel_release(pid: u32) {
    let mut ui = UI.lock();
    if let Some(p) = &mut ui.panel
        && p.owner == pid
    {
        p.owner = 0;
    }
    ui.pending.remove(&pid);
}

//! Composes the whole screen from kernel state. Layout (in 7x16 cells,
//! 182 x 50):
//!
//!   header ........................................................
//!   [ processes                          ] [ window (shared panel)]
//!   [ ghost timeline                     ] [                      ]
//!   [ console                       ] [ ghost event log            ]
//!   [                               ] [ checkpoint chains          ]
//!   footer ........................................................

use super::grid::{Grid, Style};
use super::state::{BannerKind, Ev, UI};
use super::theme::*;
use crate::arch::pit;
use crate::proc::{self, Exit, State};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

static GRID: Mutex<Option<Grid>> = Mutex::new(None);

pub fn init(fb: u64, w: usize, h: usize) {
    // paint the whole framebuffer once (the grid leaves a few pixels at the edge)
    for i in 0..w * h {
        unsafe { (fb as *mut u32).add(i).write_volatile(BG) };
    }
    *GRID.lock() = Some(Grid::new(fb, w, h));
}

fn hms(ms: u64) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

/// Syscalls per 250 ms, on a scale shared by every process on screen.
fn sparkline(g: &mut Grid, x: usize, y: usize, hist: &[u16], max: u32, fg: u32, bg: u32) {
    let max = max.max(1);
    let pad = proc::Process::ACTIVITY_LEN - hist.len();
    for i in 0..proc::Process::ACTIVITY_LEN {
        let v = if i < pad { 0 } else { hist[i - pad] as u32 };
        let (ch, c) = match v {
            0 => ('▁', TRACK),
            v => (
                char::from_u32('▁' as u32 + (v * 7).div_ceil(max).clamp(1, 7) - 1).unwrap(),
                fg,
            ),
        };
        g.put(x + i, y, ch, Style::new(c, bg));
    }
}

fn bar(g: &mut Grid, x: usize, y: usize, w: usize, pct: u8, fg: u32, bg: u32) {
    let eighths = (pct as usize).min(100) * w * 8 / 100;
    for i in 0..w {
        let n = eighths.saturating_sub(i * 8).min(8);
        let ch = match n {
            0 => ' ',
            8 => '█',
            n => char::from_u32('▉' as u32 + (7 - n as u32)).unwrap(),
        };
        g.put(x + i, y, ch, Style::new(fg, bg));
    }
}

pub fn render() {
    let Some(mut guard) = GRID.try_lock() else {
        return;
    };
    let Some(g) = guard.as_mut() else { return };
    let (rows, idle) = proc::rows();
    let fs_bytes = crate::fs::FS.lock().total_bytes();
    let (heap_used, _) = crate::mm::heap::stats();
    let now = pit::ticks();
    let ms = pit::uptime_ms();
    let mut ui = UI.lock();

    // retire banners whose time is up
    while let Some(b) = ui.banners.front_mut() {
        let life = match b.kind {
            BannerKind::Crash => 140,
            BannerKind::Restore => 300,
            BannerKind::Lost => 350,
        };
        match b.shown_at {
            None => {
                b.shown_at = Some(now);
                break;
            }
            Some(t) if now > t + life => {
                ui.banners.pop_front();
            }
            Some(_) => break,
        }
    }
    let alert = ui.banners.front().map(|b| b.kind);
    let border = match alert {
        Some(BannerKind::Crash) => RED,
        Some(BannerKind::Lost) => ORANGE,
        Some(BannerKind::Restore) => GREEN,
        None => BORDER,
    };

    g.clear(BG);
    let cols = g.cols;

    // ── header ──────────────────────────────────────────────
    // it doubles as an alarm light: red on a crash, green on a restore
    let (hb, tagline, tag_fg) = match alert {
        Some(BannerKind::Crash) => (
            RED_DEEP,
            "✖  FAULT IN RING 3  ·  the kernel is reaching for a checkpoint",
            WHITE,
        ),
        Some(BannerKind::Restore) => (
            GREEN_DEEP,
            "↻  RESTORED  ·  the process carries on as if nothing happened",
            WHITE,
        ),
        Some(BannerKind::Lost) => (
            0x431407,
            "✖  PROCESS LOST  ·  it had no ghost to come back as",
            WHITE,
        ),
        None => (HEADER, "processes that come back from the dead", DIM),
    };
    g.fill(0, 0, cols, 1, ' ', Style::new(TEXT, hb));
    let mut x = g.text(1, 0, " ", Style::new(TEXT, hb), 1);
    g.put(x, 0, '◆', Style::new(GHOST, hb));
    x = g.text(x + 2, 0, "GHOSTKERNEL", Style::new(GHOST, hb).bold(), 20);
    g.text(x + 3, 0, tagline, Style::new(tag_fg, hb).bold(), 70);

    let (free, total) = crate::mm::frame::stats();
    let used_mib = ((total - free) * 4) as u64 * 10 / 1024;
    let total_mib = total as u64 * 4 / 1024;
    let t = &ui.totals;
    let right: [(char, u32, String); 5] = [
        ('•', DIM, format!("up {}", hms(ms))),
        ('•', DIM, format!("cpu {:>3}%", 100 - idle.min(100) as u32)),
        (
            '•',
            DIM,
            format!("mem {}.{}/{} MiB", used_mib / 10, used_mib % 10, total_mib),
        ),
        ('●', GHOST, format!("{} ckpt", t.checkpoints)),
        (
            '↻',
            GREEN,
            format!("{} restored  ✖ {} lost", t.restores, t.lost),
        ),
    ];
    let width: usize = right.iter().map(|(_, _, s)| s.chars().count() + 4).sum();
    let mut rx = cols - width - 1;
    for (icon, c, s) in right.iter() {
        g.put(rx, 0, *icon, Style::new(*c, hb));
        rx = g.text(rx + 2, 0, s, Style::new(TEXT, hb), 40) + 2;
    }

    // ── processes ───────────────────────────────────────────
    let (px, py, pw, ph) = (1, 2, 104, 9);
    let spark_max = rows
        .iter()
        .flat_map(|r| r.activity.iter())
        .copied()
        .max()
        .unwrap_or(1) as u32;
    g.panel(px, py, pw, ph, "PROCESSES", BLUE, border);
    let hs = Style::new(DIM, PANEL);
    let cx = px + 2;
    for (off, h) in [
        (0, "PID"),
        (5, "NAME"),
        (17, "STATE"),
        (30, "GHOST"),
        (44, "ACTIVITY"),
        (61, "CKPT"),
        (67, "RESTORE"),
        (76, "PAGES"),
        (83, "CHAIN"),
    ] {
        g.text(cx + off, py + 1, h, hs, 12);
    }
    for (i, r) in rows.iter().take(ph - 3).enumerate() {
        let y = py + 2 + i;
        let pc = proc_color(r.pid);
        let dead = r.state == State::Dead;
        // a freshly resurrected process glows for two seconds
        let rb = if r.restored_ms.is_some_and(|t| ms.saturating_sub(t) < 2000) {
            0x10301F
        } else {
            PANEL
        };
        g.fill(px + 1, y, pw - 2, 1, ' ', Style::new(TEXT, rb));
        let base = Style::new(if dead { DIM } else { TEXT }, rb);
        g.text(
            cx,
            y,
            &format!("{:>3}", r.pid),
            Style::new(pc, rb).bold(),
            4,
        );
        g.text(
            cx + 5,
            y,
            &r.name,
            if dead {
                base
            } else {
                Style::new(WHITE, rb).bold()
            },
            11,
        );
        let (dot, sc, st) = match (&r.state, &r.exit) {
            (State::Running, _) => ('●', GREEN, String::from("running")),
            (State::Ready, _) => ('●', BLUE, String::from("ready")),
            (State::Sleeping(_), _) => ('○', DIM, String::from("sleeping")),
            (State::WaitKey, _) => ('○', PURPLE, String::from("tty wait")),
            (State::WaitPid(p), _) => ('○', DIM, format!("wait {p}")),
            (State::Dead, Some(Exit::Crashed(_))) => ('✖', RED, String::from("CRASHED")),
            (State::Dead, Some(Exit::Killed)) => ('✖', ORANGE, String::from("killed")),
            (State::Dead, _) => ('•', DIM, String::from("exited")),
        };
        g.put(cx + 17, y, dot, Style::new(sc, rb));
        g.text(cx + 19, y, &st, Style::new(sc, rb), 10);
        if r.protected {
            g.put(cx + 30, y, '◆', Style::new(GHOST, rb));
            let label = if r.auto { "ghost auto" } else { "ghost" };
            g.text(cx + 32, y, label, Style::new(GHOST, rb), 11);
        } else {
            g.text(cx + 30, y, "-  mortal", Style::new(FAINT, rb), 11);
        }
        if !dead {
            sparkline(g, cx + 44, y, &r.activity, spark_max, pc, rb);
            g.text(
                cx + 57,
                y,
                &format!("{:>2}%", r.cpu_pct.min(99)),
                Style::new(DIM, rb),
                3,
            );
        } else if let Some(Exit::Crashed(why)) = &r.exit {
            g.text(cx + 44, y, why, Style::new(RED, rb), 17);
        }
        g.text(cx + 61, y, &format!("{:>4}", r.ckpts), base, 5);
        let rs = if r.restores > 0 {
            Style::new(GREEN, rb).bold()
        } else {
            base
        };
        g.text(cx + 67, y, &format!("{:>5}", r.restores), rs, 7);
        g.text(cx + 76, y, &format!("{:>5}", r.pages), base, 6);
        let mut chx = cx + 83;
        for &(full, _) in r
            .chain
            .iter()
            .rev()
            .take(6)
            .collect::<Vec<_>>()
            .iter()
            .rev()
        {
            let (ch, c) = if *full {
                ('█', GHOST)
            } else {
                ('▌', PURPLE)
            };
            g.put(chx, y, ch, Style::new(c, rb));
            chx += 2;
        }
    }

    // ── ghost timeline ──────────────────────────────────────
    // one lane per interesting process; each cell is half a second
    let (tx0, ty0, tw, th) = (1, 11, 104, 5);
    g.panel(tx0, ty0, tw, th, "GHOST TIMELINE", GHOST, border);
    let track_x = tx0 + 16;
    let track_w = tw - 19;
    let span_ms = track_w as u64 * 500;
    let mut lanes: Vec<(u32, u64)> = Vec::new();
    for e in ui
        .events
        .iter()
        .filter(|e| e.pid != 0 && ms.saturating_sub(e.ms) < span_ms)
    {
        if matches!(e.kind, Ev::Ckpt | Ev::Crash | Ev::Restore | Ev::GiveUp) {
            match lanes.iter_mut().find(|l| l.0 == e.pid) {
                Some(l) => l.1 = e.ms,
                None => lanes.push((e.pid, e.ms)),
            }
        }
    }
    // the most recently eventful ones, shown in pid order so lanes don't jump
    lanes.sort_by_key(|l| core::cmp::Reverse(l.1));
    lanes.truncate(th - 2);
    lanes.sort_by_key(|l| l.0);
    for (lane, &(pid, _)) in lanes.iter().enumerate() {
        let y = ty0 + 1 + lane;
        let name = rows
            .iter()
            .find(|r| r.pid == pid)
            .map(|r| r.name.as_str())
            .unwrap_or("?");
        g.text(
            tx0 + 2,
            y,
            &format!("{name}#{pid}"),
            Style::new(proc_color(pid), PANEL).bold(),
            13,
        );
        for i in 0..track_w {
            g.put(track_x + i, y, '─', Style::new(TRACK, PANEL));
        }
        // later events overwrite earlier ones in the same cell, except
        // that crashes and restores always beat routine checkpoints
        let rank = |k: Ev, full: bool| match k {
            Ev::GiveUp | Ev::Crash => 4,
            Ev::Restore => 3,
            Ev::Ckpt if full => 2,
            _ => 1,
        };
        let mut best = alloc::vec![0u8; track_w];
        let mut marks: Vec<Option<(char, u32)>> = alloc::vec![None; track_w];
        for e in ui
            .events
            .iter()
            .filter(|e| e.pid == pid && ms.saturating_sub(e.ms) < span_ms)
        {
            let age = (ms - e.ms) / 500;
            let Some(i) = (track_w - 1).checked_sub(age as usize) else {
                continue;
            };
            let full = e.text.contains("FULL");
            let (ch, c) = match e.kind {
                Ev::Ckpt if full => ('◆', GHOST),
                Ev::Ckpt => ('•', PURPLE),
                Ev::Crash => ('✖', RED),
                Ev::Restore => ('↻', GREEN),
                Ev::GiveUp => ('✖', ORANGE),
                _ => continue,
            };
            let r = rank(e.kind, full);
            // a restore lands in the same half-second as its crash: nudge
            // the crash one cell left so both stay visible
            if e.kind == Ev::Restore && best[i] == 4 && i > 0 {
                marks[i - 1] = marks[i];
                best[i - 1] = 4;
                best[i] = 0;
            }
            if r >= best[i] {
                best[i] = r;
                marks[i] = Some((ch, c));
            }
        }
        for (i, m) in marks.iter().enumerate() {
            if let Some((ch, c)) = m {
                g.put(track_x + i, y, *ch, Style::new(*c, PANEL).bold());
            }
        }
    }
    if lanes.is_empty() {
        g.text(
            track_x,
            ty0 + 2,
            "waiting for the first checkpoint...",
            Style::new(FAINT, PANEL),
            50,
        );
    }
    // time axis lives in the bottom border
    let by = ty0 + th - 1;
    for s in [30, 20, 10] {
        let off = (s * 2) as usize;
        if off < track_w {
            g.text(
                track_x + track_w - 1 - off - 1,
                by,
                &format!("-{s}s"),
                Style::new(DIM, BG),
                4,
            );
        }
    }
    g.text(
        track_x + track_w - 3,
        by,
        "now",
        Style::new(GHOST, BG).bold(),
        3,
    );
    let legend = [
        ('◆', GHOST, "full"),
        ('•', PURPLE, "delta"),
        ('✖', RED, "crash"),
        ('↻', GREEN, "restore"),
    ];
    let mut lx = tx0 + 3;
    for (ch, c, label) in legend {
        g.put(lx, by, ch, Style::new(c, BG));
        lx = g.text(lx + 1, by, label, Style::new(DIM, BG), 8) + 2;
    }

    // ── window: the shared drawing panel ────────────────────
    let (wx, wy, ww, wh) = (106, 2, 75, 14);
    let title = match &ui.panel {
        Some(p) if p.owner != 0 => format!("WINDOW · pid {} · {}", p.owner, p.title),
        Some(p) => format!("WINDOW · {} · owner gone", p.title),
        None => String::from("WINDOW"),
    };
    g.panel(wx, wy, ww, wh, &title, PINK, border);
    if let Some(p) = &ui.panel {
        let alive = p.owner != 0;
        let on = if alive { GHOST } else { FAINT };
        let off = PANEL;
        let (iw, ih) = (ww - 2, wh - 2);
        for cy in 0..ih {
            for cxx in 0..iw.min(p.w) {
                let top =
                    p.cells.get((cy * 2) * p.w + cxx).copied().unwrap_or(0) != 0 && cy * 2 < p.h;
                let bot = p.cells.get((cy * 2 + 1) * p.w + cxx).copied().unwrap_or(0) != 0
                    && cy * 2 + 1 < p.h;
                let ch = match (top, bot) {
                    (true, true) => '█',
                    (true, false) => '▀',
                    (false, true) => '▄',
                    _ => ' ',
                };
                g.put(wx + 1 + cxx, wy + 1 + cy, ch, Style::new(on, off));
            }
        }
    } else {
        g.text(
            wx + 3,
            wy + 6,
            "no process is drawing.  try:  run life",
            Style::new(DIM, PANEL),
            60,
        );
    }

    // ── console ─────────────────────────────────────────────
    let (cx0, cy0, cw, ch) = (1, 16, 110, 32);
    g.panel(cx0, cy0, cw, ch, "CONSOLE", GREEN, border);
    let inner_w = cw - 4;
    let body_h = ch - 3;
    let start = ui.lines.len().saturating_sub(body_h);
    for (i, line) in ui.lines.iter().skip(start).enumerate() {
        let y = cy0 + 1 + i;
        let mut x = cx0 + 2;
        if let Some((name, c)) = &line.tag {
            g.text(x, y, &format!("{:>7}", name), Style::new(*c, PANEL), 7);
            g.put(x + 8, y, '│', Style::new(FAINT, PANEL));
            x += 10;
        }
        for (c, s) in &line.segs {
            let lim = (cx0 + 2 + inner_w).saturating_sub(x);
            x = g.text(x, y, s, Style::new(*c, PANEL), lim);
        }
    }
    // input line of whoever owns the terminal
    let iy = cy0 + ch - 2;
    g.fill(cx0 + 1, iy, cw - 2, 1, ' ', Style::new(TEXT, HEADER));
    let mut x = cx0 + 2;
    if let Some(tty) = ui.tty {
        if let Some((_, segs, _)) = ui.pending.get(&tty) {
            for (c, s) in segs {
                let lim = (cx0 + 2 + inner_w).saturating_sub(x);
                x = g.text(x, iy, s, Style::new(*c, HEADER), lim);
            }
        }
        if (ms / 500) % 2 == 0 {
            g.put(x, iy, '█', Style::new(GHOST, HEADER));
        }
    }

    // ── ghost event log ─────────────────────────────────────
    let (ex, ey, ew, eh) = (112, 16, 69, 20);
    g.panel(ex, ey, ew, eh, "GHOST LOG", GHOST, border);
    let n = eh - 2;
    let start = ui.events.len().saturating_sub(n);
    for (i, e) in ui.events.iter().skip(start).enumerate() {
        let y = ey + 1 + i;
        let x = ex + 2;
        g.text(
            x,
            y,
            &format!("{:>4}.{:02}", e.ms / 1000, (e.ms % 1000) / 10),
            Style::new(FAINT, PANEL),
            7,
        );
        g.put(x + 8, y, e.kind.icon(), Style::new(e.kind.color(), PANEL));
        g.text(
            x + 10,
            y,
            e.kind.label(),
            Style::new(e.kind.color(), PANEL).bold(),
            7,
        );
        let wx = g.text(x + 18, y, &e.who, Style::new(WHITE, PANEL), 11);
        g.text(
            wx.max(x + 29) + 1,
            y,
            &e.text,
            Style::new(TEXT, PANEL),
            ew - 33,
        );
    }

    // ── checkpoint chains ───────────────────────────────────
    let (kx, ky, kw, kh) = (112, 36, 69, 12);
    g.panel(kx, ky, kw, kh, "CHECKPOINT CHAINS", PURPLE, border);
    let mut y = ky + 1;
    for r in rows
        .iter()
        .filter(|r| r.protected && r.state != State::Dead)
    {
        if y + 2 > ky + kh - 1 {
            break;
        }
        let x = kx + 2;
        let nx = g.text(
            x,
            y,
            &format!("{}#{}", r.name, r.pid),
            Style::new(proc_color(r.pid), PANEL).bold(),
            14,
        );
        let info = format!(
            "{} images · {} KiB · last {}",
            r.chain.len(),
            r.image_bytes.div_ceil(1024),
            r.last_ckpt_ms
                .map(|t| format!("{} ms ago", ms.saturating_sub(t)))
                .unwrap_or(String::from("never"))
        );
        g.text(nx.max(x + 15), y, &info, Style::new(DIM, PANEL), kw - 19);
        let mut cxx = x;
        let chips: Vec<String> = r
            .chain
            .iter()
            .map(|&(full, pages)| {
                if full {
                    format!(" FULL {pages} ")
                } else {
                    format!(" Δ{pages} ")
                }
            })
            .collect();
        // newest generations win if the row is too short for all of them
        let room = kw - 6;
        let mut first = 0;
        while chips[first..]
            .iter()
            .map(|c| c.chars().count() + 1)
            .sum::<usize>()
            > room
        {
            first += 1;
        }
        if first > 0 {
            cxx = g.text(cxx, y + 1, "..", Style::new(DIM, PANEL), 2) + 1;
        }
        for (chip, &(full, _)) in chips.iter().zip(&r.chain).skip(first) {
            let bg = if full { GHOST } else { PURPLE };
            cxx = g.text(cxx, y + 1, chip, Style::new(BG, bg).bold(), 12) + 1;
        }
        if r.chain.is_empty() {
            g.text(cxx, y + 1, "no images yet", Style::new(FAINT, PANEL), 20);
        }
        y += 3;
    }
    if y == ky + 1 {
        g.text(
            kx + 2,
            y,
            "no protected processes",
            Style::new(FAINT, PANEL),
            40,
        );
    }

    // ── footer ──────────────────────────────────────────────
    let fy = g.rows - 2;
    let help: &[(&str, &str)] = &[
        ("ps", "processes"),
        ("ls /ckpt", "images"),
        ("run <prog>", "start"),
        ("crash <pid> [pf|gp|stack|flip]", "real fault"),
        ("ckpt <pid>", "snapshot"),
        ("protect <pid>", "give it a ghost"),
    ];
    let mut x = 2;
    for (k, v) in help {
        x = g.text(x, fy, k, Style::new(GHOST, BG), 40);
        x = g.text(x + 1, fy, v, Style::new(DIM, BG), 30) + 3;
    }
    let stat = format!(
        "heap {} KiB · ramfs {} KiB",
        heap_used / 1024,
        fs_bytes / 1024
    );
    let sl = stat.chars().count();
    g.text(cols - sl - 1, fy, &stat, Style::new(FAINT, BG), sl);

    // ── banner overlay ──────────────────────────────────────
    let mut big = None;
    if let Some(b) = ui.banners.front() {
        let (bg, fg, icon) = match b.kind {
            BannerKind::Crash => (RED_DEEP, RED, '✖'),
            BannerKind::Restore => (GREEN_DEEP, GREEN, '↻'),
            BannerKind::Lost => (0x431407, ORANGE, '✖'),
        };
        let bw = 70;
        let bh = b.lines.len() + if b.pages > 0 { 9 } else { 7 };
        let bx = cx0 + (cw - bw) / 2;
        let by = cy0 + (ch - bh) / 2;
        g.fill(bx, by, bw, bh, ' ', Style::new(WHITE, bg));
        for xx in bx..bx + bw {
            g.put(xx, by, '▀', Style::new(fg, bg));
            g.put(xx, by + bh - 1, '▄', Style::new(fg, bg));
        }
        for yy in by..by + bh {
            g.put(bx, yy, '█', Style::new(fg, bg));
            g.put(bx + bw - 1, yy, '█', Style::new(fg, bg));
        }
        // title in 32px type: each glyph spans 2x2 cells
        let tl = b.title.chars().count() * 2;
        let tx = bx + (bw - tl) / 2;
        g.put(tx - 3, by + 2, icon, Style::new(fg, bg).bold());
        g.put(tx + tl + 2, by + 2, icon, Style::new(fg, bg).bold());
        for (i, l) in b.lines.iter().enumerate() {
            let ll = l.chars().count().min(bw - 4);
            g.text(
                bx + (bw - ll) / 2,
                by + 5 + i,
                l,
                Style::new(WHITE, bg),
                bw - 4,
            );
        }
        if b.pages > 0 {
            // "rebuilding" bar that fills over the first ~0.7 s
            let elapsed = now.saturating_sub(b.shown_at.unwrap_or(now));
            let w = 40;
            let pct = (elapsed * 100 / 70).min(100) as u8;
            let done = b.pages * pct as usize / 100;
            let yb = by + 5 + b.lines.len() + 1;
            let label = format!(" {done:>3}/{} pages", b.pages);
            let start = bx + (bw - w - label.chars().count()) / 2;
            bar(g, start, yb, w, pct, GREEN, 0x0B3B20);
            g.text(start + w, yb, &label, Style::new(WHITE, bg), 16);
        }
        big = Some((tx, by + 1, b.title.clone(), bg));
    }

    drop(ui);
    g.flush();
    if let Some((x, y, title, bg)) = big {
        g.big_text(x, y, &title, WHITE, bg);
    }
}

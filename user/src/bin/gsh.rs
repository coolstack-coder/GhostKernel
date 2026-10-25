//! gsh - the ghost shell. Runs in ring 3 like everything else; every
//! command below is just a syscall or two.

#![no_std]
#![no_main]

use ulib::*;

entry!(main);

const PROMPT: &str = "\x1bcghost\x1bd@\x1bpkernel\x1b0 \x1bd$\x1b0 ";

fn main(_: &str) -> i64 {
    set_tty();
    cat("/etc/motd");
    let mut line = [0u8; 160];
    loop {
        print!("{PROMPT}");
        let n = read_line(&mut line);
        let cmd = core::str::from_utf8(&line[..n]).unwrap_or("").trim();
        if !cmd.is_empty() {
            run(cmd);
        }
    }
}

/// Line editing with echo. The console treats \x08 as "rub out".
fn read_line(buf: &mut [u8]) -> usize {
    let mut n = 0;
    let mut c = [0u8; 1];
    loop {
        if read(0, &mut c) <= 0 {
            continue;
        }
        match c[0] {
            b'\n' | b'\r' => {
                write(1, b"\n");
                return n;
            }
            0x08 | 0x7f => {
                if n > 0 {
                    n -= 1;
                    write(1, b"\x08");
                }
            }
            b if (0x20..0x7f).contains(&b) && n < buf.len() => {
                buf[n] = b;
                n += 1;
                write(1, &c);
            }
            _ => {}
        }
    }
}

fn parse_pid(s: Option<&str>) -> Option<u32> {
    s.and_then(|s| s.parse().ok())
}

fn errmsg(e: i64) -> &'static str {
    match e {
        -2 => "no such file or directory",
        -3 => "no such process",
        -8 => "not an executable",
        -9 => "bad file descriptor",
        -12 => "out of memory",
        -14 => "bad address",
        -17 => "already exists",
        -20 => "not a directory",
        -21 => "is a directory",
        -1 => "not allowed",
        _ => "error",
    }
}

fn run(cmd: &str) {
    let mut it = cmd.split_whitespace();
    let Some(word) = it.next() else { return };
    match word {
        "help" => help(),
        "ps" => ps_cmd(),
        "ls" => ls(it.next().unwrap_or("/")),
        "cat" => match it.next() {
            Some(p) => cat(p),
            None => println!("usage: cat <file>"),
        },
        "tail" => match it.next() {
            Some(p) => tail(p, it.next().and_then(|n| n.parse().ok()).unwrap_or(5)),
            None => println!("usage: tail <file> [n]"),
        },
        "run" => match it.next() {
            Some(prog) => {
                let mut path = ulib::fmt::LineBuf::<64>::new();
                let _ = core::fmt::Write::write_fmt(
                    &mut path,
                    format_args!(
                        "{}{}",
                        if prog.starts_with('/') { "" } else { "/bin/" },
                        prog
                    ),
                );
                let arg = cmd.splitn(3, char::is_whitespace).nth(2).unwrap_or("");
                let pid = spawn(path.as_str(), arg);
                if pid < 0 {
                    println!("\x1brrun: {}: {}", path.as_str(), errmsg(pid));
                } else {
                    println!("started \x1bw{prog}\x1b0 as pid \x1bb{pid}");
                }
            }
            None => println!("usage: run <program> [arg]   (see: ls /bin)"),
        },
        "kill" => match parse_pid(it.next()) {
            Some(pid) => report(kill(pid), "killed"),
            None => println!("usage: kill <pid>"),
        },
        "crash" => match parse_pid(it.next()) {
            Some(pid) => {
                let kind = match it.next().unwrap_or("pf") {
                    "pf" => 0,
                    "gp" => 1,
                    "stack" => 2,
                    "flip" => 3,
                    other => {
                        println!("unknown fault '{other}' (pf, gp, stack, flip)");
                        return;
                    }
                };
                let r = inject(pid, kind);
                if r < 0 {
                    println!("\x1brcrash: {}", errmsg(r));
                } else {
                    println!(
                        "\x1bk⚡\x1b0 fault armed in pid {pid} - it goes off the next time it runs"
                    );
                }
            }
            None => println!("usage: crash <pid> [pf|gp|stack|flip]"),
        },
        "ckpt" => match parse_pid(it.next()) {
            Some(pid) => {
                let r = checkpoint_other(pid);
                if r < 0 {
                    println!("\x1brckpt: {}", errmsg(r));
                } else {
                    println!("pid {pid} checkpointed as generation \x1bc#{r}");
                }
            }
            None => println!("usage: ckpt <pid>"),
        },
        "protect" => match parse_pid(it.next()) {
            Some(pid) => {
                let ms = it.next().and_then(|s| s.parse().ok()).unwrap_or(1000);
                let r = protect(pid, ms);
                report(r, "protected");
                if r >= 0 {
                    checkpoint_other(pid);
                }
            }
            None => println!("usage: protect <pid> [auto_ms]"),
        },
        "unprotect" => match parse_pid(it.next()) {
            Some(pid) => report(unprotect(pid), "is mortal again"),
            None => println!("usage: unprotect <pid>"),
        },
        "stat" | "stats" => stats(),
        "uptime" => {
            let ms = uptime_ms();
            println!("up {}.{:03}s", ms / 1000, ms % 1000);
        }
        "echo" => {
            println!("{}", cmd.split_once(' ').map(|x| x.1).unwrap_or(""));
        }
        "write" => {
            let (Some(path), Some(text)) = (it.next(), cmd.splitn(3, ' ').nth(2)) else {
                println!("usage: write <file> <text>");
                return;
            };
            let fd = open(path, O_WRITE | O_CREATE | O_APPEND);
            if fd < 0 {
                println!("\x1brwrite: {}", errmsg(fd));
                return;
            }
            write(fd, text.as_bytes());
            write(fd, b"\n");
            close(fd);
        }
        "rm" => match it.next() {
            Some(p) => report(unlink(p), "removed"),
            None => println!("usage: rm <file>"),
        },
        "mkdir" => match it.next() {
            Some(p) => report(mkdir(p), "created"),
            None => println!("usage: mkdir <dir>"),
        },
        "clear" => {
            for _ in 0..30 {
                println!();
            }
        }
        "exit" => exit(0),
        _ => println!("\x1br{word}\x1b0: command not found \x1bd(try help)"),
    }
}

fn report(r: i64, ok: &str) {
    if r < 0 {
        println!("\x1br{}", errmsg(r));
    } else {
        println!("{ok}");
    }
}

fn help() {
    println!("\x1bwgsh\x1b0 - the ghost shell");
    let rows: [(&str, &str); 17] = [
        ("ps", "list processes, their ghosts and restore counts"),
        ("ls [dir]", "list a directory (try /bin, /ckpt, /data)"),
        ("cat <file>", "print a file"),
        ("tail <file> [n]", "last n lines of a file"),
        ("run <prog> [arg]", "start /bin/<prog>"),
        ("kill <pid>", "terminate a process"),
        (
            "crash <pid> [kind]",
            "inject a real CPU fault: pf | gp | stack | flip",
        ),
        ("ckpt <pid>", "take a checkpoint of another process now"),
        (
            "protect <pid> [ms]",
            "give a process a ghost (auto-checkpoint every ms)",
        ),
        ("unprotect <pid>", "make it mortal again"),
        ("stats", "kernel-wide numbers"),
        ("write <f> <text>", "append a line to a file"),
        ("rm / mkdir", "file housekeeping"),
        ("echo, uptime", "the classics"),
        ("clear", "scroll the console clean"),
        ("exit", "leave (init restarts the shell)"),
        ("", ""),
    ];
    for (c, d) in rows.iter().filter(|r| !r.0.is_empty()) {
        println!("  \x1bc{c:<20}\x1b0 {d}");
    }
}

fn ps_cmd() {
    let mut list = [PsEntry::EMPTY; 16];
    let n = ps(&mut list);
    println!("\x1bd  PID  NAME        STATE      GHOST       CKPTS  RESTORES  PAGES   CPU-MS");
    for e in &list[..n] {
        let (st, col) = match e.state {
            0 => ("running", 'g'),
            1 => ("ready", 'b'),
            2 => ("sleeping", 'd'),
            3 => ("waiting", 'p'),
            5 => ("CRASHED", 'r'),
            _ => ("exited", 'd'),
        };
        let ghost = match (e.protected, e.auto) {
            (0, _) => "\x1bd-",
            (_, 0) => "\x1bcprotected",
            _ => "\x1bcauto",
        };
        println!(
            "  {:>3}  \x1bw{:<10}\x1b0  \x1b{col}{:<9}\x1b0  {:<10}\x1b0  {:>5}  {:>8}  {:>5}  {:>7}",
            e.pid,
            e.name(),
            st,
            ghost,
            e.ckpts,
            e.restores,
            e.pages,
            e.cpu_ms
        );
    }
}

fn ls(path: &str) {
    let mut buf = [0u8; 2048];
    let n = readdir(path, &mut buf);
    if n < 0 {
        println!("\x1brls: {path}: {}", errmsg(n));
        return;
    }
    let text = core::str::from_utf8(&buf[..n as usize]).unwrap_or("");
    let mut count = 0;
    for line in text.lines() {
        let mut f = line.splitn(3, ' ');
        let (kind, size, name) = (
            f.next().unwrap_or(""),
            f.next().unwrap_or("0"),
            f.next().unwrap_or(""),
        );
        if kind == "d" {
            println!("  \x1bb{name}/");
        } else {
            let size: u64 = size.parse().unwrap_or(0);
            let col = if name.ends_with(".img") {
                'c'
            } else if name.ends_with(".delta") {
                'p'
            } else {
                '0'
            };
            if size >= 1024 {
                println!("  \x1b{col}{name:<28}\x1bd{:>7} KiB", size.div_ceil(1024));
            } else {
                println!("  \x1b{col}{name:<28}\x1bd{:>7} B", size);
            }
        }
        count += 1;
    }
    if count == 0 {
        println!("\x1bd  (empty)");
    }
}

fn cat(path: &str) {
    let fd = open(path, O_READ);
    if fd < 0 {
        println!("\x1brcat: {path}: {}", errmsg(fd));
        return;
    }
    let mut buf = [0u8; 256];
    loop {
        let n = read(fd, &mut buf);
        if n <= 0 {
            break;
        }
        write(1, &buf[..n as usize]);
    }
    close(fd);
}

fn tail(path: &str, want: usize) {
    let fd = open(path, O_READ);
    if fd < 0 {
        println!("\x1brtail: {path}: {}", errmsg(fd));
        return;
    }
    // two passes, no heap: count the lines, then print the last `want`
    let mut buf = [0u8; 256];
    let mut total = 0usize;
    loop {
        let n = read(fd, &mut buf);
        if n <= 0 {
            break;
        }
        total += buf[..n as usize].iter().filter(|&&b| b == b'\n').count();
    }
    seek(fd, 0);
    let skip = total.saturating_sub(want);
    let mut seen = 0usize;
    loop {
        let n = read(fd, &mut buf);
        if n <= 0 {
            break;
        }
        let mut start = 0;
        for (i, &b) in buf[..n as usize].iter().enumerate() {
            if b == b'\n' {
                if seen >= skip {
                    write(1, &buf[start..=i]);
                }
                seen += 1;
                start = i + 1;
            }
        }
        if seen >= skip && start < n as usize {
            write(1, &buf[start..n as usize]);
        }
    }
    close(fd);
    println!("\x1bd({total} lines total)");
}

fn stats() {
    let s = sysinfo();
    let used = (s.frames_total - s.frames_free) * 4;
    println!(
        "uptime        {}.{:03} s",
        s.uptime_ms / 1000,
        s.uptime_ms % 1000
    );
    println!(
        "memory        {} KiB used of {} KiB",
        used,
        s.frames_total * 4
    );
    println!(
        "kernel heap   {} KiB of {} KiB",
        s.heap_used / 1024,
        s.heap_total / 1024
    );
    println!(
        "checkpoints   \x1bc{}\x1b0  ({} KiB written)",
        s.checkpoints,
        s.ckpt_bytes / 1024
    );
    println!("crashes       \x1br{}", s.crashes);
    println!("restores      \x1bg{}", s.restores);
}

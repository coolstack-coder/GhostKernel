//! A long-running computation that protects itself.
//!
//! It finds primes in batches, appends each batch to /data/primes.log and
//! takes a checkpoint every few batches. `checkpoint()` works like setjmp:
//! after a crash the kernel brings us back *inside* that call with
//! `Restored(seq)`, our memory exactly as it was - and the log file rolled
//! back to match, so no line is ever written twice.

#![no_std]
#![no_main]

use core::fmt::Write;
use ulib::fmt::LineBuf;
use ulib::*;

entry!(main);

const BATCH: u64 = 300;
const CKPT_EVERY: u64 = 3;
const LOG: &str = "/data/primes.log";

fn is_prime(n: u64) -> bool {
    if n < 2 {
        return false;
    }
    if n % 2 == 0 {
        return n == 2;
    }
    let mut d = 3;
    while d * d <= n {
        if n.is_multiple_of(d) {
            return false;
        }
        d += 2;
    }
    true
}

/// Count lines in the log - used to prove the file matches our memory.
fn log_lines() -> u64 {
    let fd = open(LOG, O_READ);
    if fd < 0 {
        return 0;
    }
    let mut buf = [0u8; 512];
    let mut lines = 0;
    loop {
        let n = read(fd, &mut buf);
        if n <= 0 {
            break;
        }
        lines += buf[..n as usize].iter().filter(|&&b| b == b'\n').count() as u64;
    }
    close(fd);
    lines
}

fn main(_: &str) -> i64 {
    let log = open(LOG, O_WRITE | O_CREATE | O_TRUNC);
    if log < 0 {
        println!("\x1brcannot open {LOG}: {log}");
        return 1;
    }

    let mut found: u64 = 0;
    let mut candidate: u64 = 1;
    let mut batches: u64 = 0;

    loop {
        // the safe point: everything above this line is in the image
        if batches.is_multiple_of(CKPT_EVERY) {
            match checkpoint() {
                Ckpt::Saved(_) => {}
                Ckpt::Restored(seq) => {
                    let lines = log_lines();
                    let ok = lines == batches;
                    println!(
                        "\x1bg↻ back from the dead\x1b0 at checkpoint #{seq}: resuming at prime #{found} (not #0)"
                    );
                    println!(
                        "  log check: {lines} lines on disk, {batches} batches in memory  {}",
                        if ok {
                            "\x1bg✔ consistent, no duplicates"
                        } else {
                            "\x1brMISMATCH"
                        }
                    );
                }
                Ckpt::Failed(e) => println!("\x1brcheckpoint failed: {e}"),
            }
        }

        let mut last = 0;
        let mut n = 0;
        while n < BATCH {
            candidate += 1;
            if is_prime(candidate) {
                n += 1;
                last = candidate;
            }
        }
        found += BATCH;
        batches += 1;

        let mut line = LineBuf::<96>::new();
        let _ = writeln!(
            line,
            "batch {batches:>4}: primes up to #{found} - latest {last}"
        );
        write(log, line.as_bytes());
        println!(
            "#{found:<6} latest {last:<8} \x1bd{batches} batch{} in {LOG}",
            if batches == 1 { "" } else { "es" }
        );
        sleep_ms(250);
    }
}

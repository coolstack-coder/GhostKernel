//! A program with a real bug, no injection needed.
//!
//!   run flaky        - crashes at random (a transient fault): ghost brings
//!                      it back and the retry usually goes fine
//!   run flaky det    - crashes at the same step every time: watch the
//!                      kernel fall back through older checkpoints, then
//!                      give up instead of looping forever

#![no_std]
#![no_main]

use ulib::*;

entry!(main);

fn main(arg: &str) -> i64 {
    let deterministic = arg.trim() == "det";
    protect(0, 0);
    let mut rng = Rng::seeded();
    let mut work: u64 = 0;
    println!(
        "flaky up ({} mode)",
        if deterministic {
            "deterministic bug"
        } else {
            "transient bug"
        }
    );
    loop {
        match checkpoint() {
            Ckpt::Restored(seq) => println!("\x1bgrestored\x1b0 at #{seq}, work={work}"),
            Ckpt::Saved(_) | Ckpt::Failed(_) => {}
        }
        for _ in 0..4 {
            work += 1;
            println!("step {work}");
            sleep_ms(300);
            let boom = if deterministic {
                work >= 10
            } else {
                rng.next_u64() % 7 == 0
            };
            if boom {
                println!("\x1byabout to dereference a bad pointer...");
                // a genuine wild write - the CPU raises #PF in ring 3
                unsafe { core::ptr::write_volatile(0x10 as *mut u64, work) };
            }
        }
    }
}

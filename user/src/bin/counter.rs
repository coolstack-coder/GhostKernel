//! The control group. Counts forever, holds no checkpoints. Crash it and
//! everything it ever did is gone - which is the point of the comparison.

#![no_std]
#![no_main]

use ulib::*;

entry!(main);

fn main(_: &str) -> i64 {
    let mut n: u64 = 0;
    loop {
        // a bit of busy work so it shows up in the CPU column
        let mut acc = 0u64;
        for i in 0..400_000u64 {
            acc = acc.wrapping_mul(31).wrapping_add(i);
        }
        core::hint::black_box(acc);
        n += 1;
        if n % 4 == 0 {
            println!(
                "tick {} \x1bd(no ghost - if I crash, this count is lost)",
                n / 4
            );
        }
        sleep_ms(250);
    }
}

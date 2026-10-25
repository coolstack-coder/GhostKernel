//! pid 1. Starts the cast, then babysits the shell.

#![no_std]
#![no_main]

use ulib::*;

entry!(main);

fn main(_: &str) -> i64 {
    println!("\x1bcGhostKernel init\x1b0 - bringing up user space");

    // primes protects itself: it calls checkpoint() at points it chooses
    let primes = spawn("/bin/primes", "");
    // life knows nothing about checkpoints - the kernel snapshots it on a timer
    let life = spawn("/bin/life", "");
    if life > 0 {
        protect(life as u32, 1000);
    }
    // counter is the control group: no ghost, no second chances
    let counter = spawn("/bin/counter", "");
    println!(
        "started primes=\x1bb{primes}\x1b0  life=\x1bb{life}\x1b0 (auto-ckpt 1s)  counter=\x1bb{counter}\x1b0 (unprotected)"
    );

    loop {
        let sh = spawn("/bin/gsh", "");
        if sh < 0 {
            println!("\x1brinit: cannot start shell ({sh})");
            return 1;
        }
        wait(sh as u32);
        println!("\x1byinit: shell exited, restarting it");
        sleep_ms(300);
    }
}

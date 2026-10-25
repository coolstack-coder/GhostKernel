//! Conway's Game of Life in the dashboard window.
//!
//! This program has no idea checkpoints exist. init asks the kernel to
//! snapshot it every second from the timer interrupt; when it crashes, the
//! board visibly snaps back to the last snapshot and keeps evolving.

#![no_std]
#![no_main]

use core::fmt::Write;
use ulib::fmt::LineBuf;
use ulib::*;

entry!(main);

const W: usize = 73;
const H: usize = 24;

static mut BOARD: [[u8; W * H]; 2] = [[0; W * H]; 2];

fn seed(b: &mut [u8; W * H], rng: &mut Rng) {
    for c in b.iter_mut() {
        *c = (rng.next_u64() % 100 < 28) as u8;
    }
    // and a glider gun's worth of order in the chaos: a few gliders
    for (gx, gy) in [(2, 2), (20, 4), (40, 10), (60, 3)] {
        for (dx, dy) in [(1, 0), (2, 1), (0, 2), (1, 2), (2, 2)] {
            b[(gy + dy) * W + gx + dx] = 1;
        }
    }
}

fn step(cur: &[u8; W * H], next: &mut [u8; W * H]) -> usize {
    let mut alive = 0;
    for y in 0..H {
        for x in 0..W {
            let mut n = 0;
            for dy in [H - 1, 0, 1] {
                for dx in [W - 1, 0, 1] {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    n += cur[((y + dy) % H) * W + (x + dx) % W];
                }
            }
            let live = cur[y * W + x] == 1;
            let v = (n == 3 || (live && n == 2)) as u8;
            next[y * W + x] = v;
            alive += v as usize;
        }
    }
    alive
}

fn main(_: &str) -> i64 {
    // the only reference to BOARD that ever exists
    #[allow(clippy::deref_addrof)]
    let boards = unsafe { &mut *(&raw mut BOARD) };
    let mut rng = Rng::seeded();
    seed(&mut boards[0], &mut rng);

    let mut generation: u64 = 0;
    let mut stale = 0;
    let mut last_alive = 0;
    loop {
        let (a, b) = boards.split_at_mut(1);
        let (cur, next) = if generation % 2 == 0 {
            (&a[0], &mut b[0])
        } else {
            (&b[0], &mut a[0])
        };
        let alive = step(cur, next);
        generation += 1;

        let mut title = LineBuf::<40>::new();
        let _ = write!(title, "life · gen {generation} · {alive} cells");
        draw(next, W, H, title.as_str());

        // a dead or frozen board is boring: reseed
        stale = if alive == last_alive { stale + 1 } else { 0 };
        last_alive = alive;
        if alive < 40 || stale > 30 {
            seed(next, &mut rng);
            stale = 0;
        }
        if generation % 50 == 0 {
            println!("generation {generation}, {alive} cells alive");
        }
        sleep_ms(90);
    }
}

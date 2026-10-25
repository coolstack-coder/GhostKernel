//! CPU exceptions. From ring 0 they're kernel bugs and we stop. From ring 3
//! they're a process crashing - which is exactly the moment GhostKernel
//! exists for.

use super::{Exit, PROCS};
use crate::arch::cpu;
use crate::arch::trap::{EXCEPTION_NAMES, EXCEPTION_SHORT, TrapFrame};
use crate::ghost::{self, Crash};
use alloc::format;

pub fn on_exception(frame: &mut TrapFrame) {
    let v = frame.vector as usize;
    if !frame.from_user() {
        panic!(
            "kernel {} ({}) at rip={:#x} err={:#x} cr2={:#x}",
            EXCEPTION_SHORT[v],
            EXCEPTION_NAMES[v],
            frame.rip,
            frame.error,
            cpu::read_cr2()
        );
    }

    let crash = Crash {
        vector: v as u8,
        rip: frame.rip,
        rsp: frame.rsp,
        error: frame.error,
        cr2: cpu::read_cr2(),
    };
    let mut guard = PROCS.lock();
    let t = &mut *guard;
    let pid = t
        .current
        .take()
        .expect("user fault with no current process");
    {
        let mut fs = crate::fs::FS.lock();
        let p = t.procs.get_mut(&pid).unwrap();
        p.ctx = *frame;
        match ghost::recover(p, &mut fs, &crash) {
            Ok(()) => {
                // straight back to the front of the line
                p.state = super::State::Ready;
                t.queue.push_front(pid);
            }
            Err(why) => {
                drop(fs);
                t.terminate(
                    pid,
                    Exit::Crashed(format!("{} {}", EXCEPTION_SHORT[v], why)),
                );
            }
        }
    }
    super::sched::pick_next(t, frame);
}

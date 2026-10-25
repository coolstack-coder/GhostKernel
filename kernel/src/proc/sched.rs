//! Preemptive round-robin. A context switch never touches a kernel stack:
//! we copy the interrupted frame into the outgoing process and overwrite
//! it with the incoming one, then `iretq` does the rest.

use super::{PROCS, ProcTable, State};
use crate::arch::trap::TrapFrame;
use crate::arch::{cpu, gdt, pic, pit};
use crate::mm::paging;

/// Timer ticks per time slice (10 ms each).
const QUANTUM: u32 = 3;
const IDLE_STACK_SIZE: usize = 32 * 1024;

#[repr(C, align(16))]
struct IdleStack([u8; IDLE_STACK_SIZE]);
static mut IDLE_STACK: IdleStack = IdleStack([0; IDLE_STACK_SIZE]);

extern "C" fn idle_loop() -> ! {
    loop {
        cpu::sti();
        cpu::hlt();
    }
}

/// A ring-0 frame that parks the CPU in `hlt` until the next interrupt.
pub fn idle_frame() -> TrapFrame {
    TrapFrame {
        rip: idle_loop as *const () as u64,
        cs: gdt::KERNEL_CS as u64,
        rflags: 0x202,
        rsp: (&raw const IDLE_STACK) as u64 + IDLE_STACK_SIZE as u64,
        ss: gdt::KERNEL_DS as u64,
        ..Default::default()
    }
}

pub fn on_timer(frame: &mut TrapFrame) {
    let now = pit::tick();
    pic::eoi(0);
    {
        let mut guard = PROCS.lock();
        let t = &mut *guard;
        if !t.started {
            return;
        }
        match t.current_mut() {
            Some(p) => {
                p.cpu_ticks += 1;
                p.cpu_window += 1;
            }
            None => {
                t.idle_ticks += 1;
                t.idle_window += 1;
            }
        }
        if now.is_multiple_of(pit::HZ / 4) {
            for p in t.procs.values_mut() {
                p.sample_activity();
            }
        }
        if now.is_multiple_of(pit::HZ) {
            for p in t.procs.values_mut() {
                p.cpu_pct = p.cpu_window.min(100) as u8;
                p.cpu_window = 0;
            }
            t.idle_pct = t.idle_window.min(100) as u8;
            t.idle_window = 0;
        }

        let wake: alloc::vec::Vec<u32> = t
            .procs
            .values()
            .filter(|p| matches!(p.state, State::Sleeping(w) if w <= now))
            .map(|p| p.pid)
            .collect();
        for pid in wake {
            t.make_ready(pid);
        }

        // Transparent checkpoints. The running process is caught right here
        // at a preemption point; everyone else is parked with a complete
        // saved frame. Either way the process can't tell it was saved.
        let current = t.current;
        for p in t
            .procs
            .values_mut()
            .filter(|p| p.alive() && p.ghost.auto_due(now))
        {
            let regs = if current == Some(p.pid) {
                if !frame.from_user() {
                    continue;
                }
                *frame
            } else {
                p.ctx
            };
            let mut fs = crate::fs::FS.lock();
            let _ = crate::ghost::checkpoint(p, &regs, &mut fs, crate::ghost::Kind::Auto);
        }

        t.quantum += 1;
        if t.current.is_none() || t.quantum >= QUANTUM {
            reschedule(t, frame);
        }
    }
    if now.is_multiple_of(4) {
        crate::ui::dashboard::render();
    }
}

/// Park the current process (if any) and run the next ready one.
pub fn reschedule(t: &mut ProcTable, frame: &mut TrapFrame) {
    if let Some(pid) = t.current.take()
        && let Some(p) = t.procs.get_mut(&pid)
    {
        if frame.from_user() {
            p.ctx = *frame;
        }
        if p.state == State::Running {
            p.state = State::Ready;
            t.queue.push_back(pid);
        }
    }
    pick_next(t, frame);
}

/// Load the next ready process into `frame` (or the idle loop).
pub fn pick_next(t: &mut ProcTable, frame: &mut TrapFrame) {
    t.quantum = 0;
    while let Some(pid) = t.queue.pop_front() {
        let Some(p) = t.procs.get_mut(&pid) else {
            continue;
        };
        if p.state != State::Ready {
            continue;
        }
        if let Some(kind) = p.inject.take() {
            crate::ghost::inject::apply(p, kind);
        }
        p.state = State::Running;
        p.space().activate();
        *frame = p.ctx;
        t.current = Some(pid);
        return;
    }
    t.current = None;
    unsafe { cpu::write_cr3(paging::kernel_pml4()) };
    *frame = idle_frame();
}

/// Keyboard input arrived: anyone blocked in read(0) gets another go.
pub fn wake_key_waiters() {
    let mut t = PROCS.lock();
    let waiting: alloc::vec::Vec<u32> = t
        .procs
        .values()
        .filter(|p| p.state == State::WaitKey)
        .map(|p| p.pid)
        .collect();
    for pid in waiting {
        t.make_ready(pid);
    }
}

/// Hand the CPU to the scheduler for the first time. Never returns.
pub fn start() -> ! {
    let frame = {
        let mut t = PROCS.lock();
        t.started = true;
        let mut f = idle_frame();
        pick_next(&mut t, &mut f);
        f
    };
    unsafe { crate::arch::trap::enter(&frame) }
}

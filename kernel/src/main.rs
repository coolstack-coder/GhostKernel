//! GhostKernel - a small x86-64 kernel whose processes can come back from
//! the dead.
//!
//! Boot path: boot.rs (32-bit asm, long mode) -> kmain -> scheduler.

#![no_std]
#![no_main]
// pixel loops index 2-D arrays by coordinate on purpose
#![allow(
    clippy::needless_range_loop,
    clippy::too_many_arguments,
    clippy::type_complexity
)]

extern crate alloc;

mod arch;
mod boot;
mod drivers;
mod fs;
mod ghost;
mod mm;
mod proc;
#[cfg(feature = "selftest")]
mod selftest;
mod syscall;
mod time;
mod ui;

use alloc::format;
use arch::{cpu, gdt, idt, pic, pit};
use ui::theme;

unsafe extern "C" {
    static __kernel_start: u8;
    static __kernel_end: u8;
}

static PROGRAMS: &[(&str, &[u8])] = &[
    (
        "init",
        include_bytes!("../../target/x86_64-unknown-none/release/init"),
    ),
    (
        "gsh",
        include_bytes!("../../target/x86_64-unknown-none/release/gsh"),
    ),
    (
        "primes",
        include_bytes!("../../target/x86_64-unknown-none/release/primes"),
    ),
    (
        "life",
        include_bytes!("../../target/x86_64-unknown-none/release/life"),
    ),
    (
        "counter",
        include_bytes!("../../target/x86_64-unknown-none/release/counter"),
    ),
    (
        "flaky",
        include_bytes!("../../target/x86_64-unknown-none/release/flaky"),
    ),
];

const MOTD: &str = "\
Welcome to GhostKernel.

Every process here is mortal - unless it's protected. Protected processes
leave a trail of checkpoints in /ckpt. When one of them crashes, the kernel
rebuilds it from its last checkpoint and lets it carry on as if nothing
happened. Try:

    ps                    who's running
    crash 3               inject a real CPU fault into pid 3
    ls /ckpt              the checkpoint images themselves
    cat /data/primes.log  a log that survives crashes without duplicates
";

fn ok(what: &str, detail: &str) {
    ui::state::kprint(&[
        (theme::GREEN, "[ ok ] "),
        (theme::TEXT, what),
        (theme::DIM, detail),
    ]);
}

#[unsafe(no_mangle)]
extern "C" fn kmain(magic: u32, mbi: u64) -> ! {
    drivers::serial::init();
    serial_println!("\nGhostKernel booting (multiboot magic {magic:#x})");
    assert_eq!(
        magic,
        mm::multiboot::MAGIC,
        "not loaded by a multiboot loader"
    );

    gdt::init();
    idt::init();
    let kernel_end = (&raw const __kernel_end) as u64;
    let heap_end = mm::heap::init(mm::page_ceil(kernel_end));
    mm::frame::init(mbi, heap_end);
    let (free, _) = mm::frame::stats();

    let fb =
        drivers::bga::init(ui::SCREEN_W, ui::SCREEN_H).expect("need QEMU's std VGA (-vga std)");
    ui::dashboard::init(fb.base, fb.width, fb.height);

    let kstart = (&raw const __kernel_start) as u64;
    ok(
        "long mode",
        &format!(
            "  kernel {:#x}..{:#x} ({} KiB)",
            kstart,
            kernel_end,
            (kernel_end - kstart) / 1024
        ),
    );
    ok(
        "gdt + tss",
        "  ring 0 / ring 3 segments, trap stack, double-fault IST",
    );
    ok("idt", "  32 exceptions, 16 irqs, int 0x80 (dpl 3)");
    ok(
        "memory",
        &format!(
            "  {} free frames ({} MiB), {} MiB heap",
            free,
            free * 4 / 1024,
            mm::heap::HEAP_SIZE >> 20
        ),
    );
    ok(
        "display",
        &format!("  bochs vga {}x{} @ {:#x}", fb.width, fb.height, fb.base),
    );

    pic::init();
    pit::init();
    cpu::sti();
    time::calibrate();
    cpu::cli();
    ok(
        "timer",
        &format!("  pit {} Hz, tsc ~{} MHz", pit::HZ, time::mhz()),
    );

    {
        let mut fs = fs::FS.lock();
        fs.format();
        for d in ["/bin", "/ckpt", "/data", "/etc"] {
            fs.mkdir_p(d);
        }
        for (name, elf) in PROGRAMS {
            fs.write_file(&format!("/bin/{name}"), elf).unwrap();
        }
        fs.write_file("/etc/motd", MOTD.as_bytes()).unwrap();
        fs.write_file(
            "/etc/version",
            b"GhostKernel 0.1.0 (x86_64, rust nightly)\n",
        )
        .unwrap();
    }
    ok(
        "ramfs",
        &format!("  /bin ({} programs) /ckpt /data /etc", PROGRAMS.len()),
    );

    ui::event(ui::Ev::Boot, "kernel", "all systems up");
    ui::dashboard::render();

    if cfg!(feature = "selftest") {
        #[cfg(feature = "selftest")]
        selftest::run();
    }

    {
        let fs = fs::FS.lock();
        let mut t = proc::PROCS.lock();
        t.spawn(&fs, "/bin/init", "").expect("cannot start init");
    }
    proc::sched::start()
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    cpu::cli();
    unsafe { drivers::serial::SERIAL.force_unlock() };
    serial_println!("\n*** KERNEL PANIC: {info}");
    #[cfg(feature = "selftest")]
    cpu::qemu_exit(1);
    #[cfg(not(feature = "selftest"))]
    {
        unsafe {
            ui::state::UI.force_unlock();
            proc::PROCS.force_unlock();
            fs::FS.force_unlock();
        }
        ui::state::banner(
            ui::state::BannerKind::Lost,
            alloc::string::String::from("KERNEL PANIC"),
            alloc::vec![
                format!("{}", info.message()),
                alloc::string::String::from("the kernel itself can't be resurrected (yet)")
            ],
            0,
        );
        ui::dashboard::render();
        cpu::halt_forever()
    }
}

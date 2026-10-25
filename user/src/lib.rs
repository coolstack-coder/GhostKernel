//! ulib - everything a GhostKernel program needs: an entry point, a panic
//! handler and thin wrappers around `int 0x80`.

#![no_std]

pub mod fmt;
pub mod sys;

pub use sys::*;

core::arch::global_asm!(
    r#"
.section .text._start, "ax"
.global _start
_start:
    and rsp, -16
    call gk_main
    ud2
"#
);

/// Declare the program's `main(arg: &str) -> i64`.
#[macro_export]
macro_rules! entry {
    ($main:path) => {
        #[unsafe(no_mangle)]
        extern "C" fn gk_main(arg_ptr: *const u8, arg_len: usize) -> ! {
            let arg = unsafe { core::slice::from_raw_parts(arg_ptr, arg_len) };
            let arg = core::str::from_utf8(arg).unwrap_or("");
            let code: i64 = $main(arg);
            $crate::exit(code)
        }
    };
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => { $crate::fmt::print(format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! println {
    () => { $crate::print!("\n") };
    ($($arg:tt)*) => { $crate::print!("{}\n", format_args!($($arg)*)) };
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("\x1brpanic: {}", info.message());
    exit(101)
}

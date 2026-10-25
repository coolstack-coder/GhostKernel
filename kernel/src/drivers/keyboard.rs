//! PS/2 keyboard, scancode set 1, US layout. Keys land in a small ring and
//! wake whoever is blocked in `read(0)`.

use crate::arch::cpu::inb;
use crate::arch::pic;
use spin::Mutex;

struct Kbd {
    shift: bool,
    ctrl: bool,
    buf: [u8; 64],
    head: usize,
    len: usize,
}

static KBD: Mutex<Kbd> = Mutex::new(Kbd {
    shift: false,
    ctrl: false,
    buf: [0; 64],
    head: 0,
    len: 0,
});

const NORMAL: &[u8; 58] =
    b"\0\x1b1234567890-=\x08\tqwertyuiop[]\n\0asdfghjkl;'`\0\\zxcvbnm,./\0*\0 ";
const SHIFTED: &[u8; 58] =
    b"\0\x1b!@#$%^&*()_+\x08\tQWERTYUIOP{}\n\0ASDFGHJKL:\"~\0|ZXCVBNM<>?\0*\0 ";

pub fn on_irq() {
    let sc = inb(0x60);
    pic::eoi(1);
    let mut k = KBD.lock();
    match sc {
        0x2A | 0x36 => k.shift = true,
        0xAA | 0xB6 => k.shift = false,
        0x1D => k.ctrl = true,
        0x9D => k.ctrl = false,
        s if (s as usize) < NORMAL.len() => {
            let mut c = if k.shift {
                SHIFTED[s as usize]
            } else {
                NORMAL[s as usize]
            };
            if k.ctrl && c.is_ascii_alphabetic() {
                c = c.to_ascii_lowercase() - b'a' + 1;
            }
            if c != 0 && k.len < k.buf.len() {
                let i = (k.head + k.len) % k.buf.len();
                k.buf[i] = c;
                k.len += 1;
            }
        }
        _ => {}
    }
    drop(k);
    crate::proc::sched::wake_key_waiters();
}

pub fn pop() -> Option<u8> {
    let mut k = KBD.lock();
    if k.len == 0 {
        return None;
    }
    let c = k.buf[k.head];
    k.head = (k.head + 1) % k.buf.len();
    k.len -= 1;
    Some(c)
}

//! No heap in user space: format into a stack buffer, flush in one write
//! so a line from one process never gets torn by another.

use core::fmt::{self, Write};

pub struct LineBuf<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> LineBuf<N> {
    pub const fn new() -> Self {
        LineBuf {
            buf: [0; N],
            len: 0,
        }
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(self.as_bytes()).unwrap_or("")
    }
    pub fn clear(&mut self) {
        self.len = 0;
    }
}

impl<const N: usize> Default for LineBuf<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Write for LineBuf<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for &b in s.as_bytes() {
            if self.len == N {
                crate::write(1, &self.buf[..self.len]);
                self.len = 0;
            }
            self.buf[self.len] = b;
            self.len += 1;
        }
        Ok(())
    }
}

pub fn print(args: fmt::Arguments) {
    let mut b = LineBuf::<512>::new();
    let _ = b.write_fmt(args);
    crate::write(1, b.as_bytes());
}

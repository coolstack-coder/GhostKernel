"""Drive a headless GhostKernel VM through the QEMU monitor.

Used by the demo recorder and the end-to-end test: it can type into the
guest's keyboard, grab screenshots, and read the serial log.
"""

import os
import socket
import subprocess
import time

KEYMAP = {
    " ": "spc", "\n": "ret", "-": "minus", "=": "equal", "/": "slash", ".": "dot",
    ",": "comma", ";": "semicolon", "'": "apostrophe", "[": "bracket_left",
    "]": "bracket_right", "\\": "backslash", "`": "grave_accent",
}
SHIFTED = {
    "_": "minus", "+": "equal", "?": "slash", ">": "dot", "<": "comma", ":": "semicolon",
    '"': "apostrophe", "!": "1", "@": "2", "#": "3", "$": "4", "%": "5", "^": "6",
    "&": "7", "*": "8", "(": "9", ")": "0", "|": "backslash", "~": "grave_accent",
}


def key_for(ch):
    if ch.isalpha() and ch.isupper():
        return "shift-" + ch.lower()
    if ch.isalnum():
        return ch
    if ch in KEYMAP:
        return KEYMAP[ch]
    if ch in SHIFTED:
        return "shift-" + SHIFTED[ch]
    raise ValueError(f"can't type {ch!r}")


class VM:
    def __init__(self, qemu, image, serial_log, memory="256M", display="none", paused=False):
        # next to the serial log; unix socket paths are capped at ~104 bytes
        self.sock_path = os.path.join(os.path.dirname(os.path.abspath(serial_log)), f"mon-{os.getpid()}.sock")
        if os.path.exists(self.sock_path):
            os.unlink(self.sock_path)
        self.serial_log = serial_log
        args = [
            qemu, "-kernel", image, "-m", memory, "-vga", "std", "-no-reboot", "-no-shutdown",
            "-device", "isa-debug-exit,iobase=0xf4,iosize=0x04",
            "-serial", f"file:{serial_log}", "-display", display,
            "-monitor", f"unix:{self.sock_path},server,nowait",
        ] + (["-S"] if paused else [])
        self.proc = subprocess.Popen(args)
        for _ in range(1200):  # QEMU can take a while to come up on a cold start
            if os.path.exists(self.sock_path):
                break
            time.sleep(0.05)
        self.mon = socket.socket(socket.AF_UNIX)
        self.mon.connect(self.sock_path)
        self.mon.settimeout(0.05)
        self._drain()

    def _drain(self):
        try:
            while self.mon.recv(65536):
                pass
        except (socket.timeout, BlockingIOError):
            pass

    def cmd(self, line):
        self.mon.sendall(line.encode() + b"\n")
        self._drain()

    def key(self, k):
        self.cmd(f"sendkey {k}")

    def type(self, text, delay=0.06, tick=None):
        """Type like a person; `tick` is called between keys (for recording)."""
        for ch in text:
            self.key(key_for(ch))
            end = time.time() + delay
            while time.time() < end:
                if tick:
                    tick()
                else:
                    time.sleep(0.01)

    def screendump(self, path):
        self.cmd(f"screendump {path}")

    def serial(self):
        try:
            with open(self.serial_log, encoding="utf-8", errors="replace") as f:
                return f.read()
        except FileNotFoundError:
            return ""

    def wait_for(self, needle, timeout=30, tick=None):
        end = time.time() + timeout
        while time.time() < end:
            if needle in self.serial():
                return True
            if tick:
                tick()
            else:
                time.sleep(0.1)
        return False

    def quit(self):
        try:
            os.unlink(self.sock_path)
        except OSError:
            pass
        try:
            self.cmd("quit")
        except OSError:
            pass
        try:
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.proc.kill()

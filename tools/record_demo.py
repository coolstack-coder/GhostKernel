#!/usr/bin/env python3
"""Record the GhostKernel demo: boot headless, play a script into the
shell, grab the screen ~12x a second, and burn subtitles into a caption bar.

    python3 tools/record_demo.py --qemu qemu-system-x86_64 --image build/ghostkernel.elf

Needs ffmpeg with libass (for the subtitles filter). Output: docs/demo.mp4,
docs/demo.srt and a short docs/demo.gif highlight for the README.
"""

import argparse
import os
import shutil
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from qemu_driver import VM  # noqa: E402

FPS = 12

# (action, argument, subtitle). Actions: "wait" seconds, "type" a command,
# "until" a serial-log needle shows up (then a short beat).
SCRIPT = [
    ("wait", 5.0, "GhostKernel: an x86-64 kernel in Rust. It boots from its own multiboot assembly into long mode."),
    ("wait", 6.0, "Five processes in ring 3, each with its own page tables, sharing the CPU through a preemptive scheduler."),
    ("wait", 6.0, "primes checkpoints itself every 3 batches. life never asked for it: the kernel snapshots it every second."),
    ("wait", 5.0, "The ghost timeline: every dot is a checkpoint. Diamonds are full images, small dots are dirty-page deltas."),
    ("type", "ps", "Every process is mortal unless it has a ghost: a chain of checkpoints."),
    ("wait", 4.0, None),
    ("type", "ls /ckpt", "The checkpoints are real files: one FULL image, then deltas with only the pages the CPU marked dirty."),
    ("wait", 5.0, None),
    ("type", "crash 2", "Now break primes for real: its saved RIP becomes 0x0. The CPU raises a page fault in ring 3."),
    ("until", "RESTORE primes", "The kernel throws away the dead address space and rebuilds it, page by page, from the checkpoint chain."),
    ("wait", 4.5, "primes wakes up inside checkpoint(), mid-computation, not at prime #0. Its log was rolled back to match: no duplicates."),
    ("wait", 4.0, "On the timeline: a red cross for the crash, a green arrow for the resurrection a few microseconds later."),
    ("type", "crash 3 gp", "life doesn't know checkpoints exist. A general-protection fault, and..."),
    ("until", "RESTORE life", "...the board snaps back to its last snapshot and keeps evolving."),
    ("wait", 5.0, None),
    ("type", "crash 4", "counter is the control group. No ghost. Same kind of fault."),
    ("until", "LOST    counter", "It's gone for good, along with everything it computed."),
    ("wait", 5.0, None),
    ("type", "run flaky det", "flaky has a real bug that fires at the same step every time. Watch the kernel react."),
    ("until", "fallback", "First retry: the newest checkpoint. It crashes again, so the kernel falls back one generation..."),
    ("until", "crash loop", "...and once it's clearly a crash loop, it stops instead of resurrecting it forever."),
    ("wait", 5.0, None),
    ("type", "tail /data/primes.log 4", "After all of that, primes' log has every batch exactly once, in order."),
    ("wait", 4.0, None),
    ("type", "stats", "GhostKernel: processes that come back from the dead."),
    ("wait", 5.0, None),
]


class Recorder:
    def __init__(self, vm, workdir, ffmpeg):
        self.vm = vm
        self.ppm = os.path.join(workdir, "frame.ppm")
        self.raw = os.path.join(workdir, "raw.mp4")
        self.start = None
        self.next_shot = 0.0
        self.enc = subprocess.Popen(
            [ffmpeg, "-loglevel", "error", "-y", "-use_wallclock_as_timestamps", "1",
             "-f", "image2pipe", "-c:v", "ppm", "-i", "-",
             "-vf", f"fps={FPS * 2}", "-c:v", "libx264", "-preset", "veryfast", "-crf", "16",
             "-pix_fmt", "yuv420p", self.raw],
            stdin=subprocess.PIPE,
        )

    def now(self):
        return time.time() - self.start

    def tick(self):
        """Grab a frame if one is due; otherwise nap briefly."""
        t = time.time()
        if self.start is None:
            self.start = t
        if t < self.next_shot:
            time.sleep(min(0.01, self.next_shot - t))
            return
        self.next_shot = t + 1.0 / FPS
        self.vm.screendump(self.ppm)
        for _ in range(60):
            if os.path.exists(self.ppm) and os.path.getsize(self.ppm) >= 1280 * 800 * 3:
                break
            time.sleep(0.005)
        else:
            return  # no frame this time (e.g. display not up yet); the encoder just holds the last one
        with open(self.ppm, "rb") as f:
            self.enc.stdin.write(f.read())
        os.unlink(self.ppm)

    def sleep(self, secs):
        end = time.time() + secs
        while time.time() < end:
            self.tick()

    def finish(self):
        self.enc.stdin.close()
        self.enc.wait()


def srt_time(t):
    ms = int(round(t * 1000))
    return f"{ms // 3600000:02}:{ms // 60000 % 60:02}:{ms // 1000 % 60:02},{ms % 1000:03}"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--qemu", default="qemu-system-x86_64")
    ap.add_argument("--image", default="build/ghostkernel.elf")
    ap.add_argument("--ffmpeg", default=shutil.which("ffmpeg") or "ffmpeg")
    ap.add_argument("--out", default="docs")
    a = ap.parse_args()

    work = os.path.abspath("build/rec")
    os.makedirs(work, exist_ok=True)
    os.makedirs(a.out, exist_ok=True)
    # start frozen so the recording catches the boot from the first instruction
    vm = VM(a.qemu, a.image, os.path.join(work, "serial.log"), paused=True)
    rec = Recorder(vm, work, a.ffmpeg)

    cues = []  # (start, end, text)

    def say(text):
        if text is None:
            return
        # give every caption time to be read before replacing it
        if cues and cues[-1][1] is None and rec.now() - cues[-1][0] < 3.8:
            rec.sleep(3.8 - (rec.now() - cues[-1][0]))
        t = rec.now()
        if cues and cues[-1][1] is None:
            cues[-1] = (cues[-1][0], t, cues[-1][2])
        cues.append((t, None, text))

    try:
        rec.tick()  # the first frame starts the clock
        vm.cmd("cont")
        for action, arg, text in SCRIPT:
            say(text)
            if action == "wait":
                rec.sleep(arg)
            elif action == "type":
                rec.sleep(2.4)  # let the caption land before the action does
                vm.type(arg + "\n", delay=0.075, tick=rec.tick)
                rec.sleep(0.8)
            elif action == "until":
                if not vm.wait_for(arg, 45, tick=rec.tick):
                    print(f"warning: never saw {arg!r}")
                rec.sleep(2.5)
        say("")
    finally:
        rec.finish()
        vm.quit()

    cues = [(s, e, t) for s, e, t in cues if t]
    srt = os.path.join(a.out, "demo.srt")
    with open(srt, "w") as f:
        for i, (s, e, t) in enumerate(cues, 1):
            f.write(f"{i}\n{srt_time(s)} --> {srt_time(e)}\n{t}\n\n")

    # caption bar under the screen, so subtitles never cover the dashboard
    # libass sizes are relative to a 288-line canvas: 8 ~ 24px at 880 lines
    style = ("FontName=Helvetica Neue,FontSize=8,PrimaryColour=&H00F5F7FA,"
             "BorderStyle=1,Outline=0,Shadow=0,Alignment=2,MarginV=9")
    fonts = "/System/Library/Fonts" if sys.platform == "darwin" else "/usr/share/fonts"
    vf = (f"pad=1280:880:0:0:color=0x0A0D14,"
          f"drawbox=x=0:y=800:w=1280:h=2:color=0x5EEAD4@0.9:t=fill,"
          f"subtitles={srt}:fontsdir={fonts}:force_style='{style}'")
    out = os.path.join(a.out, "demo.mp4")
    font = "/System/Library/Fonts/Helvetica.ttc" if sys.platform == "darwin" else "DejaVuSans.ttf"
    mono = "/System/Library/Fonts/Menlo.ttc" if sys.platform == "darwin" else "DejaVuSansMono.ttf"

    def card(lines, secs):
        """A dark title card: [(text, size, colour, font, y)]."""
        draws = ",".join(
            f"drawtext=fontfile={f}:text='{t}':fontsize={s}:fontcolor={c}:x=(w-text_w)/2:y={y}"
            for t, s, c, f, y in lines)
        return (f"color=c=0x0A0D14:s=1280x880:d={secs}:r=24,{draws},"
                f"fade=t=in:st=0:d=0.5,fade=t=out:st={secs - 0.5}:d=0.5,format=yuv420p")

    intro = card([
        ("GHOSTKERNEL", 84, "0x5EEAD4", font, 300),
        ("an x86-64 kernel in Rust whose processes come back from the dead", 28, "0xC9D1D9", font, 420),
        ("checkpoint  ·  crash  ·  resurrect", 24, "0x5C6680", mono, 490),
    ], 4)
    outro = card([
        ("GHOSTKERNEL", 64, "0x5EEAD4", font, 330),
        ("make run", 30, "0xF5F7FA", mono, 440),
        ("Rust  ·  x86-64 assembly  ·  QEMU  ·  LLVM", 24, "0x5C6680", font, 500),
    ], 4)
    graph = (f"[0:v]{vf},setsar=1,fps=24,fade=t=in:st=0:d=0.4[main];"
             f"{intro}[i];{outro}[o];[i][main][o]concat=n=3:v=1:a=0[v]")
    subprocess.run([a.ffmpeg, "-loglevel", "error", "-y", "-i", rec.raw, "-filter_complex", graph,
                    "-map", "[v]", "-c:v", "libx264", "-preset", "slow", "-crf", "20", "-pix_fmt", "yuv420p",
                    "-movflags", "+faststart", out], check=True)
    # subtitles were timed against the raw capture; shift them past the intro
    with open(srt, "w") as f:
        for i, (s, e, t) in enumerate(cues, 1):
            f.write(f"{i}\n{srt_time(s + 4)} --> {srt_time(e + 4)}\n{t}\n\n")

    # short looping highlight of the crash -> restore moment for the README
    crash_at = next((s for s, _, t in cues if t.startswith("Now break primes")), 30.0) + 4
    gif = os.path.join(a.out, "demo.gif")
    subprocess.run([a.ffmpeg, "-loglevel", "error", "-y", "-ss", f"{crash_at - 0.5:.2f}", "-t", "9", "-i", out,
                    "-vf", "fps=10,scale=960:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=96[p];"
                           "[b][p]paletteuse=dither=bayer:bayer_scale=4", gif], check=True)
    print(f"wrote {out}, {srt} and {gif}")


if __name__ == "__main__":
    main()

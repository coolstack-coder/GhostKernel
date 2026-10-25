#!/usr/bin/env python3
"""End-to-end test: boot GhostKernel headless, type into its shell, and
check the serial log for the behaviour we promise.

    python3 tools/e2e_test.py [--qemu PATH] [--image build/ghostkernel.elf]
"""

import argparse
import os
import re
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from qemu_driver import VM  # noqa: E402


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--qemu", default="qemu-system-x86_64")
    ap.add_argument("--image", default="build/ghostkernel.elf")
    ap.add_argument("--log", default="build/e2e-serial.log")
    a = ap.parse_args()

    failures = []

    def check(cond, what):
        print(("  ok   " if cond else "  FAIL ") + what)
        if not cond:
            failures.append(what)

    vm = VM(a.qemu, a.image, a.log)
    try:
        check(vm.wait_for("batches in /data/primes.log", 90), "boots and runs user programs")
        check(vm.wait_for("CKPT    primes#2", 10), "primes takes checkpoints")
        check(vm.wait_for("[auto]", 10) or vm.wait_for(" auto", 10), "life gets timer-driven checkpoints")

        vm.type("crash 2\n")
        check(vm.wait_for("RESTORE primes#2", 15), "crashed primes is restored")
        check(vm.wait_for("back from the dead", 10), "primes resumes inside checkpoint()")
        check(vm.wait_for("consistent, no duplicates", 10), "log file rolled back to match memory")

        vm.type("crash 3 gp\n")
        check(vm.wait_for("RESTORE life#3", 15), "life survives a #GP")

        vm.type("crash 4\n")
        check(vm.wait_for("LOST    counter#4", 15), "unprotected counter is lost for good")

        vm.type("run flaky det\n")
        check(vm.wait_for("fallback", 30), "repeated crash falls back a generation")
        check(vm.wait_for("crash loop", 40), "crash loop is detected and stopped")

        time.sleep(1)
        vm.type("tail /data/primes.log 200\n")
        time.sleep(3)
        log = vm.serial()
        batches = [int(m) for m in re.findall(r"\| batch +(\d+):", log)]
        check(len(batches) > 0, "primes.log is readable from the shell")
        check(batches == sorted(set(batches)), "no duplicated or reordered lines in primes.log")
        check("KERNEL PANIC" not in log, "kernel never panicked")
    finally:
        vm.quit()

    print(f"\n{'PASS' if not failures else 'FAIL'}: {len(failures)} failure(s)")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()

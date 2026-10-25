# GhostKernel build. `make run` is all you need.

SHELL     := /bin/bash
PROFILE   ?= release
TARGET    := target/x86_64-unknown-none/$(PROFILE)
KERNEL    := $(TARGET)/ghostkernel
IMAGE     := build/ghostkernel.elf
SYSROOT   := $(shell rustc --print sysroot)
HOST      := $(shell rustc -vV | sed -n 's/^host: //p')
OBJCOPY   := $(SYSROOT)/lib/rustlib/$(HOST)/bin/llvm-objcopy
QEMU      ?= $(shell command -v qemu-system-x86_64 || echo $(HOME)/.local/qemu/bin/qemu-system-x86_64)

QEMU_BASE := -m 256M -vga std -no-reboot -device isa-debug-exit,iobase=0xf4,iosize=0x04
# -no-shutdown keeps the window up after a triple fault, but would also
# swallow isa-debug-exit, so the test target leaves it out
QEMU_ARGS := $(QEMU_BASE) -no-shutdown
CARGO_FLAGS := $(if $(filter release,$(PROFILE)),--release,)

.PHONY: all user kernel run run-headless test e2e demo clean fmt lint

all: $(IMAGE)

user:
	cargo build $(CARGO_FLAGS) -p gk-user

kernel: user
	cargo build $(CARGO_FLAGS) -p ghostkernel $(if $(FEATURES),--features $(FEATURES),)

# QEMU's multiboot loader only takes 32-bit ELF files, so the 64-bit
# kernel is re-wrapped as ELF32. The code inside is unchanged.
$(IMAGE): kernel
	@mkdir -p build
	$(OBJCOPY) -O elf32-i386 $(KERNEL) $(IMAGE)
	@echo "built $(IMAGE) ($$(du -h $(IMAGE) | cut -f1))"

run: $(IMAGE)
	$(QEMU) -kernel $(IMAGE) $(QEMU_ARGS) -serial stdio

run-headless: $(IMAGE)
	$(QEMU) -kernel $(IMAGE) $(QEMU_ARGS) -serial stdio -display none

# In-kernel test suite: boots, runs every test, exits QEMU with the verdict.
test:
	$(MAKE) $(IMAGE) FEATURES=selftest
	@set -o pipefail; $(QEMU) -kernel $(IMAGE) $(QEMU_BASE) -serial stdio -display none \
		| tee build/selftest.log; code=$${PIPESTATUS[0]}; \
		if [ $$code -eq 33 ]; then echo "selftest: PASS"; else echo "selftest: FAIL ($$code)"; exit 1; fi

# Boot the real thing, type into gsh, and check the promises on serial.
e2e: $(IMAGE)
	python3 tools/e2e_test.py --qemu $(QEMU) --image $(IMAGE)

# Scripted end-to-end run that records docs/demo.mp4 (see tools/record_demo.py).
demo: $(IMAGE)
	python3 tools/record_demo.py --qemu $(QEMU) --image $(IMAGE)

fmt:
	cargo fmt --all

lint:
	cargo clippy -p ghostkernel -p gk-user $(CARGO_FLAGS)

clean:
	cargo clean
	rm -rf build

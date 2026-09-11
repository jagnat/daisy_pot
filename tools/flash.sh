#!/bin/sh

set -eu

elf=$1
binary="${TMPDIR:-/tmp}/daisy_pot.bin"

if ! command -v rust-objcopy >/dev/null 2>&1; then
    echo "error: rust-objcopy is not installed or is not on PATH" >&2
    echo "install it with: cargo install cargo-binutils && rustup component add llvm-tools" >&2
    exit 127
fi

if ! command -v dfu-util >/dev/null 2>&1; then
    echo "error: dfu-util is not installed or is not on PATH" >&2
    exit 127
fi

rust-objcopy -O binary "$elf" "$binary"
dfu-util -a 0 -s 0x08000000:leave -D "$binary"

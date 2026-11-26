#!/bin/bash
set -euo pipefail

# Hollow Star OS+ — QEMU Runner for Raspberry Pi 3B (raspi3b)
# Supports standalone usage (./run-qemu.sh [--smoke]) and Cargo runner invocation
# (./run-qemu.sh <path-to-elf>).

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

UART_MODE="${UART_MODE:-pl011}"
QEMU_UART_ARGS=(-serial stdio)
if [ "$UART_MODE" = "mini" ]; then
  QEMU_UART_ARGS=(-serial null -serial stdio)
fi

if [ "${1:-}" = "--smoke" ]; then
  exec ./delta.sh smoke
fi

if [ "$#" -ge 1 ] && [ -f "$1" ]; then
  ELF_PATH="$1"
  IMG_PATH="target/kernel8.img"

  OBJCOPY=""
  if command -v rust-objcopy >/dev/null 2>&1; then
    OBJCOPY="$(command -v rust-objcopy)"
  else
    SYSROOT="$(rustc --print sysroot)"
    OBJCOPY="$(find "$SYSROOT" -name "llvm-objcopy" -type f 2>/dev/null | head -n 1)"
  fi

  if [ -z "$OBJCOPY" ]; then
    echo "[run-qemu] Brak llvm-objcopy. Zainstaluj: rustup component add llvm-tools" >&2
    exit 1
  fi

  mkdir -p target
  "$OBJCOPY" --strip-all -O binary "$ELF_PATH" "$IMG_PATH"

  exec qemu-system-aarch64 \
    -M raspi3b \
    -display none \
    "${QEMU_UART_ARGS[@]}" \
    -kernel "$IMG_PATH"
fi

exec ./delta.sh run

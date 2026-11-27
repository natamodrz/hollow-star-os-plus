#!/bin/bash
set -euo pipefail

# ==============================================================================
# Hollow Star OS+ — Local Real-Hardware Test Image Builder
# Builds a bootable microSD image locally inside `target/` (NEVER `releases/`
# and NEVER pushes to GitHub Releases) for Raspberry Pi 3B / Zero 2 W.
# ==============================================================================

export PATH="/Users/nataniel/.cargo/bin:/opt/homebrew/bin:$PATH"

BOARD="${BOARD:-raspi3b}"       # raspi3b | raspi02w
UART="${UART:-pl011}"           # pl011 | mini
SIZE_MB="${SIZE_MB:-128}"       # Total .img size in MiB
FIRMWARE_DIR="${FIRMWARE_DIR:-target/rpi-firmware}"
SKIP_FW_DOWNLOAD="${SKIP_FW_DOWNLOAD:-0}"
OUT_IMG=""

usage() {
  cat <<EOF
Użycie: ./build-test-image.sh [opcje]

Opcje:
  --board <raspi3b|raspi02w>    Docelowa płytka (domyślnie: raspi3b)
  --uart <pl011|mini>           Port szeregowy GPIO 14/15 (domyślnie: pl011)
  --size-mb <N>                 Rozmiar obrazu .img w MiB (domyślnie: 128)
  --firmware-dir <katalog>      Katalog z bootcode.bin/start.elf/fixup.dat/dtb
  --skip-firmware-download      Nie pobieraj firmware RPi z sieci (np. tryb offline)
  --out <target/...img>         Ścieżka wyjściowa (wyłącznie wewnątrz target/)
  -h, --help                    Pokaż tę pomoc

Przykłady:
  ./build-test-image.sh --board raspi3b --uart mini
  ./build-test-image.sh --board raspi02w --uart mini
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --board) BOARD="$2"; shift 2 ;;
    --uart) UART="$2"; shift 2 ;;
    --size-mb) SIZE_MB="$2"; shift 2 ;;
    --firmware-dir) FIRMWARE_DIR="$2"; shift 2 ;;
    --skip-firmware-download) SKIP_FW_DOWNLOAD=1; shift ;;
    --out) OUT_IMG="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Nieznana opcja: $1" >&2; usage; exit 1 ;;
  esac
done

case "$BOARD" in
  raspi3b)  BOARD_FEAT="board-raspi3b" ;;
  raspi02w) BOARD_FEAT="board-raspi02w" ;;
  *) echo "Nieobsługiwana płytka: $BOARD (użyj raspi3b lub raspi02w)" >&2; exit 1 ;;
esac

FEATURES="$BOARD_FEAT"
if [ "$UART" = "mini" ]; then
  FEATURES="$FEATURES,mini-uart"
elif [ "$UART" != "pl011" ]; then
  echo "Nieobsługiwany tryb UART: $UART (użyj pl011 lub mini)" >&2
  exit 1
fi

if [ -z "$OUT_IMG" ]; then
  OUT_IMG="target/hollow-star-os-plus-${BOARD}-${UART}-hw-test.img"
fi

echo "[hw-image] Kompilacja jądra Delta Kernel (--release, features: $FEATURES)..."
cargo build \
  --package kernel \
  --target aarch64-unknown-none \
  --release \
  --no-default-features \
  --features "$FEATURES"

OBJCOPY=""
if command -v rust-objcopy >/dev/null 2>&1; then
  OBJCOPY="$(command -v rust-objcopy)"
else
  OBJCOPY="$(find "$(rustc --print sysroot)" -name "llvm-objcopy" -type f 2>/dev/null | head -n 1)"
fi
if [ -z "$OBJCOPY" ]; then
  echo "[hw-image] BŁĄD: brak llvm-objcopy (rustup component add llvm-tools-preview)" >&2
  exit 1
fi

KERNEL_ELF="target/aarch64-unknown-none/release/kernel"
KERNEL_IMG="target/kernel8-hw-${BOARD}.img"
"$OBJCOPY" --strip-all -O binary "$KERNEL_ELF" "$KERNEL_IMG"
echo "[hw-image] Wygenerowano $KERNEL_IMG ($(wc -c < "$KERNEL_IMG" | tr -d ' ') B)"

PY_ARGS=(
  --board "$BOARD"
  --uart "$UART"
  --kernel-img "$KERNEL_IMG"
  --userland-dir "target/userland/aarch64-unknown-none/release"
  --firmware-dir "$FIRMWARE_DIR"
  --size-mb "$SIZE_MB"
  --out "$OUT_IMG"
)
if [ "$SKIP_FW_DOWNLOAD" = "1" ]; then
  PY_ARGS+=(--skip-firmware-download)
fi

PYTHONPATH="tools" python3 tools/mk_hw_image.py "${PY_ARGS[@]}"

echo ""
echo "[hw-image] GOTOWE! Lokalny obraz testowy zapisano w: $OUT_IMG"
echo "[hw-image] Aby nagrać na kartę microSD na macOS:"
echo "  1. Sprawdź numer dysku: diskutil list"
echo "  2. Odmontuj kartę:     diskutil unmountDisk /dev/diskN"
echo "  3. Zapisz obraz:       sudo dd if=$OUT_IMG of=/dev/rdiskN bs=4m status=progress && sync"
echo "  4. Wysuń kartę:        diskutil eject /dev/diskN"

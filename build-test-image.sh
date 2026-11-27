#!/bin/bash
set -euo pipefail

# ==============================================================================
# Hollow Star OS+ — Automated Local Hardware Test Image Builder & SD Flasher
# - Auto-detects host OS (macOS / Linux)
# - Builds kernel, EL0 userland, .dkex/.dylib, and 3-partition SD .img in target/
# - Lists all host disks safely and lets you select which disk/SD card to flash
# ==============================================================================

export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"

BOARD="raspi3b"
UART="pl011"
BOARD_SET=0
UART_SET=0
SIZE_MB="${SIZE_MB:-128}"
FIRMWARE_DIR="${FIRMWARE_DIR:-target/rpi-firmware}"
SKIP_FW_DOWNLOAD="${SKIP_FW_DOWNLOAD:-0}"
OUT_IMG=""
NO_FLASH=0
TARGET_DISK=""
YES_FLASH=0
LIST_ONLY=0

HOST_OS="$(uname -s)"
case "$HOST_OS" in
  Darwin) OS_LABEL="macOS (Darwin)" ;;
  Linux)  OS_LABEL="Linux" ;;
  *)
    echo "[hw-image] Nieobsługiwany system operacyjny: $HOST_OS (wymagany macOS lub Linux)." >&2
    exit 1
    ;;
esac

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
  --disk <dev>                  Docelowy dysk do zapisu (np. /dev/disk4 lub /dev/sdb)
  --yes                         Pomiń pytanie potwierdzające przed zapisem na --disk
  --no-flash                    Tylko zbuduj obraz .img bez pytania o zapis na kartę
  --list-disks                  Wypisz wykryte dyski i zakończ
  -h, --help                    Pokaż tę pomoc

Przykłady:
  ./build-test-image.sh                         # Tryb w pełni automatyczny / interaktywny
  ./build-test-image.sh --board raspi3b --no-flash
  ./build-test-image.sh --board raspi02w --uart mini --disk /dev/disk4
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --board) BOARD="$2"; BOARD_SET=1; shift 2 ;;
    --uart) UART="$2"; UART_SET=1; shift 2 ;;
    --size-mb) SIZE_MB="$2"; shift 2 ;;
    --firmware-dir) FIRMWARE_DIR="$2"; shift 2 ;;
    --skip-firmware-download) SKIP_FW_DOWNLOAD=1; shift ;;
    --out) OUT_IMG="$2"; shift 2 ;;
    --disk) TARGET_DISK="$2"; shift 2 ;;
    --yes) YES_FLASH=1; shift ;;
    --no-flash) NO_FLASH=1; shift ;;
    --list-disks) LIST_ONLY=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Nieznana opcja: $1" >&2; usage; exit 1 ;;
  esac
done

chmod +x tools/flash_sd.py 2>/dev/null || true

if [ "$LIST_ONLY" = "1" ]; then
  python3 tools/flash_sd.py --list
  exit 0
fi

echo "=================================================================="
echo "  Hollow Star OS+ — Kreator i Flasher Obrazu Karty microSD"
echo "  Wykryty system: $OS_LABEL"
echo "=================================================================="

# Interactive board & UART wizard when running in a terminal without explicit flags.
if [ -t 0 ] && [ -t 1 ] && [ "$NO_FLASH" = "0" ] && [ -z "$TARGET_DISK" ]; then
  if [ "$BOARD_SET" = "0" ]; then
    echo ""
    echo "Wybierz docelową płytkę:"
    echo "  [1] Raspberry Pi 3B (raspi3b) [domyślna]"
    echo "  [2] Raspberry Pi Zero 2 W (raspi02w)"
    printf "Wybór [1]: "
    read -r b_ans || true
    case "${b_ans:-1}" in
      2|raspi02w) BOARD="raspi02w" ;;
      *) BOARD="raspi3b" ;;
    esac
  fi
  if [ "$UART_SET" = "0" ]; then
    echo ""
    echo "Wybierz tryb konsoli diagnostycznej UART (GPIO 14/15):"
    echo "  [1] PL011 / UART0 (zalecany) [domyślny]"
    echo "  [2] Mini-UART / UART1 (mini)"
    printf "Wybór [1]: "
    read -r u_ans || true
    case "${u_ans:-1}" in
      2|mini) UART="mini" ;;
      *) UART="pl011" ;;
    esac
  fi
fi

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

echo ""
echo "[hw-image] Kompilacja Delta Kernel + EL0 userland (--release, features: $FEATURES)..."
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
echo "[hw-image] Zbudowano obraz: $OUT_IMG"

if [ "$NO_FLASH" = "1" ]; then
  exit 0
fi

if [ -n "$TARGET_DISK" ]; then
  FLASH_ARGS=(--image "$OUT_IMG" --disk "$TARGET_DISK")
  if [ "$YES_FLASH" = "1" ]; then
    FLASH_ARGS+=(--yes)
  fi
  python3 tools/flash_sd.py "${FLASH_ARGS[@]}"
elif [ -t 0 ] && [ -t 1 ]; then
  python3 tools/flash_sd.py --image "$OUT_IMG"
else
  echo "[hw-image] Tryb nieinteraktywny — aby wgrać obraz na kartę SD, uruchom ./build-test-image.sh bez przekierowania stdin."
fi

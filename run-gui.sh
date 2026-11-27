#!/bin/bash
set -euo pipefail

# Hollow Star OS+ — Interactive Graphical QEMU Runner (Graphics + USB Keyboard + Audio + USB-Net)
# Usage:
#   ./run-gui.sh             Build release image and open QEMU graphics window
#   PROFILE=debug ./run-gui.sh

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"

PROFILE="${PROFILE:-release}"
KERNEL_IMG="target/kernel8.img"
SD_IMG="target/sdcard.img"

if ! command -v qemu-system-aarch64 >/dev/null 2>&1; then
  echo "[run-gui] qemu-system-aarch64 is not installed (brew install qemu)." >&2
  exit 1
fi

echo "[run-gui] Building Hollow Star OS+ ($PROFILE) + 3-partition SD image..."
PROFILE="$PROFILE" ./delta.sh build >/dev/null

DISPLAY_DRV="default"
AUDIO_DRV="none"
if [ "$(uname -s)" = "Darwin" ]; then
  # `zoom-to-fit=on` prevents macOS Cocoa Retina HiDPI clipping of the 640x480 framebuffer.
  DISPLAY_DRV="cocoa,zoom-to-fit=on"
  AUDIO_DRV="coreaudio"
fi

echo ""
echo "================================================================"
echo "  Hollow Star OS+ — Okno graficzne QEMU (640x480, Cyberdeck)"
echo "================================================================"
echo "  Sterowanie bezpośrednio w oknie QEMU (USB HID Keyboard):"
echo "    - Strzałki / Tab / 1-9,0 : poruszanie się i uruchamianie .hsapp"
echo "    - Enter / Spacja         : zatwierdź / uruchom"
echo "    - F1                     : pomoc skrótów (okno modalne blur)"
echo "    - Esc                    : powrót do Launchera"
echo "    - Ctrl+C (host terminal) : zamknij maszynę wirtualną"
echo "================================================================"
echo ""

SAVED_STTY=""
if [ -t 0 ]; then
  SAVED_STTY="$(stty -g)"
  trap 'stty "$SAVED_STTY" 2>/dev/null || true' EXIT INT TERM
  stty -echo -icanon min 1 time 0 isig
fi

qemu-system-aarch64 \
  -M raspi3b \
  -display "$DISPLAY_DRV" \
  -serial stdio \
  -drive "file=$SD_IMG,if=sd,format=raw" \
  -netdev user,id=net0 \
  -device usb-net,netdev=net0 \
  -audiodev "$AUDIO_DRV,id=snd0" \
  -device usb-audio,audiodev=snd0 \
  -usb \
  -device usb-kbd \
  -kernel "$KERNEL_IMG"

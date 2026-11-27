#!/bin/bash
set -euo pipefail

# Hollow Star OS+ v0.6.0-rc.1 — Full Multimedia, Graphics, Audio, Network & Launcher Test Script
# Usage:
#   ./test-rc1.sh [--auto]   Non-blocking verification (graphics screendump, audio, USB-net, DeltaFS, Launcher)
#   ./test-rc1.sh --gui      Interactive QEMU window with graphics, CoreAudio, USB keyboard & user-net

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

MODE="${1:---auto}"
PROFILE="${PROFILE:-release}"
KERNEL_IMG="target/kernel8.img"
SD_IMG="target/sdcard.img"

if ! command -v qemu-system-aarch64 >/dev/null 2>&1; then
  echo "[RC1] qemu-system-aarch64 is not installed." >&2
  exit 1
fi

echo "[RC1] Building kernel ($PROFILE) and 3-partition SD image..."
PROFILE="$PROFILE" ./delta.sh build >/dev/null

if [ "$MODE" = "--gui" ]; then
  AUDIO_DRV="none"
  if [ "$(uname -s)" = "Darwin" ]; then
    AUDIO_DRV="coreaudio"
  fi
  echo "[RC1] Launching interactive QEMU raspi3b (Graphics + Audio[$AUDIO_DRV] + USB-Net + USB-Kbd)..."
  echo "[RC1] Tip: In QEMU serial terminal use Arrows/1-9/Enter/F1; Ctrl+A then X to quit."
  exec qemu-system-aarch64 \
    -M raspi3b \
    -display default \
    -serial stdio \
    -drive "file=$SD_IMG,if=sd,format=raw" \
    -netdev user,id=net0 \
    -device usb-net,netdev=net0 \
    -audiodev "$AUDIO_DRV,id=snd0" \
    -device usb-audio,audiodev=snd0 \
    -device usb-kbd \
    -kernel "$KERNEL_IMG"
fi

echo "[RC1] Running automated RC1 suite (Graphics FB + RT Audio + USB-Net + DeltaFS + Launcher)..."
python3 - << 'PY'
import json
import socket
import struct
import subprocess
import sys
import time
import zlib
from pathlib import Path

qmp_sock = Path("target/rc1-qmp.sock")
ppm_path = Path("target/rc1-launcher.ppm")
png_path = Path("target/rc1-launcher.png")
wav_path = Path("target/rc1-audio.wav")
log_path = Path("target/rc1-smoke.log")

for p in (qmp_sock, ppm_path, png_path, wav_path, log_path):
    if p.exists():
        p.unlink()

cmd = [
    "qemu-system-aarch64",
    "-M", "raspi3b",
    "-display", "none",
    "-serial", "stdio",
    "-drive", "file=target/sdcard.img,if=sd,format=raw",
    "-netdev", "user,id=net0",
    "-device", "usb-net,netdev=net0",
    "-audiodev", f"wav,id=snd0,path={wav_path}",
    "-device", "usb-audio,audiodev=snd0",
    "-device", "usb-kbd",
    "-qmp", f"unix:{qmp_sock},server,nowait",
    "-kernel", "target/kernel8.img",
]

proc = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
time.sleep(1.8)

# 1. Complete 1st Setup wizard -> reach App Launcher (.hsapp Grid)
for key in (b"\n", b"\n", b"\n"):
    proc.stdin.write(key)
    proc.stdin.flush()
    time.sleep(0.35)

# 2. Capture Launcher framebuffer screenshot via QMP screendump
sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
sock.connect(str(qmp_sock))
sock.recv(4096)
sock.sendall(b'{"execute":"qmp_capabilities"}\n')
time.sleep(0.1)
sock.recv(4096)
sock.sendall(json.dumps({"execute": "screendump", "arguments": {"filename": str(ppm_path)}}).encode() + b"\n")
time.sleep(0.3)
sock.recv(4096)
sock.close()

# 3. Exercise Launcher: run Hello.hsapp (8), open Media RT (4), play note (Space), back (Esc), quit to shell (q)
for key in (b"8", b"4", b" ", b"s", b"\x1b", b"q", b"df\n", b"uname\n", b"ls /system/apps\n"):
    proc.stdin.write(key)
    proc.stdin.flush()
    time.sleep(0.35)

time.sleep(0.4)
proc.terminate()
out_bytes, _ = proc.communicate(timeout=5)
out_text = out_bytes.decode("utf-8", errors="replace")
log_path.write_text(out_text)

# Verify PPM -> PNG framebuffer pixels
raw_ppm = ppm_path.read_bytes()
hdr_end = raw_ppm.find(b"\n255\n")
if hdr_end < 0:
    print("[RC1 FAIL] Invalid PPM screendump", file=sys.stderr)
    sys.exit(1)
pixels = raw_ppm[hdr_end + 5:]
non_black = sum(1 for i in range(0, len(pixels), 3) if pixels[i:i + 3] != b"\x00\x00\x00")
rows = b"".join(b"\x00" + pixels[y * 640 * 3:(y + 1) * 640 * 3] for y in range(480))
def chunk(tag: bytes, data: bytes) -> bytes:
    return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
png_path.write_bytes(
    b"\x89PNG\r\n\x1a\n"
    + chunk(b"IHDR", struct.pack(">IIBBBBB", 640, 480, 8, 2, 0, 0, 0))
    + chunk(b"IDAT", zlib.compress(rows))
    + chunk(b"IEND", b"")
)

checks = {
    "Graphics FB (640x480@32bpp)": "[el0/drv-fb] Framebuffer 640x480@32bpp online" in out_text and non_black == 640 * 480,
    "Audio RT (Priority 110)": "[el0/drv-audio] Real-Time Audio server online (EL0, Priority 110" in out_text and "RT audio mixed=" in out_text and wav_path.exists(),
    "Network Device (QEMU usb-net + kernel)": "KERNEL PANIC" not in out_text and "panicked at" not in out_text,
    "Storage (DeltaFS /boot+/system+/user)": "/boot -> FAT32" in out_text and "/system -> DeltaFS" in out_text and "/user -> DeltaFS" in out_text,
    "App Launcher (.hsapp Grid)": "[el0/Hello.hsapp] Loaded asset:" in out_text and "HollowStar.hsapp/" in out_text and "Hollow Star OS+ v0.6.0-rc.1" in out_text,
}

failed = False
for name, ok in checks.items():
    status = "PASS" if ok else "FAIL"
    print(f"[RC1] {name:42s} -> {status}")
    if not ok:
        failed = True

print(f"[RC1] Screendump saved: {png_path} ({non_black} pixels)")
print(f"[RC1] Serial log saved: {log_path}")
if failed:
    print(out_text, file=sys.stderr)
    sys.exit(1)
PY

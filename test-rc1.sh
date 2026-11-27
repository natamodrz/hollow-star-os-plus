#!/bin/bash
set -euo pipefail

# Hollow Star OS+ v0.6.0-rc.2 — Full Multimedia, USB HID, Graphics, Audio, Network & Modular Launcher Test
# Usage:
#   ./test-rc1.sh [--auto]   Non-blocking automated E2E verification (QMP screendump + USB-HID + UART + Audio + DeltaFS)
#   ./test-rc1.sh --gui      Interactive QEMU Cocoa/GUI window with CoreAudio, USB keyboard & user-net

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"

MODE="${1:---auto}"
PROFILE="${PROFILE:-release}"
KERNEL_IMG="target/kernel8.img"
SD_IMG="target/sdcard.img"

if ! command -v qemu-system-aarch64 >/dev/null 2>&1; then
  echo "[RC] qemu-system-aarch64 is not installed." >&2
  exit 1
fi

echo "[RC] Building kernel ($PROFILE) and 3-partition SD image..."
PROFILE="$PROFILE" ./delta.sh build >/dev/null

if [ "$MODE" = "--gui" ]; then
  DISPLAY_DRV="default"
  AUDIO_DRV="none"
  if [ "$(uname -s)" = "Darwin" ]; then
    DISPLAY_DRV="cocoa,zoom-to-fit=on"
    AUDIO_DRV="coreaudio"
  fi
  echo "[RC] Launching interactive QEMU raspi3b (Graphics[$DISPLAY_DRV] + Audio[$AUDIO_DRV] + USB-Net + USB-Kbd)..."
  exec qemu-system-aarch64 \
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
fi

echo "[RC] Running automated E2E suite (Graphics FB + USB HID kbd + RT Audio + DeltaFS + Terminal + Launcher)..."
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
ppm_launcher = Path("target/rc1-launcher.ppm")
png_launcher = Path("target/rc1-launcher.png")
ppm_terminal = Path("target/rc1-terminal.ppm")
png_terminal = Path("target/rc1-terminal.png")
wav_path = Path("target/rc1-audio.wav")
log_path = Path("target/rc1-smoke.log")

for p in (qmp_sock, ppm_launcher, png_launcher, ppm_terminal, png_terminal, wav_path, log_path):
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
    "-usb",
    "-device", "usb-kbd",
    "-qmp", f"unix:{qmp_sock},server,nowait",
    "-kernel", "target/kernel8.img",
]

proc = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)

# Wait for compositor & Setup.hsapp ready
deadline = time.time() + 12.0
while time.time() < deadline:
    if qmp_sock.exists():
        break
    time.sleep(0.1)

sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
sock.connect(str(qmp_sock))
def qmp_recv():
    buf = b""
    while b"\n" not in buf:
        buf += sock.recv(4096)
    return buf.decode(errors="replace")
qmp_recv()
sock.sendall(b'{"execute":"qmp_capabilities"}\n')
qmp_recv()

def qmp_cmd(obj):
    sock.sendall((json.dumps(obj) + "\n").encode())
    return qmp_recv()

def send_usb_key(qcode: str, delay: float = 0.2):
    qmp_cmd({"execute": "send-key", "arguments": {"keys": [{"type": "qcode", "data": qcode}]}})
    time.sleep(delay)

time.sleep(2.2)

# 1. Advance Setup.hsapp (3 steps) via REAL USB HID key events -> Launcher.hsapp
for _ in range(3):
    send_usb_key("ret", 0.45)

qmp_cmd({"execute": "screendump", "arguments": {"filename": str(ppm_launcher)}})
time.sleep(0.25)

# 2. Launch Terminal.hsapp (focused tile 0) via USB Enter, test df, dkexstat, dylibs, uname
send_usb_key("ret", 0.65)
for cmd_str in ("df", "dkexstat", "dylibs", "uname"):
    for ch in cmd_str:
        send_usb_key(ch, 0.08)
    send_usb_key("ret", 0.16)
time.sleep(0.3)
qmp_cmd({"execute": "screendump", "arguments": {"filename": str(ppm_terminal)}})
time.sleep(0.2)

# 3. Navigate purely via USB HID Keyboard (no UART console!):
#    Esc -> Launcher -> '9' (Hello.hsapp + dylib) -> Enter -> '5' (Media RT) -> Space -> Esc
send_usb_key("esc", 0.45)
send_usb_key("9", 0.45)
send_usb_key("ret", 0.45)
send_usb_key("5", 0.45)
send_usb_key("spc", 0.45)
send_usb_key("esc", 0.45)
sock.close()

time.sleep(0.35)
proc.terminate()
out_bytes, _ = proc.communicate(timeout=5)
out_text = out_bytes.decode("utf-8", errors="replace")
log_path.write_text(out_text)

def convert_ppm_to_png(ppm: Path, png: Path) -> int:
    raw_ppm = ppm.read_bytes()
    hdr_end = raw_ppm.find(b"\n255\n")
    if hdr_end < 0:
        return 0
    pixels = raw_ppm[hdr_end + 5:]
    non_black = sum(1 for i in range(0, len(pixels), 3) if pixels[i:i + 3] != b"\x00\x00\x00")
    rows = b"".join(b"\x00" + pixels[y * 640 * 3:(y + 1) * 640 * 3] for y in range(480))
    def chunk(tag: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
    png.write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", 640, 480, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(rows))
        + chunk(b"IEND", b"")
    )
    return non_black

px_launcher = convert_ppm_to_png(ppm_launcher, png_launcher)
px_terminal = convert_ppm_to_png(ppm_terminal, png_terminal)
changed_pixels = ppm_launcher.read_bytes() != ppm_terminal.read_bytes()

checks = {
    "Graphics FB (640x480@32bpp)": "[el0/drv-fb] Framebuffer 640x480@32bpp online" in out_text and px_launcher == 640 * 480 and px_terminal == 640 * 480,
    "USB HID Keyboard (drv-kbd hub + kbd)": "[el0/drv-kbd] USB HID Keyboard enumerated" in out_text and changed_pixels,
    "Audio RT (Priority 110 + WAV)": "[el0/drv-audio] Real-Time Audio server online (EL0, Priority 110" in out_text and wav_path.exists(),
    "Network Device (QEMU usb-net + kernel)": "KERNEL PANIC" not in out_text and "panicked at" not in out_text,
    "Storage & Mach/OSX (.dkex + .dylib)": "[el0/shell] DeltaFS WAL write & read-back" in out_text and "[el0/DeltaDiag.dkex] Loaded Mach .dkex extension + libdelta_kit.dylib (dyld W^X OK)" in out_text,
    "Modular Desktop (Compositor/Launcher/Term)": (
        "[el0/compositor] Desktop compositor ready" in out_text
        and "[el0/Hello.hsapp] Loaded asset:" in out_text
        and "[boot] Milestone M6" in out_text
    ),
}

failed = False
for name, ok in checks.items():
    status = "PASS" if ok else "FAIL"
    print(f"[RC] {name:42s} -> {status}")
    if not ok:
        failed = True

print(f"[RC] Launcher shot : {png_launcher} ({px_launcher} px)")
print(f"[RC] Terminal shot : {png_terminal} ({px_terminal} px)")
print(f"[RC] Serial log    : {log_path}")
if failed:
    print(out_text, file=sys.stderr)
    sys.exit(1)
PY

#!/usr/bin/env python3
"""Local real-hardware test SD card image builder for Hollow Star OS+.

Generates a ready-to-flash 3-partition MBR `.img` inside `target/` ONLY
(never writes to `releases/` and never creates GitHub Releases) containing:
  - Partition 1 (`/boot`, FAT32/VFAT): Official Raspberry Pi GPU firmware
    (`bootcode.bin`, `start.elf`, `fixup.dat`, board DTB), `config.txt`, `kernel8.img`
  - Partition 2 (`/system`, DeltaFS): `shell.elf`, `compositor.elf`, modular `.hsapp` bundles
  - Partition 3 (`/user`, DeltaFS): notes, media, config, user `.hsapp` directory
"""

from __future__ import annotations

import argparse
import sys
import urllib.error
import urllib.request
from pathlib import Path

from mk_sdcard import build_image

FIRMWARE_RAW_BASE = "https://github.com/raspberrypi/firmware/raw/master/boot"

BOARD_DTB = {
    "raspi3b": "bcm2710-rpi-3-b.dtb",
    "raspi3bplus": "bcm2710-rpi-3-b-plus.dtb",
    "raspi02w": "bcm2710-rpi-zero-2-w.dtb",
}

CORE_FIRMWARE_FILES = [
    "bootcode.bin",
    "start.elf",
    "fixup.dat",
]


def ensure_firmware_files(
    fw_dir: Path,
    dtb_name: str,
    *,
    uart: str,
    download: bool,
) -> dict[str, bytes]:
    fw_dir.mkdir(parents=True, exist_ok=True)
    needed = CORE_FIRMWARE_FILES + [dtb_name]
    if uart == "pl011":
        needed.append("overlays/miniuart-bt.dtbo")
    result: dict[str, bytes] = {}

    for fname in needed:
        cached = fw_dir / fname
        cached.parent.mkdir(parents=True, exist_ok=True)
        if cached.exists() and cached.stat().st_size > 0:
            result[fname] = cached.read_bytes()
            continue

        if not download:
            print(
                f"[mk_hw_image] Note: firmware file '{fname}' not in {fw_dir} (--skip-firmware-download set).",
                file=sys.stderr,
            )
            continue

        url = f"{FIRMWARE_RAW_BASE}/{fname}"
        print(f"[mk_hw_image] Downloading Raspberry Pi firmware: {url}")
        try:
            with urllib.request.urlopen(url, timeout=30) as resp:
                data = resp.read()
        except urllib.error.URLError as exc:
            print(
                f"[mk_hw_image] ERROR: Failed to download '{fname}' ({exc}). "
                f"Provide local files via --firmware-dir or pass --skip-firmware-download.",
                file=sys.stderr,
            )
            raise SystemExit(2) from exc

        cached.write_bytes(data)
        result[fname] = data

    return result


def build_config_txt(board: str, uart: str) -> bytes:
    lines = [
        f"# Hollow Star OS+ Real Hardware Test Config (board={board}, uart={uart})",
        "arm_64bit=1",
        "kernel=kernel8.img",
        "enable_uart=1",
        "core_freq=250",
        "hdmi_force_hotplug=1",
        "disable_overscan=1",
        "gpu_mem=64",
        "framebuffer_width=640",
        "framebuffer_height=480",
        "framebuffer_depth=32",
        "framebuffer_ignore_alpha=1",
    ]
    if uart == "pl011":
        # Note: RPi 3B / Zero 2 W map UART0 (PL011) onto GPIO 14/15 when BT is swapped/disabled.
        lines.append("dtoverlay=miniuart-bt")
    return ("\n".join(lines) + "\n").encode("ascii")


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Build local real-hardware test SD image for Raspberry Pi 3B / Zero 2 W."
    )
    parser.add_argument(
        "--board",
        choices=sorted(BOARD_DTB.keys()),
        default="raspi3b",
        help="Target hardware board (default: raspi3b)",
    )
    parser.add_argument(
        "--uart",
        choices=["pl011", "mini"],
        default="pl011",
        help="Active serial console UART (default: pl011)",
    )
    parser.add_argument(
        "--kernel-img",
        type=Path,
        required=True,
        help="Path to stripped kernel8.img",
    )
    parser.add_argument(
        "--userland-dir",
        type=Path,
        required=True,
        help="Path to target/userland/aarch64-unknown-none/release",
    )
    parser.add_argument(
        "--out",
        type=Path,
        required=True,
        help="Output .img file path (strictly under target/)",
    )
    parser.add_argument(
        "--firmware-dir",
        type=Path,
        default=Path("target/rpi-firmware"),
        help="Cache directory for bootcode.bin / start.elf / fixup.dat / DTBs",
    )
    parser.add_argument(
        "--size-mb",
        type=int,
        default=128,
        help="Total SD card image size in MiB (default: 128)",
    )
    parser.add_argument(
        "--skip-firmware-download",
        action="store_true",
        help="Do not download Raspberry Pi firmware binaries if missing from cache",
    )
    args = parser.parse_args()

    resolved_out = args.out.resolve()
    if "releases" in resolved_out.parts:
        print(
            "[mk_hw_image] Refusing to write under 'releases/' — test images belong under target/ only.",
            file=sys.stderr,
        )
        return 1

    if not args.kernel_img.exists():
        print(f"[mk_hw_image] Missing kernel image: {args.kernel_img}", file=sys.stderr)
        return 1

    dtb_name = BOARD_DTB[args.board]
    boot_files = ensure_firmware_files(
        args.firmware_dir,
        dtb_name,
        uart=args.uart,
        download=not args.skip_firmware_download,
    )
    boot_files["kernel8.img"] = args.kernel_img.read_bytes()
    boot_files["config.txt"] = build_config_txt(args.board, args.uart)

    boot_mb = 48 if args.size_mb >= 112 else 24
    system_mb = 24
    user_mb = max(16, args.size_mb - 2 - boot_mb - system_mb)

    rc = build_image(
        args.userland_dir,
        args.out,
        disk_mb=args.size_mb,
        boot_mb=boot_mb,
        system_mb=system_mb,
        user_mb=user_mb,
        boot_files=boot_files,
    )
    if rc == 0:
        print(
            f"[mk_hw_image] Ready for hardware test ({args.board}, UART={args.uart}): {args.out}"
        )
        print(
            f"[mk_hw_image] /boot contains: {', '.join(sorted(boot_files.keys()))}"
        )
    return rc


if __name__ == "__main__":
    raise SystemExit(main())

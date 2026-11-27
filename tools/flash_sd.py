#!/usr/bin/env python3
"""Cross-platform (macOS / Linux) SD card detector and interactive image flasher
for Hollow Star OS+ test images built by `build-test-image.sh`.
"""

from __future__ import annotations

import argparse
import json
import platform
import plistlib
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path


@dataclass
class DiskEntry:
    dev: str          # e.g. /dev/disk4 or /dev/sdb
    raw_dev: str      # e.g. /dev/rdisk4 or /dev/sdb
    disk_id: str      # e.g. disk4 or sdb
    size_bytes: int
    model: str
    transport: str
    removable: bool
    virtual: bool
    is_system: bool
    mountpoints: str

    def human_size(self) -> str:
        if self.size_bytes <= 0:
            return "0 B"
        gb = self.size_bytes / 1_000_000_000
        if gb >= 1.0:
            return f"{gb:.1f} GB"
        mb = self.size_bytes / 1_000_000
        return f"{mb:.0f} MB"

    def is_safe_target(self) -> bool:
        return not self.virtual and not self.is_system and self.removable


def detect_os() -> str:
    sys_name = platform.system()
    if sys_name == "Darwin":
        return "macos"
    if sys_name == "Linux":
        return "linux"
    print(f"[flash] Nieobsługiwany system operacyjny: {sys_name} (wymagany macOS lub Linux).", file=sys.stderr)
    raise SystemExit(1)


def list_disks_macos() -> list[DiskEntry]:
    raw_list = subprocess.check_output(["diskutil", "list", "-plist"])
    pl = plistlib.loads(raw_list)

    # Identify the physical whole disk backing `/` so we never touch the OS drive.
    system_whole_disks: set[str] = set()
    try:
        root_info = plistlib.loads(subprocess.check_output(["diskutil", "info", "-plist", "/"]))
        parent = root_info.get("ParentWholeDisk")
        if parent:
            system_whole_disks.add(parent)
        for store in root_info.get("APFSPhysicalStores", []):
            ps = store.get("APFSPhysicalStore", "")
            m = re.match(r"^(disk\d+)", ps)
            if m:
                system_whole_disks.add(m.group(1))
    except Exception:
        system_whole_disks.add("disk0")

    # Collect mountpoints per whole disk from `AllDisksAndPartitions`.
    mounts_by_disk: dict[str, list[str]] = {}
    for disk_obj in pl.get("AllDisksAndPartitions", []):
        did = disk_obj.get("DeviceIdentifier", "")
        mps: list[str] = []
        if disk_obj.get("MountPoint"):
            mps.append(disk_obj["MountPoint"])
        for part in disk_obj.get("Partitions", []) + disk_obj.get("APFSVolumes", []):
            if part.get("MountPoint"):
                mps.append(part["MountPoint"])
        if did:
            mounts_by_disk[did] = mps

    entries: list[DiskEntry] = []
    for d in pl.get("WholeDisks", []):
        try:
            info = plistlib.loads(subprocess.check_output(["diskutil", "info", "-plist", d]))
        except Exception:
            continue
        virtual = info.get("VirtualOrPhysical") == "Virtual"
        internal = bool(info.get("Internal", False))
        ext_or_rm = bool(info.get("RemovableMediaOrExternalDevice", False) or info.get("RemovableMedia", False))
        mps = mounts_by_disk.get(d, [])
        is_sys = (d in system_whole_disks) or (internal and not ext_or_rm) or ("/" in mps)
        entries.append(
            DiskEntry(
                dev=f"/dev/{d}",
                raw_dev=f"/dev/r{d}",
                disk_id=d,
                size_bytes=int(info.get("TotalSize", 0)),
                model=str(info.get("MediaName") or info.get("IORegistryEntryName") or "Disk").strip(),
                transport=str(info.get("BusProtocol") or ("External" if not internal else "Internal")),
                removable=ext_or_rm or not internal,
                virtual=virtual,
                is_system=is_sys,
                mountpoints=", ".join(mps) if mps else "-",
            )
        )
    return entries


def _collect_linux_mounts(node: dict) -> list[str]:
    mps: list[str] = []
    mp = node.get("mountpoint")
    if mp:
        mps.append(str(mp))
    for m in node.get("mountpoints") or []:
        if m and str(m) not in mps:
            mps.append(str(m))
    for child in node.get("children") or []:
        for cmp in _collect_linux_mounts(child):
            if cmp not in mps:
                mps.append(cmp)
    return mps


def list_disks_linux() -> list[DiskEntry]:
    cols = "NAME,PATH,SIZE,TYPE,RM,TRAN,MODEL,MOUNTPOINTS"
    try:
        raw = subprocess.check_output(["lsblk", "--json", "-b", "-o", cols], stderr=subprocess.DEVNULL)
    except subprocess.CalledProcessError:
        raw = subprocess.check_output(["lsblk", "--json", "-b", "-o", "NAME,PATH,SIZE,TYPE,RM,TRAN,MODEL,MOUNTPOINT"])
    data = json.loads(raw.decode("utf-8", errors="replace"))

    entries: list[DiskEntry] = []
    for dev in data.get("blockdevices", []):
        if dev.get("type") != "disk":
            continue
        name = str(dev.get("name", ""))
        path = str(dev.get("path") or f"/dev/{name}")
        if name.startswith(("loop", "ram", "zram")):
            continue
        mps = _collect_linux_mounts(dev)
        rm = bool(dev.get("rm", False))
        tran = str(dev.get("tran") or "").lower()
        is_sys = any(m in ("/", "/boot", "/boot/efi", "/home", "/usr", "/var") for m in mps)
        is_removable = rm or tran in ("usb", "mmc") or name.startswith("mmcblk")
        entries.append(
            DiskEntry(
                dev=path,
                raw_dev=path,
                disk_id=name,
                size_bytes=int(dev.get("size") or 0),
                model=str(dev.get("model") or "Block Device").strip(),
                transport=tran.upper() if tran else "DISK",
                removable=is_removable and not is_sys,
                virtual=False,
                is_system=is_sys,
                mountpoints=", ".join(mps) if mps else "-",
            )
        )
    return entries


def list_disks(os_kind: str) -> list[DiskEntry]:
    return list_disks_macos() if os_kind == "macos" else list_disks_linux()


def print_disks(os_kind: str, disks: list[DiskEntry]) -> None:
    os_label = "macOS (Darwin)" if os_kind == "macos" else "Linux"
    print("")
    print(f"[flash] Wykryty system operacyjny: {os_label}")
    print("[flash] Wykryte dyski:")
    if not disks:
        print("  (brak wykrytych dysków)")
        return
    for idx, d in enumerate(disks, start=1):
        if d.virtual:
            tag = "WIRTUALNY (zablokowany)"
        elif d.is_system:
            tag = "SYSTEMOWY (zablokowany)"
        elif d.removable:
            tag = "ZEWNĘTRZNY / microSD / USB (DOSTĘPNY)"
        else:
            tag = "WEWNĘTRZNY (wymaga ostrożności)"
        print(
            f"  [{idx}] {d.dev:<14} {d.human_size():>8} | {d.transport:<12} | {d.model:<24} | {tag} | mount: {d.mountpoints}"
        )


def unmount_and_flash(os_kind: str, disk: DiskEntry, image_path: Path) -> None:
    print("")
    if os_kind == "macos":
        print(f"[flash] Odmontowuję {disk.dev}...")
        subprocess.run(["diskutil", "unmountDisk", disk.dev], check=True)
        print(f"[flash] Zapisuję {image_path} -> {disk.raw_dev} (sudo może poprosić o hasło)...")
        subprocess.run(
            ["sudo", "dd", f"if={image_path}", f"of={disk.raw_dev}", "bs=4m", "status=progress"],
            check=True,
        )
        subprocess.run(["sync"], check=True)
        print(f"[flash] Wysuwam bezpiecznie {disk.dev}...")
        subprocess.run(["diskutil", "eject", disk.dev], check=True)
    else:
        print(f"[flash] Odmontowuję partycje na {disk.dev}...")
        out = subprocess.check_output(["lsblk", "-ln", "-o", "PATH", disk.dev]).decode()
        for part_path in out.splitlines():
            p = part_path.strip()
            if p and p != disk.dev:
                subprocess.run(["sudo", "umount", p], stderr=subprocess.DEVNULL, check=False)
        print(f"[flash] Zapisuję {image_path} -> {disk.dev} (sudo może poprosić o hasło)...")
        subprocess.run(
            [
                "sudo",
                "dd",
                f"if={image_path}",
                f"of={disk.dev}",
                "bs=4M",
                "status=progress",
                "conv=fsync",
            ],
            check=True,
        )
        subprocess.run(["sync"], check=True)
        subprocess.run(["sudo", "partprobe", disk.dev], stderr=subprocess.DEVNULL, check=False)
        subprocess.run(["sudo", "eject", disk.dev], stderr=subprocess.DEVNULL, check=False)

    print("")
    print(f"[flash] GOTOWE! Obraz zapisano na {disk.dev}. Możesz bezpiecznie wyjąć kartę microSD.")


def resolve_selection(choice: str, disks: list[DiskEntry]) -> DiskEntry | None:
    c = choice.strip()
    if c.isdigit():
        idx = int(c)
        if 1 <= idx <= len(disks):
            return disks[idx - 1]
        return None
    for d in disks:
        if c in (d.dev, d.raw_dev, d.disk_id):
            return d
    return None


def main() -> int:
    parser = argparse.ArgumentParser(description="List disks and flash Hollow Star OS+ test image.")
    parser.add_argument("--list", action="store_true", help="Only list detected disks and exit")
    parser.add_argument("--image", type=Path, help="Path to built .img file")
    parser.add_argument("--disk", type=str, default="", help="Pre-selected target disk (e.g. /dev/disk4 or /dev/sdb)")
    parser.add_argument("--yes", action="store_true", help="Skip confirmation prompt when --disk is given")
    args = parser.parse_args()

    os_kind = detect_os()
    disks = list_disks(os_kind)
    print_disks(os_kind, disks)

    if args.list:
        return 0

    if not args.image or not args.image.exists():
        print(f"[flash] Brak pliku obrazu: {args.image}", file=sys.stderr)
        return 1

    target: DiskEntry | None = None
    if args.disk:
        target = resolve_selection(args.disk, disks)
        if target is None:
            print(f"[flash] Nie znaleziono dysku: {args.disk}", file=sys.stderr)
            return 1
        if not target.is_safe_target():
            print(
                f"[flash] BLOKADA BEZPIECZEŃSTWA: {target.dev} wygląda na dysk systemowy lub wirtualny.",
                file=sys.stderr,
            )
            return 1
    else:
        if not sys.stdin.isatty():
            print("[flash] Tryb nieinteraktywny — pomijam nagrywanie na kartę SD.")
            return 0
        while True:
            print("")
            try:
                ans = input(
                    "Wybierz numer dysku do zapisu (lub wpisz np. /dev/disk4; Enter lub 0 = pomiń): "
                ).strip()
            except EOFError:
                return 0
            if ans == "" or ans == "0" or ans.lower() in ("q", "quit", "exit"):
                print(f"[flash] Pominięto nagrywanie. Obraz zapisano lokalnie w: {args.image}")
                return 0
            cand = resolve_selection(ans, disks)
            if cand is None:
                print("[flash] Nieprawidłowy wybór. Podaj numer z listy lub poprawną ścieżkę urządzenia.")
                continue
            if cand.virtual or cand.is_system:
                print(
                    f"[flash] BLOKADA BEZPIECZEŃSTWA: {cand.dev} ({cand.model}) to dysk systemowy/wirtualny! Wybierz zewnętrzną kartę microSD/USB."
                )
                continue
            if not cand.removable:
                confirm_int = input(
                    f"[flash] UWAGA: {cand.dev} nie jest oznaczony jako wymienny. Wpisz nazwę '{cand.disk_id}' aby kontynuować (lub Enter aby anulować): "
                ).strip()
                if confirm_int != cand.disk_id:
                    continue
            target = cand
            break

    img_mb = args.image.stat().st_size / (1024 * 1024)
    if not args.yes:
        try:
            conf = input(
                f"Czy NA PEWNO nadpisać {target.dev} ({target.human_size()}, {target.model}) obrazem {args.image.name} ({img_mb:.0f} MiB)? [tak/N]: "
            ).strip()
        except EOFError:
            return 0
        if conf.lower() not in ("tak", "t", "yes", "y"):
            print("[flash] Anulowano zapis na kartę.")
            return 0

    try:
        unmount_and_flash(os_kind, target, args.image)
    except subprocess.CalledProcessError as exc:
        print(f"[flash] Błąd podczas zapisu na {target.dev}: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

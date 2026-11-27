#!/usr/bin/env python3
"""Builds a 3-partition MBR SD card image for Hollow Star OS+ (`v0.6.0-rc.2`):
  1. Partition 1 (`0x0C` FAT32/VFAT) -> `/boot`   (firmware boot files, config.txt, kernel8.img)
  2. Partition 2 (`0xDF` DeltaFS)    -> `/system` (shell.elf, compositor.elf, modular .hsapp bundles)
  3. Partition 3 (`0xDF` DeltaFS)    -> `/user`   (notes, config, media, user apps + free tail/expand)
"""

from __future__ import annotations

import argparse
import math
import struct
import sys
import zlib
from pathlib import Path

SECTOR_SIZE = 512
DELTA_BLOCK = 4096
SECTORS_PER_BLOCK = DELTA_BLOCK // SECTOR_SIZE

BOOT_START = 2048
FAT32_RESERVED = 32
FAT32_NUM_FATS = 2
FAT32_ROOT_CLUSTER = 2


def _lba_to_chs(lba: int) -> bytes:
    """Encodes an LBA sector into an MBR CHS 3-byte tuple (255 heads, 63 sectors/track)."""
    hpc = 255
    spt = 63
    c = min(1023, lba // (hpc * spt))
    rem = lba % (hpc * spt)
    h = rem // spt
    s = (rem % spt) + 1
    return bytes((h & 0xFF, ((c >> 2) & 0xC0) | (s & 0x3F), c & 0xFF))


class Fat32Builder:
    def __init__(self, start_lba: int, total_sectors: int) -> None:
        self.start_lba = start_lba
        self.total_sectors = total_sectors
        # Use 512 B/cluster when partition >= ~33 MiB so cluster count exceeds 65,525 (strict FAT32).
        self.spc = 1 if total_sectors >= 68_000 else 8
        self.cluster_size = self.spc * SECTOR_SIZE
        # Size FAT large enough to address every cluster in this partition.
        est_clusters = max(256, total_sectors // self.spc)
        self.spf = max(32, math.ceil((est_clusters + 4) * 4 / SECTOR_SIZE))
        self.fat = [0x0FFFFFF8, 0x0FFFFFFF, 0x0FFFFFFF]
        self.clusters: dict[int, bytearray] = {2: bytearray(self.cluster_size)}
        self.dir_chains: dict[int, list[int]] = {2: [2]}
        self.sfn_seq = 1

    def alloc_chain(self, payload: bytes) -> int:
        if not payload:
            return 0
        num = math.ceil(len(payload) / self.cluster_size)
        first = len(self.fat)
        for i in range(num):
            cid = first + i
            self.fat.append(0x0FFFFFFF if i + 1 == num else cid + 1)
            chunk = payload[i * self.cluster_size : (i + 1) * self.cluster_size]
            buf = bytearray(self.cluster_size)
            buf[: len(chunk)] = chunk
            self.clusters[cid] = buf
        return first

    @staticmethod
    def _is_clean_83(name: str) -> bool:
        if len(name) > 12 or " " in name:
            return False
        if "." in name:
            parts = name.split(".")
            if len(parts) != 2 or not (1 <= len(parts[0]) <= 8) or not (1 <= len(parts[1]) <= 3):
                return False
            allowed = parts[0] + parts[1]
        else:
            if not (1 <= len(name) <= 8):
                return False
            allowed = name
        return all(ch.isascii() and (ch.isalnum() or ch in "_-$~!#%&{}()@'`^") for ch in allowed)

    def _encode_83(self, name: str) -> bytes:
        upper = name.upper()
        base, ext = upper.rsplit(".", 1) if "." in upper else (upper, "")
        clean_base = "".join(c for c in base if c.isalnum() or c in "_-$~")
        clean_ext = "".join(c for c in ext if c.isalnum())[:3]
        if not self._is_clean_83(name):
            suffix = f"~{self.sfn_seq}"
            self.sfn_seq += 1
            clean_base = (clean_base[: max(1, 8 - len(suffix))] + suffix)[:8]
        return clean_base[:8].ljust(8, " ").encode("ascii") + clean_ext.ljust(3, " ").encode("ascii")

    @staticmethod
    def _sfn_checksum(sfn: bytes) -> int:
        chk = 0
        for b in sfn:
            chk = (((chk & 1) << 7) + (chk >> 1) + b) & 0xFF
        return chk

    def _alloc_dir_slots(self, dir_cid: int, count: int) -> tuple[bytearray, int]:
        """Finds `count` contiguous 32-byte directory slots inside `dir_cid`, growing chain if full."""
        chain = self.dir_chains.setdefault(dir_cid, [dir_cid])
        for cid in chain:
            buf = self.clusters[cid]
            for off in range(0, len(buf) - (count - 1) * 32, 32):
                if all(buf[off + k * 32] == 0 for k in range(count)):
                    return buf, off
        # Allocate a new cluster and link it to the tail of this directory chain.
        new_cid = len(self.fat)
        self.fat[chain[-1]] = new_cid
        self.fat.append(0x0FFFFFFF)
        new_buf = bytearray(self.cluster_size)
        self.clusters[new_cid] = new_buf
        chain.append(new_cid)
        return new_buf, 0

    def _ensure_subdir(self, name: str) -> int:
        upper = name.upper().encode("ascii")[:8].ljust(8, b" ") + b"   "
        for rcid in self.dir_chains.get(FAT32_ROOT_CLUSTER, [FAT32_ROOT_CLUSTER]):
            root_buf = self.clusters[rcid]
            for off in range(0, len(root_buf), 32):
                if root_buf[off : off + 11] == upper and (root_buf[off + 11] & 0x10) != 0:
                    hi = struct.unpack_from("<H", root_buf, off + 20)[0]
                    lo = struct.unpack_from("<H", root_buf, off + 26)[0]
                    return (hi << 16) | lo
        # Allocate 1 empty cluster for the subdirectory with `.` and `..`.
        sub_cid = len(self.fat)
        self.fat.append(0x0FFFFFFF)
        sub_buf = bytearray(self.cluster_size)
        sub_buf[0:11] = b".          "
        sub_buf[11] = 0x10
        struct.pack_into("<H", sub_buf, 20, (sub_cid >> 16) & 0xFFFF)
        struct.pack_into("<H", sub_buf, 26, sub_cid & 0xFFFF)
        sub_buf[32:43] = b"..         "
        sub_buf[43] = 0x10
        self.clusters[sub_cid] = sub_buf
        self.dir_chains[sub_cid] = [sub_cid]

        rbuf, off = self._alloc_dir_slots(FAT32_ROOT_CLUSTER, 1)
        rbuf[off : off + 11] = upper
        rbuf[off + 11] = 0x10
        struct.pack_into("<H", rbuf, off + 20, (sub_cid >> 16) & 0xFFFF)
        struct.pack_into("<H", rbuf, off + 26, sub_cid & 0xFFFF)
        return sub_cid

    def add_file(self, name: str, payload: bytes) -> None:
        if "/" in name:
            subdir, leaf = name.split("/", 1)
            dir_cid = self._ensure_subdir(subdir)
            name = leaf
        else:
            dir_cid = FAT32_ROOT_CLUSTER

        cid = self.alloc_chain(payload)
        sfn = self._encode_83(name)
        need_lfn = not self._is_clean_83(name) or name != name.upper()
        # Only emit LFN when name exceeds 8.3; standard 8.3 uppercase avoids extra slots.
        if not need_lfn:
            buf, off = self._alloc_dir_slots(dir_cid, 1)
            self._write_sfn_entry(buf, off, sfn, cid, len(payload))
            return

        # Build UTF-16LE LFN chunks in reverse order.
        chars = [ord(c) for c in name] + [0x0000]
        while len(chars) % 13 != 0:
            chars.append(0xFFFF)
        num_lfn = len(chars) // 13
        chk = self._sfn_checksum(sfn)

        buf, off = self._alloc_dir_slots(dir_cid, num_lfn + 1)
        for seq in range(num_lfn, 0, -1):
            base = (seq - 1) * 13
            chunk = chars[base : base + 13]
            ord_byte = seq | (0x40 if seq == num_lfn else 0x00)
            lfn_off = off + (num_lfn - seq) * 32
            buf[lfn_off] = ord_byte
            for j in range(5):
                struct.pack_into("<H", buf, lfn_off + 1 + j * 2, chunk[j])
            buf[lfn_off + 11] = 0x0F
            buf[lfn_off + 12] = 0x00
            buf[lfn_off + 13] = chk
            for j in range(6):
                struct.pack_into("<H", buf, lfn_off + 14 + j * 2, chunk[5 + j])
            struct.pack_into("<H", buf, lfn_off + 26, 0)
            for j in range(2):
                struct.pack_into("<H", buf, lfn_off + 28 + j * 2, chunk[11 + j])

        self._write_sfn_entry(buf, off + num_lfn * 32, sfn, cid, len(payload))

    @staticmethod
    def _write_sfn_entry(buf: bytearray, off: int, sfn: bytes, cid: int, size: int) -> None:
        buf[off : off + 11] = sfn
        buf[off + 11] = 0x20
        struct.pack_into("<H", buf, off + 20, (cid >> 16) & 0xFFFF)
        struct.pack_into("<H", buf, off + 26, cid & 0xFFFF)
        struct.pack_into("<I", buf, off + 28, size)

    def write_to_image(self, img: bytearray) -> None:
        p_off = self.start_lba * SECTOR_SIZE
        # Primary Boot Sector (sector 0)
        bpb = bytearray(SECTOR_SIZE)
        bpb[0:3] = b"\xEB\x58\x90"
        bpb[3:11] = b"DELTAOS "
        struct.pack_into("<H", bpb, 11, SECTOR_SIZE)
        bpb[13] = self.spc
        struct.pack_into("<H", bpb, 14, FAT32_RESERVED)
        bpb[16] = FAT32_NUM_FATS
        bpb[21] = 0xF8  # Media descriptor: fixed/removable disk
        struct.pack_into("<H", bpb, 24, 63)  # Sectors per track
        struct.pack_into("<H", bpb, 26, 255)  # Heads
        struct.pack_into("<I", bpb, 28, self.start_lba)  # Hidden sectors
        struct.pack_into("<I", bpb, 32, self.total_sectors)
        struct.pack_into("<I", bpb, 36, self.spf)
        struct.pack_into("<I", bpb, 44, FAT32_ROOT_CLUSTER)
        struct.pack_into("<H", bpb, 48, 1)  # FSInfo sector
        struct.pack_into("<H", bpb, 50, 6)  # Backup boot sector
        bpb[64] = 0x80
        bpb[66] = 0x29  # Extended boot signature
        struct.pack_into("<I", bpb, 67, 0x4445_4C54)
        bpb[71:82] = b"DELTA_BOOT "
        bpb[82:90] = b"FAT32   "
        bpb[510:512] = b"\x55\xAA"

        img[p_off : p_off + SECTOR_SIZE] = bpb
        # Backup Boot Sector (sector 6)
        bkp_off = p_off + 6 * SECTOR_SIZE
        img[bkp_off : bkp_off + SECTOR_SIZE] = bpb

        # FSInfo Sector (sector 1)
        fsinfo = bytearray(SECTOR_SIZE)
        struct.pack_into("<I", fsinfo, 0, 0x41615252)
        struct.pack_into("<I", fsinfo, 484, 0x61417272)
        data_sectors = self.total_sectors - FAT32_RESERVED - FAT32_NUM_FATS * self.spf
        free_clusters = max(0, (data_sectors // self.spc) - (len(self.fat) - 2))
        struct.pack_into("<I", fsinfo, 488, free_clusters)
        struct.pack_into("<I", fsinfo, 492, len(self.fat))
        struct.pack_into("<I", fsinfo, 508, 0xAA550000)
        img[p_off + SECTOR_SIZE : p_off + 2 * SECTOR_SIZE] = fsinfo

        for fat_copy in range(FAT32_NUM_FATS):
            fat_off = p_off + (FAT32_RESERVED + fat_copy * self.spf) * SECTOR_SIZE
            for idx, val in enumerate(self.fat):
                struct.pack_into("<I", img, fat_off + idx * 4, val)

        data_off = p_off + (FAT32_RESERVED + FAT32_NUM_FATS * self.spf) * SECTOR_SIZE
        for cid, cbuf in self.clusters.items():
            off = data_off + (cid - 2) * self.cluster_size
            img[off : off + len(cbuf)] = cbuf


class DeltaFsBuilder:
    def __init__(self, start_lba: int, sectors: int, role: int, label: str) -> None:
        self.start_lba = start_lba
        self.total_blocks = sectors // SECTORS_PER_BLOCK
        self.role = role
        self.label = label
        self.journal_start = 1
        self.journal_blocks = 16
        self.bitmap_start = 17
        needed_bm_bytes = math.ceil(self.total_blocks / 8)
        self.bitmap_blocks = max(8, math.ceil(needed_bm_bytes / DELTA_BLOCK))
        self.inode_start = self.bitmap_start + self.bitmap_blocks
        self.inode_blocks = 16
        self.inode_count = self.inode_blocks * (DELTA_BLOCK // 128)
        self.data_start = self.inode_start + self.inode_blocks
        self.next_block = self.data_start
        self.next_inode = 1

        self.blocks: dict[int, bytearray] = {}
        self.bitmap = bytearray(self.bitmap_blocks * DELTA_BLOCK)
        self.inode_table = bytearray(self.inode_blocks * DELTA_BLOCK)

        # Reserve metadata blocks
        for b in range(self.data_start):
            self._mark_used(b)

        self.root_inode = self._create_dir(0, "", kind=2, attr=b"directory")

    def _mark_used(self, blk: int) -> None:
        self.bitmap[blk // 8] |= 1 << (blk % 8)

    def _alloc_blocks(self, count: int) -> int:
        start = self.next_block
        if start + count > self.total_blocks:
            raise RuntimeError(f"DeltaFS {self.label} out of space")
        for b in range(start, start + count):
            self._mark_used(b)
            self.blocks[b] = bytearray(DELTA_BLOCK)
        self.next_block += count
        return start

    def _write_inode(
        self,
        ino_id: int,
        kind: int,
        mode: int,
        size: int,
        start_blk: int,
        blk_count: int,
        attr: bytes = b"",
    ) -> None:
        off = (ino_id - 1) * 128
        struct.pack_into("<QBBHI", self.inode_table, off, ino_id, kind, 1, 0, mode)
        struct.pack_into("<Q", self.inode_table, off + 16, size)
        struct.pack_into("<Q", self.inode_table, off + 24, start_blk)
        struct.pack_into("<I", self.inode_table, off + 72, blk_count)
        attr_pad = attr[:32]
        self.inode_table[off + 96 : off + 96 + len(attr_pad)] = attr_pad

    def _add_dirent(self, dir_blk: int, child_id: int, kind: int, name: str) -> None:
        buf = self.blocks[dir_blk]
        nbytes = name.encode("utf-8")[:118]
        for off in range(0, DELTA_BLOCK, 128):
            existing = struct.unpack_from("<Q", buf, off)[0]
            if existing == 0:
                struct.pack_into("<QBB", buf, off, child_id, kind, len(nbytes))
                buf[off + 10 : off + 10 + len(nbytes)] = nbytes
                return
        raise RuntimeError(f"DeltaFS directory full while adding {name}")

    def _create_dir(self, parent_dir_blk: int, name: str, kind: int = 2, attr: bytes = b"") -> int:
        ino_id = self.next_inode
        self.next_inode += 1
        dir_blk = self._alloc_blocks(1)
        self._write_inode(ino_id, kind, 0o755, 0, dir_blk, 1, attr)
        if parent_dir_blk != 0 and name:
            self._add_dirent(parent_dir_blk, ino_id, kind, name)
        return dir_blk

    def mkdir(self, parent_dir_blk: int, name: str, is_hsapp: bool = False) -> int:
        kind = 3 if is_hsapp else 2
        attr = b"application/x-hsapp" if is_hsapp else b"directory"
        return self._create_dir(parent_dir_blk, name, kind=kind, attr=attr)

    def add_file(self, dir_blk: int, name: str, data: bytes, attr: bytes = b"") -> None:
        ino_id = self.next_inode
        self.next_inode += 1
        blocks_needed = max(1, math.ceil(len(data) / DELTA_BLOCK))
        start_blk = self._alloc_blocks(blocks_needed)
        for i in range(blocks_needed):
            chunk = data[i * DELTA_BLOCK : (i + 1) * DELTA_BLOCK]
            self.blocks[start_blk + i][: len(chunk)] = chunk
        self._write_inode(ino_id, 1, 0o644, len(data), start_blk, blocks_needed, attr)
        self._add_dirent(dir_blk, ino_id, 1, name)

    def write_to_image(self, img: bytearray) -> None:
        base = self.start_lba * SECTOR_SIZE
        free_blocks = self.total_blocks - self.next_block

        sb = bytearray(DELTA_BLOCK)
        sb[0:8] = b"DELTAFS1"
        struct.pack_into(
            "<IIQQQQQQQQQQII",
            sb,
            8,
            1,
            DELTA_BLOCK,
            self.total_blocks,
            free_blocks,
            self.journal_start,
            self.journal_blocks,
            self.bitmap_start,
            self.bitmap_blocks,
            self.inode_start,
            self.inode_blocks,
            self.data_start,
            self.inode_count,
            0,
            self.role,
        )
        lbytes = self.label.encode("utf-8")[:32]
        sb[112 : 112 + len(lbytes)] = lbytes
        crc = zlib.crc32(sb[0:144]) & 0xFFFFFFFF
        struct.pack_into("<I", sb, 144, crc)

        img[base : base + DELTA_BLOCK] = sb
        bm_off = base + self.bitmap_start * DELTA_BLOCK
        img[bm_off : bm_off + len(self.bitmap)] = self.bitmap
        ino_off = base + self.inode_start * DELTA_BLOCK
        img[ino_off : ino_off + len(self.inode_table)] = self.inode_table

        for blk_idx, buf in self.blocks.items():
            off = base + blk_idx * DELTA_BLOCK
            img[off : off + DELTA_BLOCK] = buf


def _add_hsapp(
    sysfs: DeltaFsBuilder,
    apps_dir: int,
    bundle_name: str,
    exec_bytes: bytes,
    info_name: str,
    info_bytes: bytes,
) -> None:
    bdir = sysfs.mkdir(apps_dir, bundle_name, is_hsapp=True)
    assets = sysfs.mkdir(bdir, "Assets")
    sysfs.add_file(assets, info_name, info_bytes, b"text/plain")
    tag = bundle_name.encode("ascii")[:10]
    sysfs.add_file(bdir, "icon.png", b"\x89PNG\r\n\x1a\n" + tag, b"image/png")
    sysfs.add_file(bdir, "exec", exec_bytes, b"application/x-elf")


def _add_dkex(
    sysfs: DeltaFsBuilder,
    ext_dir: int,
    bundle_name: str,
    bundle_id: str,
    service_name: str,
    priority: int,
    exec_bytes: bytes | None,
) -> None:
    bdir = sysfs.mkdir(ext_dir, bundle_name, is_hsapp=True)
    plist = (
        f'<?xml version="1.0" encoding="UTF-8"?>\n'
        f'<plist version="1.0"><dict>\n'
        f"  <key>CFBundleIdentifier</key><string>{bundle_id}</string>\n"
        f"  <key>DeltaService</key><string>{service_name}</string>\n"
        f"  <key>DeltaPriority</key><string>{priority}</string>\n"
        f"</dict></plist>\n"
    ).encode("utf-8")
    sysfs.add_file(bdir, "Info.plist", plist, b"application/xml")
    if exec_bytes is not None:
        sysfs.add_file(bdir, "exec", exec_bytes, b"application/x-elf")


def build_image(
    userland_dir: Path,
    out_path: Path,
    *,
    disk_mb: int = 64,
    boot_mb: int = 16,
    system_mb: int = 24,
    user_mb: int | None = None,
    boot_files: dict[str, bytes] | None = None,
) -> int:
    """Builds a 3-partition Hollow Star OS+ MBR image and writes it to `out_path`."""
    required = {
        "shell": userland_dir / "shell",
        "compositor": userland_dir / "compositor",
        "setup": userland_dir / "setup",
        "launcher": userland_dir / "launcher",
        "terminal": userland_dir / "terminal",
        "hollow-star": userland_dir / "hollow-star",
        "hello": userland_dir / "hello",
        "dkex-diag": userland_dir / "dkex-diag",
        "libdelta_kit": userland_dir / "libdelta_kit.dylib",
    }
    for name, p in required.items():
        if not p.exists():
            print(f"[mk_sdcard] Missing binary: {p} ({name})", file=sys.stderr)
            return 1

    bins = {k: p.read_bytes() for k, p in required.items()}

    disk_sectors = disk_mb * 2048
    boot_sectors = boot_mb * 2048
    system_start = BOOT_START + boot_sectors
    system_sectors = system_mb * 2048
    user_start = system_start + system_sectors
    if user_mb is None:
        # Leave ~3 MiB free tail on 64 MiB default test card for /user partition auto-expand.
        user_sectors = max(16 * 2048, disk_sectors - user_start - 3 * 2048)
    else:
        user_sectors = user_mb * 2048
    if user_start + user_sectors > disk_sectors:
        print(
            f"[mk_sdcard] Image size {disk_mb} MiB too small for partitions ({(user_start + user_sectors) // 2048} MiB needed)",
            file=sys.stderr,
        )
        return 1

    out_path.parent.mkdir(parents=True, exist_ok=True)
    img = bytearray(disk_sectors * SECTOR_SIZE)

    # MBR: Part 1 (/boot FAT32 0x0C active), Part 2 (/system DeltaFS 0xDF), Part 3 (/user DeltaFS 0xDF)
    img[510:512] = b"\x55\xAA"
    img[446] = 0x80
    img[446 + 1 : 446 + 4] = _lba_to_chs(BOOT_START)
    img[446 + 4] = 0x0C
    img[446 + 5 : 446 + 8] = _lba_to_chs(BOOT_START + boot_sectors - 1)
    struct.pack_into("<II", img, 446 + 8, BOOT_START, boot_sectors)
    img[462 + 1 : 462 + 4] = _lba_to_chs(system_start)
    img[462 + 4] = 0xDF
    img[462 + 5 : 462 + 8] = _lba_to_chs(system_start + system_sectors - 1)
    struct.pack_into("<II", img, 462 + 8, system_start, system_sectors)
    img[478 + 1 : 478 + 4] = _lba_to_chs(user_start)
    img[478 + 4] = 0xDF
    img[478 + 5 : 478 + 8] = _lba_to_chs(user_start + user_sectors - 1)
    struct.pack_into("<II", img, 478 + 8, user_start, user_sectors)

    # 1. /boot (FAT32 / VFAT)
    boot = Fat32Builder(BOOT_START, boot_sectors)
    boot.add_file(
        "README.TXT",
        b"Hollow Star OS+ Boot Partition (FAT32). Place config.txt, firmware, and kernel8.img here.\n",
    )
    if boot_files:
        for fname, data in boot_files.items():
            boot.add_file(fname, data)
    else:
        boot.add_file("CONFIG.TXT", b"arm_64bit=1\nenable_uart=1\nhdmi_force_hotplug=1\n")
    boot.write_to_image(img)

    # 2. /system (DeltaFS role=1)
    sysfs = DeltaFsBuilder(system_start, system_sectors, role=1, label="SYSTEM")
    sys_bin = sysfs.mkdir(sysfs.root_inode, "bin")
    sysfs.add_file(sys_bin, "shell.elf", bins["shell"], b"application/x-elf")
    sysfs.add_file(sys_bin, "compositor.elf", bins["compositor"], b"application/x-elf")

    sys_lib = sysfs.mkdir(sysfs.root_inode, "lib")
    sysfs.add_file(sys_lib, "libdelta_kit.dylib", bins["libdelta_kit"], b"application/x-dylib")

    sys_library = sysfs.mkdir(sysfs.root_inode, "Library")
    sys_ext = sysfs.mkdir(sys_library, "Extensions")
    _add_dkex(
        sysfs,
        sys_ext,
        "DeltaDiag.dkex",
        "com.hollowstar.dkex.diag",
        "svc.diag",
        10,
        bins["dkex-diag"],
    )
    for bname, bid, svc, prio, bin_file in (
        ("DeltaUart.dkex", "com.hollowstar.dkex.uart", "svc.uart", 20, "drv-uart"),
        ("DeltaSD.dkex", "com.hollowstar.dkex.sd", "svc.sd", 20, "drv-sd"),
        ("DeltaVFS.dkex", "com.hollowstar.dkex.vfs", "svc.vfs", 20, "vfsd"),
        ("DeltaFB.dkex", "com.hollowstar.dkex.fb", "svc.fb", 15, "drv-fb"),
        ("DeltaAudio.dkex", "com.hollowstar.dkex.audio", "svc.audio", 110, "drv-audio"),
        ("DeltaKbd.dkex", "com.hollowstar.dkex.kbd", "svc.kbd", 20, "drv-kbd"),
    ):
        bpath = userland_dir / bin_file
        _add_dkex(
            sysfs,
            sys_ext,
            bname,
            bid,
            svc,
            prio,
            bpath.read_bytes() if bpath.exists() else None,
        )

    sys_apps = sysfs.mkdir(sysfs.root_inode, "apps")
    _add_hsapp(
        sysfs,
        sys_apps,
        "Hello.hsapp",
        bins["hello"],
        "info.txt",
        b"Hello.hsapp v0.6.0-rc.2 (.hsapp bundle verified on System DeltaFS!)",
    )
    _add_hsapp(
        sysfs,
        sys_apps,
        "Setup.hsapp",
        bins["setup"],
        "info.txt",
        b"Hollow Star OS+ 1st Setup Wizard v0.6.0-rc.2",
    )
    _add_hsapp(
        sysfs,
        sys_apps,
        "Launcher.hsapp",
        bins["launcher"],
        "info.txt",
        b"Hollow Star OS+ App Grid Launcher v0.6.0-rc.2",
    )
    _add_hsapp(
        sysfs,
        sys_apps,
        "Terminal.hsapp",
        bins["terminal"],
        "info.txt",
        b"Hollow Star OS+ Terminal Emulator v0.6.0-rc.2",
    )
    _add_hsapp(
        sysfs,
        sys_apps,
        "Apps.hsapp",
        bins["hollow-star"],
        "banner.txt",
        b"Hollow Star OS+ v0.6.0-rc.2 / Core Applications Suite",
    )
    sysfs.write_to_image(img)

    # 3. /user (DeltaFS role=2)
    usrfs = DeltaFsBuilder(user_start, user_sectors, role=2, label="USER")
    usrfs.mkdir(usrfs.root_inode, "config")
    notes_dir = usrfs.mkdir(usrfs.root_inode, "notes")
    media_dir = usrfs.mkdir(usrfs.root_inode, "media")
    usrfs.mkdir(usrfs.root_inode, "apps")
    usrfs.add_file(
        notes_dir,
        "welcome.md",
        b"# Hollow Star OS+ User Partition (DeltaFS)\n\n"
        b"- Boot (`/boot`): FAT32 / VFAT\n"
        b"- System (`/system`): DeltaFS (WAL journal)\n"
        b"- User (`/user`): DeltaFS (WAL journal + auto-expand)\n"
        b"- Desktop: Modular Compositor + Setup + Launcher + Terminal\n",
        b"text/markdown",
    )
    usrfs.add_file(
        media_dir,
        "chime.trk",
        b"DELTA_TRK 440,523,659,880\n",
        b"audio/x-delta-track",
    )
    usrfs.write_to_image(img)

    out_path.write_bytes(img)
    print(
        f"[mk_sdcard] Built {out_path} ({disk_mb} MiB: /boot {boot_mb}M FAT32 + /system {system_mb}M DeltaFS + /user {user_sectors // 2048}M DeltaFS)"
    )
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description="Build Hollow Star OS+ QEMU SD card image.")
    parser.add_argument("userland_dir", type=Path, help="Path to target/userland/<triple>/release")
    parser.add_argument("out_img", type=Path, help="Output image path (e.g. target/sdcard.img)")
    args = parser.parse_args()
    return build_image(args.userland_dir, args.out_img)


if __name__ == "__main__":
    raise SystemExit(main())

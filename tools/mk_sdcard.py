#!/usr/bin/env python3
"""Builds a 64 MiB MBR SD card image with 3 separated partitions for Hollow Star OS+ M6:
  1. Partition 1 (`0x0C` FAT32, 16 MiB) -> `/boot`   (firmware boot files & README)
  2. Partition 2 (`0xDF` DeltaFS, 24 MiB) -> `/system` (shell.elf + .hsapp bundles)
  3. Partition 3 (`0xDF` DeltaFS, 20 MiB) -> `/user`   (notes, media, user apps; 3.9 MiB free tail for expansion test)
"""

from __future__ import annotations

import math
import struct
import sys
import zlib
from pathlib import Path

SECTOR_SIZE = 512
DISK_SECTORS = 131072  # 64 MiB
DELTA_BLOCK = 4096
SECTORS_PER_BLOCK = DELTA_BLOCK // SECTOR_SIZE

BOOT_START = 2048
BOOT_SECTORS = 32768  # 16 MiB

SYSTEM_START = BOOT_START + BOOT_SECTORS
SYSTEM_SECTORS = 49152  # 24 MiB

USER_START = SYSTEM_START + SYSTEM_SECTORS
USER_SECTORS = 40960  # 20 MiB (leaves ~3 MiB free tail at end of 64 MiB card for auto-expand test)

FAT32_RESERVED = 32
FAT32_NUM_FATS = 1
FAT32_SPC = 8
FAT32_SPF = 32
FAT32_ROOT_CLUSTER = 2


class Fat32Builder:
    def __init__(self) -> None:
        self.cluster_size = FAT32_SPC * SECTOR_SIZE
        self.fat = [0x0FFFFFF8, 0x0FFFFFFF, 0x0FFFFFFF]
        self.clusters: dict[int, bytearray] = {2: bytearray(self.cluster_size)}

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
    def encode_83(name: str) -> bytes:
        upper = name.upper()
        base, ext = upper.rsplit(".", 1) if "." in upper else (upper, "")
        return base[:8].ljust(8, " ").encode("ascii") + ext[:3].ljust(3, " ").encode("ascii")

    def add_file(self, name: str, payload: bytes) -> None:
        cid = self.alloc_chain(payload)
        buf = self.clusters[FAT32_ROOT_CLUSTER]
        for off in range(0, len(buf), 32):
            if buf[off] == 0:
                buf[off : off + 11] = self.encode_83(name)
                buf[off + 11] = 0x20
                struct.pack_into("<H", buf, off + 20, (cid >> 16) & 0xFFFF)
                struct.pack_into("<H", buf, off + 26, cid & 0xFFFF)
                struct.pack_into("<I", buf, off + 28, len(payload))
                return
        raise RuntimeError("FAT32 root directory full")

    def write_to_image(self, img: bytearray) -> None:
        p_off = BOOT_START * SECTOR_SIZE
        img[p_off : p_off + 3] = b"\xEB\x58\x90"
        img[p_off + 3 : p_off + 11] = b"DELTAOS "
        struct.pack_into("<H", img, p_off + 11, SECTOR_SIZE)
        img[p_off + 13] = FAT32_SPC
        struct.pack_into("<H", img, p_off + 14, FAT32_RESERVED)
        img[p_off + 16] = FAT32_NUM_FATS
        struct.pack_into("<I", img, p_off + 32, BOOT_SECTORS)
        struct.pack_into("<I", img, p_off + 36, FAT32_SPF)
        struct.pack_into("<I", img, p_off + 44, FAT32_ROOT_CLUSTER)
        struct.pack_into("<H", img, p_off + 48, 1)
        struct.pack_into("<H", img, p_off + 50, 6)
        img[p_off + 82 : p_off + 90] = b"FAT32   "
        img[p_off + 510 : p_off + 512] = b"\x55\xAA"

        fat_off = p_off + FAT32_RESERVED * SECTOR_SIZE
        for idx, val in enumerate(self.fat):
            struct.pack_into("<I", img, fat_off + idx * 4, val)

        data_off = fat_off + FAT32_NUM_FATS * FAT32_SPF * SECTOR_SIZE
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
        self.bitmap_blocks = 8
        self.inode_start = 25
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


def main() -> int:
    if len(sys.argv) != 5:
        print(
            f"Usage: {sys.argv[0]} <shell_elf> <hello_elf> <hollow_star_elf> <out_img>",
            file=sys.stderr,
        )
        return 1

    shell_bytes = Path(sys.argv[1]).read_bytes()
    hello_bytes = Path(sys.argv[2]).read_bytes()
    hollow_bytes = Path(sys.argv[3]).read_bytes()
    out_path = Path(sys.argv[4])
    out_path.parent.mkdir(parents=True, exist_ok=True)

    img = bytearray(DISK_SECTORS * SECTOR_SIZE)

    # MBR: Partition 1 (Boot FAT32), Partition 2 (System DeltaFS 0xDF), Partition 3 (User DeltaFS 0xDF)
    img[510:512] = b"\x55\xAA"
    img[446 + 4] = 0x0C
    struct.pack_into("<II", img, 446 + 8, BOOT_START, BOOT_SECTORS)
    img[462 + 4] = 0xDF
    struct.pack_into("<II", img, 462 + 8, SYSTEM_START, SYSTEM_SECTORS)
    img[478 + 4] = 0xDF
    struct.pack_into("<II", img, 478 + 8, USER_START, USER_SECTORS)

    # 1. /boot (FAT32)
    boot = Fat32Builder()
    boot.add_file(
        "README.TXT",
        b"Hollow Star OS+ Boot Partition (FAT32). Place config.txt & kernel8.img here.\n",
    )
    boot.add_file("CONFIG.TXT", b"arm_64bit=1\nenable_uart=1\nhdmi_force_hotplug=1\n")
    boot.write_to_image(img)

    # 2. /system (DeltaFS role=1)
    sysfs = DeltaFsBuilder(SYSTEM_START, SYSTEM_SECTORS, role=1, label="SYSTEM")
    sys_bin = sysfs.mkdir(sysfs.root_inode, "bin")
    sysfs.add_file(sys_bin, "shell.elf", shell_bytes, b"application/x-elf")

    sys_apps = sysfs.mkdir(sysfs.root_inode, "apps")
    hello_dir = sysfs.mkdir(sys_apps, "Hello.hsapp", is_hsapp=True)
    hello_assets = sysfs.mkdir(hello_dir, "Assets")
    sysfs.add_file(
        hello_assets,
        "info.txt",
        b"Hello.hsapp v0.6.0 (.hsapp bundle verified on System DeltaFS!)",
    )
    sysfs.add_file(hello_dir, "icon.png", b"\x89PNG\r\n\x1a\nHELLO_ICON")
    sysfs.add_file(hello_dir, "exec", hello_bytes, b"application/x-elf")

    hs_dir = sysfs.mkdir(sys_apps, "HollowStar.hsapp", is_hsapp=True)
    hs_assets = sysfs.mkdir(hs_dir, "Assets")
    sysfs.add_file(
        hs_assets,
        "banner.txt",
        b"Hollow Star OS+ v0.6.0 / Delta Kernel Desktop & 1st Setup",
    )
    sysfs.add_file(hs_dir, "icon.png", b"\x89PNG\r\n\x1a\nHOLLOWSTAR")
    sysfs.add_file(hs_dir, "exec", hollow_bytes, b"application/x-elf")
    sysfs.write_to_image(img)

    # 3. /user (DeltaFS role=2)
    usrfs = DeltaFsBuilder(USER_START, USER_SECTORS, role=2, label="USER")
    usrfs.mkdir(usrfs.root_inode, "config")
    notes_dir = usrfs.mkdir(usrfs.root_inode, "notes")
    media_dir = usrfs.mkdir(usrfs.root_inode, "media")
    usrfs.mkdir(usrfs.root_inode, "apps")
    usrfs.add_file(
        notes_dir,
        "welcome.md",
        b"# Hollow Star OS+ User Partition (DeltaFS)\n\n"
        b"- Boot (`/boot`): FAT32\n"
        b"- System (`/system`): DeltaFS (WAL journal)\n"
        b"- User (`/user`): DeltaFS (WAL journal + auto-expand)\n",
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
        f"[mk_sdcard] Built {out_path} ({len(img) // (1024 * 1024)} MiB: /boot FAT32 + /system DeltaFS + /user DeltaFS)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

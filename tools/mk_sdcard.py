#!/usr/bin/env python3
"""Builds a test 64 MiB MBR SD card image (FAT32 Partition 1 + ext2 Partition 2)
for QEMU `-M raspi3b` verification of Hollow Star OS+ Milestone M5."""

from __future__ import annotations

import math
import os
import struct
import sys
from pathlib import Path

SECTOR_SIZE = 512
DISK_SECTORS = 131072  # 64 MiB

FAT32_START = 2048
FAT32_SECTORS = 65536  # 32 MiB
FAT32_RESERVED = 32
FAT32_NUM_FATS = 1
FAT32_SPC = 8  # 4 KiB clusters
FAT32_SPF = 64
FAT32_ROOT_CLUSTER = 2

EXT2_START = FAT32_START + FAT32_SECTORS
EXT2_SECTORS = 32768  # 16 MiB


def pad_cluster(data: bytes, cluster_size: int) -> bytes:
    rem = len(data) % cluster_size
    return data if rem == 0 else data + b"\x00" * (cluster_size - rem)


class Fat32Builder:
    def __init__(self) -> None:
        self.cluster_size = FAT32_SPC * SECTOR_SIZE
        self.fat = [0x0FFFFFF8, 0x0FFFFFFF, 0x0FFFFFFF]  # 0, 1, 2(root)
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

    def alloc_dir_cluster(self, parent_cluster: int) -> int:
        cid = len(self.fat)
        self.fat.append(0x0FFFFFFF)
        buf = bytearray(self.cluster_size)
        self.add_raw_entry(buf, b".          ", 0x10, cid, 0)
        parent_val = 0 if parent_cluster == FAT32_ROOT_CLUSTER else parent_cluster
        self.add_raw_entry(buf, b"..         ", 0x10, parent_val, 0)
        self.clusters[cid] = buf
        return cid

    @staticmethod
    def short_checksum(short_83: bytes) -> int:
        s = 0
        for b in short_83:
            s = (((s & 1) << 7) + (s >> 1) + b) & 0xFF
        return s

    @staticmethod
    def encode_83(name: str, seq: int = 1) -> bytes:
        upper = name.upper()
        if "." in upper:
            base, ext = upper.rsplit(".", 1)
        else:
            base, ext = upper, ""
        clean_base = "".join(c for c in base if c.isalnum() or c in "_-")
        clean_ext = "".join(c for c in ext if c.isalnum() or c in "_-")[:3]
        if len(clean_base) > 8:
            clean_base = f"{clean_base[:6]}~{seq}"
        return clean_base.ljust(8, " ").encode("ascii") + clean_ext.ljust(3, " ").encode("ascii")

    @staticmethod
    def add_raw_entry(dir_buf: bytearray, short_83: bytes, attr: int, cluster: int, size: int) -> None:
        for off in range(0, len(dir_buf), 32):
            if dir_buf[off] == 0:
                dir_buf[off : off + 11] = short_83
                dir_buf[off + 11] = attr
                struct.pack_into("<H", dir_buf, off + 20, (cluster >> 16) & 0xFFFF)
                struct.pack_into("<H", dir_buf, off + 26, cluster & 0xFFFF)
                struct.pack_into("<I", dir_buf, off + 28, size)
                return
        raise RuntimeError("Directory cluster full")

    def add_entry(self, dir_cluster: int, name: str, attr: int, cluster: int, size: int) -> None:
        buf = self.clusters[dir_cluster]
        short_83 = self.encode_83(name)
        needs_lfn = name != name.upper() or len(name.split(".")[0]) > 8 or name.count(".") > 1

        if needs_lfn:
            csum = self.short_checksum(short_83)
            utf16 = [ord(c) for c in name] + [0]
            while len(utf16) % 13 != 0:
                utf16.append(0xFFFF)
            num_lfn = len(utf16) // 13
            for seq in range(num_lfn, 0, -1):
                chunk = utf16[(seq - 1) * 13 : seq * 13]
                lfn = bytearray(32)
                lfn[0] = seq | (0x40 if seq == num_lfn else 0)
                lfn[11] = 0x0F
                lfn[13] = csum
                offs = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30]
                for idx, o in enumerate(offs):
                    struct.pack_into("<H", lfn, o, chunk[idx])
                for off in range(0, len(buf), 32):
                    if buf[off] == 0:
                        buf[off : off + 32] = lfn
                        break

        self.add_raw_entry(buf, short_83, attr, cluster, size)

    def write_to_image(self, img: bytearray) -> None:
        p_off = FAT32_START * SECTOR_SIZE
        # BPB
        img[p_off : p_off + 3] = b"\xEB\x58\x90"
        img[p_off + 3 : p_off + 11] = b"DELTAOS "
        struct.pack_into("<H", img, p_off + 11, SECTOR_SIZE)
        img[p_off + 13] = FAT32_SPC
        struct.pack_into("<H", img, p_off + 14, FAT32_RESERVED)
        img[p_off + 16] = FAT32_NUM_FATS
        struct.pack_into("<I", img, p_off + 32, FAT32_SECTORS)
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


def write_ext2_partition(img: bytearray) -> None:
    p_off = EXT2_START * SECTOR_SIZE
    block_size = 1024

    # Superblock at offset +1024
    sb = p_off + 1024
    struct.pack_into("<I", img, sb + 0, 32)  # s_inodes_count
    struct.pack_into("<I", img, sb + 4, 4096)  # s_blocks_count
    struct.pack_into("<I", img, sb + 20, 1)  # s_first_data_block
    struct.pack_into("<I", img, sb + 24, 0)  # s_log_block_size (1024)
    struct.pack_into("<I", img, sb + 32, 4096)  # s_blocks_per_group
    struct.pack_into("<I", img, sb + 40, 32)  # s_inodes_per_group
    struct.pack_into("<H", img, sb + 56, 0xEF53)  # EXT2_SUPER_MAGIC
    struct.pack_into("<I", img, sb + 76, 1)  # s_rev_level = 1
    struct.pack_into("<H", img, sb + 88, 128)  # s_inode_size

    # Block Group Descriptor at block 2
    bgd = p_off + 2 * block_size
    struct.pack_into("<I", img, bgd + 8, 3)  # bg_inode_table = block 3

    # Inode table at block 3: inode 2 (root) & inode 3 (welcome.txt)
    ino_tbl = p_off + 3 * block_size
    root_ino = ino_tbl + 128
    struct.pack_into("<H", img, root_ino + 0, 0x41ED)  # S_IFDIR
    struct.pack_into("<I", img, root_ino + 4, block_size)
    struct.pack_into("<I", img, root_ino + 40, 10)  # block 10

    note_bytes = b"Mounted ext2 Partition 2 on Hollow Star OS+!\n"
    file_ino = ino_tbl + 256
    struct.pack_into("<H", img, file_ino + 0, 0x81A4)  # S_IFREG
    struct.pack_into("<I", img, file_ino + 4, len(note_bytes))
    struct.pack_into("<I", img, file_ino + 40, 11)  # block 11

    rdir = p_off + 10 * block_size
    struct.pack_into("<I", img, rdir + 0, 3)  # inode 3
    struct.pack_into("<H", img, rdir + 4, block_size)
    img[rdir + 6] = len(b"welcome.txt")
    img[rdir + 7] = 1  # regular file
    rdir_name = b"welcome.txt"
    img[rdir + 8 : rdir + 8 + len(rdir_name)] = rdir_name

    fdata = p_off + 11 * block_size
    img[fdata : fdata + len(note_bytes)] = note_bytes


def main() -> int:
    if len(sys.argv) != 4:
        print(f"Usage: {sys.argv[0]} <shell_elf> <hello_elf> <out_img>", file=sys.stderr)
        return 1

    shell_bytes = Path(sys.argv[1]).read_bytes()
    hello_bytes = Path(sys.argv[2]).read_bytes()
    out_path = Path(sys.argv[3])
    out_path.parent.mkdir(parents=True, exist_ok=True)

    img = bytearray(DISK_SECTORS * SECTOR_SIZE)

    # MBR Partition 1 (FAT32 LBA 0x0C) & Partition 2 (Linux ext2 0x83)
    img[510 : 512] = b"\x55\xAA"
    img[446 + 4] = 0x0C
    struct.pack_into("<II", img, 446 + 8, FAT32_START, FAT32_SECTORS)
    img[462 + 4] = 0x83
    struct.pack_into("<II", img, 462 + 8, EXT2_START, EXT2_SECTORS)

    fat = Fat32Builder()
    bin_dir = fat.alloc_dir_cluster(FAT32_ROOT_CLUSTER)
    fat.add_entry(FAT32_ROOT_CLUSTER, "BIN", 0x10, bin_dir, 0)
    shell_c = fat.alloc_chain(shell_bytes)
    fat.add_entry(bin_dir, "shell.elf", 0x20, shell_c, len(shell_bytes))

    readme = b"Hollow Star OS+ (Delta Kernel) SD Card Root (FAT32)\n"
    fat.add_entry(FAT32_ROOT_CLUSTER, "README.TXT", 0x20, fat.alloc_chain(readme), len(readme))

    # Mac-style application bundle: Hello.hsapp/{Assets/, exec, icon.png}
    hsapp_dir = fat.alloc_dir_cluster(FAT32_ROOT_CLUSTER)
    fat.add_entry(FAT32_ROOT_CLUSTER, "Hello.hsapp", 0x10, hsapp_dir, 0)

    assets_dir = fat.alloc_dir_cluster(hsapp_dir)
    fat.add_entry(hsapp_dir, "Assets", 0x10, assets_dir, 0)

    info_txt = b"Hello.hsapp v0.5.0 (.hsapp bundle verified on FAT32!)"
    fat.add_entry(assets_dir, "info.txt", 0x20, fat.alloc_chain(info_txt), len(info_txt))

    fake_png = b"\x89PNG\r\n\x1a\nDELTA_ICON"
    fat.add_entry(hsapp_dir, "icon.png", 0x20, fat.alloc_chain(fake_png), len(fake_png))

    hello_c = fat.alloc_chain(hello_bytes)
    fat.add_entry(hsapp_dir, "exec", 0x20, hello_c, len(hello_bytes))

    fat.write_to_image(img)
    write_ext2_partition(img)

    out_path.write_bytes(img)
    print(f"[mk_sdcard] Built {out_path} ({len(img) // (1024 * 1024)} MiB, FAT32 + ext2)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

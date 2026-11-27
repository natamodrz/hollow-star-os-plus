#![cfg_attr(not(test), no_std)]

//! Hardware-independent VFS, MBR, FAT32 (with VFAT LFN + write), and ext2 implementation.

pub mod block;
pub mod ext2;
pub mod fat32;
pub mod mbr;

use block::{BlockDevice, FsError, PartitionSlice};
use ext2::Ext2Volume;
use fat32::Fat32Volume;
use mbr::MbrPartition;

/// Unified Virtual Filesystem router mounting FAT32 (`/`), optional ext2 (`/ext2`),
/// `/dev`, and `/proc`.
pub struct VfsRouter {
    fat_part: Option<(MbrPartition, Fat32Volume)>,
    ext2_part: Option<(MbrPartition, Ext2Volume)>,
}

impl VfsRouter {
    /// Inspects MBR on `dev` and mounts FAT32 (`0x0B`/`0x0C`) and ext2 (`0x83`) partitions.
    pub fn mount<B: BlockDevice>(dev: &mut B) -> Result<Self, FsError> {
        let parts = mbr::read_mbr(dev)?;
        let mut fat_part = None;
        let mut ext2_part = None;

        for p in parts.into_iter().flatten() {
            if p.is_fat32() && fat_part.is_none() {
                let mut slice = PartitionSlice::new(dev, p.start_lba, p.sector_count);
                if let Ok(vol) = Fat32Volume::mount(&mut slice) {
                    fat_part = Some((p, vol));
                }
            } else if p.is_linux_ext() && ext2_part.is_none() {
                let mut slice = PartitionSlice::new(dev, p.start_lba, p.sector_count);
                if let Ok(vol) = Ext2Volume::mount(&mut slice) {
                    ext2_part = Some((p, vol));
                }
            }
        }

        if fat_part.is_none() {
            return Err(FsError::InvalidFs);
        }

        Ok(Self {
            fat_part,
            ext2_part,
        })
    }

    pub fn has_ext2(&self) -> bool {
        self.ext2_part.is_some()
    }

    pub fn stat<B: BlockDevice>(&self, dev: &mut B, path: &str) -> Result<(u32, bool), FsError> {
        let norm = normalize(path);
        if norm == "/dev" || norm == "/proc" {
            return Ok((0, true));
        }
        if let Some(rel) = norm.strip_prefix("/proc/") {
            return match proc_content(rel, self.has_ext2()) {
                Some(bytes) => Ok((bytes.len() as u32, false)),
                None => Err(FsError::NotFound),
            };
        }
        if norm == "/ext2" || norm.starts_with("/ext2/") {
            let (part, vol) = self.ext2_part.ok_or(FsError::NotFound)?;
            let sub = norm.strip_prefix("/ext2").unwrap_or("");
            let mut slice = PartitionSlice::new(dev, part.start_lba, part.sector_count);
            return vol.stat(&mut slice, sub);
        }
        let (part, vol) = self.fat_part.ok_or(FsError::InvalidFs)?;
        let mut slice = PartitionSlice::new(dev, part.start_lba, part.sector_count);
        vol.stat(&mut slice, norm)
    }

    pub fn read_dir<B: BlockDevice>(
        &self,
        dev: &mut B,
        path: &str,
        out: &mut [u8],
    ) -> Result<usize, FsError> {
        let norm = normalize(path);
        if norm == "/dev" {
            return Ok(copy_str(out, "[DEV]  uart0\n[DEV]  sd0\n"));
        }
        if norm == "/proc" {
            return Ok(copy_str(out, "[PROC] version\n[PROC] mounts\n"));
        }
        if norm == "/ext2" || norm.starts_with("/ext2/") {
            let (part, vol) = self.ext2_part.ok_or(FsError::NotFound)?;
            let sub = norm.strip_prefix("/ext2").unwrap_or("");
            let mut slice = PartitionSlice::new(dev, part.start_lba, part.sector_count);
            return vol.read_dir(&mut slice, sub, out);
        }

        let (part, vol) = self.fat_part.ok_or(FsError::InvalidFs)?;
        let mut slice = PartitionSlice::new(dev, part.start_lba, part.sector_count);
        let mut n = vol.read_dir(&mut slice, norm, out)?;
        if norm == "/" {
            if self.has_ext2() {
                n += copy_str(&mut out[n..], "[DIR]  ext2/\n");
            }
            n += copy_str(&mut out[n..], "[DIR]  dev/\n[DIR]  proc/\n");
        }
        Ok(n)
    }

    pub fn read_file<B: BlockDevice>(
        &self,
        dev: &mut B,
        path: &str,
        out: &mut [u8],
    ) -> Result<usize, FsError> {
        let norm = normalize(path);
        if let Some(rel) = norm.strip_prefix("/proc/") {
            let bytes = proc_content(rel, self.has_ext2()).ok_or(FsError::NotFound)?;
            let n = bytes.len().min(out.len());
            out[..n].copy_from_slice(&bytes[..n]);
            return Ok(n);
        }
        if let Some(sub) = norm.strip_prefix("/ext2/") {
            let (part, vol) = self.ext2_part.ok_or(FsError::NotFound)?;
            let mut slice = PartitionSlice::new(dev, part.start_lba, part.sector_count);
            return vol.read_file(&mut slice, sub, out);
        }
        let (part, vol) = self.fat_part.ok_or(FsError::InvalidFs)?;
        let mut slice = PartitionSlice::new(dev, part.start_lba, part.sector_count);
        vol.read_file(&mut slice, norm, out)
    }

    pub fn write_file<B: BlockDevice>(
        &self,
        dev: &mut B,
        path: &str,
        data: &[u8],
    ) -> Result<usize, FsError> {
        let norm = normalize(path);
        if norm.starts_with("/proc") || norm.starts_with("/dev") || norm.starts_with("/ext2") {
            return Err(FsError::ReadOnly);
        }
        let (part, vol) = self.fat_part.ok_or(FsError::InvalidFs)?;
        let mut slice = PartitionSlice::new(dev, part.start_lba, part.sector_count);
        vol.write_file(&mut slice, norm, data)
    }
}

fn normalize(path: &str) -> &str {
    let trimmed = path.trim();
    if trimmed.is_empty() { "/" } else { trimmed }
}

fn copy_str(out: &mut [u8], s: &str) -> usize {
    let n = s.len().min(out.len());
    out[..n].copy_from_slice(&s.as_bytes()[..n]);
    n
}

fn proc_content(name: &str, has_ext2: bool) -> Option<&'static [u8]> {
    match name {
        "version" => Some(b"Hollow Star OS+ v0.5.0 (Delta Kernel / AArch64)\n"),
        "mounts" => {
            if has_ext2 {
                Some(b"/ -> fat32 (rw)\n/ext2 -> ext2 (ro)\n/dev -> devfs\n/proc -> procfs\n")
            } else {
                Some(b"/ -> fat32 (rw)\n/dev -> devfs\n/proc -> procfs\n")
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{BlockDevice, FsError, VfsRouter};
    use crate::block::SECTOR_SIZE;

    struct RamDisk {
        data: Vec<u8>,
    }

    impl RamDisk {
        fn new(sectors: usize) -> Self {
            Self {
                data: vec![0u8; sectors * SECTOR_SIZE],
            }
        }
    }

    impl BlockDevice for RamDisk {
        fn read_sectors(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
            let start = (lba as usize) * SECTOR_SIZE;
            let end = start + buf.len();
            if end > self.data.len() {
                return Err(FsError::Io);
            }
            buf.copy_from_slice(&self.data[start..end]);
            Ok(())
        }

        fn write_sectors(&mut self, lba: u64, buf: &[u8]) -> Result<(), FsError> {
            let start = (lba as usize) * SECTOR_SIZE;
            let end = start + buf.len();
            if end > self.data.len() {
                return Err(FsError::Io);
            }
            self.data[start..end].copy_from_slice(buf);
            Ok(())
        }
    }

    fn build_test_disk() -> RamDisk {
        let mut disk = RamDisk::new(4096);
        let d = &mut disk.data;

        // MBR Partition 1: FAT32 at LBA 16, 2048 sectors
        d[510] = 0x55;
        d[511] = 0xAA;
        d[446 + 4] = 0x0C;
        d[446 + 8..446 + 12].copy_from_slice(&16u32.to_le_bytes());
        d[446 + 12..446 + 16].copy_from_slice(&2048u32.to_le_bytes());

        // Partition 2: ext2 at LBA 2064, 1024 sectors
        d[462 + 4] = 0x83;
        d[462 + 8..462 + 12].copy_from_slice(&2064u32.to_le_bytes());
        d[462 + 12..462 + 16].copy_from_slice(&1024u32.to_le_bytes());

        // FAT32 BPB at LBA 16
        let p1 = 16 * SECTOR_SIZE;
        d[p1 + 11..p1 + 13].copy_from_slice(&512u16.to_le_bytes());
        d[p1 + 13] = 1; // 1 sector per cluster
        d[p1 + 14..p1 + 16].copy_from_slice(&4u16.to_le_bytes()); // reserved = 4
        d[p1 + 16] = 1; // 1 FAT
        d[p1 + 32..p1 + 36].copy_from_slice(&2048u32.to_le_bytes());
        d[p1 + 36..p1 + 40].copy_from_slice(&16u32.to_le_bytes()); // 16 sectors per FAT
        d[p1 + 44..p1 + 48].copy_from_slice(&2u32.to_le_bytes()); // root cluster = 2
        d[p1 + 510] = 0x55;
        d[p1 + 511] = 0xAA;

        // FAT entries: cluster 0,1 reserved; cluster 2 (root dir) = EOC; cluster 3 (README.TXT) = EOC
        let fat = p1 + 4 * SECTOR_SIZE;
        d[fat..fat + 4].copy_from_slice(&0x0FFF_FFF8u32.to_le_bytes());
        d[fat + 4..fat + 8].copy_from_slice(&0x0FFF_FFFFu32.to_le_bytes());
        d[fat + 8..fat + 12].copy_from_slice(&0x0FFF_FFFFu32.to_le_bytes());
        d[fat + 12..fat + 16].copy_from_slice(&0x0FFF_FFFFu32.to_le_bytes());

        // Root dir at data_start = 16 + 4 + 16 = LBA 36
        let root = 36 * SECTOR_SIZE;
        d[root..root + 11].copy_from_slice(b"README  TXT");
        d[root + 11] = 0x20;
        d[root + 26..root + 28].copy_from_slice(&3u16.to_le_bytes());
        d[root + 28..root + 32].copy_from_slice(&5u32.to_le_bytes());

        let file3 = 37 * SECTOR_SIZE;
        d[file3..file3 + 5].copy_from_slice(b"hello");

        // Minimal ext2 volume at LBA 2064 (block_size=1024, BGD=block 2, inode_table=block 3, root_dir=block 10)
        let p2 = 2064 * SECTOR_SIZE;
        let sb = p2 + 1024;
        d[sb + 40..sb + 44].copy_from_slice(&16u32.to_le_bytes()); // inodes_per_group
        d[sb + 56..sb + 58].copy_from_slice(&0xEF53u16.to_le_bytes());

        let bgd = p2 + 2 * 1024;
        d[bgd + 8..bgd + 12].copy_from_slice(&3u32.to_le_bytes()); // inode_table = block 3

        let ino_tbl = p2 + 3 * 1024;
        // Root inode 2 is at index 1 (offset 128)
        let r_ino = ino_tbl + 128;
        d[r_ino..r_ino + 2].copy_from_slice(&0x41EDu16.to_le_bytes()); // dir
        d[r_ino + 4..r_ino + 8].copy_from_slice(&1024u32.to_le_bytes());
        d[r_ino + 40..r_ino + 44].copy_from_slice(&10u32.to_le_bytes()); // block 10

        // File inode 3 is at index 2 (offset 256)
        let f_ino = ino_tbl + 256;
        d[f_ino..f_ino + 2].copy_from_slice(&0x81A4u16.to_le_bytes()); // regular file
        d[f_ino + 4..f_ino + 8].copy_from_slice(&4u32.to_le_bytes());
        d[f_ino + 40..f_ino + 44].copy_from_slice(&11u32.to_le_bytes()); // block 11

        // Root dir entry in block 10: "note.txt" -> inode 3
        let rdir = p2 + 10 * 1024;
        d[rdir..rdir + 4].copy_from_slice(&3u32.to_le_bytes());
        d[rdir + 4..rdir + 6].copy_from_slice(&1024u16.to_le_bytes());
        d[rdir + 6] = 8;
        d[rdir + 7] = 1;
        d[rdir + 8..rdir + 16].copy_from_slice(b"note.txt");

        let fdata = p2 + 11 * 1024;
        d[fdata..fdata + 4].copy_from_slice(b"EXT2");

        disk
    }

    #[test]
    fn vfs_mounts_fat32_and_ext2_and_writes_fat32_files() {
        let mut disk = build_test_disk();
        let vfs = VfsRouter::mount(&mut disk).expect("mount MBR + FAT32 + ext2");
        assert!(vfs.has_ext2());

        let mut buf = [0u8; 64];
        let n = vfs.read_file(&mut disk, "/README.TXT", &mut buf).unwrap();
        assert_eq!(&buf[..n], b"hello");

        let ext_n = vfs
            .read_file(&mut disk, "/ext2/note.txt", &mut buf)
            .unwrap();
        assert_eq!(&buf[..ext_n], b"EXT2");

        // Write a new file on FAT32 and read it back.
        vfs.write_file(&mut disk, "/TEST.TXT", b"DeltaM5!").unwrap();
        let tn = vfs.read_file(&mut disk, "/TEST.TXT", &mut buf).unwrap();
        assert_eq!(&buf[..tn], b"DeltaM5!");

        // Virtual /proc check
        let pn = vfs.read_file(&mut disk, "/proc/mounts", &mut buf).unwrap();
        assert!(core::str::from_utf8(&buf[..pn]).unwrap().contains("fat32"));
    }
}

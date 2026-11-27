#![cfg_attr(not(test), no_std)]

//! Hardware-independent VFS router supporting separated Boot (`FAT32`), System (`DeltaFS`),
//! User (`DeltaFS`), optional legacy `ext2`, `/dev`, and `/proc`.

pub mod block;
pub mod deltafs;
pub mod ext2;
pub mod fat32;
pub mod mbr;

use block::{BlockDevice, FsError, PartitionSlice, SECTOR_SIZE};
use deltafs::{DELTAFS_BLOCK_SIZE, DeltaFsVolume, ROLE_SYSTEM, ROLE_USER};
use ext2::Ext2Volume;
use fat32::Fat32Volume;
use libdelta::delta_abi::VfsPartitionStat;
use mbr::MbrPartition;

/// Unified Virtual Filesystem router mounting separated Boot (`/boot`), System (`/system`),
/// User (`/user`), optional `/ext2`, `/dev`, and `/proc`.
pub struct VfsRouter {
    boot_part: Option<(MbrPartition, Fat32Volume)>,
    system_part: Option<(MbrPartition, DeltaFsVolume)>,
    user_part: Option<(MbrPartition, DeltaFsVolume)>,
    ext2_part: Option<(MbrPartition, Ext2Volume)>,
}

impl VfsRouter {
    /// Inspects MBR on `dev` and mounts `/boot` (FAT32), `/system` (DeltaFS), `/user` (DeltaFS),
    /// and optional `/ext2`.
    pub fn mount<B: BlockDevice>(dev: &mut B) -> Result<Self, FsError> {
        let parts = mbr::read_mbr(dev)?;
        let mut boot_part = None;
        let mut system_part = None;
        let mut user_part = None;
        let mut ext2_part = None;

        for p in parts.into_iter().flatten() {
            if p.is_fat32() && boot_part.is_none() {
                let mut slice = PartitionSlice::new(dev, p.start_lba, p.sector_count);
                if let Ok(vol) = Fat32Volume::mount(&mut slice) {
                    boot_part = Some((p, vol));
                }
            } else if p.is_deltafs() {
                let mut slice = PartitionSlice::new(dev, p.start_lba, p.sector_count);
                if let Ok(vol) = DeltaFsVolume::mount(&mut slice) {
                    if vol.role() == ROLE_USER
                        || (vol.role() != ROLE_SYSTEM && system_part.is_some())
                    {
                        if user_part.is_none() {
                            user_part = Some((p, vol));
                        }
                    } else if system_part.is_none() {
                        system_part = Some((p, vol));
                    }
                }
            } else if p.is_linux_ext() && ext2_part.is_none() {
                let mut slice = PartitionSlice::new(dev, p.start_lba, p.sector_count);
                if let Ok(vol) = Ext2Volume::mount(&mut slice) {
                    ext2_part = Some((p, vol));
                }
            }
        }

        if boot_part.is_none() && system_part.is_none() {
            return Err(FsError::InvalidFs);
        }

        Ok(Self {
            boot_part,
            system_part,
            user_part,
            ext2_part,
        })
    }

    pub fn has_boot(&self) -> bool {
        self.boot_part.is_some()
    }

    pub fn has_system(&self) -> bool {
        self.system_part.is_some()
    }

    pub fn has_user(&self) -> bool {
        self.user_part.is_some()
    }

    pub fn has_ext2(&self) -> bool {
        self.ext2_part.is_some()
    }

    pub fn stat<B: BlockDevice>(&self, dev: &mut B, path: &str) -> Result<(u32, bool), FsError> {
        let norm = normalize(path);
        if matches!(norm, "/" | "/dev" | "/proc") {
            return Ok((0, true));
        }
        if let Some(rel) = norm.strip_prefix("/proc/") {
            return match self.proc_content(rel) {
                Some(bytes) => Ok((bytes.len() as u32, false)),
                None => Err(FsError::NotFound),
            };
        }
        if let Some(sub) = strip_mount(norm, "/boot") {
            let (part, vol) = self.boot_part.ok_or(FsError::NotFound)?;
            let mut slice = PartitionSlice::new(dev, part.start_lba, part.sector_count);
            return vol.stat(&mut slice, sub);
        }
        if let Some(sub) = strip_mount(norm, "/system") {
            let (part, vol) = self.system_part.ok_or(FsError::NotFound)?;
            let mut slice = PartitionSlice::new(dev, part.start_lba, part.sector_count);
            return vol.stat(&mut slice, sub);
        }
        if let Some(sub) = strip_mount(norm, "/user") {
            let (part, vol) = self.user_part.ok_or(FsError::NotFound)?;
            let mut slice = PartitionSlice::new(dev, part.start_lba, part.sector_count);
            return vol.stat(&mut slice, sub);
        }
        if let Some(sub) = strip_mount(norm, "/ext2") {
            let (part, vol) = self.ext2_part.ok_or(FsError::NotFound)?;
            let mut slice = PartitionSlice::new(dev, part.start_lba, part.sector_count);
            return vol.stat(&mut slice, sub);
        }
        Err(FsError::NotFound)
    }

    pub fn read_dir<B: BlockDevice>(
        &self,
        dev: &mut B,
        path: &str,
        out: &mut [u8],
    ) -> Result<usize, FsError> {
        let norm = normalize(path);
        if norm == "/" {
            let mut n = 0usize;
            if self.has_boot() {
                n += copy_str(&mut out[n..], "[DIR]  boot/\n");
            }
            if self.has_system() {
                n += copy_str(&mut out[n..], "[DIR]  system/\n");
            }
            if self.has_user() {
                n += copy_str(&mut out[n..], "[DIR]  user/\n");
            }
            if self.has_ext2() {
                n += copy_str(&mut out[n..], "[DIR]  ext2/\n");
            }
            n += copy_str(&mut out[n..], "[DIR]  dev/\n[DIR]  proc/\n");
            return Ok(n);
        }
        if norm == "/dev" {
            return Ok(copy_str(
                out,
                "[DEV]  uart0\n[DEV]  sd0\n[DEV]  fb0\n[DEV]  audio0\n",
            ));
        }
        if norm == "/proc" {
            return Ok(copy_str(
                out,
                "[PROC] version\n[PROC] mounts\n[PROC] partitions\n",
            ));
        }
        if let Some(sub) = strip_mount(norm, "/boot") {
            let (part, vol) = self.boot_part.ok_or(FsError::NotFound)?;
            let mut slice = PartitionSlice::new(dev, part.start_lba, part.sector_count);
            return vol.read_dir(&mut slice, sub, out);
        }
        if let Some(sub) = strip_mount(norm, "/system") {
            let (part, vol) = self.system_part.ok_or(FsError::NotFound)?;
            let mut slice = PartitionSlice::new(dev, part.start_lba, part.sector_count);
            return vol.read_dir(&mut slice, sub, out);
        }
        if let Some(sub) = strip_mount(norm, "/user") {
            let (part, vol) = self.user_part.ok_or(FsError::NotFound)?;
            let mut slice = PartitionSlice::new(dev, part.start_lba, part.sector_count);
            return vol.read_dir(&mut slice, sub, out);
        }
        if let Some(sub) = strip_mount(norm, "/ext2") {
            let (part, vol) = self.ext2_part.ok_or(FsError::NotFound)?;
            let mut slice = PartitionSlice::new(dev, part.start_lba, part.sector_count);
            return vol.read_dir(&mut slice, sub, out);
        }
        Err(FsError::NotFound)
    }

    pub fn read_file<B: BlockDevice>(
        &self,
        dev: &mut B,
        path: &str,
        out: &mut [u8],
    ) -> Result<usize, FsError> {
        let norm = normalize(path);
        if let Some(rel) = norm.strip_prefix("/proc/") {
            let bytes = self.proc_content(rel).ok_or(FsError::NotFound)?;
            let n = bytes.len().min(out.len());
            out[..n].copy_from_slice(&bytes[..n]);
            return Ok(n);
        }
        if let Some(sub) = strip_mount(norm, "/boot") {
            let (part, vol) = self.boot_part.ok_or(FsError::NotFound)?;
            let mut slice = PartitionSlice::new(dev, part.start_lba, part.sector_count);
            return vol.read_file(&mut slice, sub, out);
        }
        if let Some(sub) = strip_mount(norm, "/system") {
            let (part, vol) = self.system_part.ok_or(FsError::NotFound)?;
            let mut slice = PartitionSlice::new(dev, part.start_lba, part.sector_count);
            return vol.read_file(&mut slice, sub, out);
        }
        if let Some(sub) = strip_mount(norm, "/user") {
            let (part, vol) = self.user_part.ok_or(FsError::NotFound)?;
            let mut slice = PartitionSlice::new(dev, part.start_lba, part.sector_count);
            return vol.read_file(&mut slice, sub, out);
        }
        if let Some(sub) = strip_mount(norm, "/ext2") {
            let (part, vol) = self.ext2_part.ok_or(FsError::NotFound)?;
            let mut slice = PartitionSlice::new(dev, part.start_lba, part.sector_count);
            return vol.read_file(&mut slice, sub, out);
        }
        Err(FsError::NotFound)
    }

    pub fn write_file<B: BlockDevice>(
        &mut self,
        dev: &mut B,
        path: &str,
        data: &[u8],
    ) -> Result<usize, FsError> {
        let norm = normalize(path);
        if norm.starts_with("/proc") || norm.starts_with("/dev") || norm.starts_with("/ext2") {
            return Err(FsError::ReadOnly);
        }
        if let Some(sub) = strip_mount(norm, "/boot") {
            let (part, vol) = self.boot_part.ok_or(FsError::NotFound)?;
            let mut slice = PartitionSlice::new(dev, part.start_lba, part.sector_count);
            return vol.write_file(&mut slice, sub, data);
        }
        if let Some(sub) = strip_mount(norm, "/system") {
            let (part, vol) = self.system_part.as_mut().ok_or(FsError::NotFound)?;
            let mut slice = PartitionSlice::new(dev, part.start_lba, part.sector_count);
            return vol.write_file(&mut slice, sub, data);
        }
        if let Some(sub) = strip_mount(norm, "/user") {
            let (part, vol) = self.user_part.as_mut().ok_or(FsError::NotFound)?;
            let mut slice = PartitionSlice::new(dev, part.start_lba, part.sector_count);
            return vol.write_file(&mut slice, sub, data);
        }
        Err(FsError::ReadOnly)
    }

    pub fn mkdir<B: BlockDevice>(&mut self, dev: &mut B, path: &str) -> Result<(), FsError> {
        let norm = normalize(path);
        if let Some(sub) = strip_mount(norm, "/user") {
            let (part, vol) = self.user_part.as_mut().ok_or(FsError::NotFound)?;
            let mut slice = PartitionSlice::new(dev, part.start_lba, part.sector_count);
            return vol.mkdir(&mut slice, sub);
        }
        if let Some(sub) = strip_mount(norm, "/system") {
            let (part, vol) = self.system_part.as_mut().ok_or(FsError::NotFound)?;
            let mut slice = PartitionSlice::new(dev, part.start_lba, part.sector_count);
            return vol.mkdir(&mut slice, sub);
        }
        Err(FsError::ReadOnly)
    }

    pub fn remove<B: BlockDevice>(&mut self, dev: &mut B, path: &str) -> Result<(), FsError> {
        let norm = normalize(path);
        if let Some(sub) = strip_mount(norm, "/user") {
            let (part, vol) = self.user_part.as_mut().ok_or(FsError::NotFound)?;
            let mut slice = PartitionSlice::new(dev, part.start_lba, part.sector_count);
            return vol.remove(&mut slice, sub);
        }
        if let Some(sub) = strip_mount(norm, "/system") {
            let (part, vol) = self.system_part.as_mut().ok_or(FsError::NotFound)?;
            let mut slice = PartitionSlice::new(dev, part.start_lba, part.sector_count);
            return vol.remove(&mut slice, sub);
        }
        Err(FsError::ReadOnly)
    }

    pub fn statfs<B: BlockDevice>(&self, dev: &mut B, out: &mut [VfsPartitionStat]) -> usize {
        let mut idx = 0usize;
        if let Some((part, vol)) = self.boot_part
            && idx < out.len()
        {
            let mut slice = PartitionSlice::new(dev, part.start_lba, part.sector_count);
            let free = vol.free_bytes(&mut slice);
            out[idx] = make_stat(
                "/boot",
                "FAT32",
                part.sector_count * SECTOR_SIZE as u64,
                free,
                true,
                false,
            );
            idx += 1;
        }
        if let Some((_, vol)) = self.system_part
            && idx < out.len()
        {
            out[idx] = make_stat(
                "/system",
                "DeltaFS",
                vol.capacity_bytes(),
                vol.free_bytes(),
                true,
                false,
            );
            idx += 1;
        }
        if let Some((_, vol)) = self.user_part
            && idx < out.len()
        {
            out[idx] = make_stat(
                "/user",
                "DeltaFS",
                vol.capacity_bytes(),
                vol.free_bytes(),
                true,
                true,
            );
            idx += 1;
        }
        idx
    }

    /// Expands the `/user` DeltaFS partition in MBR and DeltaFS superblock up to `target_disk_sectors`.
    pub fn expand_user_to_disk<B: BlockDevice>(
        &mut self,
        dev: &mut B,
        target_disk_sectors: u64,
    ) -> Result<u64, FsError> {
        let (part, vol) = self.user_part.as_mut().ok_or(FsError::NotFound)?;
        let sectors_per_blk = (DELTAFS_BLOCK_SIZE / SECTOR_SIZE) as u64;
        if target_disk_sectors <= part.start_lba + part.sector_count {
            return Ok(vol.capacity_bytes());
        }
        let new_part_sectors = target_disk_sectors - part.start_lba;
        let new_blocks = new_part_sectors / sectors_per_blk;
        let expanded_blocks = {
            let mut slice = PartitionSlice::new(dev, part.start_lba, new_part_sectors);
            vol.expand_to_blocks(&mut slice, new_blocks)?
        };
        let aligned_sectors = (expanded_blocks * sectors_per_blk) as u32;
        mbr::update_partition_sectors(dev, part.slot_index, aligned_sectors)?;
        part.sector_count = u64::from(aligned_sectors);
        Ok(vol.capacity_bytes())
    }

    fn proc_content(&self, name: &str) -> Option<&'static [u8]> {
        match name {
            "version" => Some(
                concat!(
                    "Hollow Star OS+ v",
                    env!("CARGO_PKG_VERSION"),
                    " (Delta Kernel / AArch64)\n"
                )
                .as_bytes(),
            ),
            "mounts" => {
                if self.has_system() && self.has_user() {
                    Some(
                        b"/boot -> fat32 (rw)\n/system -> deltafs (rw, journal)\n/user -> deltafs (rw, journal)\n/dev -> devfs\n/proc -> procfs\n",
                    )
                } else {
                    Some(b"/boot -> fat32 (rw)\n/dev -> devfs\n/proc -> procfs\n")
                }
            }
            "partitions" => {
                Some(b"p1: /boot (FAT32) | p2: /system (DeltaFS) | p3: /user (DeltaFS)\n")
            }
            _ => None,
        }
    }
}

fn strip_mount<'a>(norm: &'a str, prefix: &str) -> Option<&'a str> {
    if norm == prefix {
        Some("/")
    } else {
        norm.strip_prefix(prefix)
            .filter(|rest| rest.starts_with('/'))
    }
}

fn make_stat(
    mount_str: &str,
    fs_str: &str,
    total_bytes: u64,
    free_bytes: u64,
    writable: bool,
    expandable: bool,
) -> VfsPartitionStat {
    let mut stat = VfsPartitionStat {
        total_bytes,
        free_bytes,
        mount_len: mount_str.len().min(16) as u8,
        fs_len: fs_str.len().min(12) as u8,
        writable: u8::from(writable),
        expandable: u8::from(expandable),
        mount: [0; 16],
        fs_type: [0; 12],
    };
    stat.mount[..stat.mount_len as usize]
        .copy_from_slice(&mount_str.as_bytes()[..stat.mount_len as usize]);
    stat.fs_type[..stat.fs_len as usize]
        .copy_from_slice(&fs_str.as_bytes()[..stat.fs_len as usize]);
    stat
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

#[cfg(test)]
mod tests {
    use super::{BlockDevice, VfsRouter};
    use crate::block::{FsError, SECTOR_SIZE};
    use crate::deltafs::{DELTAFS_BLOCK_SIZE, ROLE_SYSTEM, ROLE_USER, crc32};

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

    fn format_deltafs_in_ram(d: &mut [u8], start_lba: usize, blocks: u64, role: u32, label: &str) {
        let base = start_lba * SECTOR_SIZE;
        let journal_start = 1u64;
        let journal_blocks = 16u64;
        let bitmap_start = 17u64;
        let bitmap_blocks = 8u64;
        let inode_start = 25u64;
        let inode_blocks = 4u64;
        let data_start = inode_start + inode_blocks;

        // Mark metadata + root dir block (data_start) as allocated in bitmap.
        let used_end = data_start + 1;
        let bm_off = base + (bitmap_start as usize) * DELTAFS_BLOCK_SIZE;
        for b in 0..used_end {
            d[bm_off + (b as usize / 8)] |= 1 << (b % 8);
        }

        // Root inode (id=1, Dir, 1 block at data_start)
        let ino_off = base + (inode_start as usize) * DELTAFS_BLOCK_SIZE;
        d[ino_off..ino_off + 8].copy_from_slice(&1u64.to_le_bytes());
        d[ino_off + 8] = 2; // Dir
        d[ino_off + 9] = 1; // 1 extent
        d[ino_off + 12..ino_off + 16].copy_from_slice(&0o755u32.to_le_bytes());
        d[ino_off + 24..ino_off + 32].copy_from_slice(&data_start.to_le_bytes());
        d[ino_off + 72..ino_off + 76].copy_from_slice(&1u32.to_le_bytes());

        // Superblock at block 0
        let sb = &mut d[base..base + DELTAFS_BLOCK_SIZE];
        sb[0..8].copy_from_slice(b"DELTAFS1");
        sb[8..12].copy_from_slice(&1u32.to_le_bytes());
        sb[12..16].copy_from_slice(&(DELTAFS_BLOCK_SIZE as u32).to_le_bytes());
        sb[16..24].copy_from_slice(&blocks.to_le_bytes());
        sb[24..32].copy_from_slice(&(blocks - used_end).to_le_bytes());
        sb[32..40].copy_from_slice(&journal_start.to_le_bytes());
        sb[40..48].copy_from_slice(&journal_blocks.to_le_bytes());
        sb[48..56].copy_from_slice(&bitmap_start.to_le_bytes());
        sb[56..64].copy_from_slice(&bitmap_blocks.to_le_bytes());
        sb[64..72].copy_from_slice(&inode_start.to_le_bytes());
        sb[72..80].copy_from_slice(&inode_blocks.to_le_bytes());
        sb[80..88].copy_from_slice(&data_start.to_le_bytes());
        sb[88..96].copy_from_slice(&(inode_blocks * 32).to_le_bytes());
        sb[96..104].copy_from_slice(&0u64.to_le_bytes());
        sb[104..108].copy_from_slice(&role.to_le_bytes());
        let lbytes = label.as_bytes();
        let ln = lbytes.len().min(32);
        sb[112..112 + ln].copy_from_slice(&lbytes[..ln]);
        let sum = crc32(&sb[0..144]);
        sb[144..148].copy_from_slice(&sum.to_le_bytes());
    }

    fn build_three_part_disk() -> RamDisk {
        let mut disk = RamDisk::new(8192); // 4 MiB
        let d = &mut disk.data;
        d[510] = 0x55;
        d[511] = 0xAA;

        // P1: FAT32 /boot at LBA 16, 1024 sectors
        d[446 + 4] = 0x0C;
        d[446 + 8..446 + 12].copy_from_slice(&16u32.to_le_bytes());
        d[446 + 12..446 + 16].copy_from_slice(&1024u32.to_le_bytes());

        let p1 = 16 * SECTOR_SIZE;
        d[p1 + 11..p1 + 13].copy_from_slice(&512u16.to_le_bytes());
        d[p1 + 13] = 1;
        d[p1 + 14..p1 + 16].copy_from_slice(&4u16.to_le_bytes());
        d[p1 + 16] = 1;
        d[p1 + 32..p1 + 36].copy_from_slice(&1024u32.to_le_bytes());
        d[p1 + 36..p1 + 40].copy_from_slice(&8u32.to_le_bytes());
        d[p1 + 44..p1 + 48].copy_from_slice(&2u32.to_le_bytes());
        d[p1 + 510] = 0x55;
        d[p1 + 511] = 0xAA;

        let fat = p1 + 4 * SECTOR_SIZE;
        d[fat..fat + 4].copy_from_slice(&0x0FFF_FFF8u32.to_le_bytes());
        d[fat + 4..fat + 8].copy_from_slice(&0x0FFF_FFFFu32.to_le_bytes());
        d[fat + 8..fat + 12].copy_from_slice(&0x0FFF_FFFFu32.to_le_bytes());

        // P2: DeltaFS /system at LBA 1040, 1024 sectors (128 blocks)
        d[462 + 4] = 0xDF;
        d[462 + 8..462 + 12].copy_from_slice(&1040u32.to_le_bytes());
        d[462 + 12..462 + 16].copy_from_slice(&1024u32.to_le_bytes());
        format_deltafs_in_ram(d, 1040, 128, ROLE_SYSTEM, "SYSTEM");

        // P3: DeltaFS /user at LBA 2064, 1024 sectors (128 blocks; tail free up to 8192)
        d[478 + 4] = 0xDF;
        d[478 + 8..478 + 12].copy_from_slice(&2064u32.to_le_bytes());
        d[478 + 12..478 + 16].copy_from_slice(&1024u32.to_le_bytes());
        format_deltafs_in_ram(d, 2064, 128, ROLE_USER, "USER");

        disk
    }

    #[test]
    fn mounts_boot_system_user_partitions_with_deltafs_rw_and_expand() {
        let mut disk = build_three_part_disk();
        let mut vfs = VfsRouter::mount(&mut disk).expect("mount 3-part disk");
        assert!(vfs.has_boot() && vfs.has_system() && vfs.has_user());

        vfs.mkdir(&mut disk, "/user/notes").unwrap();
        vfs.write_file(&mut disk, "/user/notes/hello.md", b"# DeltaFS!")
            .unwrap();

        let mut buf = [0u8; 64];
        let n = vfs
            .read_file(&mut disk, "/user/notes/hello.md", &mut buf)
            .unwrap();
        assert_eq!(&buf[..n], b"# DeltaFS!");

        // Expand /user to end of 8192-sector RamDisk and verify increased capacity after remount.
        let new_bytes = vfs.expand_user_to_disk(&mut disk, 8192).unwrap();
        assert!(new_bytes > 512 * 1024);

        let vfs_re = VfsRouter::mount(&mut disk).expect("remount expanded user deltafs");
        let mut stats = [unsafe { core::mem::zeroed() }; 3];
        let sc = vfs_re.statfs(&mut disk, &mut stats);
        assert_eq!(sc, 3);
        assert_eq!(stats[2].total_bytes, new_bytes);

        let n2 = vfs_re
            .read_file(&mut disk, "/user/notes/hello.md", &mut buf)
            .unwrap();
        assert_eq!(&buf[..n2], b"# DeltaFS!");
    }

    #[test]
    fn deltafs_journal_replays_committed_tx_on_mount() {
        let mut disk = build_three_part_disk();
        let mut vfs = VfsRouter::mount(&mut disk).expect("mount 3-part disk");
        vfs.write_file(&mut disk, "/user/crash.txt", b"safe-data")
            .unwrap();

        // Simulate crash right after writing a committed WAL entry for block 0 (updating label).
        let user_base = 2064 * SECTOR_SIZE;
        let sb_orig = disk.data[user_base..user_base + DELTAFS_BLOCK_SIZE].to_vec();
        let mut sb_patched = sb_orig.clone();
        sb_patched[112..118].copy_from_slice(b"USER2\0");
        let new_crc = crc32(&sb_patched[0..144]);
        sb_patched[144..148].copy_from_slice(&new_crc.to_le_bytes());

        let j0 = user_base + DELTAFS_BLOCK_SIZE; // journal_start = 1
        let j1 = user_base + 2 * DELTAFS_BLOCK_SIZE;
        disk.data[j1..j1 + DELTAFS_BLOCK_SIZE].copy_from_slice(&sb_patched);
        disk.data[j0..j0 + DELTAFS_BLOCK_SIZE].fill(0);
        disk.data[j0..j0 + 4].copy_from_slice(b"DLTX");
        disk.data[j0 + 4..j0 + 8].copy_from_slice(&2u32.to_le_bytes()); // COMMITTED
        disk.data[j0 + 8..j0 + 16].copy_from_slice(&99u64.to_le_bytes());
        disk.data[j0 + 16..j0 + 20].copy_from_slice(&1u32.to_le_bytes()); // 1 entry
        disk.data[j0 + 24..j0 + 32].copy_from_slice(&0u64.to_le_bytes()); // target block 0

        let mut tx_data = Vec::new();
        tx_data.extend_from_slice(&disk.data[j0 + 16..j0 + 20]);
        tx_data.extend_from_slice(&disk.data[j0 + 24..j0 + 32]);
        tx_data.extend_from_slice(&sb_patched);
        let tx_crc = crc32(&tx_data);
        disk.data[j0 + 20..j0 + 24].copy_from_slice(&tx_crc.to_le_bytes());

        let mut slice = crate::block::PartitionSlice::new(&mut disk, 2064, 1024);
        let vol = super::DeltaFsVolume::mount(&mut slice).unwrap();
        assert!(vol.journal_replayed);
        assert_eq!(vol.label_str(), "USER2");
    }
}

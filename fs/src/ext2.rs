//! Read-only ext2 filesystem driver (Superblock, BGD, Inodes, Directories, Files).

use crate::block::{BlockDevice, FsError, SECTOR_SIZE};

const EXT2_SUPER_MAGIC: u16 = 0xEF53;
const EXT2_ROOT_INO: u32 = 2;
const S_IFDIR: u16 = 0x4000;

/// Mounted ext2 filesystem descriptor.
#[derive(Debug, Clone, Copy)]
pub struct Ext2Volume {
    pub block_size: u32,
    pub inodes_per_group: u32,
    pub inode_size: u16,
    pub bgd_block: u32,
}

#[derive(Clone, Copy)]
struct Inode {
    mode: u16,
    size: u32,
    direct_blocks: [u32; 12],
}

impl Ext2Volume {
    /// Mounts an ext2 filesystem from `dev`.
    pub fn mount<B: BlockDevice>(dev: &mut B) -> Result<Self, FsError> {
        let mut sb = [0u8; 1024];
        dev.read_sectors(2, &mut sb[0..512])?;
        dev.read_sectors(3, &mut sb[512..1024])?;

        let magic = u16::from_le_bytes([sb[56], sb[57]]);
        if magic != EXT2_SUPER_MAGIC {
            return Err(FsError::InvalidFs);
        }

        let log_bs = u32::from_le_bytes([sb[24], sb[25], sb[26], sb[27]]);
        if log_bs > 3 {
            return Err(FsError::InvalidFs);
        }
        let block_size = 1024u32 << log_bs;
        let inodes_per_group = u32::from_le_bytes([sb[40], sb[41], sb[42], sb[43]]);
        let rev_level = u32::from_le_bytes([sb[76], sb[77], sb[78], sb[79]]);
        let inode_size = if rev_level >= 1 {
            u16::from_le_bytes([sb[88], sb[89]]).max(128)
        } else {
            128
        };
        if inodes_per_group == 0 {
            return Err(FsError::InvalidFs);
        }

        let bgd_block = if block_size == 1024 { 2 } else { 1 };
        Ok(Self {
            block_size,
            inodes_per_group,
            inode_size,
            bgd_block,
        })
    }

    fn read_block<B: BlockDevice>(
        &self,
        dev: &mut B,
        block_no: u32,
        dst: &mut [u8],
    ) -> Result<(), FsError> {
        let sectors_per_block = self.block_size / (SECTOR_SIZE as u32);
        let lba = u64::from(block_no) * u64::from(sectors_per_block);
        dev.read_sectors(lba, &mut dst[..self.block_size as usize])
    }

    fn read_inode<B: BlockDevice>(&self, dev: &mut B, ino: u32) -> Result<Inode, FsError> {
        if ino == 0 {
            return Err(FsError::NotFound);
        }
        let group = (ino - 1) / self.inodes_per_group;
        let index = (ino - 1) % self.inodes_per_group;

        let mut blk = [0u8; 4096];
        let bgd_sector_blk = self.bgd_block + (group * 32) / self.block_size;
        let bgd_off = ((group * 32) % self.block_size) as usize;
        self.read_block(dev, bgd_sector_blk, &mut blk)?;
        let inode_table_blk = u32::from_le_bytes([
            blk[bgd_off + 8],
            blk[bgd_off + 9],
            blk[bgd_off + 10],
            blk[bgd_off + 11],
        ]);

        let byte_offset = index * u32::from(self.inode_size);
        let tbl_blk = inode_table_blk + byte_offset / self.block_size;
        let in_off = (byte_offset % self.block_size) as usize;
        self.read_block(dev, tbl_blk, &mut blk)?;

        let mode = u16::from_le_bytes([blk[in_off], blk[in_off + 1]]);
        let size = u32::from_le_bytes([
            blk[in_off + 4],
            blk[in_off + 5],
            blk[in_off + 6],
            blk[in_off + 7],
        ]);
        let mut direct_blocks = [0u32; 12];
        for (i, slot) in direct_blocks.iter_mut().enumerate() {
            let bo = in_off + 40 + i * 4;
            *slot = u32::from_le_bytes([blk[bo], blk[bo + 1], blk[bo + 2], blk[bo + 3]]);
        }

        Ok(Inode {
            mode,
            size,
            direct_blocks,
        })
    }

    fn walk_dir<B: BlockDevice, F>(
        &self,
        dev: &mut B,
        inode: &Inode,
        mut visitor: F,
    ) -> Result<(), FsError>
    where
        F: FnMut(&str, u32, u8) -> bool,
    {
        if (inode.mode & S_IFDIR) == 0 {
            return Err(FsError::NotADir);
        }
        let mut blk = [0u8; 4096];
        for &b in &inode.direct_blocks {
            if b == 0 {
                break;
            }
            self.read_block(dev, b, &mut blk)?;
            let mut off = 0usize;
            while off + 8 <= self.block_size as usize {
                let rec_ino =
                    u32::from_le_bytes([blk[off], blk[off + 1], blk[off + 2], blk[off + 3]]);
                let rec_len = u16::from_le_bytes([blk[off + 4], blk[off + 5]]) as usize;
                let name_len = usize::from(blk[off + 6]);
                let file_type = blk[off + 7];
                if rec_len < 8 || off + rec_len > self.block_size as usize {
                    break;
                }
                if rec_ino != 0
                    && name_len > 0
                    && off + 8 + name_len <= off + rec_len
                    && let Ok(name) = core::str::from_utf8(&blk[off + 8..off + 8 + name_len])
                    && name != "."
                    && name != ".."
                    && !visitor(name, rec_ino, file_type)
                {
                    return Ok(());
                }
                off += rec_len;
            }
        }
        Ok(())
    }

    fn resolve_inode<B: BlockDevice>(&self, dev: &mut B, path: &str) -> Result<Inode, FsError> {
        let mut inode = self.read_inode(dev, EXT2_ROOT_INO)?;
        let trimmed = path.trim_matches('/');
        if trimmed.is_empty() {
            return Ok(inode);
        }

        for part in trimmed.split('/') {
            if part.is_empty() {
                continue;
            }
            let mut target_ino = None;
            self.walk_dir(dev, &inode, |name, child_ino, _| {
                if name == part {
                    target_ino = Some(child_ino);
                    return false;
                }
                true
            })?;
            let ino = target_ino.ok_or(FsError::NotFound)?;
            inode = self.read_inode(dev, ino)?;
        }
        Ok(inode)
    }

    pub fn stat<B: BlockDevice>(&self, dev: &mut B, path: &str) -> Result<(u32, bool), FsError> {
        let inode = self.resolve_inode(dev, path)?;
        Ok((inode.size, (inode.mode & S_IFDIR) != 0))
    }

    pub fn read_dir<B: BlockDevice>(
        &self,
        dev: &mut B,
        path: &str,
        out: &mut [u8],
    ) -> Result<usize, FsError> {
        let inode = self.resolve_inode(dev, path)?;
        let mut pos = 0usize;
        self.walk_dir(dev, &inode, |name, _, file_type| {
            let prefix = if file_type == 2 {
                b"[DIR]  "
            } else {
                b"[FILE] "
            };
            pos = append_slice(out, pos, prefix);
            pos = append_slice(out, pos, name.as_bytes());
            if file_type == 2 {
                pos = append_slice(out, pos, b"/\n");
            } else {
                pos = append_slice(out, pos, b"\n");
            }
            pos < out.len()
        })?;
        Ok(pos)
    }

    pub fn read_file<B: BlockDevice>(
        &self,
        dev: &mut B,
        path: &str,
        out: &mut [u8],
    ) -> Result<usize, FsError> {
        let inode = self.resolve_inode(dev, path)?;
        if (inode.mode & S_IFDIR) != 0 {
            return Err(FsError::IsADir);
        }
        let target_len = (inode.size as usize).min(out.len());
        let mut copied = 0usize;
        let mut blk = [0u8; 4096];

        for &b in &inode.direct_blocks {
            if copied >= target_len || b == 0 {
                break;
            }
            self.read_block(dev, b, &mut blk)?;
            let chunk = (target_len - copied).min(self.block_size as usize);
            out[copied..copied + chunk].copy_from_slice(&blk[..chunk]);
            copied += chunk;
        }
        Ok(copied)
    }
}

fn append_slice(out: &mut [u8], pos: usize, src: &[u8]) -> usize {
    let n = src.len().min(out.len().saturating_sub(pos));
    out[pos..pos + n].copy_from_slice(&src[..n]);
    pos + n
}

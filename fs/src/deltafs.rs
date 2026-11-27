//! DeltaFS v1 — BeOS BFS-inspired journaling filesystem for Hollow Star OS+.
//!
//! Layout (`4096`-byte blocks, `8` sectors/block):
//! - Block `0`: Superblock (`DELTAFS1`, CRC32-verified, partition role `System`/`User`, volume label)
//! - Blocks `journal_start..`: Write-Ahead Journal (Block 0 = TxHeader `DLTX`, Blocks 1..=12 = payloads)
//! - Blocks `bitmap_start..`: Block allocation bitmap (reserved up to 16 blocks for online growth)
//! - Blocks `inode_table_start..`: 128-byte Inode table (`1` = root directory `/`)
//! - Blocks `data_start..`: Extent-mapped file contents and 128-byte directory entry blocks

use crate::block::{BlockDevice, FsError, SECTOR_SIZE};

pub const DELTAFS_MAGIC: &[u8; 8] = b"DELTAFS1";
pub const DELTAFS_BLOCK_SIZE: usize = 4096;
const SECTORS_PER_BLOCK: u64 = (DELTAFS_BLOCK_SIZE / SECTOR_SIZE) as u64;

const JOURNAL_MAGIC: &[u8; 4] = b"DLTX";
const JOURNAL_STATE_FREE: u32 = 0;
const JOURNAL_STATE_COMMITTED: u32 = 2;
const JOURNAL_MAX_ENTRIES: usize = 12;

const INODE_SIZE: usize = 128;
const INODES_PER_BLOCK: usize = DELTAFS_BLOCK_SIZE / INODE_SIZE;
const DIRENT_SIZE: usize = 128;
const DIRENTS_PER_BLOCK: usize = DELTAFS_BLOCK_SIZE / DIRENT_SIZE;
const DIRENT_MAX_NAME: usize = 118;
const MAX_EXTENTS: usize = 6;

pub const ROLE_GENERIC: u32 = 0;
pub const ROLE_SYSTEM: u32 = 1;
pub const ROLE_USER: u32 = 2;

const KIND_UNUSED: u8 = 0;
const KIND_FILE: u8 = 1;
const KIND_DIR: u8 = 2;
const KIND_HSAPP: u8 = 3;

#[derive(Debug, Clone, Copy)]
pub struct DeltaFsVolume {
    pub total_blocks: u64,
    pub free_blocks: u64,
    pub journal_start: u64,
    pub journal_blocks: u64,
    pub bitmap_start: u64,
    pub bitmap_blocks: u64,
    pub inode_table_start: u64,
    pub inode_table_blocks: u64,
    pub data_start: u64,
    pub inode_count: u64,
    pub journal_seq: u64,
    pub role: u32,
    pub flags: u32,
    pub label: [u8; 32],
    pub journal_replayed: bool,
}

#[derive(Clone, Copy)]
struct DeltaInode {
    id: u64,
    kind: u8,
    extent_count: u8,
    mode: u32,
    size: u64,
    start_blocks: [u64; MAX_EXTENTS],
    block_counts: [u32; MAX_EXTENTS],
    attr: [u8; 32],
}

impl DeltaFsVolume {
    /// Mounts a DeltaFS partition, verifies its superblock CRC32, and replays any
    /// committed un-checkpointed write-ahead journal transaction automatically.
    pub fn mount<B: BlockDevice>(dev: &mut B) -> Result<Self, FsError> {
        let mut blk = [0u8; DELTAFS_BLOCK_SIZE];
        read_block(dev, 0, &mut blk)?;
        let mut vol = Self::parse_superblock(&blk)?;
        vol.replay_journal_if_needed(dev)?;
        Ok(vol)
    }

    pub fn role(&self) -> u32 {
        self.role
    }

    pub fn capacity_bytes(&self) -> u64 {
        self.total_blocks * DELTAFS_BLOCK_SIZE as u64
    }

    pub fn free_bytes(&self) -> u64 {
        self.free_blocks * DELTAFS_BLOCK_SIZE as u64
    }

    pub fn label_str(&self) -> &str {
        let len = self
            .label
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.label.len());
        core::str::from_utf8(&self.label[..len]).unwrap_or("DELTAFS")
    }

    pub fn stat<B: BlockDevice>(&self, dev: &mut B, path: &str) -> Result<(u32, bool), FsError> {
        let inode = self.resolve_path(dev, path)?;
        let is_dir = matches!(inode.kind, KIND_DIR | KIND_HSAPP);
        Ok((inode.size as u32, is_dir))
    }

    pub fn read_dir<B: BlockDevice>(
        &self,
        dev: &mut B,
        path: &str,
        out: &mut [u8],
    ) -> Result<usize, FsError> {
        let dir_inode = self.resolve_path(dev, path)?;
        if !matches!(dir_inode.kind, KIND_DIR | KIND_HSAPP) {
            return Err(FsError::NotADir);
        }

        let mut pos = 0usize;
        let mut blk = [0u8; DELTAFS_BLOCK_SIZE];
        let mut child_buf = [0u8; DELTAFS_BLOCK_SIZE];

        for ext_idx in 0..(dir_inode.extent_count as usize).min(MAX_EXTENTS) {
            let start = dir_inode.start_blocks[ext_idx];
            let count = dir_inode.block_counts[ext_idx];
            for rel in 0..count {
                read_block(dev, start + u64::from(rel), &mut blk)?;
                for slot in 0..DIRENTS_PER_BLOCK {
                    let off = slot * DIRENT_SIZE;
                    let child_id = read_u64(&blk, off);
                    if child_id == 0 {
                        continue;
                    }
                    let kind = blk[off + 8];
                    let nlen = (blk[off + 9] as usize).min(DIRENT_MAX_NAME);
                    let Ok(name) = core::str::from_utf8(&blk[off + 10..off + 10 + nlen]) else {
                        continue;
                    };
                    if matches!(kind, KIND_DIR | KIND_HSAPP) {
                        pos += copy_str(&mut out[pos..], "[DIR]  ");
                        pos += copy_str(&mut out[pos..], name);
                        pos += copy_str(&mut out[pos..], "/\n");
                    } else {
                        let size = self
                            .read_inode_with_buf(dev, child_id, &mut child_buf)
                            .map(|ino| ino.size)
                            .unwrap_or(0);
                        pos += copy_str(&mut out[pos..], "[FILE] ");
                        pos += copy_str(&mut out[pos..], name);
                        pos += copy_str(&mut out[pos..], " (");
                        pos += write_dec(&mut out[pos..], size as u32);
                        pos += copy_str(&mut out[pos..], " B)\n");
                    }
                }
            }
        }
        Ok(pos)
    }

    pub fn read_file<B: BlockDevice>(
        &self,
        dev: &mut B,
        path: &str,
        out: &mut [u8],
    ) -> Result<usize, FsError> {
        let inode = self.resolve_path(dev, path)?;
        if matches!(inode.kind, KIND_DIR | KIND_HSAPP) {
            return Err(FsError::IsADir);
        }

        let target = (inode.size as usize).min(out.len());
        let mut copied = 0usize;
        let mut blk = [0u8; DELTAFS_BLOCK_SIZE];

        for ext_idx in 0..(inode.extent_count as usize).min(MAX_EXTENTS) {
            let start = inode.start_blocks[ext_idx];
            let count = inode.block_counts[ext_idx];
            for rel in 0..count {
                if copied >= target {
                    return Ok(copied);
                }
                read_block(dev, start + u64::from(rel), &mut blk)?;
                let chunk = (target - copied).min(DELTAFS_BLOCK_SIZE);
                out[copied..copied + chunk].copy_from_slice(&blk[..chunk]);
                copied += chunk;
            }
        }
        Ok(copied)
    }

    pub fn write_file<B: BlockDevice>(
        &mut self,
        dev: &mut B,
        path: &str,
        data: &[u8],
    ) -> Result<usize, FsError> {
        let (parent_path, file_name) = split_parent(path).ok_or(FsError::InvalidFs)?;
        if file_name.is_empty() || file_name.len() > DIRENT_MAX_NAME {
            return Err(FsError::InvalidFs);
        }
        let parent = self.resolve_path(dev, parent_path)?;
        if !matches!(parent.kind, KIND_DIR | KIND_HSAPP) {
            return Err(FsError::NotADir);
        }

        let blocks_needed = data.len().div_ceil(DELTAFS_BLOCK_SIZE).max(1) as u32;
        let mut inode_id = self.find_in_dir(dev, &parent, file_name)?;

        // If the file already exists, recycle its previous blocks cleanly.
        if let Some(existing_id) = inode_id {
            let existing = self.read_inode(dev, existing_id)?;
            if matches!(existing.kind, KIND_DIR | KIND_HSAPP) {
                return Err(FsError::IsADir);
            }
            self.free_inode_extents(dev, &existing)?;
        }

        let start_blk = self.alloc_contiguous_blocks(dev, blocks_needed)?;

        // Write payload blocks directly to the freshly allocated region before journal commit.
        let mut blk = [0u8; DELTAFS_BLOCK_SIZE];
        for b in 0..blocks_needed as usize {
            let start_byte = b * DELTAFS_BLOCK_SIZE;
            let end_byte = (start_byte + DELTAFS_BLOCK_SIZE).min(data.len());
            blk.fill(0);
            if start_byte < data.len() {
                blk[..end_byte - start_byte].copy_from_slice(&data[start_byte..end_byte]);
            }
            write_block(dev, start_blk + b as u64, &blk)?;
        }

        let is_new = inode_id.is_none();
        if inode_id.is_none() {
            inode_id = Some(self.alloc_inode_slot(dev)?);
        }
        let id = inode_id.ok_or(FsError::NoSpace)?;

        let mut ino = DeltaInode::empty(id, KIND_FILE);
        ino.mode = 0o644;
        ino.size = data.len() as u64;
        ino.extent_count = 1;
        ino.start_blocks[0] = start_blk;
        ino.block_counts[0] = blocks_needed;
        if file_name.ends_with(".md") {
            set_attr(&mut ino.attr, b"text/markdown");
        } else if file_name.ends_with(".wav") {
            set_attr(&mut ino.attr, b"audio/wav");
        } else {
            set_attr(&mut ino.attr, b"application/octet-stream");
        }

        // Update inode block, directory entry (if new), and superblock inside a WAL transaction.
        let ino_blk_idx = self.inode_table_start + ((id - 1) / INODES_PER_BLOCK as u64);
        let mut ino_blk = [0u8; DELTAFS_BLOCK_SIZE];
        read_block(dev, ino_blk_idx, &mut ino_blk)?;
        ino.encode_into(
            &mut ino_blk,
            ((id - 1) as usize % INODES_PER_BLOCK) * INODE_SIZE,
        );

        let mut sb_blk = [0u8; DELTAFS_BLOCK_SIZE];
        self.journal_seq += 1;
        self.encode_superblock(&mut sb_blk);

        if is_new {
            let (dir_blk_idx, mut dir_blk) =
                self.insert_dir_entry_buf(dev, &parent, id, KIND_FILE, file_name)?;
            self.commit_journal_tx(
                dev,
                &[
                    (ino_blk_idx, &ino_blk),
                    (dir_blk_idx, &dir_blk),
                    (0, &sb_blk),
                ],
            )?;
            let _ = &mut dir_blk;
        } else {
            self.commit_journal_tx(dev, &[(ino_blk_idx, &ino_blk), (0, &sb_blk)])?;
        }

        Ok(data.len())
    }

    pub fn mkdir<B: BlockDevice>(&mut self, dev: &mut B, path: &str) -> Result<(), FsError> {
        let (parent_path, dir_name) = split_parent(path).ok_or(FsError::InvalidFs)?;
        if dir_name.is_empty() || dir_name.len() > DIRENT_MAX_NAME {
            return Err(FsError::InvalidFs);
        }
        let parent = self.resolve_path(dev, parent_path)?;
        if !matches!(parent.kind, KIND_DIR | KIND_HSAPP) {
            return Err(FsError::NotADir);
        }
        if self.find_in_dir(dev, &parent, dir_name)?.is_some() {
            return Ok(());
        }

        let dir_blk_num = self.alloc_contiguous_blocks(dev, 1)?;
        let empty_dir = [0u8; DELTAFS_BLOCK_SIZE];
        write_block(dev, dir_blk_num, &empty_dir)?;

        let id = self.alloc_inode_slot(dev)?;
        let kind = if dir_name.ends_with(".hsapp") {
            KIND_HSAPP
        } else {
            KIND_DIR
        };
        let mut ino = DeltaInode::empty(id, kind);
        ino.mode = 0o755;
        ino.extent_count = 1;
        ino.start_blocks[0] = dir_blk_num;
        ino.block_counts[0] = 1;
        if kind == KIND_HSAPP {
            set_attr(&mut ino.attr, b"application/x-hsapp");
        }

        let ino_blk_idx = self.inode_table_start + ((id - 1) / INODES_PER_BLOCK as u64);
        let mut ino_blk = [0u8; DELTAFS_BLOCK_SIZE];
        read_block(dev, ino_blk_idx, &mut ino_blk)?;
        ino.encode_into(
            &mut ino_blk,
            ((id - 1) as usize % INODES_PER_BLOCK) * INODE_SIZE,
        );

        let (parent_dir_blk_idx, parent_dir_blk) =
            self.insert_dir_entry_buf(dev, &parent, id, kind, dir_name)?;

        let mut sb_blk = [0u8; DELTAFS_BLOCK_SIZE];
        self.journal_seq += 1;
        self.encode_superblock(&mut sb_blk);

        self.commit_journal_tx(
            dev,
            &[
                (ino_blk_idx, &ino_blk),
                (parent_dir_blk_idx, &parent_dir_blk),
                (0, &sb_blk),
            ],
        )
    }

    pub fn remove<B: BlockDevice>(&mut self, dev: &mut B, path: &str) -> Result<(), FsError> {
        let (parent_path, file_name) = split_parent(path).ok_or(FsError::InvalidFs)?;
        let parent = self.resolve_path(dev, parent_path)?;
        if !matches!(parent.kind, KIND_DIR | KIND_HSAPP) {
            return Err(FsError::NotADir);
        }

        let mut dir_blk = [0u8; DELTAFS_BLOCK_SIZE];
        for ext_idx in 0..(parent.extent_count as usize).min(MAX_EXTENTS) {
            let start = parent.start_blocks[ext_idx];
            let count = parent.block_counts[ext_idx];
            for rel in 0..count {
                let blk_idx = start + u64::from(rel);
                read_block(dev, blk_idx, &mut dir_blk)?;
                for slot in 0..DIRENTS_PER_BLOCK {
                    let off = slot * DIRENT_SIZE;
                    let child_id = read_u64(&dir_blk, off);
                    if child_id == 0 {
                        continue;
                    }
                    let nlen = (dir_blk[off + 9] as usize).min(DIRENT_MAX_NAME);
                    if &dir_blk[off + 10..off + 10 + nlen] == file_name.as_bytes() {
                        let ino = self.read_inode(dev, child_id)?;
                        if matches!(ino.kind, KIND_DIR | KIND_HSAPP) {
                            return Err(FsError::IsADir);
                        }
                        self.free_inode_extents(dev, &ino)?;

                        dir_blk[off..off + DIRENT_SIZE].fill(0);
                        let ino_blk_idx =
                            self.inode_table_start + ((child_id - 1) / INODES_PER_BLOCK as u64);
                        let mut ino_blk = [0u8; DELTAFS_BLOCK_SIZE];
                        read_block(dev, ino_blk_idx, &mut ino_blk)?;
                        let ino_off = ((child_id - 1) as usize % INODES_PER_BLOCK) * INODE_SIZE;
                        ino_blk[ino_off..ino_off + INODE_SIZE].fill(0);

                        let mut sb_blk = [0u8; DELTAFS_BLOCK_SIZE];
                        self.journal_seq += 1;
                        self.encode_superblock(&mut sb_blk);
                        self.commit_journal_tx(
                            dev,
                            &[(blk_idx, &dir_blk), (ino_blk_idx, &ino_blk), (0, &sb_blk)],
                        )?;
                        return Ok(());
                    }
                }
            }
        }
        Err(FsError::NotFound)
    }

    /// Expands this DeltaFS volume cleanly to `new_total_blocks` (capped by pre-allocated bitmap capacity).
    pub fn expand_to_blocks<B: BlockDevice>(
        &mut self,
        dev: &mut B,
        new_total_blocks: u64,
    ) -> Result<u64, FsError> {
        let max_bitmap_blocks = self.bitmap_blocks * (DELTAFS_BLOCK_SIZE as u64) * 8;
        let target = new_total_blocks.min(max_bitmap_blocks);
        if target <= self.total_blocks {
            return Ok(self.total_blocks);
        }
        let added = target - self.total_blocks;
        self.total_blocks = target;
        self.free_blocks += added;
        self.journal_seq += 1;

        let mut sb_blk = [0u8; DELTAFS_BLOCK_SIZE];
        self.encode_superblock(&mut sb_blk);
        self.commit_journal_tx(dev, &[(0, &sb_blk)])?;
        Ok(self.total_blocks)
    }

    fn parse_superblock(blk: &[u8; DELTAFS_BLOCK_SIZE]) -> Result<Self, FsError> {
        if &blk[0..8] != DELTAFS_MAGIC {
            return Err(FsError::InvalidFs);
        }
        let version = read_u32(blk, 8);
        let block_size = read_u32(blk, 12);
        if version != 1 || block_size as usize != DELTAFS_BLOCK_SIZE {
            return Err(FsError::InvalidFs);
        }
        let stored_crc = read_u32(blk, 144);
        if crc32(&blk[0..144]) != stored_crc {
            return Err(FsError::InvalidFs);
        }

        let mut label = [0u8; 32];
        label.copy_from_slice(&blk[112..144]);

        Ok(Self {
            total_blocks: read_u64(blk, 16),
            free_blocks: read_u64(blk, 24),
            journal_start: read_u64(blk, 32),
            journal_blocks: read_u64(blk, 40),
            bitmap_start: read_u64(blk, 48),
            bitmap_blocks: read_u64(blk, 56),
            inode_table_start: read_u64(blk, 64),
            inode_table_blocks: read_u64(blk, 72),
            data_start: read_u64(blk, 80),
            inode_count: read_u64(blk, 88),
            journal_seq: read_u64(blk, 96),
            role: read_u32(blk, 104),
            flags: read_u32(blk, 108),
            label,
            journal_replayed: false,
        })
    }

    fn encode_superblock(&self, blk: &mut [u8; DELTAFS_BLOCK_SIZE]) {
        blk.fill(0);
        blk[0..8].copy_from_slice(DELTAFS_MAGIC);
        write_u32(blk, 8, 1);
        write_u32(blk, 12, DELTAFS_BLOCK_SIZE as u32);
        write_u64(blk, 16, self.total_blocks);
        write_u64(blk, 24, self.free_blocks);
        write_u64(blk, 32, self.journal_start);
        write_u64(blk, 40, self.journal_blocks);
        write_u64(blk, 48, self.bitmap_start);
        write_u64(blk, 56, self.bitmap_blocks);
        write_u64(blk, 64, self.inode_table_start);
        write_u64(blk, 72, self.inode_table_blocks);
        write_u64(blk, 80, self.data_start);
        write_u64(blk, 88, self.inode_count);
        write_u64(blk, 96, self.journal_seq);
        write_u32(blk, 104, self.role);
        write_u32(blk, 108, self.flags);
        blk[112..144].copy_from_slice(&self.label);
        let sum = crc32(&blk[0..144]);
        write_u32(blk, 144, sum);
    }

    fn replay_journal_if_needed<B: BlockDevice>(&mut self, dev: &mut B) -> Result<(), FsError> {
        if self.journal_blocks < 2 {
            return Ok(());
        }
        let mut hdr = [0u8; DELTAFS_BLOCK_SIZE];
        read_block(dev, self.journal_start, &mut hdr)?;
        if &hdr[0..4] != JOURNAL_MAGIC || read_u32(&hdr, 4) != JOURNAL_STATE_COMMITTED {
            return Ok(());
        }

        let count = (read_u32(&hdr, 16) as usize).min(JOURNAL_MAX_ENTRIES);
        let expected_crc = read_u32(&hdr, 20);
        let mut running_crc = crc32_update(0xFFFF_FFFF, &hdr[16..20]);
        running_crc = crc32_update(running_crc, &hdr[24..24 + count * 8]);

        let mut payload = [0u8; DELTAFS_BLOCK_SIZE];
        for i in 0..count {
            read_block(dev, self.journal_start + 1 + i as u64, &mut payload)?;
            running_crc = crc32_update(running_crc, &payload);
        }
        if (running_crc ^ 0xFFFF_FFFF) != expected_crc {
            return Ok(());
        }

        for i in 0..count {
            let target = read_u64(&hdr, 24 + i * 8);
            read_block(dev, self.journal_start + 1 + i as u64, &mut payload)?;
            write_block(dev, target, &payload)?;
        }

        // Clear journal commit marker after checkpointing.
        hdr[0..4].copy_from_slice(JOURNAL_MAGIC);
        write_u32(&mut hdr, 4, JOURNAL_STATE_FREE);
        write_block(dev, self.journal_start, &hdr)?;

        // Reload updated superblock if block 0 was replayed.
        read_block(dev, 0, &mut payload)?;
        let mut refreshed = Self::parse_superblock(&payload)?;
        refreshed.journal_replayed = true;
        *self = refreshed;
        Ok(())
    }

    fn commit_journal_tx<B: BlockDevice>(
        &self,
        dev: &mut B,
        entries: &[(u64, &[u8; DELTAFS_BLOCK_SIZE])],
    ) -> Result<(), FsError> {
        if entries.len() > JOURNAL_MAX_ENTRIES || self.journal_blocks < (entries.len() as u64 + 1) {
            return Err(FsError::Io);
        }

        let mut hdr = [0u8; DELTAFS_BLOCK_SIZE];
        hdr[0..4].copy_from_slice(JOURNAL_MAGIC);
        write_u32(&mut hdr, 4, JOURNAL_STATE_COMMITTED);
        write_u64(&mut hdr, 8, self.journal_seq);
        write_u32(&mut hdr, 16, entries.len() as u32);

        let mut running = crc32_update(0xFFFF_FFFF, &hdr[16..20]);
        for (i, (target, _)) in entries.iter().enumerate() {
            write_u64(&mut hdr, 24 + i * 8, *target);
        }
        running = crc32_update(running, &hdr[24..24 + entries.len() * 8]);

        for (i, (_, data)) in entries.iter().enumerate() {
            running = crc32_update(running, &data[..]);
            write_block(dev, self.journal_start + 1 + i as u64, data)?;
        }
        write_u32(&mut hdr, 20, running ^ 0xFFFF_FFFF);
        write_block(dev, self.journal_start, &hdr)?;

        // Checkpoint committed blocks into their target locations.
        for (target, data) in entries {
            write_block(dev, *target, data)?;
        }

        // Mark transaction checkpointed.
        write_u32(&mut hdr, 4, JOURNAL_STATE_FREE);
        write_block(dev, self.journal_start, &hdr)
    }

    fn read_inode<B: BlockDevice>(&self, dev: &mut B, id: u64) -> Result<DeltaInode, FsError> {
        let mut blk = [0u8; DELTAFS_BLOCK_SIZE];
        self.read_inode_with_buf(dev, id, &mut blk)
    }

    fn read_inode_with_buf<B: BlockDevice>(
        &self,
        dev: &mut B,
        id: u64,
        blk: &mut [u8; DELTAFS_BLOCK_SIZE],
    ) -> Result<DeltaInode, FsError> {
        if id == 0 || id > self.inode_count {
            return Err(FsError::NotFound);
        }
        let blk_idx = self.inode_table_start + ((id - 1) / INODES_PER_BLOCK as u64);
        let off = ((id - 1) as usize % INODES_PER_BLOCK) * INODE_SIZE;
        read_block(dev, blk_idx, blk)?;
        let ino = DeltaInode::decode(blk, off);
        if ino.kind == KIND_UNUSED {
            return Err(FsError::NotFound);
        }
        Ok(ino)
    }

    fn alloc_inode_slot<B: BlockDevice>(&self, dev: &mut B) -> Result<u64, FsError> {
        let mut blk = [0u8; DELTAFS_BLOCK_SIZE];
        for b in 0..self.inode_table_blocks {
            read_block(dev, self.inode_table_start + b, &mut blk)?;
            for slot in 0..INODES_PER_BLOCK {
                let id = b * (INODES_PER_BLOCK as u64) + slot as u64 + 1;
                if id > self.inode_count {
                    return Err(FsError::NoSpace);
                }
                let off = slot * INODE_SIZE;
                if blk[off + 8] == KIND_UNUSED {
                    return Ok(id);
                }
            }
        }
        Err(FsError::NoSpace)
    }

    fn alloc_contiguous_blocks<B: BlockDevice>(
        &mut self,
        dev: &mut B,
        needed: u32,
    ) -> Result<u64, FsError> {
        let mut bm = [0u8; DELTAFS_BLOCK_SIZE];
        let mut run_start = 0u64;
        let mut run_len = 0u32;
        let mut cur_bm_blk = u64::MAX;

        for b in self.data_start..self.total_blocks {
            let bm_blk = b / (DELTAFS_BLOCK_SIZE as u64 * 8);
            if bm_blk != cur_bm_blk {
                read_block(dev, self.bitmap_start + bm_blk, &mut bm)?;
                cur_bm_blk = bm_blk;
            }
            let bit_in_blk = (b as usize) % (DELTAFS_BLOCK_SIZE * 8);
            let is_used = (bm[bit_in_blk / 8] >> (bit_in_blk % 8)) & 1 != 0;
            if !is_used {
                if run_len == 0 {
                    run_start = b;
                }
                run_len += 1;
                if run_len == needed {
                    self.mark_bitmap_range(dev, run_start, needed, true)?;
                    self.free_blocks = self.free_blocks.saturating_sub(u64::from(needed));
                    return Ok(run_start);
                }
            } else {
                run_len = 0;
            }
        }
        Err(FsError::NoSpace)
    }

    fn free_inode_extents<B: BlockDevice>(
        &mut self,
        dev: &mut B,
        inode: &DeltaInode,
    ) -> Result<(), FsError> {
        for ext_idx in 0..(inode.extent_count as usize).min(MAX_EXTENTS) {
            let start = inode.start_blocks[ext_idx];
            let count = inode.block_counts[ext_idx];
            if count > 0 {
                self.mark_bitmap_range(dev, start, count, false)?;
                self.free_blocks = self.free_blocks.saturating_add(u64::from(count));
            }
        }
        Ok(())
    }

    fn mark_bitmap_range<B: BlockDevice>(
        &self,
        dev: &mut B,
        start_block: u64,
        count: u32,
        used: bool,
    ) -> Result<(), FsError> {
        let mut bm = [0u8; DELTAFS_BLOCK_SIZE];
        let mut loaded_blk = u64::MAX;

        for rel in 0..u64::from(count) {
            let b = start_block + rel;
            let bm_blk = b / (DELTAFS_BLOCK_SIZE as u64 * 8);
            if bm_blk >= self.bitmap_blocks {
                return Err(FsError::NoSpace);
            }
            if bm_blk != loaded_blk {
                if loaded_blk != u64::MAX {
                    write_block(dev, self.bitmap_start + loaded_blk, &bm)?;
                }
                read_block(dev, self.bitmap_start + bm_blk, &mut bm)?;
                loaded_blk = bm_blk;
            }
            let bit = (b as usize) % (DELTAFS_BLOCK_SIZE * 8);
            if used {
                bm[bit / 8] |= 1 << (bit % 8);
            } else {
                bm[bit / 8] &= !(1 << (bit % 8));
            }
        }
        if loaded_blk != u64::MAX {
            write_block(dev, self.bitmap_start + loaded_blk, &bm)?;
        }
        Ok(())
    }

    fn resolve_path<B: BlockDevice>(&self, dev: &mut B, path: &str) -> Result<DeltaInode, FsError> {
        let mut cur = self.read_inode(dev, 1)?;
        for part in path.split('/').filter(|s| !s.is_empty() && *s != ".") {
            if !matches!(cur.kind, KIND_DIR | KIND_HSAPP) {
                return Err(FsError::NotADir);
            }
            let next_id = self
                .find_in_dir(dev, &cur, part)?
                .ok_or(FsError::NotFound)?;
            cur = self.read_inode(dev, next_id)?;
        }
        Ok(cur)
    }

    fn find_in_dir<B: BlockDevice>(
        &self,
        dev: &mut B,
        dir_inode: &DeltaInode,
        name: &str,
    ) -> Result<Option<u64>, FsError> {
        let mut blk = [0u8; DELTAFS_BLOCK_SIZE];
        for ext_idx in 0..(dir_inode.extent_count as usize).min(MAX_EXTENTS) {
            let start = dir_inode.start_blocks[ext_idx];
            let count = dir_inode.block_counts[ext_idx];
            for rel in 0..count {
                read_block(dev, start + u64::from(rel), &mut blk)?;
                for slot in 0..DIRENTS_PER_BLOCK {
                    let off = slot * DIRENT_SIZE;
                    let child_id = read_u64(&blk, off);
                    if child_id == 0 {
                        continue;
                    }
                    let nlen = (blk[off + 9] as usize).min(DIRENT_MAX_NAME);
                    if &blk[off + 10..off + 10 + nlen] == name.as_bytes() {
                        return Ok(Some(child_id));
                    }
                }
            }
        }
        Ok(None)
    }

    fn insert_dir_entry_buf<B: BlockDevice>(
        &self,
        dev: &mut B,
        dir_inode: &DeltaInode,
        child_id: u64,
        kind: u8,
        name: &str,
    ) -> Result<(u64, [u8; DELTAFS_BLOCK_SIZE]), FsError> {
        let mut blk = [0u8; DELTAFS_BLOCK_SIZE];
        let name_bytes = name.as_bytes();
        let nlen = name_bytes.len().min(DIRENT_MAX_NAME);

        for ext_idx in 0..(dir_inode.extent_count as usize).min(MAX_EXTENTS) {
            let start = dir_inode.start_blocks[ext_idx];
            let count = dir_inode.block_counts[ext_idx];
            for rel in 0..count {
                let blk_idx = start + u64::from(rel);
                read_block(dev, blk_idx, &mut blk)?;
                for slot in 0..DIRENTS_PER_BLOCK {
                    let off = slot * DIRENT_SIZE;
                    if read_u64(&blk, off) == 0 {
                        blk[off..off + DIRENT_SIZE].fill(0);
                        write_u64(&mut blk, off, child_id);
                        blk[off + 8] = kind;
                        blk[off + 9] = nlen as u8;
                        blk[off + 10..off + 10 + nlen].copy_from_slice(&name_bytes[..nlen]);
                        return Ok((blk_idx, blk));
                    }
                }
            }
        }
        Err(FsError::NoSpace)
    }
}

impl DeltaInode {
    const fn empty(id: u64, kind: u8) -> Self {
        Self {
            id,
            kind,
            extent_count: 0,
            mode: 0,
            size: 0,
            start_blocks: [0; MAX_EXTENTS],
            block_counts: [0; MAX_EXTENTS],
            attr: [0; 32],
        }
    }

    fn decode(blk: &[u8; DELTAFS_BLOCK_SIZE], off: usize) -> Self {
        let mut start_blocks = [0u64; MAX_EXTENTS];
        let mut block_counts = [0u32; MAX_EXTENTS];
        for i in 0..MAX_EXTENTS {
            start_blocks[i] = read_u64(blk, off + 24 + i * 8);
            block_counts[i] = read_u32(blk, off + 72 + i * 4);
        }
        let mut attr = [0u8; 32];
        attr.copy_from_slice(&blk[off + 96..off + 128]);
        Self {
            id: read_u64(blk, off),
            kind: blk[off + 8],
            extent_count: blk[off + 9],
            mode: read_u32(blk, off + 12),
            size: read_u64(blk, off + 16),
            start_blocks,
            block_counts,
            attr,
        }
    }

    fn encode_into(&self, blk: &mut [u8; DELTAFS_BLOCK_SIZE], off: usize) {
        blk[off..off + INODE_SIZE].fill(0);
        write_u64(blk, off, self.id);
        blk[off + 8] = self.kind;
        blk[off + 9] = self.extent_count;
        write_u32(blk, off + 12, self.mode);
        write_u64(blk, off + 16, self.size);
        for i in 0..MAX_EXTENTS {
            write_u64(blk, off + 24 + i * 8, self.start_blocks[i]);
            write_u32(blk, off + 72 + i * 4, self.block_counts[i]);
        }
        blk[off + 96..off + 128].copy_from_slice(&self.attr);
    }
}

fn read_block<B: BlockDevice>(
    dev: &mut B,
    block_idx: u64,
    out: &mut [u8; DELTAFS_BLOCK_SIZE],
) -> Result<(), FsError> {
    dev.read_sectors(block_idx * SECTORS_PER_BLOCK, out)
}

fn write_block<B: BlockDevice>(
    dev: &mut B,
    block_idx: u64,
    data: &[u8; DELTAFS_BLOCK_SIZE],
) -> Result<(), FsError> {
    dev.write_sectors(block_idx * SECTORS_PER_BLOCK, data)
}

fn split_parent(path: &str) -> Option<(&str, &str)> {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.rsplit_once('/') {
        Some((p, n)) => Some((p, n)),
        None => Some(("", trimmed)),
    }
}

fn set_attr(dst: &mut [u8; 32], val: &[u8]) {
    let n = val.len().min(dst.len());
    dst.fill(0);
    dst[..n].copy_from_slice(&val[..n]);
}

fn read_u32(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

fn write_u32(buf: &mut [u8], off: usize, val: u32) {
    buf[off..off + 4].copy_from_slice(&val.to_le_bytes());
}

fn read_u64(buf: &[u8], off: usize) -> u64 {
    u64::from_le_bytes([
        buf[off],
        buf[off + 1],
        buf[off + 2],
        buf[off + 3],
        buf[off + 4],
        buf[off + 5],
        buf[off + 6],
        buf[off + 7],
    ])
}

fn write_u64(buf: &mut [u8], off: usize, val: u64) {
    buf[off..off + 8].copy_from_slice(&val.to_le_bytes());
}

fn copy_str(out: &mut [u8], s: &str) -> usize {
    let n = s.len().min(out.len());
    out[..n].copy_from_slice(&s.as_bytes()[..n]);
    n
}

fn write_dec(out: &mut [u8], mut val: u32) -> usize {
    let mut digits = [0u8; 10];
    let mut i = digits.len();
    if val == 0 {
        return copy_str(out, "0");
    }
    while val > 0 && i > 0 {
        i -= 1;
        digits[i] = b'0' + (val % 10) as u8;
        val /= 10;
    }
    let slice = &digits[i..];
    let n = slice.len().min(out.len());
    out[..n].copy_from_slice(&slice[..n]);
    n
}

pub fn crc32(data: &[u8]) -> u32 {
    crc32_update(0xFFFF_FFFF, data) ^ 0xFFFF_FFFF
}

fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    crc
}

//! Read/Write FAT32 filesystem driver with VFAT Long File Name (LFN) support.

use crate::block::{BlockDevice, FsError, SECTOR_SIZE};

const ATTR_READ_ONLY: u8 = 0x01;
const ATTR_DIRECTORY: u8 = 0x10;
const ATTR_ARCHIVE: u8 = 0x20;
const ATTR_LFN: u8 = 0x0F;

const FAT32_EOC: u32 = 0x0FFF_FFF8;
const FAT32_MASK: u32 = 0x0FFF_FFFF;

/// Parsed FAT32 volume parameters.
#[derive(Debug, Clone, Copy)]
pub struct Fat32Volume {
    pub sectors_per_cluster: u32,
    pub reserved_sectors: u32,
    pub num_fats: u32,
    pub sectors_per_fat: u32,
    pub root_cluster: u32,
    pub data_start_lba: u64,
    pub total_clusters: u32,
}

#[derive(Clone, Copy)]
struct DirEntryInfo {
    name: [u8; 64],
    name_len: usize,
    is_dir: bool,
    size: u32,
    first_cluster: u32,
    entry_lba: u64,
    entry_offset: usize,
}

impl DirEntryInfo {
    fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("")
    }
}

impl Fat32Volume {
    /// Mounts a FAT32 filesystem from sector 0 of `dev`.
    pub fn mount<B: BlockDevice>(dev: &mut B) -> Result<Self, FsError> {
        let mut bpb = [0u8; SECTOR_SIZE];
        dev.read_sectors(0, &mut bpb)?;
        if bpb[510] != 0x55 || bpb[511] != 0xAA {
            return Err(FsError::InvalidFs);
        }

        let bytes_per_sector = u16::from_le_bytes([bpb[11], bpb[12]]);
        let sectors_per_cluster = u32::from(bpb[13]);
        let reserved_sectors = u32::from(u16::from_le_bytes([bpb[14], bpb[15]]));
        let num_fats = u32::from(bpb[16]);
        let total_sectors_16 = u32::from(u16::from_le_bytes([bpb[19], bpb[20]]));
        let total_sectors_32 = u32::from_le_bytes([bpb[32], bpb[33], bpb[34], bpb[35]]);
        let sectors_per_fat = u32::from_le_bytes([bpb[36], bpb[37], bpb[38], bpb[39]]);
        let root_cluster = u32::from_le_bytes([bpb[44], bpb[45], bpb[46], bpb[47]]);

        if bytes_per_sector != SECTOR_SIZE as u16
            || sectors_per_cluster == 0
            || num_fats == 0
            || sectors_per_fat == 0
            || root_cluster < 2
        {
            return Err(FsError::InvalidFs);
        }

        let total_sectors = if total_sectors_16 != 0 {
            total_sectors_16
        } else {
            total_sectors_32
        };
        let data_start = reserved_sectors + num_fats * sectors_per_fat;
        if total_sectors <= data_start {
            return Err(FsError::InvalidFs);
        }
        let total_clusters = (total_sectors - data_start) / sectors_per_cluster;

        Ok(Self {
            sectors_per_cluster,
            reserved_sectors,
            num_fats,
            sectors_per_fat,
            root_cluster,
            data_start_lba: u64::from(data_start),
            total_clusters,
        })
    }

    fn cluster_to_lba(&self, cluster: u32) -> u64 {
        self.data_start_lba
            + u64::from(cluster.saturating_sub(2)) * u64::from(self.sectors_per_cluster)
    }

    fn read_fat_entry<B: BlockDevice>(&self, dev: &mut B, cluster: u32) -> Result<u32, FsError> {
        let fat_offset = u64::from(cluster) * 4;
        let fat_sector = u64::from(self.reserved_sectors) + fat_offset / (SECTOR_SIZE as u64);
        let ent_off = (fat_offset % (SECTOR_SIZE as u64)) as usize;
        let mut sec = [0u8; SECTOR_SIZE];
        dev.read_sectors(fat_sector, &mut sec)?;
        let raw = u32::from_le_bytes([
            sec[ent_off],
            sec[ent_off + 1],
            sec[ent_off + 2],
            sec[ent_off + 3],
        ]);
        Ok(raw & FAT32_MASK)
    }

    fn write_fat_entry<B: BlockDevice>(
        &self,
        dev: &mut B,
        cluster: u32,
        value: u32,
    ) -> Result<(), FsError> {
        let fat_offset = u64::from(cluster) * 4;
        let rel_sector = fat_offset / (SECTOR_SIZE as u64);
        let ent_off = (fat_offset % (SECTOR_SIZE as u64)) as usize;

        for fat_idx in 0..self.num_fats {
            let fat_sector =
                u64::from(self.reserved_sectors + fat_idx * self.sectors_per_fat) + rel_sector;
            let mut sec = [0u8; SECTOR_SIZE];
            dev.read_sectors(fat_sector, &mut sec)?;
            let old = u32::from_le_bytes([
                sec[ent_off],
                sec[ent_off + 1],
                sec[ent_off + 2],
                sec[ent_off + 3],
            ]);
            let merged = (old & !FAT32_MASK) | (value & FAT32_MASK);
            sec[ent_off..ent_off + 4].copy_from_slice(&merged.to_le_bytes());
            dev.write_sectors(fat_sector, &sec)?;
        }
        Ok(())
    }

    fn alloc_cluster<B: BlockDevice>(
        &self,
        dev: &mut B,
        prev: Option<u32>,
    ) -> Result<u32, FsError> {
        let max_cluster = self.total_clusters + 2;
        let mut sec = [0u8; SECTOR_SIZE];
        let mut loaded_sec = u64::MAX;

        for c in 2..max_cluster {
            let fat_offset = u64::from(c) * 4;
            let fat_sector = u64::from(self.reserved_sectors) + fat_offset / (SECTOR_SIZE as u64);
            let ent_off = (fat_offset % (SECTOR_SIZE as u64)) as usize;
            if fat_sector != loaded_sec {
                dev.read_sectors(fat_sector, &mut sec)?;
                loaded_sec = fat_sector;
            }
            let val = u32::from_le_bytes([
                sec[ent_off],
                sec[ent_off + 1],
                sec[ent_off + 2],
                sec[ent_off + 3],
            ]) & FAT32_MASK;
            if val == 0 {
                self.write_fat_entry(dev, c, 0x0FFF_FFFF)?;
                if let Some(p) = prev {
                    self.write_fat_entry(dev, p, c)?;
                }
                // Zero-initialize the newly allocated cluster.
                let zero = [0u8; SECTOR_SIZE];
                let base_lba = self.cluster_to_lba(c);
                for s in 0..self.sectors_per_cluster {
                    dev.write_sectors(base_lba + u64::from(s), &zero)?;
                }
                return Ok(c);
            }
        }
        Err(FsError::NoSpace)
    }

    fn free_chain<B: BlockDevice>(&self, dev: &mut B, start: u32) -> Result<(), FsError> {
        if start < 2 {
            return Ok(());
        }
        let mut cur = start;
        for _ in 0..=self.total_clusters {
            let next = self.read_fat_entry(dev, cur)?;
            self.write_fat_entry(dev, cur, 0)?;
            if !(2..FAT32_EOC).contains(&next) {
                return Ok(());
            }
            cur = next;
        }
        Ok(())
    }

    fn walk_dir<B: BlockDevice, F>(
        &self,
        dev: &mut B,
        dir_cluster: u32,
        mut visitor: F,
    ) -> Result<(), FsError>
    where
        F: FnMut(&DirEntryInfo) -> bool,
    {
        let mut cluster = dir_cluster;
        let mut sec = [0u8; SECTOR_SIZE];
        let mut lfn_buf = [0u8; 64];
        let mut lfn_len = 0usize;

        while (2..FAT32_EOC).contains(&cluster) {
            let base_lba = self.cluster_to_lba(cluster);
            for s in 0..self.sectors_per_cluster {
                let lba = base_lba + u64::from(s);
                dev.read_sectors(lba, &mut sec)?;
                for off in (0..SECTOR_SIZE).step_by(32) {
                    let first = sec[off];
                    if first == 0x00 {
                        return Ok(());
                    }
                    if first == 0xE5 {
                        lfn_len = 0;
                        continue;
                    }
                    let attr = sec[off + 11];
                    if attr == ATTR_LFN {
                        let order = sec[off];
                        if (order & 0x40) != 0 {
                            lfn_buf.fill(0);
                            lfn_len = 0;
                        }
                        let seq = usize::from((order & 0x1F).saturating_sub(1));
                        lfn_len =
                            lfn_len.max(decode_lfn_chunk(&sec[off..off + 32], seq, &mut lfn_buf));
                        continue;
                    }
                    if (attr & 0x08) != 0 {
                        // Volume ID entry
                        lfn_len = 0;
                        continue;
                    }

                    let mut entry = DirEntryInfo {
                        name: [0; 64],
                        name_len: 0,
                        is_dir: (attr & ATTR_DIRECTORY) != 0,
                        size: u32::from_le_bytes([
                            sec[off + 28],
                            sec[off + 29],
                            sec[off + 30],
                            sec[off + 31],
                        ]),
                        first_cluster: (u32::from(u16::from_le_bytes([
                            sec[off + 20],
                            sec[off + 21],
                        ])) << 16)
                            | u32::from(u16::from_le_bytes([sec[off + 26], sec[off + 27]])),
                        entry_lba: lba,
                        entry_offset: off,
                    };

                    if lfn_len > 0 {
                        entry.name[..lfn_len].copy_from_slice(&lfn_buf[..lfn_len]);
                        entry.name_len = lfn_len;
                    } else {
                        entry.name_len = decode_short_name(&sec[off..off + 11], &mut entry.name);
                    }
                    lfn_len = 0;

                    if entry.name_str() == "." || entry.name_str() == ".." {
                        continue;
                    }
                    if !visitor(&entry) {
                        return Ok(());
                    }
                }
            }
            cluster = self.read_fat_entry(dev, cluster)?;
        }
        Ok(())
    }

    fn resolve_path<B: BlockDevice>(
        &self,
        dev: &mut B,
        path: &str,
    ) -> Result<(Option<DirEntryInfo>, u32), FsError> {
        let trimmed = path.trim_matches('/');
        if trimmed.is_empty() {
            return Ok((None, self.root_cluster));
        }

        let mut cur_cluster = self.root_cluster;
        let mut last_entry: Option<DirEntryInfo> = None;

        for part in trimmed.split('/') {
            if part.is_empty() {
                continue;
            }
            if let Some(prev) = last_entry
                && !prev.is_dir
            {
                return Err(FsError::NotADir);
            }
            let mut found: Option<DirEntryInfo> = None;
            self.walk_dir(dev, cur_cluster, |ent| {
                if eq_ignore_ascii_case(ent.name_str(), part) {
                    found = Some(*ent);
                    return false;
                }
                true
            })?;
            let entry = found.ok_or(FsError::NotFound)?;
            cur_cluster = if entry.first_cluster >= 2 {
                entry.first_cluster
            } else {
                self.root_cluster
            };
            last_entry = Some(entry);
        }

        Ok((last_entry, cur_cluster))
    }

    /// Returns `(file_size, is_dir)` for `path`.
    pub fn stat<B: BlockDevice>(&self, dev: &mut B, path: &str) -> Result<(u32, bool), FsError> {
        let (entry_opt, _) = self.resolve_path(dev, path)?;
        match entry_opt {
            None => Ok((0, true)),
            Some(ent) => Ok((ent.size, ent.is_dir)),
        }
    }

    /// Formats a human-readable directory listing for `path` into `out`.
    pub fn read_dir<B: BlockDevice>(
        &self,
        dev: &mut B,
        path: &str,
        out: &mut [u8],
    ) -> Result<usize, FsError> {
        let (entry_opt, dir_cluster) = self.resolve_path(dev, path)?;
        if let Some(ent) = entry_opt
            && !ent.is_dir
        {
            return Err(FsError::NotADir);
        }

        let mut pos = 0usize;
        self.walk_dir(dev, dir_cluster, |ent| {
            let prefix = if ent.is_dir { b"[DIR]  " } else { b"[FILE] " };
            pos = append_bytes(out, pos, prefix);
            pos = append_bytes(out, pos, ent.name_str().as_bytes());
            if ent.is_dir {
                pos = append_bytes(out, pos, b"/\n");
            } else {
                pos = append_bytes(out, pos, b" (");
                pos = append_u32(out, pos, ent.size);
                pos = append_bytes(out, pos, b" B)\n");
            }
            pos < out.len()
        })?;
        Ok(pos)
    }

    /// Reads the entire contents of `path` into `out` and returns bytes read.
    pub fn read_file<B: BlockDevice>(
        &self,
        dev: &mut B,
        path: &str,
        out: &mut [u8],
    ) -> Result<usize, FsError> {
        let (entry_opt, _) = self.resolve_path(dev, path)?;
        let ent = entry_opt.ok_or(FsError::IsADir)?;
        if ent.is_dir {
            return Err(FsError::IsADir);
        }
        let target_len = (ent.size as usize).min(out.len());
        if target_len == 0 || ent.first_cluster < 2 {
            return Ok(0);
        }

        let mut copied = 0usize;
        let mut cluster = ent.first_cluster;
        let mut sec = [0u8; SECTOR_SIZE];

        while copied < target_len && (2..FAT32_EOC).contains(&cluster) {
            let base_lba = self.cluster_to_lba(cluster);
            for s in 0..self.sectors_per_cluster {
                if copied >= target_len {
                    break;
                }
                dev.read_sectors(base_lba + u64::from(s), &mut sec)?;
                let chunk = (target_len - copied).min(SECTOR_SIZE);
                out[copied..copied + chunk].copy_from_slice(&sec[..chunk]);
                copied += chunk;
            }
            cluster = self.read_fat_entry(dev, cluster)?;
        }

        Ok(copied)
    }

    /// Creates or overwrites a file in the root or existing parent directory on FAT32.
    pub fn write_file<B: BlockDevice>(
        &self,
        dev: &mut B,
        path: &str,
        data: &[u8],
    ) -> Result<usize, FsError> {
        let trimmed = path.trim_matches('/');
        if trimmed.is_empty() {
            return Err(FsError::IsADir);
        }

        let (parent_path, file_name) = match trimmed.rsplit_once('/') {
            Some((p, f)) => (p, f),
            None => ("", trimmed),
        };
        if file_name.is_empty() {
            return Err(FsError::IsADir);
        }

        let (parent_ent, parent_cluster) = self.resolve_path(dev, parent_path)?;
        if let Some(pe) = parent_ent
            && !pe.is_dir
        {
            return Err(FsError::NotADir);
        }

        let mut existing: Option<DirEntryInfo> = None;
        self.walk_dir(dev, parent_cluster, |ent| {
            if eq_ignore_ascii_case(ent.name_str(), file_name) {
                existing = Some(*ent);
                return false;
            }
            true
        })?;

        if let Some(ent) = existing {
            if ent.is_dir {
                return Err(FsError::IsADir);
            }
            self.free_chain(dev, ent.first_cluster)?;
        }

        let first_cluster = self.write_data_chain(dev, data)?;

        if let Some(ent) = existing {
            self.update_dir_entry(
                dev,
                ent.entry_lba,
                ent.entry_offset,
                first_cluster,
                data.len() as u32,
            )?;
        } else {
            let short_83 = encode_short_83(file_name)?;
            self.create_dir_entry(
                dev,
                parent_cluster,
                &short_83,
                first_cluster,
                data.len() as u32,
            )?;
        }

        Ok(data.len())
    }

    fn write_data_chain<B: BlockDevice>(&self, dev: &mut B, data: &[u8]) -> Result<u32, FsError> {
        if data.is_empty() {
            return Ok(0);
        }
        let cluster_bytes = (self.sectors_per_cluster as usize) * SECTOR_SIZE;
        let clusters_needed = data.len().div_ceil(cluster_bytes);

        let mut first_cluster = 0u32;
        let mut prev_cluster: Option<u32> = None;
        let mut offset = 0usize;
        let mut sec = [0u8; SECTOR_SIZE];

        for _ in 0..clusters_needed {
            let c = self.alloc_cluster(dev, prev_cluster)?;
            if first_cluster == 0 {
                first_cluster = c;
            }
            let base_lba = self.cluster_to_lba(c);
            for s in 0..self.sectors_per_cluster {
                if offset >= data.len() {
                    break;
                }
                sec.fill(0);
                let chunk = (data.len() - offset).min(SECTOR_SIZE);
                sec[..chunk].copy_from_slice(&data[offset..offset + chunk]);
                dev.write_sectors(base_lba + u64::from(s), &sec)?;
                offset += chunk;
            }
            prev_cluster = Some(c);
        }

        Ok(first_cluster)
    }

    fn update_dir_entry<B: BlockDevice>(
        &self,
        dev: &mut B,
        entry_lba: u64,
        off: usize,
        first_cluster: u32,
        size: u32,
    ) -> Result<(), FsError> {
        let mut sec = [0u8; SECTOR_SIZE];
        dev.read_sectors(entry_lba, &mut sec)?;
        sec[off + 11] = ATTR_ARCHIVE;
        sec[off + 20..off + 22].copy_from_slice(&((first_cluster >> 16) as u16).to_le_bytes());
        sec[off + 26..off + 28].copy_from_slice(&(first_cluster as u16).to_le_bytes());
        sec[off + 28..off + 32].copy_from_slice(&size.to_le_bytes());
        dev.write_sectors(entry_lba, &sec)
    }

    fn create_dir_entry<B: BlockDevice>(
        &self,
        dev: &mut B,
        dir_cluster: u32,
        short_83: &[u8; 11],
        first_cluster: u32,
        size: u32,
    ) -> Result<(), FsError> {
        let mut cluster = dir_cluster;
        let mut sec = [0u8; SECTOR_SIZE];

        loop {
            let base_lba = self.cluster_to_lba(cluster);
            for s in 0..self.sectors_per_cluster {
                let lba = base_lba + u64::from(s);
                dev.read_sectors(lba, &mut sec)?;
                for off in (0..SECTOR_SIZE).step_by(32) {
                    if sec[off] == 0x00 || sec[off] == 0xE5 {
                        sec[off..off + 32].fill(0);
                        sec[off..off + 11].copy_from_slice(short_83);
                        sec[off + 11] = ATTR_ARCHIVE | ATTR_READ_ONLY;
                        sec[off + 11] = ATTR_ARCHIVE;
                        sec[off + 20..off + 22]
                            .copy_from_slice(&((first_cluster >> 16) as u16).to_le_bytes());
                        sec[off + 26..off + 28]
                            .copy_from_slice(&(first_cluster as u16).to_le_bytes());
                        sec[off + 28..off + 32].copy_from_slice(&size.to_le_bytes());
                        return dev.write_sectors(lba, &sec);
                    }
                }
            }

            let next = self.read_fat_entry(dev, cluster)?;
            if next >= FAT32_EOC {
                cluster = self.alloc_cluster(dev, Some(cluster))?;
            } else if next < 2 {
                return Err(FsError::InvalidFs);
            } else {
                cluster = next;
            }
        }
    }
}

fn decode_short_name(raw: &[u8], out: &mut [u8; 64]) -> usize {
    let name_part = trim_right_spaces(&raw[0..8]);
    let ext_part = trim_right_spaces(&raw[8..11]);
    let mut len = 0usize;
    for &b in name_part {
        if len < out.len() {
            out[len] = b;
            len += 1;
        }
    }
    if !ext_part.is_empty() && len < out.len() {
        out[len] = b'.';
        len += 1;
        for &b in ext_part {
            if len < out.len() {
                out[len] = b;
                len += 1;
            }
        }
    }
    len
}

fn decode_lfn_chunk(raw: &[u8], seq: usize, out: &mut [u8; 64]) -> usize {
    const OFFSETS: [usize; 13] = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
    let base = seq * 13;
    let mut max_len = 0usize;

    for (idx, &byte_off) in OFFSETS.iter().enumerate() {
        let ch = u16::from_le_bytes([raw[byte_off], raw[byte_off + 1]]);
        if ch == 0x0000 || ch == 0xFFFF {
            break;
        }
        let pos = base + idx;
        if pos < out.len() {
            out[pos] = if ch < 0x80 { ch as u8 } else { b'?' };
            max_len = pos + 1;
        }
    }
    max_len
}

fn encode_short_83(name: &str) -> Result<[u8; 11], FsError> {
    let mut out = [b' '; 11];
    let (base, ext) = match name.rsplit_once('.') {
        Some((b, e)) => (b, e),
        None => (name, ""),
    };
    if base.is_empty() || base.len() > 8 || ext.len() > 3 {
        return Err(FsError::NameTooLong);
    }
    for (i, b) in base.bytes().enumerate() {
        out[i] = b.to_ascii_uppercase();
    }
    for (i, b) in ext.bytes().enumerate() {
        out[8 + i] = b.to_ascii_uppercase();
    }
    Ok(out)
}

fn trim_right_spaces(mut s: &[u8]) -> &[u8] {
    while let Some(&b' ') = s.last() {
        s = &s[..s.len() - 1];
    }
    s
}

fn eq_ignore_ascii_case(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn append_bytes(out: &mut [u8], pos: usize, data: &[u8]) -> usize {
    let avail = out.len().saturating_sub(pos);
    let n = data.len().min(avail);
    out[pos..pos + n].copy_from_slice(&data[..n]);
    pos + n
}

fn append_u32(out: &mut [u8], pos: usize, mut val: u32) -> usize {
    let mut tmp = [0u8; 10];
    let mut len = 0usize;
    if val == 0 {
        return append_bytes(out, pos, b"0");
    }
    while val > 0 {
        tmp[len] = b'0' + (val % 10) as u8;
        val /= 10;
        len += 1;
    }
    let mut p = pos;
    for i in (0..len).rev() {
        p = append_bytes(out, p, &tmp[i..i + 1]);
    }
    p
}

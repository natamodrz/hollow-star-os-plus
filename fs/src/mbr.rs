//! MBR partition table parser.

use crate::block::{BlockDevice, FsError, SECTOR_SIZE};

/// Parsed MBR partition descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MbrPartition {
    pub part_type: u8,
    pub start_lba: u64,
    pub sector_count: u64,
}

impl MbrPartition {
    pub const fn is_fat32(self) -> bool {
        matches!(self.part_type, 0x0B | 0x0C)
    }

    pub const fn is_linux_ext(self) -> bool {
        self.part_type == 0x83
    }
}

/// Reads sector 0 and parses up to 4 primary MBR partition entries.
pub fn read_mbr<B: BlockDevice>(dev: &mut B) -> Result<[Option<MbrPartition>; 4], FsError> {
    let mut sec = [0u8; SECTOR_SIZE];
    dev.read_sectors(0, &mut sec)?;
    if sec[510] != 0x55 || sec[511] != 0xAA {
        return Err(FsError::InvalidFs);
    }

    let mut out = [None; 4];
    for (idx, slot) in out.iter_mut().enumerate() {
        let off = 446 + idx * 16;
        let part_type = sec[off + 4];
        let start_lba =
            u32::from_le_bytes([sec[off + 8], sec[off + 9], sec[off + 10], sec[off + 11]]);
        let sector_count =
            u32::from_le_bytes([sec[off + 12], sec[off + 13], sec[off + 14], sec[off + 15]]);
        if part_type != 0 && sector_count > 0 {
            *slot = Some(MbrPartition {
                part_type,
                start_lba: u64::from(start_lba),
                sector_count: u64::from(sector_count),
            });
        }
    }
    Ok(out)
}

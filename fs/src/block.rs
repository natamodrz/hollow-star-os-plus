//! Block device trait, errors, and partition-scoped wrapper.

/// Sector size assumed by SD/MBR/FAT32 drivers.
pub const SECTOR_SIZE: usize = 512;

/// Filesystem and VFS error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsError {
    Io,
    NotFound,
    NotADir,
    IsADir,
    InvalidFs,
    NoSpace,
    NameTooLong,
    BufferTooSmall,
    ReadOnly,
}

impl FsError {
    /// Converts [`FsError`] into a negative Delta ABI errno.
    pub const fn to_errno(self) -> i32 {
        match self {
            Self::Io | Self::InvalidFs => -5,
            Self::NotFound => -2,
            Self::NotADir | Self::NameTooLong | Self::BufferTooSmall => -22,
            Self::IsADir => -21,
            Self::NoSpace => -28,
            Self::ReadOnly => -30,
        }
    }
}

/// Abstract 512-byte sector block device.
pub trait BlockDevice {
    fn read_sectors(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), FsError>;
    fn write_sectors(&mut self, lba: u64, buf: &[u8]) -> Result<(), FsError>;
}

/// Sub-window view over a single MBR partition.
pub struct PartitionSlice<'a, B: BlockDevice> {
    dev: &'a mut B,
    start_lba: u64,
    sector_count: u64,
}

impl<'a, B: BlockDevice> PartitionSlice<'a, B> {
    pub fn new(dev: &'a mut B, start_lba: u64, sector_count: u64) -> Self {
        Self {
            dev,
            start_lba,
            sector_count,
        }
    }

    fn check_bounds(&self, rel_lba: u64, byte_len: usize) -> Result<u64, FsError> {
        let sectors = byte_len.div_ceil(SECTOR_SIZE) as u64;
        if rel_lba.saturating_add(sectors) > self.sector_count {
            return Err(FsError::Io);
        }
        Ok(self.start_lba + rel_lba)
    }
}

impl<B: BlockDevice> BlockDevice for PartitionSlice<'_, B> {
    fn read_sectors(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
        let abs = self.check_bounds(lba, buf.len())?;
        self.dev.read_sectors(abs, buf)
    }

    fn write_sectors(&mut self, lba: u64, buf: &[u8]) -> Result<(), FsError> {
        let abs = self.check_bounds(lba, buf.len())?;
        self.dev.write_sectors(abs, buf)
    }
}

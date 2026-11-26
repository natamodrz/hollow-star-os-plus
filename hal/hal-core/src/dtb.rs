//! Minimal zero-allocation Flattened Device Tree (FDT / DTB) parser.
//!
//! Used during early boot to inspect firmware-supplied hardware metadata
//! (board model, physical memory ranges, and compatible device registers)
//! without requiring dynamic allocation.

use core::slice;
use core::str;

const FDT_MAGIC: u32 = 0xd00d_feed;
const FDT_HEADER_SIZE: usize = 40;
const MAX_DTB_SIZE: usize = 16 * 1024 * 1024;

const FDT_BEGIN_NODE: u32 = 0x0000_0001;
const FDT_END_NODE: u32 = 0x0000_0002;
const FDT_PROP: u32 = 0x0000_0003;
const FDT_NOP: u32 = 0x0000_0004;
const FDT_END: u32 = 0x0000_0009;

/// Errors returned when validating or parsing a Device Tree Blob.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DtbError {
    /// Raw pointer was null or not 4-byte aligned.
    InvalidPointer,
    /// Slice is smaller than the 40-byte FDT header.
    TruncatedHeader,
    /// Header magic does not equal `0xd00dfeed`.
    InvalidMagic(u32),
    /// Declared `totalsize` is out of bounds or exceeds [`MAX_DTB_SIZE`].
    InvalidTotalSize(u32),
    /// Structure or strings block offsets extend past `totalsize`.
    InvalidBlockBounds,
    /// Encountered an unexpected token or malformed entry in the structure block.
    MalformedStructure,
}

/// Parsed Flattened Device Tree header (all fields converted from big-endian).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FdtHeader {
    pub magic: u32,
    pub totalsize: u32,
    pub off_dt_struct: u32,
    pub off_dt_strings: u32,
    pub off_mem_rsvmap: u32,
    pub version: u32,
    pub last_comp_version: u32,
    pub boot_cpuid_phys: u32,
    pub size_dt_strings: u32,
    pub size_dt_struct: u32,
}

impl FdtHeader {
    fn parse(bytes: &[u8]) -> Result<Self, DtbError> {
        if bytes.len() < FDT_HEADER_SIZE {
            return Err(DtbError::TruncatedHeader);
        }

        let magic = read_be_u32(bytes, 0)?;
        if magic != FDT_MAGIC {
            return Err(DtbError::InvalidMagic(magic));
        }

        let totalsize = read_be_u32(bytes, 4)?;
        if (totalsize as usize) < FDT_HEADER_SIZE || (totalsize as usize) > MAX_DTB_SIZE {
            return Err(DtbError::InvalidTotalSize(totalsize));
        }

        let header = Self {
            magic,
            totalsize,
            off_dt_struct: read_be_u32(bytes, 8)?,
            off_dt_strings: read_be_u32(bytes, 12)?,
            off_mem_rsvmap: read_be_u32(bytes, 16)?,
            version: read_be_u32(bytes, 20)?,
            last_comp_version: read_be_u32(bytes, 24)?,
            boot_cpuid_phys: read_be_u32(bytes, 28)?,
            size_dt_strings: read_be_u32(bytes, 32)?,
            size_dt_struct: read_be_u32(bytes, 36)?,
        };

        header.validate_bounds()?;
        Ok(header)
    }

    fn validate_bounds(&self) -> Result<(), DtbError> {
        let total = self.totalsize as usize;
        let struct_end = (self.off_dt_struct as usize).checked_add(self.size_dt_struct as usize);
        let strings_end = (self.off_dt_strings as usize).checked_add(self.size_dt_strings as usize);

        let struct_ok = matches!(struct_end, Some(end) if end <= total)
            && (self.off_dt_struct as usize) >= FDT_HEADER_SIZE
            && (self.off_dt_struct as usize).is_multiple_of(4);
        let strings_ok = matches!(strings_end, Some(end) if end <= total)
            && (self.off_dt_strings as usize) >= FDT_HEADER_SIZE;

        if !struct_ok || !strings_ok {
            return Err(DtbError::InvalidBlockBounds);
        }

        Ok(())
    }
}

/// Borrowed view over a validated Flattened Device Tree blob.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dtb<'a> {
    data: &'a [u8],
    header: FdtHeader,
}

impl<'a> Dtb<'a> {
    /// Parses and validates a DTB from a raw physical address (e.g. `x0` at boot).
    ///
    /// # Safety
    ///
    /// If `ptr` is non-null, 4-byte aligned, and begins with a valid FDT header,
    /// the caller must guarantee that the memory range `[ptr, ptr + totalsize)`
    /// is readable, identity-mapped, and immutable for `'a`.
    pub unsafe fn from_raw_ptr(ptr: usize) -> Result<Self, DtbError> {
        if ptr == 0 || !ptr.is_multiple_of(4) {
            return Err(DtbError::InvalidPointer);
        }

        // SAFETY: Caller guarantees at least 40 readable bytes at a valid aligned non-null ptr
        // when firmware passes a DTB address; we validate magic and totalsize before sizing the full slice.
        let hdr_bytes = unsafe { slice::from_raw_parts(ptr as *const u8, FDT_HEADER_SIZE) };
        let header = FdtHeader::parse(hdr_bytes)?;
        let total = header.totalsize as usize;

        // SAFETY: Header validation proved `total` is within [40, MAX_DTB_SIZE], and caller
        // guarantees `[ptr, ptr + totalsize)` is readable for `'a`.
        let full_bytes = unsafe { slice::from_raw_parts(ptr as *const u8, total) };
        Ok(Self {
            data: full_bytes,
            header,
        })
    }

    /// Parses and validates a DTB from an existing byte slice.
    pub fn from_bytes(bytes: &'a [u8]) -> Result<Self, DtbError> {
        let header = FdtHeader::parse(bytes)?;
        let total = header.totalsize as usize;
        if bytes.len() < total {
            return Err(DtbError::InvalidTotalSize(header.totalsize));
        }
        Ok(Self {
            data: &bytes[..total],
            header,
        })
    }

    /// Returns the parsed FDT header.
    pub fn header(&self) -> &FdtHeader {
        &self.header
    }

    /// Returns the root node's `"model"` property if present.
    pub fn model(&self) -> Option<&'a str> {
        let mut found = None;
        let _ = self.walk_properties(|depth, node_name, prop_name, value| {
            if depth == 1 && node_name.is_empty() && prop_name == "model" {
                found = c_str_from_bytes(value);
                return false;
            }
            true
        });
        found
    }

    /// Returns `(base_addr, size_bytes)` from the first `/memory` node's `"reg"` property.
    pub fn memory_region(&self) -> Option<(u64, u64)> {
        let mut cells_stack = [(2usize, 1usize); 16];
        let mut region = None;

        let _ = self.walk_events(|event| match event {
            WalkEvent::BeginNode { depth, .. } => {
                if depth > 1 && depth <= cells_stack.len() {
                    cells_stack[depth - 1] = cells_stack[depth - 2];
                }
                true
            }
            WalkEvent::Property {
                depth,
                node_name,
                prop_name,
                value,
            } => {
                update_cells(&mut cells_stack, depth, prop_name, value);
                let is_memory = node_name == "memory" || node_name.starts_with("memory@");
                if depth == 2 && is_memory && prop_name == "reg" {
                    let (addr_cells, size_cells) = cells_stack[0];
                    region = parse_reg_pair(value, addr_cells, size_cells);
                    return false;
                }
                true
            }
            WalkEvent::EndNode { .. } => true,
        });

        region
    }

    /// Finds the first node matching `compatible` and returns its first `(addr, size)` `"reg"` entry.
    pub fn find_compatible_reg(&self, compatible: &str) -> Option<(u64, u64)> {
        let mut cells_stack = [(2usize, 1usize); 16];
        let mut matched_depth: Option<usize> = None;
        let mut pending_reg: Option<(usize, &'a [u8])> = None;
        let mut result: Option<(u64, u64)> = None;

        let _ = self.walk_events(|event| match event {
            WalkEvent::BeginNode { depth, .. } => {
                if depth > 1 && depth <= cells_stack.len() {
                    cells_stack[depth - 1] = cells_stack[depth - 2];
                }
                if matched_depth.is_some_and(|d| depth <= d) {
                    matched_depth = None;
                }
                pending_reg = None;
                true
            }
            WalkEvent::EndNode { depth } => {
                if matched_depth == Some(depth) {
                    if let Some((reg_depth, reg)) = pending_reg
                        && reg_depth == depth
                    {
                        let (addr_cells, size_cells) = parent_cells(&cells_stack, depth);
                        result = parse_reg_pair(reg, addr_cells, size_cells);
                        return false;
                    }
                    matched_depth = None;
                }
                if pending_reg.is_some_and(|(d, _)| d >= depth) {
                    pending_reg = None;
                }
                true
            }
            WalkEvent::Property {
                depth,
                prop_name,
                value,
                ..
            } => {
                update_cells(&mut cells_stack, depth, prop_name, value);
                if prop_name == "compatible" && string_list_contains(value, compatible) {
                    matched_depth = Some(depth);
                } else if prop_name == "reg" {
                    if matched_depth == Some(depth) {
                        let (addr_cells, size_cells) = parent_cells(&cells_stack, depth);
                        result = parse_reg_pair(value, addr_cells, size_cells);
                        return false;
                    }
                    pending_reg = Some((depth, value));
                }
                true
            }
        });

        result
    }

    fn struct_slice(&self) -> &'a [u8] {
        let start = self.header.off_dt_struct as usize;
        let end = start + self.header.size_dt_struct as usize;
        &self.data[start..end]
    }

    fn strings_slice(&self) -> &'a [u8] {
        let start = self.header.off_dt_strings as usize;
        let end = start + self.header.size_dt_strings as usize;
        &self.data[start..end]
    }

    fn string_at(&self, offset: usize) -> Option<&'a str> {
        let strings = self.strings_slice();
        if offset >= strings.len() {
            return None;
        }
        c_str_from_bytes(&strings[offset..])
    }

    fn walk_properties<F>(&self, mut visitor: F) -> Result<(), DtbError>
    where
        F: FnMut(usize, &'a str, &'a str, &'a [u8]) -> bool,
    {
        self.walk_events(|event| match event {
            WalkEvent::Property {
                depth,
                node_name,
                prop_name,
                value,
            } => visitor(depth, node_name, prop_name, value),
            _ => true,
        })
    }

    fn walk_events<F>(&self, mut visitor: F) -> Result<(), DtbError>
    where
        F: FnMut(WalkEvent<'a>) -> bool,
    {
        let block = self.struct_slice();
        let mut cursor = 0usize;
        let mut depth = 0usize;
        let mut node_stack: [&'a str; 16] = [""; 16];

        while cursor < block.len() {
            let token = read_be_u32(block, cursor)?;
            cursor += 4;

            match token {
                FDT_BEGIN_NODE => {
                    let name =
                        c_str_from_bytes(&block[cursor..]).ok_or(DtbError::MalformedStructure)?;
                    let step = align_up_4(name.len() + 1);
                    cursor = cursor
                        .checked_add(step)
                        .ok_or(DtbError::MalformedStructure)?;
                    if cursor > block.len() {
                        return Err(DtbError::MalformedStructure);
                    }
                    depth += 1;
                    if depth <= node_stack.len() {
                        node_stack[depth - 1] = name;
                    }
                    if !visitor(WalkEvent::BeginNode { depth }) {
                        return Ok(());
                    }
                }
                FDT_END_NODE => {
                    if depth == 0 {
                        return Err(DtbError::MalformedStructure);
                    }
                    if !visitor(WalkEvent::EndNode { depth }) {
                        return Ok(());
                    }
                    depth -= 1;
                }
                FDT_PROP => {
                    let len = read_be_u32(block, cursor)? as usize;
                    let nameoff = read_be_u32(block, cursor + 4)? as usize;
                    cursor += 8;

                    let val_end = cursor
                        .checked_add(len)
                        .ok_or(DtbError::MalformedStructure)?;
                    let next_cursor = cursor
                        .checked_add(align_up_4(len))
                        .ok_or(DtbError::MalformedStructure)?;
                    if next_cursor > block.len() {
                        return Err(DtbError::MalformedStructure);
                    }

                    let prop_name = self
                        .string_at(nameoff)
                        .ok_or(DtbError::MalformedStructure)?;
                    let node_name = if depth > 0 && depth <= node_stack.len() {
                        node_stack[depth - 1]
                    } else {
                        ""
                    };
                    let value = &block[cursor..val_end];
                    cursor = next_cursor;

                    if !visitor(WalkEvent::Property {
                        depth,
                        node_name,
                        prop_name,
                        value,
                    }) {
                        return Ok(());
                    }
                }
                FDT_NOP => {}
                FDT_END => return Ok(()),
                _ => return Err(DtbError::MalformedStructure),
            }
        }

        Err(DtbError::MalformedStructure)
    }
}

enum WalkEvent<'a> {
    BeginNode {
        depth: usize,
    },
    EndNode {
        depth: usize,
    },
    Property {
        depth: usize,
        node_name: &'a str,
        prop_name: &'a str,
        value: &'a [u8],
    },
}

fn read_be_u32(bytes: &[u8], offset: usize) -> Result<u32, DtbError> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or(DtbError::MalformedStructure)?;
    Ok(u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn read_be_u32_opt(bytes: &[u8]) -> Option<u32> {
    let arr: [u8; 4] = bytes.get(..4)?.try_into().ok()?;
    Some(u32::from_be_bytes(arr))
}

fn read_cells(bytes: &[u8], cells: usize) -> Option<u64> {
    match cells {
        1 => Some(u64::from(read_be_u32_opt(bytes)?)),
        2 => {
            let hi = u64::from(read_be_u32_opt(bytes)?);
            let lo = u64::from(read_be_u32_opt(bytes.get(4..)?)?);
            Some((hi << 32) | lo)
        }
        _ => None,
    }
}

fn parse_reg_pair(value: &[u8], addr_cells: usize, size_cells: usize) -> Option<(u64, u64)> {
    let addr_bytes = addr_cells.checked_mul(4)?;
    let size_bytes = size_cells.checked_mul(4)?;
    if value.len() < addr_bytes + size_bytes {
        return None;
    }
    let addr = read_cells(&value[..addr_bytes], addr_cells)?;
    let size = read_cells(&value[addr_bytes..addr_bytes + size_bytes], size_cells)?;
    Some((addr, size))
}

fn c_str_from_bytes(bytes: &[u8]) -> Option<&str> {
    let nul_pos = bytes.iter().position(|&b| b == 0)?;
    str::from_utf8(&bytes[..nul_pos]).ok()
}

fn string_list_contains(bytes: &[u8], target: &str) -> bool {
    bytes
        .split(|&b| b == 0)
        .filter(|item| !item.is_empty())
        .any(|item| item == target.as_bytes())
}

fn update_cells(stack: &mut [(usize, usize)], depth: usize, prop_name: &str, value: &[u8]) {
    if depth == 0 || depth > stack.len() {
        return;
    }
    if prop_name == "#address-cells"
        && let Some(v) = read_be_u32_opt(value)
    {
        stack[depth - 1].0 = v as usize;
    } else if prop_name == "#size-cells"
        && let Some(v) = read_be_u32_opt(value)
    {
        stack[depth - 1].1 = v as usize;
    }
}

fn parent_cells(stack: &[(usize, usize)], depth: usize) -> (usize, usize) {
    if depth >= 2 && depth - 2 < stack.len() {
        stack[depth - 2]
    } else {
        (2, 1)
    }
}

const fn align_up_4(val: usize) -> usize {
    (val + 3) & !3
}

#[cfg(test)]
mod tests {
    use super::{
        Dtb, DtbError, FDT_BEGIN_NODE, FDT_END, FDT_END_NODE, FDT_HEADER_SIZE, FDT_MAGIC, FDT_PROP,
    };

    struct TestFdtBuilder {
        struct_buf: Vec<u8>,
        strings_buf: Vec<u8>,
    }

    impl TestFdtBuilder {
        fn new() -> Self {
            Self {
                struct_buf: Vec::new(),
                strings_buf: Vec::new(),
            }
        }

        fn push_u32(buf: &mut Vec<u8>, val: u32) {
            buf.extend_from_slice(&val.to_be_bytes());
        }

        fn pad4(buf: &mut Vec<u8>) {
            while !buf.len().is_multiple_of(4) {
                buf.push(0);
            }
        }

        fn intern_string(&mut self, name: &str) -> u32 {
            let off = self.strings_buf.len() as u32;
            self.strings_buf.extend_from_slice(name.as_bytes());
            self.strings_buf.push(0);
            off
        }

        fn begin_node(&mut self, name: &str) {
            Self::push_u32(&mut self.struct_buf, FDT_BEGIN_NODE);
            self.struct_buf.extend_from_slice(name.as_bytes());
            self.struct_buf.push(0);
            Self::pad4(&mut self.struct_buf);
        }

        fn end_node(&mut self) {
            Self::push_u32(&mut self.struct_buf, FDT_END_NODE);
        }

        fn prop_bytes(&mut self, name: &str, value: &[u8]) {
            let nameoff = self.intern_string(name);
            Self::push_u32(&mut self.struct_buf, FDT_PROP);
            Self::push_u32(&mut self.struct_buf, value.len() as u32);
            Self::push_u32(&mut self.struct_buf, nameoff);
            self.struct_buf.extend_from_slice(value);
            Self::pad4(&mut self.struct_buf);
        }

        fn prop_u32(&mut self, name: &str, val: u32) {
            self.prop_bytes(name, &val.to_be_bytes());
        }

        fn build(mut self) -> Vec<u8> {
            Self::push_u32(&mut self.struct_buf, FDT_END);

            let off_mem_rsvmap = FDT_HEADER_SIZE as u32;
            let off_dt_struct = off_mem_rsvmap + 16; // empty terminator (16 zero bytes)
            let size_dt_struct = self.struct_buf.len() as u32;
            let off_dt_strings = off_dt_struct + size_dt_struct;
            let size_dt_strings = self.strings_buf.len() as u32;
            let totalsize = off_dt_strings + size_dt_strings;

            let mut out = Vec::with_capacity(totalsize as usize);
            Self::push_u32(&mut out, FDT_MAGIC);
            Self::push_u32(&mut out, totalsize);
            Self::push_u32(&mut out, off_dt_struct);
            Self::push_u32(&mut out, off_dt_strings);
            Self::push_u32(&mut out, off_mem_rsvmap);
            Self::push_u32(&mut out, 17);
            Self::push_u32(&mut out, 16);
            Self::push_u32(&mut out, 0);
            Self::push_u32(&mut out, size_dt_strings);
            Self::push_u32(&mut out, size_dt_struct);
            out.extend_from_slice(&[0u8; 16]);
            out.extend_from_slice(&self.struct_buf);
            out.extend_from_slice(&self.strings_buf);
            out
        }
    }

    #[test]
    fn parses_model_memory_and_compatible_node() {
        let mut fdt = TestFdtBuilder::new();
        fdt.begin_node("");
        fdt.prop_u32("#address-cells", 1);
        fdt.prop_u32("#size-cells", 1);
        fdt.prop_bytes("model", b"Raspberry Pi 3 Model B\0");

        fdt.begin_node("memory@0");
        fdt.prop_bytes("device_type", b"memory\0");
        let mut mem_reg = Vec::new();
        mem_reg.extend_from_slice(&0x0000_0000u32.to_be_bytes());
        mem_reg.extend_from_slice(&0x3b40_0000u32.to_be_bytes());
        fdt.prop_bytes("reg", &mem_reg);
        fdt.end_node();

        fdt.begin_node("serial@3f201000");
        fdt.prop_bytes("compatible", b"arm,pl011\0arm,primecell\0");
        let mut uart_reg = Vec::new();
        uart_reg.extend_from_slice(&0x3f20_1000u32.to_be_bytes());
        uart_reg.extend_from_slice(&0x0000_1000u32.to_be_bytes());
        fdt.prop_bytes("reg", &uart_reg);
        fdt.end_node();

        fdt.end_node();

        let blob = fdt.build();
        let dtb = Dtb::from_bytes(&blob).expect("valid FDT blob");

        assert_eq!(dtb.model(), Some("Raspberry Pi 3 Model B"));
        assert_eq!(dtb.memory_region(), Some((0x0, 0x3b40_0000)));
        assert_eq!(
            dtb.find_compatible_reg("arm,pl011"),
            Some((0x3f20_1000, 0x1000))
        );
        assert_eq!(dtb.find_compatible_reg("unknown,dev"), None);
    }

    #[test]
    fn rejects_invalid_magic_and_truncated_input() {
        assert_eq!(Dtb::from_bytes(&[0u8; 16]), Err(DtbError::TruncatedHeader));

        let mut bad_magic = [0u8; 40];
        bad_magic[0..4].copy_from_slice(&0x1234_5678u32.to_be_bytes());
        assert_eq!(
            Dtb::from_bytes(&bad_magic),
            Err(DtbError::InvalidMagic(0x1234_5678))
        );
    }

    #[test]
    fn rejects_null_or_unaligned_raw_pointer() {
        // SAFETY: Null and unaligned pointers are rejected before any memory dereference.
        unsafe {
            assert_eq!(Dtb::from_raw_ptr(0), Err(DtbError::InvalidPointer));
            assert_eq!(Dtb::from_raw_ptr(0x1002), Err(DtbError::InvalidPointer));
        }
    }
}

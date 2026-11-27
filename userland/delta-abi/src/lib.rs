#![no_std]

//! Shared Delta Kernel <-> EL0 Userspace ABI definitions.

/// Service name registered by the userspace UART driver server (`drv-uart`).
pub const SVC_UART: &str = "svc.uart";
/// Service name registered by the userspace SD/EMMC block driver (`drv-sd`).
pub const SVC_SD: &str = "svc.sd";
/// Service name registered by the userspace VFS server (`vfsd`).
pub const SVC_VFS: &str = "svc.vfs";

/// IPC message tag asking `drv-uart` to write payload bytes to the serial console.
pub const MSG_UART_WRITE: u64 = 1;
/// IPC reply tag acknowledging bytes written by `drv-uart`.
pub const MSG_UART_ACK: u64 = 2;
/// IPC tag asking `drv-uart` to read available input byte(s) without blocking.
pub const MSG_UART_READ: u64 = 3;

/// Kernel-injected IRQ forwarding notification delivered to a bound driver port.
pub const MSG_IRQ_NOTIFY: u64 = 0x100;

/// SD block driver IPC opcodes (`svc.sd`).
pub const MSG_SD_INFO: u64 = 0x200;
pub const MSG_SD_READ: u64 = 0x201;
pub const MSG_SD_WRITE: u64 = 0x202;
pub const MSG_SD_ACK: u64 = 0x2FF;

/// VFS server IPC opcodes (`svc.vfs`).
pub const MSG_VFS_READ_FILE: u64 = 0x301;
pub const MSG_VFS_WRITE_FILE: u64 = 0x302;
pub const MSG_VFS_READDIR: u64 = 0x303;
pub const MSG_VFS_STAT: u64 = 0x304;
pub const MSG_VFS_ACK: u64 = 0x3FF;

/// Maximum inline payload size inside [`IpcMessage`].
pub const IPC_MAX_DATA: usize = 96;

/// Maximum VFS path bytes carried inside [`VfsPacket`].
pub const VFS_MAX_PATH: usize = 64;

/// System call numbers passed in register `x8` on AArch64 `svc #0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum SyscallId {
    Exit = 0,
    Yield = 1,
    GetPid = 2,
    SpawnElf = 3,
    WaitPid = 4,
    PortCreate = 10,
    PortSend = 11,
    PortRecv = 12,
    PortCall = 13,
    PortReply = 14,
    ServiceRegister = 20,
    ServiceLookup = 21,
    GetUartMmioInfo = 30,
    GetSdMmioInfo = 31,
    IrqBind = 32,
    IrqAck = 33,
    ShmCreate = 40,
    ShmAllowPort = 41,
    ShmMap = 42,
    ShmUnmap = 43,
}

impl SyscallId {
    /// Converts a raw `x8` syscall number into [`SyscallId`].
    pub const fn from_raw(raw: usize) -> Option<Self> {
        match raw {
            0 => Some(Self::Exit),
            1 => Some(Self::Yield),
            2 => Some(Self::GetPid),
            3 => Some(Self::SpawnElf),
            4 => Some(Self::WaitPid),
            10 => Some(Self::PortCreate),
            11 => Some(Self::PortSend),
            12 => Some(Self::PortRecv),
            13 => Some(Self::PortCall),
            14 => Some(Self::PortReply),
            20 => Some(Self::ServiceRegister),
            21 => Some(Self::ServiceLookup),
            30 => Some(Self::GetUartMmioInfo),
            31 => Some(Self::GetSdMmioInfo),
            32 => Some(Self::IrqBind),
            33 => Some(Self::IrqAck),
            40 => Some(Self::ShmCreate),
            41 => Some(Self::ShmAllowPort),
            42 => Some(Self::ShmMap),
            43 => Some(Self::ShmUnmap),
            _ => None,
        }
    }
}

/// Negative POSIX-aligned error codes returned in `x0`.
pub mod errno {
    pub const EPERM: isize = -1;
    pub const ENOENT: isize = -2;
    pub const EIO: isize = -5;
    pub const EAGAIN: isize = -11;
    pub const ENOMEM: isize = -12;
    pub const EFAULT: isize = -14;
    pub const EINVAL: isize = -22;
    pub const ENOSYS: isize = -38;
}

/// Capability rights bitmask for IPC ports.
pub mod rights {
    pub const SEND: u32 = 1 << 0;
    pub const RECEIVE: u32 = 1 << 1;
    pub const MANAGE: u32 = 1 << 2;
    pub const ALL: u32 = SEND | RECEIVE | MANAGE;
}

/// Fixed 128-byte IPC message passed between EL0 ports.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct IpcMessage {
    /// User-defined operation code (`MSG_*`).
    pub tag: u64,
    /// Stamped by kernel on delivery with the sender's process ID (`0` for kernel IRQ notify).
    pub sender_pid: u64,
    /// Kernel-supplied one-shot reply port handle for synchronous `port_call`.
    pub reply_port: u64,
    /// Valid byte count inside `data` (`0..=96`).
    pub len: u32,
    /// Optional status / flags field.
    pub flags: u32,
    /// Inline message payload.
    pub data: [u8; IPC_MAX_DATA],
}

const _: () = assert!(core::mem::size_of::<IpcMessage>() == 128);

impl IpcMessage {
    /// Creates an empty zeroed [`IpcMessage`] with `tag`.
    pub const fn new(tag: u64) -> Self {
        Self {
            tag,
            sender_pid: 0,
            reply_port: 0,
            len: 0,
            flags: 0,
            data: [0; IPC_MAX_DATA],
        }
    }

    /// Creates an [`IpcMessage`] containing up to [`IPC_MAX_DATA`] bytes from `bytes`.
    pub fn from_bytes(tag: u64, bytes: &[u8]) -> Self {
        let mut msg = Self::new(tag);
        let copy_len = bytes.len().min(IPC_MAX_DATA);
        msg.data[..copy_len].copy_from_slice(&bytes[..copy_len]);
        msg.len = copy_len as u32;
        msg
    }

    /// Returns the valid payload slice.
    pub fn payload(&self) -> &[u8] {
        let len = (self.len as usize).min(IPC_MAX_DATA);
        &self.data[..len]
    }
}

/// UART controller variant exposed to `drv-uart` via [`SyscallId::GetUartMmioInfo`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum UartKind {
    Pl011 = 0,
    MiniUart = 1,
}

/// Device MMIO capability metadata returned to the userspace UART driver server.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct UartMmioInfo {
    /// Mapped EL0 virtual base address of the UART register page.
    pub virt_base: usize,
    /// Controller type (`0` = PL011, `1` = Mini-UART).
    pub kind: u32,
    pub _pad: u32,
}

/// Device MMIO + IRQ metadata returned to the userspace SD/EMMC driver (`drv-sd`).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SdMmioInfo {
    /// Mapped EL0 virtual base address of the EMMC register page.
    pub virt_base: usize,
    /// Hardware GPU IRQ line number assigned to EMMC.
    pub irq_num: u32,
    pub _pad: u32,
}

/// Block I/O request payload packed into [`IpcMessage::data`] for `svc.sd`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SdIoPacket {
    pub lba: u64,
    pub shm_id: u64,
    pub count: u32,
    pub status: i32,
}

const _: () = assert!(core::mem::size_of::<SdIoPacket>() <= IPC_MAX_DATA);

/// VFS request/reply payload packed into [`IpcMessage::data`] for `svc.vfs`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VfsPacket {
    pub shm_id: u64,
    pub offset: u64,
    pub size: u32,
    pub path_len: u16,
    pub is_dir: u8,
    pub _pad: u8,
    pub status: i32,
    pub path: [u8; VFS_MAX_PATH],
}

const _: () = assert!(core::mem::size_of::<VfsPacket>() <= IPC_MAX_DATA);

impl VfsPacket {
    /// Creates an empty [`VfsPacket`] for `path`.
    pub fn for_path(path_bytes: &[u8]) -> Self {
        let mut pkt = Self {
            shm_id: 0,
            offset: 0,
            size: 0,
            path_len: 0,
            is_dir: 0,
            _pad: 0,
            status: 0,
            path: [0; VFS_MAX_PATH],
        };
        let n = path_bytes.len().min(VFS_MAX_PATH);
        pkt.path[..n].copy_from_slice(&path_bytes[..n]);
        pkt.path_len = n as u16;
        pkt
    }

    /// Returns the path slice.
    pub fn path_bytes(&self) -> &[u8] {
        let n = (self.path_len as usize).min(VFS_MAX_PATH);
        &self.path[..n]
    }
}

/// Encodes a `Copy` packet struct into an [`IpcMessage`].
pub fn pack_msg<T: Copy>(tag: u64, pkt: &T) -> IpcMessage {
    let mut msg = IpcMessage::new(tag);
    let sz = core::mem::size_of::<T>().min(IPC_MAX_DATA);
    // SAFETY: `pkt` is a valid `T` value and `sz <= IPC_MAX_DATA`.
    let src = unsafe { core::slice::from_raw_parts((pkt as *const T).cast::<u8>(), sz) };
    msg.data[..sz].copy_from_slice(src);
    msg.len = sz as u32;
    msg
}

/// Decodes a `Copy` packet struct from an [`IpcMessage`].
pub fn unpack_msg<T: Copy>(msg: &IpcMessage) -> Option<T> {
    let sz = core::mem::size_of::<T>();
    if (msg.len as usize) < sz || sz > IPC_MAX_DATA {
        return None;
    }
    // SAFETY: `msg.data` holds at least `sz` initialized bytes; `read_unaligned` handles alignment.
    Some(unsafe { core::ptr::read_unaligned(msg.data.as_ptr().cast::<T>()) })
}

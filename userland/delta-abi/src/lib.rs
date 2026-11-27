#![no_std]

//! Shared Delta Kernel <-> EL0 Userspace ABI definitions.

/// Service name registered by the userspace UART driver server (`drv-uart`).
pub const SVC_UART: &str = "svc.uart";

/// IPC message tag asking `drv-uart` to write payload bytes to the serial console.
pub const MSG_UART_WRITE: u64 = 1;
/// IPC reply tag acknowledging bytes written by `drv-uart`.
pub const MSG_UART_ACK: u64 = 2;

/// Maximum inline payload size inside [`IpcMessage`].
pub const IPC_MAX_DATA: usize = 96;

/// System call numbers passed in register `x8` on AArch64 `svc #0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum SyscallId {
    Exit = 0,
    Yield = 1,
    GetPid = 2,
    PortCreate = 10,
    PortSend = 11,
    PortRecv = 12,
    PortCall = 13,
    PortReply = 14,
    ServiceRegister = 20,
    ServiceLookup = 21,
    GetUartMmioInfo = 30,
}

impl SyscallId {
    /// Converts a raw `x8` syscall number into [`SyscallId`].
    pub const fn from_raw(raw: usize) -> Option<Self> {
        match raw {
            0 => Some(Self::Exit),
            1 => Some(Self::Yield),
            2 => Some(Self::GetPid),
            10 => Some(Self::PortCreate),
            11 => Some(Self::PortSend),
            12 => Some(Self::PortRecv),
            13 => Some(Self::PortCall),
            14 => Some(Self::PortReply),
            20 => Some(Self::ServiceRegister),
            21 => Some(Self::ServiceLookup),
            30 => Some(Self::GetUartMmioInfo),
            _ => None,
        }
    }
}

/// Negative POSIX-aligned error codes returned in `x0`.
pub mod errno {
    pub const EPERM: isize = -1;
    pub const ENOENT: isize = -2;
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
    /// Stamped by kernel on delivery with the sender's thread/process ID.
    pub sender_pid: u64,
    /// Kernel-supplied one-shot reply port handle for synchronous `port_call`.
    pub reply_port: u64,
    /// Valid byte count inside `data` (`0..=96`).
    pub len: u32,
    /// Optional flags.
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

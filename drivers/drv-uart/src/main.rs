#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

//! Userspace (`EL0`) UART driver server for Hollow Star OS+.
//!
//! Receives [`delta_abi::MSG_UART_WRITE`] messages over its registered IPC port
//! (`svc.uart`) and writes bytes directly to its isolated Device MMIO page.

use core::ptr::{read_volatile, write_volatile};

use libdelta::delta_abi::{
    IpcMessage, MSG_UART_ACK, MSG_UART_WRITE, SVC_UART, UartKind, UartMmioInfo,
};

struct El0Uart {
    virt_base: usize,
    is_mini: bool,
}

impl El0Uart {
    fn from_info(info: UartMmioInfo) -> Self {
        Self {
            virt_base: info.virt_base,
            is_mini: info.kind == UartKind::MiniUart as u32,
        }
    }

    fn write_byte(&self, byte: u8) {
        if self.is_mini {
            const AUX_MU_IO: usize = 0x40;
            const AUX_MU_LSR: usize = 0x54;
            const LSR_TX_EMPTY: u32 = 1 << 5;
            // SAFETY: `self.virt_base` is the isolated 4 KiB Device-nGnRnE UART page
            // mapped into this driver's `TTBR0_EL1` by Delta Kernel.
            unsafe {
                while (read_volatile((self.virt_base + AUX_MU_LSR) as *const u32) & LSR_TX_EMPTY)
                    == 0
                {
                    core::hint::spin_loop();
                }
                write_volatile((self.virt_base + AUX_MU_IO) as *mut u32, u32::from(byte));
            }
        } else {
            const PL011_DR: usize = 0x00;
            const PL011_FR: usize = 0x18;
            const FR_TXFF: u32 = 1 << 5;
            // SAFETY: `self.virt_base` is the isolated 4 KiB Device-nGnRnE PL011 page
            // mapped into this driver's `TTBR0_EL1` by Delta Kernel.
            unsafe {
                while (read_volatile((self.virt_base + PL011_FR) as *const u32) & FR_TXFF) != 0 {
                    core::hint::spin_loop();
                }
                write_volatile((self.virt_base + PL011_DR) as *mut u32, u32::from(byte));
            }
        }
    }

    fn write_bytes_crlf(&self, bytes: &[u8]) {
        for &b in bytes {
            if b == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(b);
        }
    }
}

#[unsafe(no_mangle)]
fn delta_main() -> usize {
    let Ok(mmio_info) = libdelta::uart_mmio_info() else {
        return 1;
    };
    let uart = El0Uart::from_info(mmio_info);

    let Ok(port) = libdelta::port_create(16) else {
        return 2;
    };
    if libdelta::service_register(SVC_UART, port).is_err() {
        return 3;
    }

    uart.write_bytes_crlf(b"[el0/drv-uart] Userspace UART server online (EL0, svc.uart)\n");

    let mut req = IpcMessage::new(0);
    loop {
        if libdelta::port_recv(port, &mut req).is_err() {
            libdelta::yield_now();
            continue;
        }

        if req.tag == MSG_UART_WRITE {
            let payload = req.payload();
            uart.write_bytes_crlf(payload);
            if req.reply_port != 0 {
                let mut ack = IpcMessage::new(MSG_UART_ACK);
                ack.len = payload.len() as u32;
                let _ = libdelta::port_reply(req.reply_port as usize, &ack);
            }
        }
    }
}

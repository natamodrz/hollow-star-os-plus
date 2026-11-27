#![no_std]
#![no_main]

//! First EL0 userspace process (`init`) for Hollow Star OS+.

use libdelta::delta_abi::{IpcMessage, MSG_UART_ACK, MSG_UART_WRITE, SVC_UART};

#[unsafe(no_mangle)]
fn delta_main() -> usize {
    let uart_cap = loop {
        if let Ok(cap) = libdelta::service_lookup(SVC_UART) {
            break cap;
        }
        libdelta::yield_now();
    };

    let req = IpcMessage::from_bytes(
        MSG_UART_WRITE,
        b"[el0/init] Hello from EL0 userspace via Delta IPC -> drv-uart!\n",
    );
    let mut reply = IpcMessage::new(0);
    if libdelta::port_call(uart_cap, &req, &mut reply).is_err() || reply.tag != MSG_UART_ACK {
        return 1;
    }

    let confirm = IpcMessage::from_bytes(
        MSG_UART_WRITE,
        b"[el0/init] Synchronous IPC ACK verified; init exiting cleanly.\n",
    );
    let _ = libdelta::port_call(uart_cap, &confirm, &mut reply);

    0
}

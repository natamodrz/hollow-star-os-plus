//! Capability-based port IPC, named service registry, and EL0 IRQ forwarding.

use delta_abi::{IpcMessage, MSG_IRQ_NOTIFY, errno, rights};
use hal_core::InterruptController;

use crate::sched::{self, Priority, ThreadId};
use crate::sync::IrqSpinLock;
use crate::{board, proc};

const MAX_PORTS: usize = 32;
const PORT_QUEUE_CAP: usize = 16;
const MAX_WAITERS: usize = 8;
const MAX_SERVICES: usize = 16;
const MAX_CAPS_TOTAL: usize = 256;
const MAX_IRQ_LINES: usize = 128;

#[derive(Clone, Copy)]
struct CapabilityEntry {
    in_use: bool,
    owner_pid: u64,
    handle: usize,
    port_id: usize,
    rights: u32,
    one_shot: bool,
}

impl CapabilityEntry {
    const fn empty() -> Self {
        Self {
            in_use: false,
            owner_pid: 0,
            handle: 0,
            port_id: 0,
            rights: 0,
            one_shot: false,
        }
    }
}

#[derive(Clone, Copy)]
struct Port {
    in_use: bool,
    capacity: usize,
    owner_pid: u64,
    owner_tid: ThreadId,
    queue: [IpcMessage; PORT_QUEUE_CAP],
    head: usize,
    tail: usize,
    count: usize,
    recv_waiter: Option<ThreadId>,
    send_waiters: [Option<ThreadId>; MAX_WAITERS],
}

impl Port {
    const fn empty() -> Self {
        Self {
            in_use: false,
            capacity: PORT_QUEUE_CAP,
            owner_pid: 0,
            owner_tid: ThreadId(0),
            queue: [const { IpcMessage::new(0) }; PORT_QUEUE_CAP],
            head: 0,
            tail: 0,
            count: 0,
            recv_waiter: None,
            send_waiters: [None; MAX_WAITERS],
        }
    }

    fn push_msg(&mut self, msg: IpcMessage) -> bool {
        if self.count >= self.capacity {
            return false;
        }
        self.queue[self.tail] = msg;
        self.tail = (self.tail + 1) % self.capacity;
        self.count += 1;
        true
    }

    fn pop_msg(&mut self) -> Option<IpcMessage> {
        if self.count == 0 {
            return None;
        }
        let msg = self.queue[self.head];
        self.head = (self.head + 1) % self.capacity;
        self.count -= 1;
        Some(msg)
    }

    fn push_send_waiter(&mut self, tid: ThreadId) {
        for slot in &mut self.send_waiters {
            if slot.is_none() {
                *slot = Some(tid);
                return;
            }
        }
    }

    fn pop_send_waiter(&mut self) -> Option<ThreadId> {
        for slot in &mut self.send_waiters {
            if let Some(tid) = slot.take() {
                return Some(tid);
            }
        }
        None
    }
}

#[derive(Clone, Copy)]
struct ServiceEntry {
    in_use: bool,
    name: [u8; 32],
    name_len: usize,
    port_id: usize,
}

impl ServiceEntry {
    const fn empty() -> Self {
        Self {
            in_use: false,
            name: [0; 32],
            name_len: 0,
            port_id: 0,
        }
    }
}

struct IpcState {
    ports: [Port; MAX_PORTS],
    caps: [CapabilityEntry; MAX_CAPS_TOTAL],
    services: [ServiceEntry; MAX_SERVICES],
    irq_ports: [Option<usize>; MAX_IRQ_LINES],
    next_cap_handle: usize,
}

impl IpcState {
    const fn new() -> Self {
        Self {
            ports: [const { Port::empty() }; MAX_PORTS],
            caps: [const { CapabilityEntry::empty() }; MAX_CAPS_TOTAL],
            services: [const { ServiceEntry::empty() }; MAX_SERVICES],
            irq_ports: [None; MAX_IRQ_LINES],
            next_cap_handle: 1,
        }
    }

    fn alloc_port(
        &mut self,
        capacity: usize,
        owner_pid: u64,
        owner_tid: ThreadId,
    ) -> Result<usize, isize> {
        let cap = capacity.clamp(1, PORT_QUEUE_CAP);
        for (idx, p) in self.ports.iter_mut().enumerate() {
            if !p.in_use {
                *p = Port::empty();
                p.in_use = true;
                p.capacity = cap;
                p.owner_pid = owner_pid;
                p.owner_tid = owner_tid;
                return Ok(idx);
            }
        }
        Err(errno::ENOMEM)
    }

    fn free_port(&mut self, port_id: usize) {
        if port_id < MAX_PORTS {
            self.ports[port_id].in_use = false;
        }
    }

    fn insert_cap(
        &mut self,
        owner_pid: u64,
        port_id: usize,
        cap_rights: u32,
        one_shot: bool,
    ) -> Result<usize, isize> {
        for slot in &mut self.caps {
            if !slot.in_use {
                let handle = self.next_cap_handle;
                self.next_cap_handle += 1;
                *slot = CapabilityEntry {
                    in_use: true,
                    owner_pid,
                    handle,
                    port_id,
                    rights: cap_rights,
                    one_shot,
                };
                return Ok(handle);
            }
        }
        Err(errno::ENOMEM)
    }

    fn resolve_cap(&self, owner_pid: u64, handle: usize, required: u32) -> Result<usize, isize> {
        for cap in &self.caps {
            if cap.in_use && cap.owner_pid == owner_pid && cap.handle == handle {
                if (cap.rights & required) != required {
                    return Err(errno::EPERM);
                }
                return Ok(cap.port_id);
            }
        }
        Err(errno::ENOENT)
    }

    fn consume_one_shot_cap(&mut self, owner_pid: u64, handle: usize) {
        for cap in &mut self.caps {
            if cap.in_use && cap.owner_pid == owner_pid && cap.handle == handle && cap.one_shot {
                cap.in_use = false;
                return;
            }
        }
    }
}

static IPC: IrqSpinLock<IpcState> = IrqSpinLock::new(IpcState::new());

/// Creates a new IPC port for `(owner_pid, owner_tid)` and returns a full-rights capability.
pub fn port_create(owner_pid: u64, owner_tid: ThreadId, capacity: usize) -> Result<usize, isize> {
    let mut ipc = IPC.lock();
    let port_id = ipc.alloc_port(capacity, owner_pid, owner_tid)?;
    match ipc.insert_cap(owner_pid, port_id, rights::ALL, false) {
        Ok(handle) => Ok(handle),
        Err(e) => {
            ipc.free_port(port_id);
            Err(e)
        }
    }
}

/// Returns the owning PID of a port capability held by `caller_pid`.
pub fn port_owner_pid(caller_pid: u64, cap_handle: usize) -> Result<u64, isize> {
    let ipc = IPC.lock();
    let port_id = ipc.resolve_cap(caller_pid, cap_handle, rights::SEND)?;
    if !ipc.ports[port_id].in_use {
        return Err(errno::ENOENT);
    }
    Ok(ipc.ports[port_id].owner_pid)
}

/// Binds a hardware peripheral IRQ `irq_num` to an EL0 port capability and unmasks it.
pub fn irq_bind(owner_pid: u64, cap_handle: usize, irq_num: u32) -> Result<(), isize> {
    if (irq_num as usize) >= MAX_IRQ_LINES || !proc::has_irq_cap(owner_pid, irq_num) {
        return Err(errno::EPERM);
    }
    {
        let mut ipc = IPC.lock();
        let port_id = ipc.resolve_cap(owner_pid, cap_handle, rights::RECEIVE)?;
        ipc.irq_ports[irq_num as usize] = Some(port_id);
    }
    board::irq_handle().enable_peripheral_irq(irq_num);
    Ok(())
}

/// Called from EL1 ISR when a peripheral IRQ fires: forwards `MSG_IRQ_NOTIFY` to the bound EL0 port.
pub fn notify_irq_from_isr(irq_num: u32) {
    if (irq_num as usize) >= MAX_IRQ_LINES {
        return;
    }
    let wake_tid = {
        let mut ipc = IPC.lock();
        let Some(port_id) = ipc.irq_ports[irq_num as usize] else {
            return;
        };
        if !ipc.ports[port_id].in_use {
            return;
        }
        let mut msg = IpcMessage::new(MSG_IRQ_NOTIFY);
        msg.flags = irq_num;
        let _ = ipc.ports[port_id].push_msg(msg);
        ipc.ports[port_id].recv_waiter.take()
    };
    if let Some(tid) = wake_tid {
        sched::unblock(tid);
    }
}

/// Registers `name` against a `MANAGE`-capable port handle held by `owner_pid`.
pub fn service_register(owner_pid: u64, name: &[u8], cap_handle: usize) -> Result<(), isize> {
    if name.is_empty() || name.len() > 32 {
        return Err(errno::EINVAL);
    }
    let mut ipc = IPC.lock();
    let port_id = ipc.resolve_cap(owner_pid, cap_handle, rights::MANAGE)?;

    for svc in &mut ipc.services {
        if svc.in_use && &svc.name[..svc.name_len] == name {
            svc.port_id = port_id;
            return Ok(());
        }
    }
    for svc in &mut ipc.services {
        if !svc.in_use {
            svc.in_use = true;
            svc.name[..name.len()].copy_from_slice(name);
            svc.name_len = name.len();
            svc.port_id = port_id;
            return Ok(());
        }
    }
    Err(errno::ENOMEM)
}

/// Looks up a named service and grants `caller_pid` a `SEND`-only capability.
pub fn service_lookup(caller_pid: u64, name: &[u8]) -> Result<usize, isize> {
    if name.is_empty() || name.len() > 32 {
        return Err(errno::EINVAL);
    }
    let mut ipc = IPC.lock();
    let port_id = ipc
        .services
        .iter()
        .find(|s| s.in_use && &s.name[..s.name_len] == name)
        .map(|s| s.port_id)
        .ok_or(errno::ENOENT)?;

    ipc.insert_cap(caller_pid, port_id, rights::SEND, false)
}

/// Sends `msg` to the port identified by `cap_handle`, blocking if the queue is full.
pub fn port_send(
    sender_pid: u64,
    sender_tid: ThreadId,
    cap_handle: usize,
    mut msg: IpcMessage,
) -> Result<(), isize> {
    msg.sender_pid = sender_pid;
    msg.reply_port = 0;

    loop {
        let sent_and_wake = {
            let mut ipc = IPC.lock();
            let port_id = ipc.resolve_cap(sender_pid, cap_handle, rights::SEND)?;
            let port = &mut ipc.ports[port_id];
            if !port.in_use {
                return Err(errno::ENOENT);
            }
            if port.push_msg(msg) {
                (true, port.recv_waiter.take())
            } else {
                port.push_send_waiter(sender_tid);
                sched::prepare_wait(sender_tid);
                (false, None)
            }
        };

        if let Some(recv_tid) = sent_and_wake.1 {
            sched::unblock(recv_tid);
        }
        if sent_and_wake.0 {
            return Ok(());
        }

        sched::block_current();
    }
}

/// Receives a message from `cap_handle` into `receiver_pid`, blocking until available.
pub fn port_recv(
    receiver_pid: u64,
    receiver_tid: ThreadId,
    cap_handle: usize,
) -> Result<IpcMessage, isize> {
    loop {
        let outcome = {
            let mut ipc = IPC.lock();
            let port_id = ipc.resolve_cap(receiver_pid, cap_handle, rights::RECEIVE)?;
            if !ipc.ports[port_id].in_use {
                return Err(errno::ENOENT);
            }

            if let Some(mut msg) = ipc.ports[port_id].pop_msg() {
                let wake_sender = ipc.ports[port_id].pop_send_waiter();
                if msg.reply_port != 0 {
                    let reply_port_id = (msg.reply_port - 1) as usize;
                    let reply_cap =
                        ipc.insert_cap(receiver_pid, reply_port_id, rights::SEND, true)?;
                    msg.reply_port = reply_cap as u64;
                }
                Ok((msg, wake_sender))
            } else {
                ipc.ports[port_id].recv_waiter = Some(receiver_tid);
                sched::prepare_wait(receiver_tid);
                Err(())
            }
        };

        match outcome {
            Ok((msg, wake_sender)) => {
                if let Some(sender_tid) = wake_sender {
                    sched::unblock(sender_tid);
                }
                return Ok(msg);
            }
            Err(()) => {
                sched::block_current();
            }
        }
    }
}

/// Sends a synchronous request on `cap_handle` and blocks until the server replies.
pub fn port_call(
    caller_pid: u64,
    caller_tid: ThreadId,
    cap_handle: usize,
    mut req: IpcMessage,
) -> Result<IpcMessage, isize> {
    req.sender_pid = caller_pid;

    let (reply_port_id, server_owner_tid) = {
        let mut ipc = IPC.lock();
        let target_port_id = ipc.resolve_cap(caller_pid, cap_handle, rights::SEND)?;
        if !ipc.ports[target_port_id].in_use {
            return Err(errno::ENOENT);
        }
        let server_tid = ipc.ports[target_port_id].owner_tid;
        let reply_port_id = ipc.alloc_port(1, caller_pid, caller_tid)?;
        req.reply_port = (reply_port_id + 1) as u64;
        (reply_port_id, server_tid)
    };

    let caller_prio = sched::effective_priority(caller_tid).unwrap_or(Priority::NORMAL);
    sched::boost_priority(server_owner_tid, caller_prio);

    loop {
        let sent_and_wake = {
            let mut ipc = IPC.lock();
            let target_port_id = match ipc.resolve_cap(caller_pid, cap_handle, rights::SEND) {
                Ok(id) => id,
                Err(e) => {
                    ipc.free_port(reply_port_id);
                    return Err(e);
                }
            };
            let port = &mut ipc.ports[target_port_id];
            if port.push_msg(req) {
                (true, port.recv_waiter.take())
            } else {
                port.push_send_waiter(caller_tid);
                sched::prepare_wait(caller_tid);
                (false, None)
            }
        };

        if let Some(recv_tid) = sent_and_wake.1 {
            sched::unblock(recv_tid);
        }
        if sent_and_wake.0 {
            break;
        }
        sched::block_current();
    }

    let reply = loop {
        let popped = {
            let mut ipc = IPC.lock();
            let rport = &mut ipc.ports[reply_port_id];
            if let Some(msg) = rport.pop_msg() {
                ipc.free_port(reply_port_id);
                Some(msg)
            } else {
                rport.recv_waiter = Some(caller_tid);
                sched::prepare_wait(caller_tid);
                None
            }
        };
        if let Some(msg) = popped {
            break msg;
        }
        sched::block_current();
    };

    sched::clear_priority_boost(server_owner_tid);
    Ok(reply)
}

/// Sends a reply to a one-shot `reply_cap` received via [`port_recv`].
pub fn port_reply(server_pid: u64, reply_cap: usize, mut reply: IpcMessage) -> Result<(), isize> {
    reply.sender_pid = server_pid;
    reply.reply_port = 0;

    let wake_caller = {
        let mut ipc = IPC.lock();
        let reply_port_id = ipc.resolve_cap(server_pid, reply_cap, rights::SEND)?;
        if !ipc.ports[reply_port_id].in_use {
            ipc.consume_one_shot_cap(server_pid, reply_cap);
            return Err(errno::ENOENT);
        }
        let ok = ipc.ports[reply_port_id].push_msg(reply);
        if !ok {
            return Err(errno::EAGAIN);
        }
        let waiter = ipc.ports[reply_port_id].recv_waiter.take();
        ipc.consume_one_shot_cap(server_pid, reply_cap);
        waiter
    };

    if let Some(caller_tid) = wake_caller {
        sched::unblock(caller_tid);
    }
    Ok(())
}

//! EL0 client wrapper for the Hollow Star OS+ Desktop Compositor (`svc.desktop`).

use crate::delta_abi::{
    DesktopPacket, IpcMessage, KeyCode, MSG_DESKTOP_ACK, MSG_DESKTOP_CONNECT,
    MSG_DESKTOP_EXIT_TO_SHELL, MSG_DESKTOP_LAUNCH, MSG_DESKTOP_POLL_KEY, MSG_DESKTOP_PRESENT,
    MSG_DESKTOP_REFRESH_CFG, SVC_DESKTOP, pack_msg, unpack_msg,
};
use crate::gfx::{APP_H, APP_W, Canvas};

pub struct DesktopClient {
    port_cap: usize,
    shm_id: u64,
    pub canvas: Canvas,
    pub launch_arg: u64,
}

impl DesktopClient {
    /// Connects to `svc.desktop`, allocates a `640x452` app surface `Shm` granted to the
    /// compositor, and retrieves the launch argument passed to this process.
    pub fn connect() -> Result<Self, isize> {
        let port_cap = libdelta::wait_for_service(SVC_DESKTOP);
        let shm_bytes = APP_W * APP_H * 4;
        let (shm_id, shm_va) = libdelta::shm_create(shm_bytes)?;
        if let Err(e) = libdelta::shm_allow_port(shm_id, port_cap) {
            let _ = libdelta::shm_unmap(shm_id);
            return Err(e);
        }

        let mut req_pkt = DesktopPacket::empty();
        req_pkt.shm_id = shm_id;
        req_pkt.width = APP_W as u32;
        req_pkt.height = APP_H as u32;

        let req = pack_msg(MSG_DESKTOP_CONNECT, &req_pkt);
        let mut reply = IpcMessage::new(0);
        if let Err(e) = libdelta::port_call(port_cap, &req, &mut reply) {
            let _ = libdelta::shm_unmap(shm_id);
            return Err(e);
        }
        if reply.tag != MSG_DESKTOP_ACK {
            let _ = libdelta::shm_unmap(shm_id);
            return Err(-5);
        }
        let Some(pkt) = unpack_msg::<DesktopPacket>(&reply) else {
            let _ = libdelta::shm_unmap(shm_id);
            return Err(-5);
        };
        if pkt.status < 0 {
            let _ = libdelta::shm_unmap(shm_id);
            return Err(pkt.status as isize);
        }

        // SAFETY: `shm_va` maps `APP_W * APP_H * 4` bytes of writable client-owned `Shm`.
        let canvas = unsafe { Canvas::from_shm(shm_va, APP_W, APP_H) };
        Ok(Self {
            port_cap,
            shm_id,
            canvas,
            launch_arg: pkt.arg,
        })
    }

    /// Tells the compositor to composite the current app surface under the status bar and present.
    pub fn present(&self) {
        let mut pkt = DesktopPacket::empty();
        pkt.shm_id = self.shm_id;
        let req = pack_msg(MSG_DESKTOP_PRESENT, &pkt);
        let mut reply = IpcMessage::new(0);
        let _ = libdelta::port_call(self.port_cap, &req, &mut reply);
    }

    /// Polls one unified keyboard event (USB HID `svc.kbd` + `svc.uart`) from the compositor.
    pub fn poll_key(&self) -> Option<(KeyCode, u8)> {
        let req = pack_msg(MSG_DESKTOP_POLL_KEY, &DesktopPacket::empty());
        let mut reply = IpcMessage::new(0);
        libdelta::port_call(self.port_cap, &req, &mut reply).ok()?;
        if reply.tag != MSG_DESKTOP_ACK {
            return None;
        }
        let pkt = unpack_msg::<DesktopPacket>(&reply)?;
        let code = KeyCode::from_u8(pkt.key_kind);
        if code == KeyCode::None {
            None
        } else {
            Some((code, pkt.key_ch))
        }
    }

    /// Asks the compositor to launch `exec_path` (e.g., `"/system/apps/Terminal.hsapp/exec"`)
    /// with an optional `arg` as soon as this application exits.
    pub fn request_launch(&self, exec_path: &str, arg: u64) {
        let mut pkt = DesktopPacket::with_text(exec_path.as_bytes());
        pkt.arg = arg;
        let req = pack_msg(MSG_DESKTOP_LAUNCH, &pkt);
        let mut reply = IpcMessage::new(0);
        let _ = libdelta::port_call(self.port_cap, &req, &mut reply);
    }

    /// Notifies the compositor that `/user/config/os.cfg` changed (language, clock, volume).
    pub fn notify_config_updated(&self) {
        let req = pack_msg(MSG_DESKTOP_REFRESH_CFG, &DesktopPacket::empty());
        let mut reply = IpcMessage::new(0);
        let _ = libdelta::port_call(self.port_cap, &req, &mut reply);
    }

    /// Asks the compositor to terminate the graphical session cleanly and return to the UART shell.
    pub fn request_exit_to_shell(&self) {
        let req = pack_msg(MSG_DESKTOP_EXIT_TO_SHELL, &DesktopPacket::empty());
        let mut reply = IpcMessage::new(0);
        let _ = libdelta::port_call(self.port_cap, &req, &mut reply);
    }
}

impl Drop for DesktopClient {
    fn drop(&mut self) {
        let _ = libdelta::shm_unmap(self.shm_id);
    }
}

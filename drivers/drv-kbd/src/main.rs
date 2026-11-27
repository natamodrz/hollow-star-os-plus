#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

//! Userspace (`EL0`) BCM2837 Synopsys DWC2 USB Host & HID Boot Keyboard driver (`svc.kbd`).
//!
//! Runs in Slave/PIO mode against the isolated DWC2 register aperture (`Board::usb_mmio_region`),
//! supporting QEMU `-device usb-kbd`, Raspberry Pi Zero 2 W direct OTG keyboards, and
//! downstream keyboards behind a USB Hub (such as Raspberry Pi 3B's LAN9514).

use core::ptr::{read_volatile, write_volatile};

use libdelta::delta_abi::{
    IpcMessage, KbdPacket, KeyCode, MSG_KBD_ACK, MSG_KBD_READ, SVC_FB, SVC_KBD, errno, pack_msg,
};

// DWC2 Core & Host register offsets
const GAHBCFG: usize = 0x008;
const GUSBCFG: usize = 0x00C;
const GRSTCTL: usize = 0x010;
const GINTSTS: usize = 0x014;
const GINTMSK: usize = 0x018;
const GRXFSIZ: usize = 0x024;
const GNPTXFSIZ: usize = 0x028;
const HPTXFSIZ: usize = 0x100;

const HCFG: usize = 0x400;
const HFIR: usize = 0x404;
const HPRT: usize = 0x440;

const HCCHAR0: usize = 0x500;
const HCSPLT0: usize = 0x504;
const HCINT0: usize = 0x508;
const HCINTMSK0: usize = 0x50C;
const HCTSIZ0: usize = 0x510;
const HCDMA0: usize = 0x514;

// GRSTCTL / HPRT bits
const GRST_CSFTRST: u32 = 1 << 0;
const GRST_RXFFLSH: u32 = 1 << 4;
const GRST_TXFFLSH: u32 = 1 << 5;
const GRST_TXFNUM_ALL: u32 = 0x10 << 6;
const GRST_AHBIDLE: u32 = 1 << 31;

const HPRT_CONNSTS: u32 = 1 << 0;
const HPRT_CONNDET: u32 = 1 << 1;
const HPRT_ENA: u32 = 1 << 2;
const HPRT_ENCHNG: u32 = 1 << 3;
const HPRT_OVRCURRCHNG: u32 = 1 << 5;
const HPRT_RST: u32 = 1 << 8;
const HPRT_PWR: u32 = 1 << 12;
const HPRT_SPD_SHIFT: u32 = 17;
const HPRT_W1C_MASK: u32 = HPRT_CONNDET | HPRT_ENA | HPRT_ENCHNG | HPRT_OVRCURRCHNG;

// HCINT bits
const HCINT_XFERCOMP: u32 = 1 << 0;
const HCINT_CHHLTD: u32 = 1 << 1;
const HCINT_STALL: u32 = 1 << 3;
const HCINT_NAK: u32 = 1 << 4;
const HCINT_XACTERR: u32 = 1 << 7;
const HCINT_BBLERR: u32 = 1 << 8;
const HCINT_DATATGLERR: u32 = 1 << 10;

const PID_DATA0: u32 = 0;
const PID_DATA1: u32 = 2;
const PID_SETUP: u32 = 3;

const EPTYPE_CTRL: u32 = 0;
const EPTYPE_INTR: u32 = 3;

#[derive(Clone, Copy)]
struct KeyQueue {
    buf: [KbdPacket; 64],
    head: usize,
    tail: usize,
}

impl KeyQueue {
    const fn new() -> Self {
        Self {
            buf: [KbdPacket {
                kind: 0,
                ch: 0,
                _pad: [0; 2],
            }; 64],
            head: 0,
            tail: 0,
        }
    }

    fn push(&mut self, kind: KeyCode, ch: u8) {
        let next = (self.tail + 1) % self.buf.len();
        if next == self.head {
            return;
        }
        self.buf[self.tail] = KbdPacket {
            kind: kind as u8,
            ch,
            _pad: [0; 2],
        };
        self.tail = next;
    }

    fn pop(&mut self) -> Option<KbdPacket> {
        if self.head == self.tail {
            None
        } else {
            let p = self.buf[self.head];
            self.head = (self.head + 1) % self.buf.len();
            Some(p)
        }
    }
}

#[derive(Clone, Copy)]
struct UsbTarget {
    addr: u8,
    intr_ep: u8,
    max_pkt: u16,
    low_speed: bool,
    hub_addr: u8,
    hub_port: u8,
    data_pid: u32,
}

struct Dwc2Host {
    base: usize,
    dma_pa: usize,
    dma_va: usize,
    dma_bus_offset: u32,
    kbd: Option<UsbTarget>,
    prev_report: [u8; 8],
    repeat_key: u8,
    repeat_start_ms: u64,
    last_repeat_ms: u64,
}

impl Dwc2Host {
    fn new(base: usize, dma_pa: usize, dma_va: usize) -> Self {
        Self {
            base,
            dma_pa,
            dma_va,
            dma_bus_offset: 0,
            kbd: None,
            prev_report: [0; 8],
            repeat_key: 0,
            repeat_start_ms: 0,
            last_repeat_ms: 0,
        }
    }

    fn now_ms() -> u64 {
        libdelta::sys_caps().map(|c| c.uptime_ms).unwrap_or(0)
    }

    #[inline]
    fn hcdma_addr(&self) -> u32 {
        (self.dma_pa as u32) | self.dma_bus_offset
    }

    #[inline]
    fn read(&self, off: usize) -> u32 {
        // SAFETY: `self.base + off` lies within the isolated 16 KiB DWC2 Device-nGnRnE mapping.
        unsafe { read_volatile((self.base + off) as *const u32) }
    }

    #[inline]
    fn write(&self, off: usize, val: u32) {
        // SAFETY: `self.base + off` lies within the isolated 16 KiB DWC2 Device-nGnRnE mapping.
        unsafe { write_volatile((self.base + off) as *mut u32, val) }
    }

    fn spin_delay(&self, iters: u32) {
        for _ in 0..iters {
            core::hint::spin_loop();
        }
    }

    fn flush_fifos(&self) {
        self.write(GRSTCTL, GRST_RXFFLSH | GRST_TXFFLSH | GRST_TXFNUM_ALL);
        for _ in 0..50_000 {
            if (self.read(GRSTCTL) & (GRST_RXFFLSH | GRST_TXFFLSH)) == 0 {
                break;
            }
            core::hint::spin_loop();
        }
    }

    fn reset_core(&self) {
        for _ in 0..100_000 {
            if (self.read(GRSTCTL) & GRST_AHBIDLE) != 0 {
                break;
            }
            core::hint::spin_loop();
        }
        self.write(GRSTCTL, GRST_CSFTRST);
        for _ in 0..100_000 {
            if (self.read(GRSTCTL) & GRST_CSFTRST) == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        self.spin_delay(50_000);

        // Force Host mode, enable Buffer DMA mode (`GAHBCFG.DMAEn`), configure FIFOs & clocks.
        self.write(GAHBCFG, 1 << 5);
        let mut gusb = self.read(GUSBCFG);
        gusb &= !((1 << 29) | (1 << 30) | (0xF << 10));
        gusb |= (1 << 29) | (5 << 10); // Force host mode
        self.write(GUSBCFG, gusb);
        self.spin_delay(100_000);

        self.write(GRXFSIZ, 512);
        self.write(GNPTXFSIZ, (256 << 16) | 512);
        self.write(HPTXFSIZ, (256 << 16) | 768);
        self.flush_fifos();

        self.write(HCFG, 0x05); // 48 MHz clock, FS/LS PHY
        self.write(HFIR, 48_000);
        self.write(GINTMSK, 0);
        self.write(GINTSTS, 0xFFFF_FFFF);

        // Power root port.
        let hprt = (self.read(HPRT) & !HPRT_W1C_MASK) | HPRT_PWR;
        self.write(HPRT, hprt);
    }

    fn reset_root_port(&self) -> Option<u32> {
        let mut hprt = self.read(HPRT);
        if (hprt & HPRT_CONNSTS) == 0 {
            return None;
        }
        // Assert USB bus reset
        hprt = (self.read(HPRT) & !HPRT_W1C_MASK) | HPRT_RST;
        self.write(HPRT, hprt);
        self.spin_delay(200_000);

        // Deassert reset
        hprt = (self.read(HPRT) & !HPRT_W1C_MASK) & !HPRT_RST;
        self.write(HPRT, hprt);
        self.spin_delay(100_000);

        let status = self.read(HPRT);
        // Clear connect/enable change flags while preserving PRTENA.
        self.write(HPRT, (status & !HPRT_W1C_MASK) | HPRT_CONNDET | HPRT_ENCHNG);
        if (status & HPRT_ENA) == 0 {
            // QEMU sometimes needs a yield tick for PRTENA.
            libdelta::yield_now();
        }
        Some((self.read(HPRT) >> HPRT_SPD_SHIFT) & 0x3)
    }

    #[allow(clippy::too_many_arguments)]
    fn xfer_out(
        &self,
        dev_addr: u8,
        ep: u8,
        ep_type: u32,
        mps: u16,
        low_speed: bool,
        hub_addr: u8,
        hub_port: u8,
        pid: u32,
        data: &[u8],
    ) -> Result<(), ()> {
        self.write(HCINT0, 0xFFFF_FFFF);
        self.write(HCINTMSK0, 0x7FF);
        if hub_addr != 0 {
            self.write(
                HCSPLT0,
                (1 << 31) | (3 << 14) | (u32::from(hub_addr) << 7) | u32::from(hub_port),
            );
        } else {
            self.write(HCSPLT0, 0);
        }

        let len = data.len().min(512);
        if len > 0 {
            // SAFETY: `self.dma_va` is a 4 KiB Non-Cacheable DMA page and `len <= 512`.
            unsafe {
                core::ptr::copy_nonoverlapping(data.as_ptr(), self.dma_va as *mut u8, len);
            }
        }
        #[cfg(target_arch = "aarch64")]
        // SAFETY: Ensure `NormalNc` stores reach memory before DWC2 DMA starts.
        unsafe {
            core::arch::asm!("dsb sy", options(nostack, preserves_flags));
        }

        let len32 = len as u32;
        let pktcnt = if len32 == 0 {
            1
        } else {
            len32.div_ceil(u32::from(mps.max(8)))
        };
        self.write(HCTSIZ0, (pid << 29) | (pktcnt << 19) | len32);
        self.write(HCDMA0, self.hcdma_addr());

        let ls_bit = if low_speed { 1 << 17 } else { 0 };
        let hcchar = u32::from(mps.max(8))
            | (u32::from(ep & 0xF) << 11)
            | ls_bit
            | (ep_type << 18)
            | (1 << 20)
            | (u32::from(dev_addr) << 22)
            | (1 << 31);
        self.write(HCCHAR0, hcchar);

        for _ in 0..100_000 {
            let hcint = self.read(HCINT0);
            if (hcint & HCINT_XFERCOMP) != 0 {
                return Ok(());
            }
            if (hcint & (HCINT_STALL | HCINT_XACTERR | HCINT_BBLERR)) != 0 {
                return Err(());
            }
            if (hcint & HCINT_NAK) != 0 || (hcint & HCINT_CHHLTD) != 0 {
                return Err(());
            }
            core::hint::spin_loop();
        }
        Err(())
    }

    #[allow(clippy::too_many_arguments)]
    fn xfer_in(
        &self,
        dev_addr: u8,
        ep: u8,
        ep_type: u32,
        mps: u16,
        low_speed: bool,
        hub_addr: u8,
        hub_port: u8,
        pid: u32,
        dst: &mut [u8],
    ) -> Result<usize, u32> {
        self.write(HCINT0, 0xFFFF_FFFF);
        self.write(HCINTMSK0, 0x7FF);
        if hub_addr != 0 {
            self.write(
                HCSPLT0,
                (1 << 31) | (3 << 14) | (u32::from(hub_addr) << 7) | u32::from(hub_port),
            );
        } else {
            self.write(HCSPLT0, 0);
        }

        let want = dst.len().min(512);
        let pktcnt = if want == 0 {
            1
        } else {
            want.div_ceil(usize::from(mps.max(8))) as u32
        };
        self.write(HCTSIZ0, (pid << 29) | (pktcnt << 19) | (want as u32));
        self.write(HCDMA0, self.hcdma_addr());

        let ls_bit = if low_speed { 1 << 17 } else { 0 };
        let hcchar = u32::from(mps.max(8))
            | (u32::from(ep & 0xF) << 11)
            | (1 << 15) // EPDIR = IN
            | ls_bit
            | (ep_type << 18)
            | (1 << 20)
            | (u32::from(dev_addr) << 22)
            | (1 << 31);
        self.write(HCCHAR0, hcchar);

        for _ in 0..80_000 {
            let hcint = self.read(HCINT0);
            if (hcint & HCINT_XFERCOMP) != 0 {
                #[cfg(target_arch = "aarch64")]
                // SAFETY: Order DMA payload reads after hardware completion flag.
                unsafe {
                    core::arch::asm!("dsb sy", options(nostack, preserves_flags));
                }
                let rem = (self.read(HCTSIZ0) & 0x7_FFFF) as usize;
                let received = want.saturating_sub(rem);
                if received > 0 {
                    // SAFETY: `self.dma_va` holds `received <= dst.len()` DMA IN bytes.
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            self.dma_va as *const u8,
                            dst.as_mut_ptr(),
                            received,
                        );
                    }
                }
                return Ok(received);
            }
            if (hcint & (HCINT_STALL | HCINT_XACTERR | HCINT_BBLERR | HCINT_DATATGLERR)) != 0 {
                return Err(hcint);
            }
            if (hcint & HCINT_NAK) != 0 {
                self.write(HCCHAR0, (hcchar & !(1 << 31)) | (1 << 30));
                return Err(HCINT_NAK);
            }
            if (hcint & HCINT_CHHLTD) != 0 {
                let rem = (self.read(HCTSIZ0) & 0x7_FFFF) as usize;
                let received = want.saturating_sub(rem);
                if received > 0 {
                    // SAFETY: `self.dma_va` holds `received <= dst.len()` bytes before halt.
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            self.dma_va as *const u8,
                            dst.as_mut_ptr(),
                            received,
                        );
                    }
                    return Ok(received);
                }
                return Err(hcint);
            }
            core::hint::spin_loop();
        }
        self.write(HCCHAR0, (hcchar & !(1 << 31)) | (1 << 30));
        Err(0)
    }

    #[allow(clippy::too_many_arguments)]
    fn control_transfer(
        &self,
        dev_addr: u8,
        mps: u16,
        low_speed: bool,
        hub_addr: u8,
        hub_port: u8,
        setup: [u8; 8],
        data_in: Option<&mut [u8]>,
    ) -> Result<usize, ()> {
        // SETUP stage
        self.xfer_out(
            dev_addr,
            0,
            EPTYPE_CTRL,
            mps,
            low_speed,
            hub_addr,
            hub_port,
            PID_SETUP,
            &setup,
        )?;

        let dir_in = (setup[0] & 0x80) != 0;
        let mut total_in = 0usize;

        if dir_in && let Some(buf) = data_in {
            let mut pid = PID_DATA1;
            let step = usize::from(mps.max(8));
            while total_in < buf.len() {
                let chunk_end = (total_in + step).min(buf.len());
                let n = match self.xfer_in(
                    dev_addr,
                    0,
                    EPTYPE_CTRL,
                    mps,
                    low_speed,
                    hub_addr,
                    hub_port,
                    pid,
                    &mut buf[total_in..chunk_end],
                ) {
                    Ok(n) => n,
                    Err(_) => return Err(()),
                };
                total_in += n;
                if n < step {
                    break;
                }
                pid = if pid == PID_DATA1 {
                    PID_DATA0
                } else {
                    PID_DATA1
                };
            }
            // Status OUT ZLP
            let _ = self.xfer_out(
                dev_addr,
                0,
                EPTYPE_CTRL,
                mps,
                low_speed,
                hub_addr,
                hub_port,
                PID_DATA1,
                &[],
            );
        } else {
            // Status IN ZLP
            let mut empty = [0u8; 0];
            let _ = self.xfer_in(
                dev_addr,
                0,
                EPTYPE_CTRL,
                mps,
                low_speed,
                hub_addr,
                hub_port,
                PID_DATA1,
                &mut empty,
            );
        }

        Ok(total_in)
    }

    fn probe_and_bind_keyboard(&mut self) -> bool {
        for offset in [0u32, 0xC000_0000] {
            self.dma_bus_offset = offset;
            if self.try_probe_with_current_dma() {
                return true;
            }
        }
        self.dma_bus_offset = 0;
        false
    }

    fn try_probe_with_current_dma(&mut self) -> bool {
        self.reset_core();
        let Some(root_spd) = self.reset_root_port() else {
            return false;
        };
        let root_ls = root_spd == 2;
        let root_hs = root_spd == 0;

        // Read first 18 bytes of Device Descriptor at address 0
        let mut dev_desc = [0u8; 18];
        if self
            .control_transfer(
                0,
                8,
                root_ls,
                0,
                0,
                [0x80, 0x06, 0x00, 0x01, 0x00, 0x00, 18, 0x00],
                Some(&mut dev_desc),
            )
            .is_err()
        {
            return false;
        }
        let mps0 = u16::from(dev_desc[7]).max(8);

        // SET_ADDRESS = 1
        if self
            .control_transfer(
                0,
                mps0,
                root_ls,
                0,
                0,
                [0x00, 0x05, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00],
                None,
            )
            .is_err()
        {
            return false;
        }
        self.spin_delay(20_000);

        if dev_desc[4] == 0x09 {
            // USB Hub (e.g. Raspberry Pi 3B LAN9514 or multi-device QEMU hub):
            // probe all 5+ downstream ports with distinct addresses `2..=6`.
            // Note: Only High-Speed hubs (`root_hs`) use DWC2 Split (`HCSPLT.SpltEna`).
            return self.probe_hub_ports(1, mps0, root_hs);
        }

        self.configure_hid_keyboard(1, mps0, root_ls, 0, 0)
    }

    fn probe_hub_ports(&mut self, hub_addr: u8, hub_mps: u16, hub_is_hs: bool) -> bool {
        // SET_CONFIGURATION(1) on Hub
        if self
            .control_transfer(
                hub_addr,
                hub_mps,
                false,
                0,
                0,
                [0x00, 0x09, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00],
                None,
            )
            .is_err()
        {
            return false;
        }

        for port in 1..=6u8 {
            // SET_FEATURE(PORT_POWER = 8)
            let _ = self.control_transfer(
                hub_addr,
                hub_mps,
                false,
                0,
                0,
                [0x23, 0x03, 8, 0x00, port, 0x00, 0x00, 0x00],
                None,
            );
        }
        self.spin_delay(120_000);

        let split_hub = if hub_is_hs { hub_addr } else { 0 };
        for port in 1..=6u8 {
            let mut pstat = [0u8; 4];
            if self
                .control_transfer(
                    hub_addr,
                    hub_mps,
                    false,
                    0,
                    0,
                    [0xA3, 0x00, 0x00, 0x00, port, 0x00, 4, 0x00],
                    Some(&mut pstat),
                )
                .is_err()
            {
                continue;
            }
            if (pstat[0] & 0x01) == 0 {
                continue; // Not connected
            }
            // SET_FEATURE(PORT_RESET = 4)
            let _ = self.control_transfer(
                hub_addr,
                hub_mps,
                false,
                0,
                0,
                [0x23, 0x03, 4, 0x00, port, 0x00, 0x00, 0x00],
                None,
            );
            self.spin_delay(120_000);
            let _ = self.control_transfer(
                hub_addr,
                hub_mps,
                false,
                0,
                0,
                [0xA3, 0x00, 0x00, 0x00, port, 0x00, 4, 0x00],
                Some(&mut pstat),
            );
            let is_ls = (pstat[1] & 0x02) != 0;

            let mut ddesc = [0u8; 8];
            if self
                .control_transfer(
                    0,
                    8,
                    is_ls,
                    split_hub,
                    port,
                    [0x80, 0x06, 0x00, 0x01, 0x00, 0x00, 8, 0x00],
                    Some(&mut ddesc),
                )
                .is_ok()
            {
                let child_addr = 1 + port;
                let child_mps = u16::from(ddesc[7]).max(8);
                if self
                    .control_transfer(
                        0,
                        child_mps,
                        is_ls,
                        split_hub,
                        port,
                        [0x00, 0x05, child_addr, 0x00, 0x00, 0x00, 0x00, 0x00],
                        None,
                    )
                    .is_ok()
                    && self.configure_hid_keyboard(child_addr, child_mps, is_ls, split_hub, port)
                {
                    return true;
                }
            }
        }
        false
    }

    fn configure_hid_keyboard(
        &mut self,
        dev_addr: u8,
        ep0_mps: u16,
        low_speed: bool,
        hub_addr: u8,
        hub_port: u8,
    ) -> bool {
        let mut cfg = [0u8; 64];
        let Ok(cfg_len) = self.control_transfer(
            dev_addr,
            ep0_mps,
            low_speed,
            hub_addr,
            hub_port,
            [0x80, 0x06, 0x00, 0x02, 0x00, 0x00, 64, 0x00],
            Some(&mut cfg),
        ) else {
            return false;
        };

        let mut config_val = 1u8;
        let mut kbd_iface = None;
        let mut intr_ep = None;
        let mut intr_mps = 8u16;
        let mut in_kbd_iface = false;

        let mut i = 0usize;
        while i + 2 <= cfg_len {
            let dlen = cfg[i] as usize;
            let dtype = cfg[i + 1];
            if dlen < 2 || i + dlen > cfg_len {
                break;
            }
            if dtype == 2 && dlen >= 6 {
                config_val = cfg[i + 5];
            } else if dtype == 4 && dlen >= 9 {
                // Interface: Class=3 (HID), Protocol=1 (Keyboard) (also accept Class=3 Sub=1)
                let class = cfg[i + 5];
                let proto = cfg[i + 7];
                if class == 3 && (proto == 1 || kbd_iface.is_none()) {
                    kbd_iface = Some(cfg[i + 2]);
                    in_kbd_iface = true;
                } else {
                    in_kbd_iface = false;
                }
            } else if dtype == 5 && dlen >= 7 && in_kbd_iface {
                let ep_addr = cfg[i + 2];
                let attrs = cfg[i + 3];
                if (ep_addr & 0x80) != 0 && (attrs & 0x03) == 3 && intr_ep.is_none() {
                    intr_ep = Some(ep_addr & 0x0F);
                    intr_mps = u16::from_le_bytes([cfg[i + 4], cfg[i + 5]]).clamp(8, 64);
                }
            }
            i += dlen;
        }

        let (Some(iface_num), Some(ep_num)) = (kbd_iface, intr_ep) else {
            return false;
        };

        // SET_CONFIGURATION
        let _ = self.control_transfer(
            dev_addr,
            ep0_mps,
            low_speed,
            hub_addr,
            hub_port,
            [0x00, 0x09, config_val, 0x00, 0x00, 0x00, 0x00, 0x00],
            None,
        );
        // HID SET_PROTOCOL(0 = Boot Protocol)
        let _ = self.control_transfer(
            dev_addr,
            ep0_mps,
            low_speed,
            hub_addr,
            hub_port,
            [0x21, 0x0B, 0x00, 0x00, iface_num, 0x00, 0x00, 0x00],
            None,
        );
        // HID SET_IDLE(0)
        let _ = self.control_transfer(
            dev_addr,
            ep0_mps,
            low_speed,
            hub_addr,
            hub_port,
            [0x21, 0x0A, 0x00, 0x00, iface_num, 0x00, 0x00, 0x00],
            None,
        );

        self.kbd = Some(UsbTarget {
            addr: dev_addr,
            intr_ep: ep_num,
            max_pkt: intr_mps,
            low_speed,
            hub_addr,
            hub_port,
            data_pid: PID_DATA0,
        });
        true
    }

    fn poll_keyboard(&mut self, queue: &mut KeyQueue) {
        let Some(mut target) = self.kbd else {
            return;
        };
        let mut report = [0u8; 8];
        match self.xfer_in(
            target.addr,
            target.intr_ep,
            EPTYPE_INTR,
            target.max_pkt,
            target.low_speed,
            target.hub_addr,
            target.hub_port,
            target.data_pid,
            &mut report[..usize::from(target.max_pkt).min(8)],
        ) {
            Ok(n) if n >= 3 => {
                target.data_pid = if target.data_pid == PID_DATA0 {
                    PID_DATA1
                } else {
                    PID_DATA0
                };
                self.kbd = Some(target);
                self.decode_hid_report(&report, queue);
                self.prev_report = report;
            }
            Ok(_) => {}
            Err(hcint) if (hcint & HCINT_DATATGLERR) != 0 => {
                target.data_pid = if target.data_pid == PID_DATA0 {
                    PID_DATA1
                } else {
                    PID_DATA0
                };
                self.kbd = Some(target);
            }
            Err(_) => {
                // Repeat held navigation key smoothly using real uptime_ms timing (420ms delay, 95ms rate).
                if self.repeat_key != 0 {
                    let t = Self::now_ms();
                    if t.saturating_sub(self.repeat_start_ms) >= 420
                        && t.saturating_sub(self.last_repeat_ms) >= 95
                    {
                        self.last_repeat_ms = t;
                        emit_hid_usage(self.repeat_key, self.prev_report[0], queue);
                    }
                }
            }
        }
    }

    fn decode_hid_report(&mut self, rep: &[u8; 8], queue: &mut KeyQueue) {
        let mods = rep[0];
        let mut active_nav = 0u8;
        for &code in &rep[2..8] {
            if code == 0 {
                continue;
            }
            if matches!(code, 0x4F..=0x52 | 0x2A) {
                active_nav = code;
            }
            if !self.prev_report[2..8].contains(&code) {
                emit_hid_usage(code, mods, queue);
            }
        }
        if active_nav != self.repeat_key {
            self.repeat_key = active_nav;
            let t = Self::now_ms();
            self.repeat_start_ms = t;
            self.last_repeat_ms = t;
        }
    }
}

fn emit_hid_usage(usage: u8, mods: u8, queue: &mut KeyQueue) {
    let shift = (mods & 0x22) != 0;
    let ctrl = (mods & 0x11) != 0;

    if ctrl {
        match usage {
            0x16 => queue.push(KeyCode::CtrlS, 0), // S
            0x14 => queue.push(KeyCode::CtrlQ, 0), // Q
            0x06 => queue.push(KeyCode::CtrlC, 0), // C
            0x11 => queue.push(KeyCode::CtrlN, 0), // N
            0x09 => queue.push(KeyCode::CtrlF, 0), // F
            _ => {}
        }
        return;
    }

    match usage {
        0x28 | 0x58 => queue.push(KeyCode::Enter, 0),
        0x29 => queue.push(KeyCode::Esc, 0),
        0x2A => queue.push(KeyCode::Backspace, 0),
        0x2B => queue.push(KeyCode::Tab, 0),
        0x3A => queue.push(KeyCode::F1, 0),
        0x3B => queue.push(KeyCode::F2, 0),
        0x4F => queue.push(KeyCode::Right, 0),
        0x50 => queue.push(KeyCode::Left, 0),
        0x51 => queue.push(KeyCode::Down, 0),
        0x52 => queue.push(KeyCode::Up, 0),
        _ => {
            if let Some(ch) = hid_to_ascii(usage, shift) {
                queue.push(KeyCode::Char, ch);
            }
        }
    }
}

fn hid_to_ascii(usage: u8, shift: bool) -> Option<u8> {
    match usage {
        0x04..=0x1D => {
            let base = if shift { b'A' } else { b'a' };
            Some(base + (usage - 0x04))
        }
        0x1E..=0x26 => {
            const UNSHIFTED: [u8; 9] = *b"123456789";
            const SHIFTED: [u8; 9] = *b"!@#$%^&*(";
            let i = usize::from(usage - 0x1E);
            Some(if shift { SHIFTED[i] } else { UNSHIFTED[i] })
        }
        0x27 => Some(if shift { b')' } else { b'0' }),
        0x2C => Some(b' '),
        0x2D => Some(if shift { b'_' } else { b'-' }),
        0x2E => Some(if shift { b'+' } else { b'=' }),
        0x2F => Some(if shift { b'{' } else { b'[' }),
        0x30 => Some(if shift { b'}' } else { b']' }),
        0x31 => Some(if shift { b'|' } else { b'\\' }),
        0x33 => Some(if shift { b':' } else { b';' }),
        0x34 => Some(if shift { b'"' } else { b'\'' }),
        0x35 => Some(if shift { b'~' } else { b'`' }),
        0x36 => Some(if shift { b'<' } else { b',' }),
        0x37 => Some(if shift { b'>' } else { b'.' }),
        0x38 => Some(if shift { b'?' } else { b'/' }),
        0x54 => Some(b'/'),
        0x55 => Some(b'*'),
        0x56 => Some(b'-'),
        0x57 => Some(b'+'),
        0x59..=0x61 => Some(b'1' + (usage - 0x59)),
        0x62 => Some(b'0'),
        0x63 => Some(b'.'),
        _ => None,
    }
}

#[unsafe(no_mangle)]
fn delta_main() -> usize {
    // Wait for `drv-fb` to initialize VideoCore power domains (including USB HCD power bit).
    let _fb_cap = libdelta::wait_for_service(SVC_FB);
    let Ok(mmio) = libdelta::usb_mmio_info() else {
        return 1;
    };
    let Ok(dma) = libdelta::alloc_dma_page() else {
        return 1;
    };

    let Ok(port) = libdelta::port_create(24) else {
        return 2;
    };
    if libdelta::service_register(SVC_KBD, port).is_err() {
        return 3;
    }

    let mut host = Dwc2Host::new(mmio.virt_base, dma.phys_addr, dma.user_va);
    let mut queue = KeyQueue::new();

    if host.probe_and_bind_keyboard() {
        libdelta::print("[el0/drv-kbd] USB HID Keyboard enumerated & online (EL0, svc.kbd)\n");
    } else {
        libdelta::print("[el0/drv-kbd] DWC2 USB Host ready (awaiting USB keyboard, svc.kbd)\n");
    }

    let mut msg = IpcMessage::new(0);
    let mut idle_reprobe = 0u32;
    loop {
        if libdelta::port_recv(port, &mut msg).is_ok() {
            match msg.tag {
                MSG_KBD_READ => {
                    if host.kbd.is_some() {
                        host.poll_keyboard(&mut queue);
                    } else {
                        idle_reprobe += 1;
                        if idle_reprobe >= 48 {
                            idle_reprobe = 0;
                            if host.probe_and_bind_keyboard() {
                                libdelta::print("[el0/drv-kbd] USB HID Keyboard attached!\n");
                            }
                        }
                    }
                    let pkt = queue.pop().unwrap_or(KbdPacket {
                        kind: 0,
                        ch: 0,
                        _pad: [0; 2],
                    });
                    if msg.reply_port != 0 {
                        let reply = pack_msg(MSG_KBD_ACK, &pkt);
                        let _ = libdelta::port_reply(msg.reply_port as usize, &reply);
                    }
                }
                _ => {
                    if msg.reply_port != 0 {
                        let mut err = IpcMessage::new(MSG_KBD_ACK);
                        err.flags = errno::ENOSYS as u32;
                        let _ = libdelta::port_reply(msg.reply_port as usize, &err);
                    }
                }
            }
        } else {
            libdelta::yield_now();
        }
    }
}

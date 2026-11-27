#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

//! Userspace (`EL0`) BCM2837 Arasan EMMC/SDHCI block driver (`svc.sd`).
//!
//! Runs completely in unprivileged `EL0` with isolated access to its 4 KiB EMMC MMIO page,
//! hardware IRQ forwarding (`irq_bind` / `irq_ack`), and zero-copy shared memory (`Shm`).

use core::ptr::{read_volatile, write_volatile};

use libdelta::delta_abi::{
    IpcMessage, MSG_IRQ_NOTIFY, MSG_SD_ACK, MSG_SD_INFO, MSG_SD_READ, MSG_SD_WRITE, SVC_SD,
    SdIoPacket, errno, pack_msg, unpack_msg,
};

const BLKSIZECNT: usize = 0x04;
const ARG1: usize = 0x08;
const CMDTM: usize = 0x0C;
const RESP0: usize = 0x10;
const RESP1: usize = 0x14;
const RESP2: usize = 0x18;
const RESP3: usize = 0x1C;
const DATA: usize = 0x20;
const STATUS: usize = 0x24;
const CONTROL0: usize = 0x28;
const CONTROL1: usize = 0x2C;
const INTERRUPT: usize = 0x30;
const IRPT_MASK: usize = 0x34;
const IRPT_EN: usize = 0x38;

const STATUS_CMD_INHIBIT: u32 = 1 << 0;
const STATUS_DAT_INHIBIT: u32 = 1 << 1;

const CTRL1_CLK_INTLEN: u32 = 1 << 0;
const CTRL1_CLK_STABLE: u32 = 1 << 1;
const CTRL1_CLK_EN: u32 = 1 << 2;
const CTRL1_SRST_HC: u32 = 1 << 24;

const INT_CMD_DONE: u32 = 1 << 0;
const INT_DATA_DONE: u32 = 1 << 1;
const INT_WRITE_RDY: u32 = 1 << 4;
const INT_READ_RDY: u32 = 1 << 5;
const INT_ERROR_MASK: u32 = 0xFFFF_0000 | (1 << 15);

const CMD_RSPNS_NONE: u32 = 0b00 << 16;
const CMD_RSPNS_136: u32 = 0b01 << 16;
const CMD_RSPNS_48: u32 = 0b10 << 16;
const CMD_RSPNS_48_BUSY: u32 = 0b11 << 16;
const CMD_CRCCHK_EN: u32 = 1 << 19;
const CMD_IXCHK_EN: u32 = 1 << 20;
const CMD_ISDATA: u32 = 1 << 21;
const TM_DAT_DIR_READ: u32 = 1 << 4;
const TM_BLKCNT_EN: u32 = 1 << 1;

const fn make_cmd(idx: u32, flags: u32) -> u32 {
    (idx << 24) | flags
}

struct EmmcDriver {
    base: usize,
    irq_num: u32,
    irq_port: usize,
    is_sdhc: bool,
    rca: u32,
    total_sectors: u64,
}

impl EmmcDriver {
    fn new(base: usize, irq_num: u32, irq_port: usize) -> Self {
        Self {
            base,
            irq_num,
            irq_port,
            is_sdhc: false,
            rca: 0,
            total_sectors: 0,
        }
    }

    #[inline]
    fn read(&self, off: usize) -> u32 {
        // SAFETY: `self.base` is the isolated 4 KiB Device-nGnRnE EMMC page in `TTBR0`.
        unsafe { read_volatile((self.base + off) as *const u32) }
    }

    #[inline]
    fn write(&self, off: usize, val: u32) {
        // SAFETY: `self.base` is the isolated 4 KiB Device-nGnRnE EMMC page in `TTBR0`.
        unsafe { write_volatile((self.base + off) as *mut u32, val) }
    }

    fn wait_inhibit(&self, mask: u32) -> bool {
        for _ in 0..1_000_000 {
            if (self.read(STATUS) & mask) == 0 {
                return true;
            }
            core::hint::spin_loop();
        }
        false
    }

    fn wait_interrupt(&self, mask: u32) -> Result<u32, isize> {
        // Fast check in case command/transfer completed synchronously (e.g. QEMU SDHCI).
        for spins in 0..200_000u32 {
            let irpt = self.read(INTERRUPT);
            if (irpt & INT_ERROR_MASK) != 0 {
                self.write(INTERRUPT, irpt);
                return Err(errno::EIO);
            }
            if (irpt & mask) != 0 {
                self.write(INTERRUPT, irpt & mask);
                return Ok(irpt);
            }
            if spins == 50_000 {
                // Enable IRQ delivery and acknowledge line if hardware is still busy.
                self.write(IRPT_EN, mask | INT_ERROR_MASK);
                let _ = libdelta::irq_ack(self.irq_num);
            }
            core::hint::spin_loop();
        }

        // Wait on bound EL0 IRQ port if interrupt hasn't fired yet.
        let mut msg = IpcMessage::new(0);
        for _ in 0..10 {
            let irpt = self.read(INTERRUPT);
            if (irpt & INT_ERROR_MASK) != 0 {
                self.write(INTERRUPT, irpt);
                return Err(errno::EIO);
            }
            if (irpt & mask) != 0 {
                self.write(INTERRUPT, irpt & mask);
                return Ok(irpt);
            }
            if libdelta::port_recv(self.irq_port, &mut msg).is_ok() && msg.tag == MSG_IRQ_NOTIFY {
                let _ = libdelta::irq_ack(self.irq_num);
            }
        }
        Err(errno::EIO)
    }

    fn send_cmd(&self, cmdtm: u32, arg: u32) -> Result<u32, isize> {
        let inhibit = if (cmdtm & CMD_ISDATA) != 0 {
            STATUS_CMD_INHIBIT | STATUS_DAT_INHIBIT
        } else {
            STATUS_CMD_INHIBIT
        };
        if !self.wait_inhibit(inhibit) {
            return Err(errno::EIO);
        }
        self.write(INTERRUPT, 0xFFFF_FFFF);
        self.write(ARG1, arg);
        self.write(CMDTM, cmdtm);
        self.wait_interrupt(INT_CMD_DONE)?;
        Ok(self.read(RESP0))
    }

    fn set_clock(&self, div_low: u32) {
        let mut c1 = self.read(CONTROL1);
        c1 &= !CTRL1_CLK_EN;
        self.write(CONTROL1, c1);

        c1 &= !(0xFF << 8);
        c1 |= (div_low & 0xFF) << 8;
        c1 |= CTRL1_CLK_INTLEN | (0xE << 16);
        self.write(CONTROL1, c1);

        for _ in 0..100_000 {
            if (self.read(CONTROL1) & CTRL1_CLK_STABLE) != 0 {
                break;
            }
            core::hint::spin_loop();
        }

        c1 = self.read(CONTROL1) | CTRL1_CLK_EN;
        self.write(CONTROL1, c1);
    }

    fn init_card(&mut self) -> Result<(), isize> {
        // Reset host controller.
        self.write(CONTROL0, 0);
        self.write(CONTROL1, CTRL1_SRST_HC);
        for _ in 0..100_000 {
            if (self.read(CONTROL1) & CTRL1_SRST_HC) == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // Enable internal clock at slow init rate and unmask status bits.
        self.set_clock(0x40);
        self.write(IRPT_MASK, 0xFFFF_FFFF);
        self.write(IRPT_EN, 0);
        self.write(INTERRUPT, 0xFFFF_FFFF);

        // CMD0: GO_IDLE_STATE
        self.send_cmd(make_cmd(0, CMD_RSPNS_NONE), 0)?;

        // CMD8: SEND_IF_COND (2.7-3.6V, check pattern 0xAA)
        let r7 = self.send_cmd(
            make_cmd(8, CMD_RSPNS_48 | CMD_CRCCHK_EN | CMD_IXCHK_EN),
            0x0000_01AA,
        )?;
        let v2 = (r7 & 0xFFF) == 0x1AA;

        // ACMD41: SD_SEND_OP_COND with HCS=1 for SDHC/SDXC
        let hcs = if v2 { 1 << 30 } else { 0 };
        let mut ocr = 0u32;
        for _ in 0..1000 {
            self.send_cmd(make_cmd(55, CMD_RSPNS_48 | CMD_CRCCHK_EN | CMD_IXCHK_EN), 0)?;
            ocr = self.send_cmd(make_cmd(41, CMD_RSPNS_48), hcs | 0x00FF_8000)?;
            if (ocr & (1 << 31)) != 0 {
                break;
            }
            libdelta::yield_now();
        }
        if (ocr & (1 << 31)) == 0 {
            return Err(errno::EIO);
        }
        self.is_sdhc = (ocr & (1 << 30)) != 0;

        // CMD2: ALL_SEND_CID (136-bit response)
        self.send_cmd(make_cmd(2, CMD_RSPNS_136 | CMD_CRCCHK_EN), 0)?;

        // CMD3: SEND_RELATIVE_ADDR
        let rca_resp =
            self.send_cmd(make_cmd(3, CMD_RSPNS_48 | CMD_CRCCHK_EN | CMD_IXCHK_EN), 0)?;
        self.rca = rca_resp & 0xFFFF_0000;

        // CMD9: SEND_CSD (136-bit response) to read physical card capacity
        if self
            .send_cmd(make_cmd(9, CMD_RSPNS_136 | CMD_CRCCHK_EN), self.rca)
            .is_ok()
        {
            let r1 = u64::from(self.read(RESP1));
            let r2 = u64::from(self.read(RESP2));
            let r3 = u64::from(self.read(RESP3));
            let csd_struct = (r3 >> 22) & 0x3;
            self.total_sectors = if csd_struct == 1 {
                let c_size = ((r2 & 0x3F) << 16) | (r1 >> 16);
                (c_size + 1) * 1024
            } else {
                let read_bl_len = (r2 >> 8) & 0xF;
                let c_size = ((r2 & 0x3) << 10) | ((r1 >> 22) & 0x3FF);
                let c_size_mult = (r1 >> 7) & 0x7;
                let blocknr = (c_size + 1) << (c_size_mult + 2);
                (blocknr << read_bl_len) / 512
            };
        }

        // CMD7: SELECT_CARD
        self.send_cmd(
            make_cmd(7, CMD_RSPNS_48_BUSY | CMD_CRCCHK_EN | CMD_IXCHK_EN),
            self.rca,
        )?;

        if !self.is_sdhc {
            // CMD16: SET_BLOCKLEN = 512 bytes for SDSC cards
            self.send_cmd(
                make_cmd(16, CMD_RSPNS_48 | CMD_CRCCHK_EN | CMD_IXCHK_EN),
                512,
            )?;
        }

        // Switch to fast transfer clock.
        self.set_clock(0x01);
        Ok(())
    }

    fn read_sector(&self, lba: u64, dst: &mut [u8]) -> Result<(), isize> {
        if dst.len() < 512 {
            return Err(errno::EINVAL);
        }
        let addr = if self.is_sdhc {
            lba as u32
        } else {
            (lba as u32).saturating_mul(512)
        };

        self.write(BLKSIZECNT, (1 << 16) | 512);
        let cmd = make_cmd(
            17,
            CMD_RSPNS_48
                | CMD_CRCCHK_EN
                | CMD_IXCHK_EN
                | CMD_ISDATA
                | TM_DAT_DIR_READ
                | TM_BLKCNT_EN,
        );
        self.send_cmd(cmd, addr)?;
        self.wait_interrupt(INT_READ_RDY)?;

        for i in 0..128 {
            let word = self.read(DATA).to_le_bytes();
            dst[i * 4..i * 4 + 4].copy_from_slice(&word);
        }
        self.wait_interrupt(INT_DATA_DONE)?;
        Ok(())
    }

    fn write_sector(&self, lba: u64, src: &[u8]) -> Result<(), isize> {
        if src.len() < 512 {
            return Err(errno::EINVAL);
        }
        let addr = if self.is_sdhc {
            lba as u32
        } else {
            (lba as u32).saturating_mul(512)
        };

        self.write(BLKSIZECNT, (1 << 16) | 512);
        let cmd = make_cmd(
            24,
            CMD_RSPNS_48 | CMD_CRCCHK_EN | CMD_IXCHK_EN | CMD_ISDATA | TM_BLKCNT_EN,
        );
        self.send_cmd(cmd, addr)?;
        self.wait_interrupt(INT_WRITE_RDY)?;

        for i in 0..128 {
            let word =
                u32::from_le_bytes([src[i * 4], src[i * 4 + 1], src[i * 4 + 2], src[i * 4 + 3]]);
            self.write(DATA, word);
        }
        self.wait_interrupt(INT_DATA_DONE)?;
        Ok(())
    }
}

#[unsafe(no_mangle)]
fn delta_main() -> usize {
    let Ok(mmio) = libdelta::sd_mmio_info() else {
        return 1;
    };
    let Ok(irq_port) = libdelta::port_create(16) else {
        return 2;
    };
    let _ = libdelta::irq_bind(irq_port, mmio.irq_num);

    let mut sd = EmmcDriver::new(mmio.virt_base, mmio.irq_num, irq_port);
    if sd.init_card().is_err() {
        libdelta::print("[el0/drv-sd] SD card init failed!\n");
        return 3;
    }

    let Ok(svc_port) = libdelta::port_create(16) else {
        return 4;
    };
    if libdelta::service_register(SVC_SD, svc_port).is_err() {
        return 5;
    }

    if sd.is_sdhc {
        libdelta::print("[el0/drv-sd] EMMC SDHC driver online (EL0, svc.sd)\n");
    } else {
        libdelta::print("[el0/drv-sd] EMMC SDSC driver online (EL0, svc.sd)\n");
    }

    let mut msg = IpcMessage::new(0);
    loop {
        if libdelta::port_recv(svc_port, &mut msg).is_err() {
            libdelta::yield_now();
            continue;
        }

        let Some(mut pkt) = unpack_msg::<SdIoPacket>(&msg) else {
            continue;
        };

        pkt.status = match msg.tag {
            MSG_SD_INFO => {
                pkt.lba = sd.total_sectors;
                pkt.count = 512;
                0
            }
            MSG_SD_READ => handle_io(&sd, &pkt, false),
            MSG_SD_WRITE => handle_io(&sd, &pkt, true),
            _ => errno::ENOSYS as i32,
        };

        if msg.reply_port != 0 {
            let reply = pack_msg(MSG_SD_ACK, &pkt);
            let _ = libdelta::port_reply(msg.reply_port as usize, &reply);
        }
    }
}

fn handle_io(sd: &EmmcDriver, pkt: &SdIoPacket, is_write: bool) -> i32 {
    let count = pkt.count as usize;
    let needed = count.saturating_mul(512);
    if count == 0 {
        return 0;
    }
    let Ok((va, size)) = libdelta::shm_map(pkt.shm_id) else {
        return errno::EPERM as i32;
    };
    if size < needed {
        let _ = libdelta::shm_unmap(pkt.shm_id);
        return errno::EINVAL as i32;
    }

    let mut status = 0i32;
    // SAFETY: `va` is mapped with `size >= needed` bytes inside our `TTBR0`.
    let buf = unsafe { core::slice::from_raw_parts_mut(va as *mut u8, needed) };
    for s in 0..count {
        let sec_slice = &mut buf[s * 512..(s + 1) * 512];
        let res = if is_write {
            sd.write_sector(pkt.lba + s as u64, sec_slice)
        } else {
            sd.read_sector(pkt.lba + s as u64, sec_slice)
        };
        if let Err(e) = res {
            status = e as i32;
            break;
        }
    }

    let _ = libdelta::shm_unmap(pkt.shm_id);
    status
}

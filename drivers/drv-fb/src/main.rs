#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

//! Userspace (`EL0`) VideoCore Mailbox Framebuffer & thermal sensor driver (`svc.fb`).

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{Ordering, fence};

use libdelta::delta_abi::{
    FbPacket, IpcMessage, MSG_FB_ACK, MSG_FB_GET_INFO, MSG_FB_PRESENT, SVC_FB, errno, pack_msg,
    unpack_msg,
};

const FB_WIDTH: u32 = 640;
const FB_HEIGHT: u32 = 480;
const FB_BPP: u32 = 32;

const MBOX_READ: usize = 0x00;
const MBOX_STATUS: usize = 0x18;
const MBOX_WRITE: usize = 0x20;
const MBOX_FULL: u32 = 1 << 31;
const MBOX_EMPTY: u32 = 1 << 30;
const MBOX_CH_PROP: u32 = 8;

const TAG_SET_PHYS_WH: u32 = 0x0004_8003;
const TAG_SET_VIRT_WH: u32 = 0x0004_8004;
const TAG_SET_DEPTH: u32 = 0x0004_8005;
const TAG_SET_PIXEL_ORDER: u32 = 0x0004_8006;
const TAG_ALLOC_BUFFER: u32 = 0x0004_0001;
const TAG_GET_PITCH: u32 = 0x0004_0008;
const TAG_GET_TEMP: u32 = 0x0003_0006;

struct MailboxFb {
    mbox_va: usize,
    vc_bus_offset: usize,
    dma_pa: usize,
    dma_va: usize,
    fb_va: usize,
    width: u32,
    height: u32,
    pitch: u32,
    fb_bytes: usize,
    hw_active: bool,
    cached_shm_id: u64,
    cached_shm_va: usize,
    cached_shm_size: usize,
}

impl MailboxFb {
    fn mbox_read(&self, off: usize) -> u32 {
        // SAFETY: `self.mbox_va` is our isolated 4 KiB Device-nGnRnE Mailbox page.
        unsafe { read_volatile((self.mbox_va + off) as *const u32) }
    }

    fn mbox_write(&self, off: usize, val: u32) {
        // SAFETY: `self.mbox_va` is our isolated 4 KiB Device-nGnRnE Mailbox page.
        unsafe { write_volatile((self.mbox_va + off) as *mut u32, val) }
    }

    fn call_property(&mut self, words_len: usize) -> bool {
        fence(Ordering::SeqCst);
        let dma_words = self.dma_va as *mut u32;
        // SAFETY: `dma_va` is a mapped 4 KiB Non-Cacheable DMA page.
        unsafe {
            write_volatile(dma_words.add(0), (words_len * 4) as u32);
            write_volatile(dma_words.add(1), 0); // request
        }
        fence(Ordering::SeqCst);

        let bus_addr = (self.dma_pa.wrapping_add(self.vc_bus_offset) as u32) & !0xF;
        let msg = bus_addr | MBOX_CH_PROP;

        for _ in 0..500_000 {
            if (self.mbox_read(MBOX_STATUS) & MBOX_FULL) == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        self.mbox_write(MBOX_WRITE, msg);

        for _ in 0..500_000 {
            for _ in 0..500_000 {
                if (self.mbox_read(MBOX_STATUS) & MBOX_EMPTY) == 0 {
                    break;
                }
                core::hint::spin_loop();
            }
            let resp = self.mbox_read(MBOX_READ);
            if resp == msg {
                fence(Ordering::SeqCst);
                // SAFETY: `dma_va` holds the property response status word at index 1.
                let code = unsafe { read_volatile(dma_words.add(1)) };
                return code == 0x8000_0000;
            }
        }
        false
    }

    fn init_framebuffer(&mut self) {
        let dma = self.dma_va as *mut u32;
        let mut idx = 2usize;
        let put = |pos: &mut usize, v: u32| {
            // SAFETY: `*pos < 1024` inside our 4 KiB DMA buffer.
            unsafe {
                write_volatile(dma.add(*pos), v);
            }
            *pos += 1;
        };
        // Set Phys WH
        put(&mut idx, TAG_SET_PHYS_WH);
        put(&mut idx, 8);
        put(&mut idx, 8);
        put(&mut idx, FB_WIDTH);
        put(&mut idx, FB_HEIGHT);
        // Set Virt WH
        put(&mut idx, TAG_SET_VIRT_WH);
        put(&mut idx, 8);
        put(&mut idx, 8);
        put(&mut idx, FB_WIDTH);
        put(&mut idx, FB_HEIGHT);
        // Set Depth
        put(&mut idx, TAG_SET_DEPTH);
        put(&mut idx, 4);
        put(&mut idx, 4);
        put(&mut idx, FB_BPP);
        // Set Pixel Order (0 = BGR -> little-endian 0x00RRGGBB in XRGB8888 VRAM)
        put(&mut idx, TAG_SET_PIXEL_ORDER);
        put(&mut idx, 4);
        put(&mut idx, 4);
        put(&mut idx, 0);
        // Alloc Buffer
        let alloc_val_idx = idx + 3;
        put(&mut idx, TAG_ALLOC_BUFFER);
        put(&mut idx, 8);
        put(&mut idx, 4);
        put(&mut idx, 4096);
        put(&mut idx, 0);
        // Get Pitch
        let pitch_val_idx = idx + 3;
        put(&mut idx, TAG_GET_PITCH);
        put(&mut idx, 4);
        put(&mut idx, 4);
        put(&mut idx, 0);
        // End tag
        put(&mut idx, 0);

        if !self.call_property(idx) {
            return;
        }

        // SAFETY: Reading back VideoCore Mailbox response fields from our 4 KiB DMA page.
        let (bus_addr, fb_size, pitch) = unsafe {
            (
                read_volatile(dma.add(alloc_val_idx)),
                read_volatile(dma.add(alloc_val_idx + 1)),
                read_volatile(dma.add(pitch_val_idx)),
            )
        };
        let fb_pa = (bus_addr & 0x3FFF_FFFF) as usize;
        if fb_pa == 0 || fb_size == 0 || pitch == 0 {
            return;
        }
        if let Ok(va) = libdelta::map_fb_phys(fb_pa, fb_size as usize) {
            self.fb_va = va;
            self.pitch = pitch;
            self.fb_bytes = fb_size as usize;
            self.hw_active = true;
        }
    }

    fn read_soc_temp_mc(&mut self) -> i32 {
        let dma = self.dma_va as *mut u32;
        // SAFETY: `dma` maps a 4 KiB DMA page.
        unsafe {
            write_volatile(dma.add(2), TAG_GET_TEMP);
            write_volatile(dma.add(3), 8);
            write_volatile(dma.add(4), 4);
            write_volatile(dma.add(5), 0); // domain id 0
            write_volatile(dma.add(6), 0);
            write_volatile(dma.add(7), 0);
        }
        if self.call_property(8) {
            // SAFETY: Reading response word 6 from DMA page.
            let t = unsafe { read_volatile(dma.add(6)) } as i32;
            if t > 0 {
                return t;
            }
        }
        42_000
    }

    fn present_shm(&mut self, shm_id: u64, brightness: u8) -> i32 {
        if self.cached_shm_id != shm_id {
            if self.cached_shm_id != 0 {
                let _ = libdelta::shm_unmap(self.cached_shm_id);
                self.cached_shm_id = 0;
            }
            let Ok((va, size)) = libdelta::shm_map(shm_id) else {
                return errno::EPERM as i32;
            };
            self.cached_shm_id = shm_id;
            self.cached_shm_va = va;
            self.cached_shm_size = size;
        }

        let needed = (self.width * self.height * 4) as usize;
        if self.cached_shm_size < needed {
            return errno::EINVAL as i32;
        }
        if !self.hw_active || self.fb_va == 0 {
            return 0;
        }

        let scale = u32::from(brightness.clamp(20, 100));
        let row_pixels = self.width as usize;
        let dst_stride_u32 = (self.pitch as usize) / 4;
        // SAFETY: `cached_shm_va` maps at least `width * height * 4` bytes and `fb_va` maps VideoCore VRAM.
        unsafe {
            let src = self.cached_shm_va as *const u32;
            let dst = self.fb_va as *mut u32;
            for y in 0..self.height as usize {
                let s_row = src.add(y * row_pixels);
                let d_row = dst.add(y * dst_stride_u32);
                if scale == 100 {
                    core::ptr::copy_nonoverlapping(s_row, d_row, row_pixels);
                } else {
                    for x in 0..row_pixels {
                        let px = *s_row.add(x);
                        let r = (((px >> 16) & 0xFF) * scale) / 100;
                        let g = (((px >> 8) & 0xFF) * scale) / 100;
                        let b = ((px & 0xFF) * scale) / 100;
                        write_volatile(d_row.add(x), (r << 16) | (g << 8) | b);
                    }
                }
            }
        }
        0
    }
}

#[unsafe(no_mangle)]
fn delta_main() -> usize {
    let Ok(mmio) = libdelta::fb_mmio_info() else {
        return 1;
    };
    let Ok(dma) = libdelta::alloc_dma_page() else {
        return 2;
    };

    let mut fb = MailboxFb {
        mbox_va: mmio.mbox_virt_base,
        vc_bus_offset: mmio.vc_bus_offset,
        dma_pa: dma.phys_addr,
        dma_va: dma.user_va,
        fb_va: 0,
        width: FB_WIDTH,
        height: FB_HEIGHT,
        pitch: FB_WIDTH * 4,
        fb_bytes: (FB_WIDTH * FB_HEIGHT * 4) as usize,
        hw_active: false,
        cached_shm_id: 0,
        cached_shm_va: 0,
        cached_shm_size: 0,
    };
    fb.init_framebuffer();

    let Ok(port) = libdelta::port_create(16) else {
        return 3;
    };
    if libdelta::service_register(SVC_FB, port).is_err() {
        return 4;
    }

    if fb.hw_active {
        libdelta::print("[el0/drv-fb] Framebuffer 640x480@32bpp online (EL0, svc.fb)\n");
    } else {
        libdelta::print("[el0/drv-fb] Framebuffer fallback 640x480 online (EL0, svc.fb)\n");
    }

    let mut msg = IpcMessage::new(0);
    loop {
        if libdelta::port_recv(port, &mut msg).is_err() {
            libdelta::yield_now();
            continue;
        }

        let Some(mut pkt) = unpack_msg::<FbPacket>(&msg) else {
            continue;
        };

        match msg.tag {
            MSG_FB_GET_INFO => {
                pkt.width = fb.width;
                pkt.height = fb.height;
                pkt.pitch = fb.pitch;
                pkt.bpp = FB_BPP;
                pkt.shm_bytes = fb.width * fb.height * 4;
                pkt.hw_active = u8::from(fb.hw_active);
                pkt.soc_temp_mc = fb.read_soc_temp_mc();
                pkt.status = 0;
            }
            MSG_FB_PRESENT => {
                pkt.status = fb.present_shm(pkt.shm_id, pkt.brightness);
            }
            _ => pkt.status = errno::ENOSYS as i32,
        }

        if msg.reply_port != 0 {
            let reply = pack_msg(MSG_FB_ACK, &pkt);
            let _ = libdelta::port_reply(msg.reply_port as usize, &reply);
        }
    }
}

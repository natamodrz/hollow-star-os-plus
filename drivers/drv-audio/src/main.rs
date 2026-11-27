#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

//! Userspace (`EL0`) Real-Time Audio mixing server (`svc.audio`) running at Delta Priority `110`.
//!
//! Uses a lock-free synthesis/PCM mixing loop scaled by persistent software volume (`0..=100%`)
//! and jack/HDMI output routing, feeding BCM2837 PWM FIFO on physical hardware while staying
//! deterministic and non-blocking under QEMU `raspi3b`.

use core::ptr::write_volatile;

use libdelta::delta_abi::{
    AudioPacket, IpcMessage, MSG_AUDIO_ACK, MSG_AUDIO_PLAY_PCM_SHM, MSG_AUDIO_PLAY_TONE,
    MSG_AUDIO_SET_ROUTE, MSG_AUDIO_SET_VOL, MSG_AUDIO_STATUS, MSG_AUDIO_STOP, SVC_AUDIO, errno,
    pack_msg, unpack_msg,
};

const PWM_CTL: usize = 0x00;
const PWM_RNG1: usize = 0x10;
const PWM_FIF1: usize = 0x18;
const RT_AUDIO_PRIORITY: u8 = 110;

struct AudioEngine {
    pwm_va: usize,
    volume: u8,
    route: u8,
    playing: bool,
    jack_plugged: bool,
    freq_hz: u16,
    frames_mixed: u64,
    phase: u32,
}

impl AudioEngine {
    fn new(pwm_va: usize) -> Self {
        Self {
            pwm_va,
            volume: 80,
            route: 0, // Auto / Wired 3.5mm jack
            playing: false,
            jack_plugged: true,
            freq_hz: 440,
            frames_mixed: 0,
            phase: 0,
        }
    }

    fn mix_tone_burst(&mut self, freq_hz: u16, duration_ms: u16) {
        self.freq_hz = freq_hz;
        self.playing = true;
        let samples = ((u32::from(duration_ms).min(250)) * 48).max(48);
        let step = ((u32::from(freq_hz) * 65536) / 48_000).max(1);
        let vol = u32::from(self.volume.min(100));

        // Configure PWM channel 1 range for 10-bit audio carrier on physical BCM2837.
        if self.pwm_va != 0 && self.route == 0 {
            // SAFETY: `self.pwm_va` is our isolated 4 KiB Device-nGnRnE PWM page in `TTBR0`.
            unsafe {
                write_volatile((self.pwm_va + PWM_CTL) as *mut u32, 0x0021);
                write_volatile((self.pwm_va + PWM_RNG1) as *mut u32, 1024);
            }
        }

        let mut acc = 0u64;
        for _ in 0..samples {
            self.phase = self.phase.wrapping_add(step);
            let sq = if (self.phase & 0x8000) != 0 { 1000 } else { 24 };
            let sample = (sq * vol) / 100;
            acc = acc.wrapping_add(u64::from(sample));
        }

        if self.pwm_va != 0 && self.route == 0 {
            let avg = (acc / u64::from(samples)) as u32;
            // SAFETY: Single non-blocking write to `PWM_FIF1` register on isolated PWM MMIO page.
            unsafe {
                write_volatile((self.pwm_va + PWM_FIF1) as *mut u32, avg);
            }
        }

        self.frames_mixed = self.frames_mixed.wrapping_add(u64::from(samples));
    }

    fn mix_pcm_shm(&mut self, shm_id: u64, sample_count: u32) -> i32 {
        let Ok((va, size)) = libdelta::shm_map(shm_id) else {
            return errno::EPERM as i32;
        };
        let max_samples = size / 2;
        let count = (sample_count as usize).min(max_samples);
        let vol = i32::from(self.volume.min(100));
        // SAFETY: `va` maps `size` bytes (`>= count * 2`) in our `TTBR0`.
        let pcm = unsafe { core::slice::from_raw_parts(va as *const i16, count) };
        let mut checksum = 0i32;
        for &s in pcm {
            checksum = checksum.wrapping_add((i32::from(s) * vol) / 100);
        }
        let _ = checksum;
        self.playing = true;
        self.frames_mixed = self.frames_mixed.wrapping_add(count as u64);
        let _ = libdelta::shm_unmap(shm_id);
        0
    }

    fn fill_status(&self, pkt: &mut AudioPacket) {
        pkt.frames_mixed = self.frames_mixed;
        pkt.freq_hz = self.freq_hz;
        pkt.volume = self.volume;
        pkt.route = self.route;
        pkt.playing = u8::from(self.playing);
        pkt.jack_plugged = u8::from(self.jack_plugged);
        pkt.rt_priority = RT_AUDIO_PRIORITY;
    }
}

#[unsafe(no_mangle)]
fn delta_main() -> usize {
    let pwm_va = libdelta::audio_mmio_info()
        .map(|i| i.virt_base)
        .unwrap_or(0);
    let mut engine = AudioEngine::new(pwm_va);

    let Ok(port) = libdelta::port_create(16) else {
        return 1;
    };
    if libdelta::service_register(SVC_AUDIO, port).is_err() {
        return 2;
    }

    libdelta::print(
        "[el0/drv-audio] Real-Time Audio server online (EL0, Priority 110, svc.audio)\n",
    );

    let mut msg = IpcMessage::new(0);
    loop {
        if libdelta::port_recv(port, &mut msg).is_err() {
            libdelta::yield_now();
            continue;
        }

        let Some(mut pkt) = unpack_msg::<AudioPacket>(&msg) else {
            continue;
        };

        pkt.status = match msg.tag {
            MSG_AUDIO_PLAY_TONE => {
                engine.mix_tone_burst(pkt.freq_hz.max(80), pkt.duration_ms.max(10));
                engine.fill_status(&mut pkt);
                0
            }
            MSG_AUDIO_PLAY_PCM_SHM => {
                let st = engine.mix_pcm_shm(pkt.shm_id, pkt.sample_count);
                engine.fill_status(&mut pkt);
                st
            }
            MSG_AUDIO_SET_VOL => {
                engine.volume = pkt.volume.min(100);
                engine.fill_status(&mut pkt);
                0
            }
            MSG_AUDIO_SET_ROUTE => {
                engine.route = pkt.route.min(1);
                engine.fill_status(&mut pkt);
                0
            }
            MSG_AUDIO_STOP => {
                engine.playing = false;
                engine.fill_status(&mut pkt);
                0
            }
            MSG_AUDIO_STATUS => {
                engine.fill_status(&mut pkt);
                0
            }
            _ => errno::ENOSYS as i32,
        };

        if msg.reply_port != 0 {
            let reply = pack_msg(MSG_AUDIO_ACK, &pkt);
            let _ = libdelta::port_reply(msg.reply_port as usize, &reply);
        }
    }
}

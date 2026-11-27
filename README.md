# Hollow Star OS+ (Delta Kernel)

> [!WARNING]
> **Vibe-Coded Project Notice:** This project is **vibe-coded** — built and iterated rapidly with AI coding assistance (`Google Antigravity`). While every kernel milestone is tested under `qemu-system-aarch64 -M raspi3b`, treat this OS and microkernel as experimental hobby software: review code and hardware wiring before flashing physical SD cards, keep backups of important data, and report or PR any edge-case bugs you encounter.

A modular, Unix-like operating system written in **Rust** (`no_std` **Delta Microkernel** + modular EL0 userspace servers) targeting **Raspberry Pi 3B** (`BCM2837`, 4× Cortex-A53, 1 GB RAM) and **Raspberry Pi Zero 2 W** (`raspi02w`, `RP3A0` / `BCM2837`), inspired by the BeOS multimedia scheduler and capability-based microkernels (`seL4` / Mach).

---

## Demo Gallery

> Screenshots and hardware photos from `/Users/nataniel/project/demo photos/hollow-star-os-plus/` will be added under `docs/demo/` as visual/framebuffer milestones (`M6`) come online.

---

## Architecture & Implemented Features

- **Delta Microkernel (`kernel/`, EL1):**
  - Strictly hardware-agnostic kernel code — all SoC MMIO addresses are isolated in board crates (`hal/board-raspi3b`, `hal/board-raspi02w`) behind `hal-core` traits and Device Tree Blob (`DTB`) parsing.
  - Physical 4 KiB frame allocator, AArch64 MMU with W^X kernel permissions (`TTBR0_EL1` isolated per-process EL0 + `TTBR1_EL1` higher-half kernel + `NormalNc` DMA/VRAM), EL1 exception vector table, and ARM Generic Timer IRQ handling.
  - **BeOS-Inspired SMP Multimedia Scheduler (M3):** 4-core Cortex-A53 SMP with priority tiers `1..=120` including Real-Time audio/video (`100..=120`) and priority inheritance on IPC.
  - **Capability, Port IPC, Shm & IRQ Forwarding (M4–M6):** Synchronous/asynchronous port message passing (`svc.uart`, `svc.sd`, `svc.vfs`, `svc.fb`, `svc.audio`), zero-copy `Shm` buffers, userspace peripheral IRQ forwarding, and EL0 drivers.
- **Separated Storage (`DeltaFS` + 3-Partition MBR), Display, RT Audio & Desktop (`drivers/`, `fs/`, `userland/`, `apps/`):**
  - **3-Partition Layout & DeltaFS WAL Journaling (M5–M6):** Separated **`/boot`** (`FAT32`), **`/system`** (**DeltaFS** with CRC32 + Write-Ahead Journal + `.hsapp` bundle metadata), and **`/user`** (**DeltaFS** user config/notes/media with online partition expansion to end of SD card), plus `/dev` and `/proc`.
  - **Display & Real-Time Audio Servers (M6):** Isolated `drv-fb` VideoCore Mailbox `640x480@32bpp` framebuffer + SoC thermal sensor, and `drv-audio` running at Delta Scheduler **Real-Time Priority `110`**.
  - **Desktop Compositor & `1st Setup` (`HollowStar.hsapp`, M6):** Bilingual **`English` / `Polski`** UI, `1st Setup` wizard, universal **`F1`** help overlay, automatic hiding of Wi-Fi/BT/FTP controls on non-wireless builds, System Monitor (`/boot`, `/system`, `/user` stats & expand), Markdown Notes, File Explorer, RT Media Player, Calculator (`^`, `sqrt`, `F2` fractions), Classic Games (`Brick Breaker`, `Tetris`, `Racing`), and Settings.

---

## Hardware Targets

| Target | SoC / CPU | RAM | Status |
| :--- | :--- | :--- | :--- |
| **QEMU `raspi3b`** | Emulated BCM2837 (4× Cortex-A53) | 1 GB | Primary automated CI target (`./delta.sh smoke`) |
| **Raspberry Pi 3B** | BCM2837 (4× Cortex-A53) | 1 GB | Primary physical development target (`hal/board-raspi3b`) |
| **Raspberry Pi Zero 2 W** | RP3A0 / BCM2837 (4× Cortex-A53) | 512 MB | Secondary target (`hal/board-raspi02w`) |

---

## Prebuilt Downloads (`.img`)

Once versioned board images are published, downloadable `.img` binaries for `raspi3b` and `raspi02w` will be attached directly as assets under **[GitHub Releases](../../releases)** using the naming scheme:
- `hollow-star-os-plus-raspi3b-vX.Y.Z.img`
- `hollow-star-os-plus-raspi02w-vX.Y.Z.img`

*(Currently in pre-`v1.0.0` kernel milestone development — build from source below.)*

---

## Versioning Policy

This repository follows strict **Semantic Versioning** (`MAJOR.MINOR.PATCH`):
- **`MAJOR`**: Major architectural rewrites or breaking kernel/ABI changes.
- **`MINOR`**: New subsystems or user-facing features.
- **`PATCH`**: Bugfixes, refinements, and minor improvements.

---

## Build & Run (macOS / Linux + QEMU)

### Prerequisites
- Rust toolchain (`stable` with target `aarch64-unknown-none` and components `rustfmt`, `clippy`, `llvm-tools`, `rust-src` — pinned in [rust-toolchain.toml](rust-toolchain.toml))
- `qemu-system-aarch64` (e.g. `brew install qemu` on macOS)
- `python3` (used by `tools/mk_sdcard.py` to generate the test `target/sdcard.img` MBR FAT32+ext2 image)

### Commands
```bash
# Build kernel ELF and raw target/kernel8.img
./delta.sh build

# Formatting and Clippy checks (-D warnings)
./delta.sh check

# Host unit tests for hardware-independent crates (hal-core, vfs-server)
./delta.sh unit

# Non-blocking QEMU raspi3b boot smoke test
./delta.sh smoke

# Interactive QEMU GUI window with Graphics, CoreAudio, USB-Net & instant keyboard navigation
./run-gui.sh

# Full M6 RC1 verification (Graphics FB + RT Audio + USB-Net + DeltaFS + .hsapp Launcher)
./test-rc1.sh --auto

# Full CI pipeline (check + unit + smoke)
./delta.sh ci

# Interactive QEMU session (Ctrl+A then X to exit)
./run-qemu.sh
```

To test with BCM2837 Mini-UART (UART1) instead of PL011 (UART0):
```bash
cargo build --package kernel --features mini-uart --target aarch64-unknown-none
UART_MODE=mini ./run-qemu.sh target/aarch64-unknown-none/debug/kernel
```

---

## Documentation

- [Architecture Specification](docs/architecture.md)
- [Delta Scheduler Design (BeOS-Inspired)](docs/scheduler.md)
- [Milestone Roadmap (M1–M6)](docs/MILESTONES.md)
- [Planned Repository Layout](folder_structure.txt)

---

## License

Licensed under the **GNU General Public License v3.0** — see [LICENSE](LICENSE) for the full text.

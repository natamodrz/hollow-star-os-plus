# Hollow Star OS+ (Delta Kernel)

> [!WARNING]
> **Vibe-Coded Project Notice:** This project is **vibe-coded** — built and iterated rapidly with AI coding assistance (`Google Antigravity`). While every kernel milestone is tested under `qemu-system-aarch64 -M raspi3b`, treat this OS and microkernel as experimental hobby software: review code and hardware wiring before flashing physical SD cards, keep backups of important data, and report or PR any edge-case bugs you encounter.

A modular, Unix-like operating system written in **Rust** (`no_std` **Delta Microkernel** + modular EL0 userspace servers) targeting **Raspberry Pi 3B** (`BCM2837`, 4× Cortex-A53, 1 GB RAM) and **Raspberry Pi Compute Module Zero** (`CM0`, `RP3A0` / `BCM2837`), inspired by the BeOS multimedia scheduler and capability-based microkernels (`seL4` / Mach).

---

## Demo Gallery

> Screenshots and hardware photos from `/Users/nataniel/project/demo photos/hollow-star-os-plus/` will be added under `docs/demo/` as visual/framebuffer milestones (`M6`) come online.

---

## Architecture & Planned Features

- **Delta Microkernel (`kernel/`, EL1):**
  - Strictly hardware-agnostic kernel code — all SoC MMIO addresses are isolated in board crates (`hal/board-raspi3b`, future `hal/board-raspicm0`) behind `hal-core` traits and Device Tree Blob (`DTB`) parsing.
  - Physical 4 KiB frame allocator, AArch64 MMU with W^X kernel permissions (`TTBR0_EL1` identity/user + `TTBR1_EL1` higher-half kernel), EL1 exception vector table, and ARM Generic Timer IRQ handling.
  - **BeOS-Inspired SMP Multimedia Scheduler (planned M3):** Priority tiers `1..=120` including Real-Time audio/video (`100..=120`) and priority inheritance on IPC/mutexes.
  - **Capability & Port IPC (planned M4):** Synchronous/asynchronous port message passing and userspace IRQ forwarding.
- **Userspace Parity with Hollow Star OS Basic (`userland/`, `apps/`, planned M5–M6):**
  - **First-Time Setup Wizard:** Language (`English` default / `Polski`) → Wi-Fi *(wireless boards only)* → Time → Hollow Star welcome screen.
  - **Board Capability Adaptation:** Wireless/Bluetooth/FTP features are cleanly disabled and hidden from UI/menus on non-wireless builds or hardware variants without wireless radios.
  - **Core Apps:** Dual/multi-drive File Explorer, Markdown Notes (`.md` + preview), System Info Monitor, Expression Calculator, Media/MP3 Player leveraging Delta RT audio priorities, Classic Games, and universal `F1` help overlays.

---

## Hardware Targets

| Target | SoC / CPU | RAM | Status |
| :--- | :--- | :--- | :--- |
| **QEMU `raspi3b`** | Emulated BCM2837 (4× Cortex-A53) | 1 GB | Primary automated CI target (`./delta.sh smoke`) |
| **Raspberry Pi 3B** | BCM2837 (4× Cortex-A53, Wi-Fi/BT) | 1 GB | Primary physical development target |
| **Raspberry Pi CM0** | RP3A0 / BCM2837 (4× Cortex-A53, optional eMMC, CYW43439) | 512 MB | Secondary/future target (`hal/board-raspicm0`) |

---

## Prebuilt Downloads (`.img`)

Once versioned board images are published, downloadable `.img` binaries for `raspi3b` and `cm0` will be attached directly as assets under **[GitHub Releases](../../releases)** using the naming scheme:
- `hollow-star-os-plus-raspi3b-vX.Y.Z.img`
- `hollow-star-os-plus-cm0-vX.Y.Z.img`

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

### Commands
```bash
# Build kernel ELF and raw target/kernel8.img
./delta.sh build

# Formatting and Clippy checks (-D warnings)
./delta.sh check

# Host unit tests for hardware-independent crates (hal-core)
./delta.sh unit

# Non-blocking QEMU raspi3b boot smoke test
./delta.sh smoke

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

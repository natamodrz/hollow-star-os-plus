# Hollow Star OS+ — Project Context (Delta Microkernel in Rust)

## Vision & Design Principles
**Hollow Star OS+** is a modular, Unix-like operating system written in **Rust** (`no_std` kernel + modular userspace).
- **Kernel Name:** **Delta Kernel** (strict microkernel architecture).
- **Primary Development Target:** Raspberry Pi 3B (`aarch64-unknown-none`, BCM2837, 4x Cortex-A53, 1 GB RAM) + **QEMU `raspi3b`** for automated testing.
- **Secondary / Future Target:** Raspberry Pi Zero 2 W (`raspi02w`, RP3A0 / BCM2837, 4x Cortex-A53, 512 MB RAM, microSD, CYW43439) and other ARM64/RISC-V SBCs.
- **Portability First:** Never hardcode board-specific MMIO addresses inside `kernel/`. Always go through `hal-core` traits and Device Tree Blob (DTB) parsing so porting from RPi 3B to Zero 2 W or another SBC only requires a board crate / DTB.

## Core Architecture (Modular Microkernel)
1. **Delta Kernel (`kernel/`):** Minimal trusted computing base running in EL1:
   - Physical & Virtual Memory Manager (4 KB pages, per-process TTBR0 address spaces).
   - Synchronous/Asynchronous IPC (message passing + capability-based ports/endpoints, inspired by BeOS ports & seL4/Mach).
   - Interrupt routing to userspace driver servers.
   - **BeOS-Inspired Multimedia Scheduler:**
     - Pervasive multithreading across all 4 Cortex-A53 cores (SMP).
     - Priority tiers (e.g., `1..=120`: Idle, Low, Normal/Interactive `10`, Display/UI `15`, Urgent `20`, Real-Time Audio/Video `100..=120`).
     - Low-latency preemption + short quantum for interactive/RT threads + priority inheritance on IPC/mutexes to prevent audio/video glitches.
2. **Modular Userspace Servers (`drivers/`, `fs/`):**
   - Drivers (UART, Framebuffer, SD/eMMC, USB, Audio, Wi-Fi/BT) and Filesystems (VFS, FAT32, ext2) run in EL0 and communicate via Delta IPC.
3. **Unix-like Userland (`userland/`, `apps/`):**
   - `libdelta` providing clean Rust APIs and a POSIX/Unix-like compatibility layer (file descriptors, signals, `/dev`, `/proc`, hierarchical VFS).
   - Clean separation between system apps (shell, file manager, settings, compositor) and user apps (Markdown editor, media player, games).

## Engineering & Coding Rules for Agents
- **Language:** Rust (nightly/stable with `aarch64-unknown-none` target; document exact toolchain in `rust-toolchain.toml`).
- **Safety:** Minimize `unsafe` blocks; every `unsafe` block must have a `// SAFETY:` comment explaining the invariant.
- **Verification:** Every kernel milestone must boot cleanly and pass smoke tests under `qemu-system-aarch64 -M raspi3b` before testing on physical hardware.
- **Documentation:** Keep `docs/architecture.md` and `docs/scheduler.md` updated whenever IPC syscalls, memory maps, or scheduling policies change.

## Milestone Roadmap
- **M1 (Boot & Serial):** EL2 -> EL1 drop, Rust entry point, Mini-UART/PL011 output in QEMU `raspi3b` & real RPi 3B, DTB parsing.
- **M2 (Memory & Exceptions):** Exception vector table, ARM Generic Timer IRQ, physical frame allocator, MMU + identity/higher-half paging.
- **M3 (Delta Scheduler & SMP):** Context switching (AArch64), BeOS-style priority scheduler with RT tier, wake up secondary Cortex-A53 cores.
- **M4 (EL0 & IPC):** Drop to EL0 userspace, syscall ABI, port-based message passing, initial `init` + userspace UART server.
- **M5 (Storage & VFS):** Userspace SD/EMMC driver, VFS server, FAT32 filesystem, ELF binary loader.
- **M6 (Multimedia & UI):** Framebuffer/HDMI compositor, audio pipeline leveraging RT scheduler priorities, shell & core apps.

## Build & Automated Testing (macOS / QEMU)
- Build kernel ELF + raw `target/kernel8.img`: `./delta.sh build`
- Fast static checks (`fmt` + `clippy`): `./delta.sh check`
- Host unit tests for hardware-independent crates: `./delta.sh unit`
- Non-blocking QEMU `raspi3b` boot smoke test: `./delta.sh smoke`
- Full verification pipeline before completing a milestone: `./delta.sh ci`
- Note: Use `UART_MODE=mini` if Mini-UART (UART1) is used instead of PL011 (UART0). Never run `./delta.sh run` interactively from an automated agent turn — always use `./delta.sh smoke`.

## Cross-Agent Ecosystem Memory & Sync Rules
- Shared ecosystem memory lives at `/Users/nataniel/project/memory/` (`INDEX.md`, `CONVENTIONS.md`, `FEATURES_AND_UX.md`, `PROJECT_PLUS.md`, `PROJECT_STANDARD.md`, `PROJECT_BASIC.md`). **On every task**, automatically re-read BOTH local (`PROJECT_PLUS.md`) and global shared memory (`INDEX.md`, `FEATURES_AND_UX.md`, `CONVENTIONS.md`, `PROJECT_STANDARD.md`, `PROJECT_BASIC.md`) as well as `/Users/nataniel/project/bugs/hollow-star-os-plus/`, moving any verified resolved bugs into `resolved/`.
- `userland/delta-abi`, `fs/src/deltafs.rs`, and `.hsapp` layout here are the canonical upstream ABI for `hollow-star-os` (x86/x86_64 netbook edition with ARM64 translation). Whenever updating ABI, UI conventions, bugs, or milestones, update `/Users/nataniel/project/memory/PROJECT_PLUS.md` (and `FEATURES_AND_UX.md` if shared UX changes) directly — no manual sync prompt block is needed at the end of responses.

## Imprtant! Always read folder_structure.txt for the planned directory layout and keep it updated when adding new top-level modules.

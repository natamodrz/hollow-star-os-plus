# Architektura Hollow Star OS+ (Delta Microkernel)

## 1. Wizja i zasady projektowe

**Hollow Star OS+** to modularny, uniksopodobny system operacyjny napisany w języku **Rust** (`no_std` dla jądra i warstwy HAL, modularne serwery w przestrzeni użytkownika).

- **Jądro:** **Delta Kernel** — ścisłe mikrojądro działające w trybie uprzywilejowanym **EL1** (AArch64).
- **Minimalne TCB (Trusted Computing Base):** w EL1 pozostaje wyłącznie to, co niezbędne do izolacji pamięci, komunikacji międzyprocesowej (IPC), przełączania wątków oraz dystrybucji przerwań sprzętowych.
- **Przenośność przede wszystkim:** kod w `kernel/` nie może zawierać zaszytych na stałe adresów MMIO specyficznych dla danego SoC. Dostęp do sprzętu odbywa się poprzez cechy w `hal/hal-core/`, implementacje płytek (`hal/board-raspi3b/`, docelowo `hal/board-raspi02w/`) oraz parsowanie Device Tree Blob (DTB).

---

## 2. Podział warstwowy systemu

```
+-----------------------------------------------------------------------+
|  apps/       Aplikacje systemowe i użytkownika (Shell, Editor, Media) |
+-----------------------------------------------------------------------+
|  userland/   libdelta (Rust API + warstwa kompatybilności POSIX/Unix) |
|              init, powłoka, usługi systemowe (/dev, /proc)            |
+-----------------------------------------------------------------------+
|  drivers/ & fs/   Serwery w przestrzeni użytkownika (EL0)             |
|              drv-uart | drv-fb | drv-sd | drv-audio | VFS/FAT32/ext2  |
+=======================================================================+
|              Granica wywołań systemowych (SVC EL0 -> EL1)             |
+=======================================================================+
|  kernel/     Delta Kernel (EL1)                                       |
|              - Zarządzanie pamięcią fizyczną i wirtualną (MMU 4 KiB)  |
|              - Planista SMP inspirowany BeOS (wątki + priorytety RT)  |
|              - IPC oparte na portach i capabilities (seL4 / BeOS)     |
|              - Routing przerwań (IRQ) do serwerów EL0                 |
+-----------------------------------------------------------------------+
|  hal/        hal-core (cechy: SerialConsole, Board, InterruptController,  |
|              Timer, MemoryRegion, Dtb) + board-raspi3b (BCM2837 MMIO)|
+-----------------------------------------------------------------------+
|  Sprzęt      Raspberry Pi 3B (BCM2837) / QEMU raspi3b / RPi Zero 2 W  |
+-----------------------------------------------------------------------+
```

---

## 3. Model pamięci i wyjątków (M2 zaimplementowane)

- **Rozmiar strony:** 4 KiB (`ARMv8-A` 4-poziomowe tablice stron, 48-bitowa przestrzeń VA, `T0SZ=16`, `T1SZ=16`, dynamiczny `IPS` z `ID_AA64MMFR0_EL1`).
- **Podział przestrzeni adresowej:**
  - `TTBR0_EL1` (dolna połowa `0x0000_...`): aktualnie mapa tożsamościowa jądra i urządzeń MMIO; docelowo od M4 prywatna przestrzeń adresowa procesu użytkownika (EL0).
  - `TTBR1_EL1` (górna połowa `0xFFFF_0000_0000_0000 + phys`): bezpośrednie odwzorowanie pamięci fizycznej oraz sekcji Delta Kernel (EL1), niedostępne z EL0 (`AP` + `UXN`).
- **Polityka W^X oraz atrybuty `MAIR_EL1`:**
  - `.text` (`__text_start..__text_end`): `RO + X` (`Normal WB/WA`, `UXN=1`, `PXN=0`).
  - `.rodata` (`__rodata_start..__rodata_end`) oraz obszar poniżej `0x80000`: `RO + XN` (`PXN=1`, `UXN=1`).
  - `.data`, `.bss`, `.stack` (`__rw_start..__kernel_end`) oraz wolny RAM: `RW + XN` (`PXN=1`, `UXN=1`).
  - Regiony MMIO (`MemoryKind::DeviceMmio`, np. BCM2837 `0x3F00_0000..0x4000_0000` oraz ARM Local `0x4000_0000..0x4002_0000`): `Device-nGnRnE` (`RW + XN`).
- **Fizyczny alokator ramek (PMM):** bitmapowy alokator ramek `4 KiB` (`kernel/src/mem/pmm.rs`) chroniony przez `IrqSpinLock`, automatycznie rezerwujący obszar firmware (`0..0x80000`), obraz jądra, opcjonalny DTB oraz MMIO.
- **Wyjątki i zegar systemowy (`VBAR_EL1`):** 2 KiB-wyrównana tablica 16 wektorów w `kernel/src/arch/aarch64/vectors.S` zapisująca pełny `TrapFrame` (`x0..x30`, `ELR_EL1`, `SPSR_EL1`, `ESR_EL1`, `FAR_EL1`), obsługująca przerwania `CNTPNSIRQ` (ARM Generic Timer `100 Hz` przez `hal_core::InterruptController` i `hal_core::Timer`).

---

## 4. Model IPC i Capabilities (plan docelowy M4+)

Komunikacja między serwerami sterowników, systemami plików i aplikacjami opiera się na **portach komunikatów** (inspirowanych portami BeOS) połączonych z **prawami dostępu (capabilities)** (inspirowanymi seL4/Mach):

1. **Porty / Endpointy:**
   - Wątek posiadający capability `Send` lub `Receive` do portu może przesyłać krótkie komunikaty synchroniczne (przekazywane przez rejestry CPU) lub buforowane komunikaty asynchroniczne.
   - Przekazywanie dużych buforów (np. klatki wideo, bloki dyskowe, bufory audio) będzie używać współdzielonych regionów pamięci przekazywanych przez granty/capabilities zamiast wielokrotnego kopiowania.
2. **Obsługa przerwań w przestrzeni użytkownika (EL0):**
   - Jądro w EL1 odbiera przerwanie sprzętowe, maskuje źródło IRQ w kontrolerze przerwań i sygnalizuje powiązany port powiadomień serwera sterownika w EL0.
   - Sterownik (np. `drv-sd`, `drv-uart`) wykonuje faktyczną obsługę urządzenia w przestrzeni nieuprzywilejowanej.

---

## 5. Ścieżka rozwojowa (Milestones)

Szczegółowy opis wszystkich kamieni milowych znajduje się w [docs/MILESTONES.md](MILESTONES.md):

- **M1 (Boot & Serial) — ukończony:** szkielet workspace, abstrakcja `hal-core` + `board-raspi3b`, zejście `EL2 -> EL1`, UART (PL011 / Mini-UART), parser DTB, smoke test w QEMU `raspi3b`.
- **M2 (Memory & Exceptions) — ukończony:** wektory wyjątków `VBAR_EL1`, ARM Generic Timer IRQ (`100 Hz`), PMM `4 KiB`, MMU `TTBR0`/`TTBR1` (wyższa połowa + W^X).
- **M3 (Delta Scheduler & SMP) — następny etap:** przełączanie kontekstu AArch64, wielowątkowość na 4 rdzeniach Cortex-A53, planista z warstwą RT.
- **M4 (EL0 & IPC):** przejście do EL0, ABI wywołań systemowych (`svc`), porty IPC, pierwszy proces `init` i serwer UART w userlandzie.
- **M5 (Storage & VFS):** serwer SD/eMMC, VFS, FAT32, ładowanie plików ELF.
- **M6 (Multimedia & UI):** kompozytor graficzny (Framebuffer/HDMI), potok audio wykorzystujący priorytety RT, powłoka i aplikacje.

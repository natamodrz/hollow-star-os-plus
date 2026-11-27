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

## 3. Model pamięci i wyjątków (M2 + M4 higher-half)

- **Rozmiar strony:** 4 KiB (`ARMv8-A` 4-poziomowe tablice stron, 48-bitowa przestrzeń VA, `T0SZ=16`, `T1SZ=16`, dynamiczny `IPS` z `ID_AA64MMFR0_EL1`).
- **Podział przestrzeni adresowej (aktywny od M4):**
  - `TTBR1_EL1` (górna połowa `0xFFFF_0000_0000_0000 + phys`): pełna przestrzeń jądra Delta Kernel (EL1) — kod (`0xFFFF_0000_0008_0000+`), stosy jądra, `VBAR_EL1`, physmap oraz MMIO (`AP` tylko EL1, `UXN=1`).
  - `TTBR0_EL1` (dolna połowa `0x0000_0000_0000_0000 .. 0x0000_8000_0000_0000`): w pełni izolowana przestrzeń adresowa aktywnego procesu użytkownika (EL0), przełączana przy zmianie kontekstu (`switch_ttbr0`), pusta w wątkach samego jądra. Wszystkie strony EL0 mają ustawiony bit `PXN=1`.
- **Polityka W^X oraz atrybuty `MAIR_EL1`:**
  - `.text` (`__text_start..__text_end`): `RO + X` (`Normal WB/WA`, `UXN=1`, `PXN=0`).
  - `.rodata` (`__rodata_start..__rodata_end`) oraz obszar poniżej `0x80000`: `RO + XN` (`PXN=1`, `UXN=1`).
  - `.data`, `.bss`, `.stack` (`__rw_start..__kernel_end`) oraz wolny RAM: `RW + XN` (`PXN=1`, `UXN=1`).
  - Regiony MMIO (`MemoryKind::DeviceMmio`, np. BCM2837 `0x3F00_0000..0x4000_0000` oraz ARM Local `0x4000_0000..0x4002_0000`): `Device-nGnRnE` (`RW + XN`).
- **Fizyczny alokator ramek (PMM):** bitmapowy alokator ramek `4 KiB` (`kernel/src/mem/pmm.rs`) chroniony przez `IrqSpinLock`, automatycznie rezerwujący obszar firmware (`0..0x80000`), obraz jądra, opcjonalny DTB oraz MMIO.
- **Wyjątki i zegar systemowy (`VBAR_EL1`):** 2 KiB-wyrównana tablica 16 wektorów w `kernel/src/arch/aarch64/vectors.S` zapisująca pełny `TrapFrame` (`x0..x30`, `ELR_EL1`, `SPSR_EL1`, `ESR_EL1`, `FAR_EL1`), obsługująca przerwania `CNTPNSIRQ` (ARM Generic Timer `100 Hz` przez `hal_core::InterruptController` i `hal_core::Timer`).

---

## 4. Model IPC, Capabilities i Syscalle (M4 zaimplementowane)

Komunikacja między serwerami sterowników, systemami plików i aplikacjami opiera się na **portach komunikatów** (inspirowanych portami BeOS) połączonych z **prawami dostępu (capabilities)** (inspirowanymi seL4/Mach):

1. **ABI wywołań systemowych (`userland/delta-abi`, `svc #0`):**
   - `x8` = numer wywołania (`Exit=0`, `Yield=1`, `GetPid=2`, `PortCreate=10`, `PortSend=11`, `PortRecv=12`, `PortCall=13`, `PortReply=14`, `ServiceRegister=20`, `ServiceLookup=21`, `GetUartMmioInfo=30`), `x0..x5` = argumenty, `x0` = wynik / `-errno`.
   - Jądro bezpiecznie waliduje wskaźniki z EL0 (`copy_from_user` / `copy_to_user`) w tablicach `TTBR0_EL1` przed dostępem z EL1.
2. **Porty, Capabilities i Priority Inheritance (`kernel/src/ipc/`):**
   - Stałe 128-bajtowe komunikaty `IpcMessage` (`tag`, `sender_pid` nadawany przez jądro, `reply_port`, `len`, `data[96]`).
   - `PortCall` tworzy jednorazowy port odpowiedzi i automatycznie przekazuje jednorazową capability `SEND` odbiorcy w `PortRecv`, podnosząc priorytet serwera (Priority Inheritance) na czas obsługi żądania.
   - Rejestr nazw usług (`svc.uart`) wydaje klientom wyłącznie capability `SEND`.
3. **Izolacja sterowników urządzeń w EL0 (`drivers/drv-uart`):**
   - Wyłącznie serwer `drv-uart` otrzymuje mapowanie 4 KiB strony MMIO UART (`0x0000_1000_0000`, `Device-nGnRnE`, `PXN=1`, `UXN=1`) pozyskanej z `Board::uart_mmio_region()`. Proces `init` nie ma dostępu do MMIO i wypisuje logi wyłącznie przez IPC.
   - Przekazywanie przerwań sprzętowych (IRQ forwarding) i współdzielone bufory pamięci dla sterowników blokowych/multimedialnych zostaną rozszerzone w M5/M6.

---

## 5. Format paczek aplikacji (`.hsapp`)

Aplikacje w przestrzeni użytkownika (`apps/`) są dystrybuowane i uruchamiane jako samowystarczalne katalogi-paczki wzorowane na paczkach `.app` z systemu macOS:

```text
<AppName>.hsapp/
├── Assets/       # Biblioteki, ikony wewnętrzne, tekstury, dźwięki i zasoby aplikacji
├── exec          # Główny plik wykonywalny programu (binarka ELF AArch64 dla EL0)
└── icon.png      # Ikona aplikacji wyświetlana przez launcher i eksplorator plików
```

- **Uruchamianie (od M5/M6):** loader ELF oraz launcher systemowy rozpoznają katalogi z rozszerzeniem `.hsapp`, ładują binarkę `<AppName>.hsapp/exec` do nowej przestrzeni `TTBR0_EL1` w EL0 i udostępniają ścieżkę do `<AppName>.hsapp/Assets/` jako katalog zasobów procesu.
- **Prezentacja w UI:** eksplorator plików oraz menu startowe traktują `<AppName>.hsapp` jako pojedynczy program z ikoną `icon.png` (z możliwością podglądu zawartości paczki jako katalogu na żądanie).

---

## 6. Ścieżka rozwojowa (Milestones)

Szczegółowy opis wszystkich kamieni milowych znajduje się w [docs/MILESTONES.md](MILESTONES.md):

- **M1 (Boot & Serial) — ukończony:** szkielet workspace, abstrakcja `hal-core` + `board-raspi3b`, zejście `EL2 -> EL1`, UART (PL011 / Mini-UART), parser DTB, smoke test w QEMU `raspi3b`.
- **M2 (Memory & Exceptions) — ukończony:** wektory wyjątków `VBAR_EL1`, ARM Generic Timer IRQ (`100 Hz`), PMM `4 KiB`, MMU `TTBR0`/`TTBR1` (wyższa połowa + W^X).
- **M3 (Delta Scheduler & SMP) — ukończony:** przełączanie kontekstu AArch64 (`CpuContext`), wybudzenie wszystkich 4 rdzeni Cortex-A53 (`_start_secondary` + spin-table), kolejki priorytetowe `1..=120` z warstwą Real-Time i dziedziczeniem priorytetów.
- **M4 (EL0 & IPC) — ukończony:** pełny higher-half kernel w `TTBR1_EL1`, izolowane przestrzenie `TTBR0_EL1` ładowane przez loader ELF64, ABI `svc #0`, porty IPC z capabilities i PI, proces `init` komunikujący się z serwerem `drv-uart` w EL0.
- **M5 (Storage & VFS) — następny etap:** serwer SD/eMMC (`drv-sd`), IRQ forwarding do EL0, VFS, FAT32/ext2, ładowanie plików ELF i paczek `.hsapp` z systemu plików.
- **M6 (Multimedia & UI):** kompozytor graficzny (Framebuffer/HDMI), potok audio wykorzystujący priorytety RT, powłoka i aplikacje.

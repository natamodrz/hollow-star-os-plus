# Roadmap kamieni milowych — Hollow Star OS+ (Delta Kernel)

Dokument definiuje wszystkie kamienie milowe projektu od pierwszego uruchomienia kodu w Ruście na QEMU `raspi3b` po kompletny system mikrojądrowy z interfejsem graficznym i multimediami czasu rzeczywistego.

> Zasada nadrzędna dla wszystkich etapów: kod w `kernel/` nie zawiera adresów MMIO specyficznych dla płytki (`BCM2837` / `RP3A0`), a każdy kamień milowy przechodzi `./delta.sh ci` (`fmt`, `clippy -D warnings`, testy jednostkowe `hal-core` oraz smoke test w `qemu-system-aarch64 -M raspi3b`).

---

## Przegląd statusu

| Kamień | Nazwa | Status | Kluczowy cel |
| :--- | :--- | :--- | :--- |
| **M1** | Boot & Serial | Ukończony | Zejście `EL2 -> EL1`, BSS, `hal-core` + `board-raspi3b`, baner UART, parser DTB |
| **M2** | Memory & Exceptions | Ukończony | Wektory wyjątków `VBAR_EL1`, ARM Generic Timer IRQ, PMM (4 KiB), MMU (`TTBR0`/`TTBR1`) |
| **M3** | Delta Scheduler & SMP | Ukończony | Przełączanie kontekstu AArch64, wielowątkowość na 4 rdzeniach Cortex-A53, priorytety `1..=120` + RT |
| **M4** | EL0 & IPC | Ukończony | Higher-half `TTBR1`, izolowane `TTBR0` ELF64, ABI `svc #0`, porty IPC + PI, `init` + `drv-uart` |
| **M5** | Storage & VFS | Ukończony | Serwer SD/eMMC (`drv-sd`), IRQ forwarding, `Shm`, VFS (`FAT32`/`ext2`), `/BIN/shell.elf` i `.hsapp` |
| **M6** | Multimedia, DeltaFS & UI | Ukończony | **DeltaFS** + 3 partycje (`boot`/`system`/`user`), `drv-fb` + `drv-audio` RT (`110`), `board-raspi02w`, `HollowStar.hsapp` |
| **1.0.0** | Production Release & SD Install | Następny etap | Poradnik flashowania karty SD, opcjonalny CYW43 Wi-Fi/BT/FTP, scalenie `testing -> main`, wydania GitHub Releases |

---

## Szczegóły kamieni milowych

### M1 — Boot & Serial *(Ukończony)*
- **Zakres:**
  - Szkielet Cargo Workspace (`kernel/`, `hal/hal-core/`, `hal/board-raspi3b/`), `rust-toolchain.toml`, `.cargo/config.toml` (`aarch64-unknown-none`), `delta.sh` i `run-qemu.sh`.
  - Cechy `SerialConsole` i `Board` w `hal-core` oraz bezalokacyjny parser FDT/DTB (`Dtb`).
  - Odizolowanie adresów MMIO BCM2837 (`0x3F00_0000`) i skryptu linkera (`0x80000`) w `board-raspi3b`.
  - AArch64 `_start`: parkowanie rdzeni pomocniczych (`MPIDR_EL1`), zejście `EL3 -> EL2 -> EL1h`, odblokowanie FP/SIMD, osobna sekcja stosu `.stack`, wyzerowanie `.bss`, baner `"Hollow Star OS+ / Delta Kernel"`.
  - Obsługa zarówno `PL011 (UART0)`, jak i opcjonalnego `Mini-UART (UART1)`.
- **Kryterium akceptacji:** `./delta.sh ci` zalicza `fmt`, `clippy`, 4 testy `hal-core` oraz smoke test w QEMU `raspi3b`.

---

### M2 — Memory & Exceptions *(Ukończony)*
- **Zakres:**
  - **Tablica wektorów wyjątków EL1 (`VBAR_EL1`):** 16 wpisów wyrównanych do 2 KiB, pełny zapis/odtworzenie `TrapFrame` (`x0..x30`, `ELR_EL1`, `SPSR_EL1`, `ESR_EL1`, `FAR_EL1`), obsługa wyjątków synchronicznych i IRQ.
  - **ARM Generic Timer & kontroler przerwań:** cechy `InterruptController` oraz `Timer` w `hal-core`; implementacja dla BCM2837 ARM Local Peripherals (`0x4000_0000`, `CORE0_TIMER_IRQCNTL` / `CORE0_IRQ_SOURCE`) oraz `CNTP_*_EL0` w `board-raspi3b`; periodyczne przerwania zegarowe `100 Hz` (`10 ms`).
  - **Fizyczny alokator ramek (PMM):** alokator bitmapowy stron `4 KiB` oparty o regiony pamięci z `Board` / `DTB`, chroniący obszar startowy firmware (`0..0x80000`), obraz jądra oraz obszary MMIO.
  - **MMU i stronicowanie (`TTBR0_EL1` + `TTBR1_EL1`):** 4-poziomowe tablice stron (granulacja `4 KiB`), ścisłe uprawnienia W^X dla sekcji jądra (`.text` RX, `.rodata` RO+XN, `.data/.bss/.stack` RW+XN), pamięć `Device-nGnRnE` dla MMIO, mapa tożsamościowa w `TTBR0` oraz odbicie wyższej połówki (`0xFFFF_0000_0000_0000 + phys`) w `TTBR1`.
- **Kryterium akceptacji:** zaliczone `./delta.sh ci` (6 testów jednostkowych `hal-core` + smoke test w QEMU `raspi3b` z włączonym MMU, zweryfikowanym odczytem z wyższej połówki `TTBR1`, testem PMM i obsługą przerwań zegara `62.5 MHz`).

---

### M3 — Delta Scheduler & SMP *(Ukończony)*
- **Zakres:**
  - Przełączanie kontekstu AArch64 (`CpuContext`: `x19..x30`, `sp` w `context.S` + `thread_entry_trampoline`) z bezpiecznym, wolnym od wyścigów odroczonym zwrotem wątku wychodzącego (`pending_requeue`).
  - Struktura `ThreadControlBlock` oraz kolejka priorytetowa w skali BeOS `1..=120` (`Idle 1`, `Low 5`, `Normal 10`, `Display 15`, `Urgent 20`, `Real-Time 100..=120`), obsługa `boost_priority` / `clear_priority_boost` i zwalnianie stosów po zakończeniu wątku.
  - Wybudzenie rdzeni `1..=3` Cortex-A53 (`Board::wake_secondary_core`, spin-table `0xE0/0xE8/0xF0` + `dc civac` + `sev`, `_start_secondary` z wspólnym MMU `TTBR0`/`TTBR1`, `VBAR_EL1` i zegarem `100 Hz` per rdzeń).
  - Preempting z niskim opóźnieniem na ticku timera oraz priorytetowe kolejkowanie wątków RT.
- **Kryterium akceptacji:** zaliczone `./delta.sh ci` oraz test w QEMU `raspi3b` potwierdzający uruchomienie `4/4` rdzeni Cortex-A53 (`exec_mask=0b1111`) i ścisłą kolejność `RT(110) -> PI(100) -> UI(15) -> Normal(10) -> Low(5)`.

---

### M4 — EL0 & IPC *(Ukończony)*
- **Zakres:**
  - Przeniesienie całego jądra do wyższej połówki `TTBR1_EL1` (`0xFFFF_0000_0008_0000+`), pozostawiając `TTBR0_EL1` wyłącznie dla izolowanych procesów EL0 (`PXN=1`).
  - Loader binarnych plików **ELF64 AArch64** (`kernel/src/proc/elf.rs`), zapis/odtwarzanie `SP_EL0`, bezpieczne zejście `enter_usermode` (`eret` do `EL0t`) oraz walidowane kopiowanie pamięci `copy_from_user` / `copy_to_user`.
  - Współdzielone ABI (`userland/delta-abi`, `userland/libdelta`) oraz obsługa wywołań systemowych (`svc #0`, `EC=0x15`).
  - Porty IPC oparte na capabilities (`PortCreate`, `PortSend`, `PortRecv`, `PortCall`, `PortReply`, `ServiceRegister`, `ServiceLookup`) z jednorazową capability odpowiedzi i Priority Inheritance.
  - Pierwsze procesy EL0: serwer `drivers/drv-uart` z odizolowanym mapowaniem 4 KiB UART MMIO oraz `userland/init` wysyłający komunikaty przez IPC `svc.uart`.
- **Kryterium akceptacji:** zaliczone `./delta.sh ci` oraz start `drv-uart` (`pid=1`) i `init` (`pid=2`) w EL0 pod QEMU `raspi3b` z potwierdzonym synchronicznym ACK przez IPC i czystym wyjściem `init` (`exited=0`).

---

### M5 — Storage & VFS *(Ukończony)*
- **Zakres:**
  - Obiekty pamięci współdzielonej `Shm` (`kernel/src/shm.rs`: `ShmCreate`, `ShmAllowPort`, `ShmMap`, `ShmUnmap`) z mapowaniem bez wykonywania kodu oraz stroną strażniczą dla bezpiecznego I/O zero-copy.
  - Przekazywanie przerwań sprzętowych (`hal_core::InterruptController::{enable_peripheral_irq, disable_peripheral_irq}`, GPU IRQ 62 w `board-raspi3b`, syscalle `IrqBind`/`IrqAck`, zdarzenia `MSG_IRQ_NOTIFY`).
  - Sterownik SD/eMMC w przestrzeni użytkownika (`drivers/drv-sd`, usługa `svc.sd` na BCM2837 Arasan EMMC z odizolowaną stroną MMIO `Board::sd_mmio_region()`).
  - Serwer VFS w przestrzeni użytkownika (`fs/`, binarka `vfsd`, usługa `svc.vfs`) z obsługą tabeli partycji MBR, systemu plików **FAT32** (LFN + odczyt/zapis plików pod `/`), **ext2** (odczyt pod `/ext2`), `/dev` (`uart0`, `sd0`) oraz `/proc` (`/proc/mounts`).
  - Dynamiczne uruchamianie procesów z VFS (`SpawnElf`, `WaitPid`), obsługa odczytu UART (`MSG_UART_READ`), interaktywna powłoka `/BIN/shell.elf` (`userland/shell`) oraz paczka `/Hello.hsapp/` (`apps/hello` -> `exec` + `Assets/` + `icon.png`) na obrazie `target/sdcard.img` generowanym przez `tools/mk_sdcard.py`.
  - Poprawka synchronizacji SMP w `kernel/src/sched/mod.rs` (`prepare_wait`, `on_cpu`, `pending_wakeup`), eliminująca wyścig stosu przy współbieżnym `unblock`/`switch_context` podczas intensywnego IPC.
- **Kryterium akceptacji:** zaliczone `./delta.sh ci` (`fmt`, `clippy -D warnings`, testy jednostkowe `hal-core` i `vfs-server`), uruchomienie `/BIN/shell.elf` oraz `/Hello.hsapp/exec` z obrazu SD w QEMU `raspi3b`, odczyt `/ext2/welcome.txt` i zapis/odczyt `/M5_TEST.TXT` na FAT32.

---

### M6 — Multimedia, DeltaFS & UI *(Ukończony — `v0.6.0-rc.1`)*
- **Zakres:**
  - **System plików DeltaFS (`fs/src/deltafs.rs`) + separacja 3 partycji (`tools/mk_sdcard.py`):**
    - Partycja 1 (`0x0C` FAT32) → `/boot` (pliki startowe RPi); Partycja 2 (`0xDF` DeltaFS) → `/system` (`/system/bin/shell.elf`, `/system/apps/*.hsapp`); Partycja 3 (`0xDF` DeltaFS) → `/user` (konfiguracja `os.cfg`, notatki `.md`, media, dane użytkownika).
    - DeltaFS v1 z blokami `4 KiB`, sumą CRC32, ekstensjami, atrybutami inspirowanymi BeOS BFS (`KIND_HSAPP`), dziennikiem Write-Ahead Log (WAL z automatycznym odtwarzaniem po awarii przy montowaniu) oraz dynamicznym rozszerzaniem `/user` do końca karty SD (`expand_to_blocks` + aktualizacja MBR).
  - **Wyświetlacz i dźwięk czasu rzeczywistego (`drivers/drv-fb`, `drivers/drv-audio`):**
    - `drv-fb` (`Priority::DISPLAY = 15`) inicjuje framebuffer `640x480@32bpp` przez VideoCore Mailbox, mapuje VRAM jako `NormalNc`, odczytuje temperaturę SoC i prezentuje klatki z `Shm` z regulacją jasności.
    - `drv-audio` działa na priorytecie **Real-Time (`110`)**, mieszając dźwięk i obsługując routing Jack 3.5 mm / HDMI oraz głośność `0–100%`.
  - **HAL dla Zero 2 W i adaptacja funkcji (`hal/board-raspi02w`):**
    - Dodano crate `board-raspi02w` współdzielący prymitywy BCM2837/RP3A0 z `board-raspi3b` oraz syscall `GetSysCaps`, dzięki czemu UI automatycznie ukrywa kontrolki Wi-Fi/BT/FTP, gdy moduł radiowy/sterownik jest niedostępny.
  - **Launcher 2D, Desktop i Aplikacje (`apps/hollow-star` -> `HollowStar.hsapp/exec`) + Skrypt RC1 (`test-rc1.sh`):**
    - Graficzny **Launcher (.hsapp Grid)** z pikselowymi ikonami `16x16` (`icons.rs`), dynamicznym skanowaniem `/system/apps` i `/user/apps` oraz bezpośrednim uruchamianiem paczek `.hsapp` (np. `Hello.hsapp`).
    - Kreator `1st Setup` (`EN`/`PL` → automatyczne pominięcie Wi-Fi przy braku sterownika → zegar → ekran powitalny z 5-ramienną gwiazdą Hollow Star i zapisem w `/user/config/os.cfg`), uniwersalna pomoc `F1`, `Sys Info` (temperatura, RAM, 3 partycje + rozszerzanie `/user`), `Markdown Notes` (`Ctrl+S` + `Tab` podgląd), `File Explorer`, `Media Player` (Delta RT Audio), `Calculator` (`^`, `sqrt`, `F2` ułamki), klasyczne gry (`Brick Breaker`, `Tetris`, `Racing`) i `Settings`.
    - Skrypt `./test-rc1.sh` (`--auto` oraz `--gui`) sprawdzający grafikę (VideoCore FB), dźwięk RT (`usb-audio`/`CoreAudio`), sieć (`usb-net` + SLIRP), partycje DeltaFS i App Launcher.
- **Kryterium akceptacji:** zaliczone `./delta.sh ci` (`hal-core` 6 testów + `vfs-server` 2 testy z testem WAL replay i rozszerzania `/user` + `board-raspi02w` clippy), `./test-rc1.sh --auto` (grafika `640x480`, RT audio, `usb-net`, 3 partycje DeltaFS, Launcher uruchamiający `Hello.hsapp/exec`) oraz powrót do interaktywnej powłoki.

---

### v1.0.0 — Wydanie produkcyjne, dokumentacja SDK/LLM i akceleracja 3D (`testing`) *(Następny etap)*
- **Zakres:**
  - **Dokumentacja dla programistów (`docs/DEVELOPER_GUIDE.md`):** kompletny podręcznik tworzenia aplikacji `.hsapp` (ABI syscalli `svc #0`, `libdelta`, VFS/DeltaFS, IPC, priorytety RT audio i kompozytor FB) oraz architektury mikrojądra Delta.
  - **Pliki kontekstowe dla LLM (`docs/llm/`):** wyspecjalizowane prompty/konteksty dla modeli LLM do szybkiego tworzenia aplikacji `.hsapp` oraz portowania sterowników urządzeń pod `hal-core` / EL0 (`Shm`, MMIO, IRQ forwarding).
  - **Akceleracja graficzna OpenGL ES / Vulkan (`testing` w `1.0.0`):** wsparcie dla układu **BCM2837 VideoCore IV (`V3D`)** w osobnym serwerze EL0 (`drv-v3d`), obsługa potoku OpenGL ES 2.0 / interfejsu zbliżonego do Vulkan przez `Shm` i DMA.
  - **Poradnik instalacji na kartę SD i automatyczne rozszerzanie `/user`:** instrukcja flashowania karty microSD dla Raspberry Pi 3B oraz Zero 2 W (Boot FAT32 + System DeltaFS + User DeltaFS z auto-expand przy pierwszym starcie).
  - **Promocja gałęzi i publikacja:** scalenie `testing -> main`, tag `v1.0.0` oraz publikacja gotowych obrazów `.img` dla `raspi3b` i `raspi02w` poprzez GitHub Releases.

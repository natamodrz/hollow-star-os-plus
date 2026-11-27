# Roadmap kamieni milowych — Hollow Star OS+ (Delta Kernel)

Dokument definiuje wszystkie kamienie milowe projektu od pierwszego uruchomienia kodu w Ruście na QEMU `raspi3b` po kompletny system mikrojądrowy z interfejsem graficznym i multimediami czasu rzeczywistego.

> Zasada nadrzędna dla wszystkich etapów: kod w `kernel/` nie zawiera adresów MMIO specyficznych dla płytki (`BCM2837` / `RP3A0`), a każdy kamień milowy przechodzi `./delta.sh ci` (`fmt`, `clippy -D warnings`, testy jednostkowe `hal-core` oraz smoke test w `qemu-system-aarch64 -M raspi3b`).

---

## Przegląd statusu

| Kamień | Nazwa | Status | Kluczowy cel |
| :--- | :--- | :--- | :--- |
| **M1** | Boot & Serial | Ukończony | Zejście `EL2 -> EL1`, BSS, `hal-core` + `board-raspi3b`, baner UART, parser DTB |
| **M2** | Memory & Exceptions | Ukończony | Wektory wyjątków `VBAR_EL1`, ARM Generic Timer IRQ, PMM (4 KiB), MMU (`TTBR0`/`TTBR1`) |
| **M3** | Delta Scheduler & SMP | Następny etap | Przełączanie kontekstu AArch64, wielowątkowość na 4 rdzeniach Cortex-A53, priorytety `1..=120` + RT |
| **M4** | EL0 & IPC | Planowany | Przestrzeń użytkownika `EL0`, ABI `svc`, porty IPC z capabilities, `init` + `drv-uart` |
| **M5** | Storage & VFS | Planowany | Serwer SD/eMMC (`drv-sd`), VFS w EL0, FAT32 / ext2, ładowanie plików ELF |
| **M6** | Multimedia & UI | Planowany | Kompozytor Framebuffer/HDMI (`drv-fb`), audio RT (`drv-audio`), `1st Setup`, `EN/PL`, aplikacje |

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

### M3 — Delta Scheduler & SMP
- **Zakres:**
  - Przełączanie kontekstu AArch64 (`callee-saved` `x19..x30`, `SP`, `TPIDR_EL1`, opcjonalnie stan FP/SIMD).
  - Struktura `Thread` i kolejek gotowych z priorytetami `1..=120` (Idle `1`, Low `2..=9`, Normal `10`, UI `15`, Urgent `20`, Real-Time `100..=120`).
  - Wybudzenie rdzeni `1..=3` Cortex-A53 (spin-table / mailbox startowy) oraz per-CPU runqueues chronione `IrqSpinLock`.
  - Preempting z niskim opóźnieniem na ticku timera oraz priorytetowe kolejkowanie wątków RT.
- **Kryterium akceptacji:** wielowątkowy test w QEMU `raspi3b` (`-smp 4`) pokazujący działanie wątków na wszystkich 4 rdzeniach i natychmiastowe wywłaszczanie przez wątek RT.

---

### M4 — EL0 & IPC
- **Zakres:**
  - Przejście do trybu użytkownika **EL0** (`eret` z izolowanym `TTBR0_EL1` na proces) oraz obsługa wywołań systemowych (`svc #0`).
  - Porty IPC oparte na capabilities (tworzenie portu, wysyłanie/odbieranie komunikatów synchronicznych i asynchronicznych, dziedziczenie priorytetów przy blokowaniu na IPC).
  - Przekazywanie przerwań sprzętowych (IRQ forwarding) do serwerów sterowników w EL0.
  - Pierwsze procesy przestrzeni użytkownika: `userland/libdelta`, `userland/init`, `drivers/drv-uart` obsługujący konsolę przez IPC zamiast bezpośrednich zapisów z jądra.
- **Kryterium akceptacji:** `init` w EL0 wysyła powitanie przez serwer `drv-uart` przy użyciu portów IPC w QEMU `raspi3b`.

---

### M5 — Storage & VFS
- **Zakres:**
  - Sterownik SD/eMMC w przestrzeni użytkownika (`drivers/drv-sd`, EMMC / SDHOST na RPi 3B oraz RPi Zero 2 W) komunikujący się przez IRQ forwarding i współdzielone bufory pamięci.
  - Serwer VFS (`fs/`) z obsługą `FAT32` i `ext2`, montowaniem drzewa katalogów, `/dev` oraz `/proc`.
  - Loader binarnych plików ELF (`userland/libdelta` + `init`) ładujący programy, serwery oraz paczki aplikacji `<AppName>.hsapp/exec` bezpośrednio z systemu plików.
  - Rozszerzenie `libdelta` o ergonomiczny interfejs plikowy (deskryptory, ścieżki, sygnały).
- **Kryterium akceptacji:** uruchomienie procesu powłoki (`userland/shell`) z obrazu karty SD pod QEMU `raspi3b` oraz odczyt/zapis testowego pliku na partycji FAT32.

---

### M6 — Multimedia & UI (Parytet UX z `FEATURES_AND_UX.md`)
- **Zakres:**
  - Sterownik graficzny `drivers/drv-fb` (Mailbox Property Interface + Framebuffer HDMI/DSI/SPI) oraz kompozytor okien o ostrej retro-estetyce (brak zaokrągleń, pasek statusu, czcionka monospace).
  - Sterownik audio `drivers/drv-audio` (PWM/jack 3.5 mm / HDMI / BT na obsługiwanych wariantach) działający na priorytetach **Real-Time (`100..=120`)** planisty Delta.
  - Obsługa klawiatury/wejścia oraz obsługa płytek z `board-raspi02w` (adaptacja opcji bezprzewodowych: ukrywanie Wi-Fi/BT/FTP na wariantach bez modułu radiowego).
  - Pełna lokalizacja **`English` (domyślny) + `Polski`**, kreator **`1st Setup`** (język → Wi-Fi jeśli dostępne → czas → ekran powitalny z logo Hollow Star), uniwersalna pomoc pod **`F1`**, obsługa paczek aplikacji **`.hsapp`** (`Assets/`, `exec` ELF, `icon.png`) w launcherze oraz aplikacje z `FEATURES_AND_UX.md` (`Sys Info`, `Markdown Notes`, `File Explorer`, `MP3/Media Player`, `Calculator`, gry).
- **Kryterium akceptacji:** pełny start graficzny z kreatorem `1st Setup` i testem odtwarzania audio bez przycięć pod obciążeniem UI w QEMU oraz na fizycznym Raspberry Pi 3B / Zero 2 W; wydanie pierwszego pełnego obrazu `.img` przez **GitHub Releases** (`gh release create`).

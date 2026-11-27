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
| **M5** | Storage & VFS | Następny etap | Serwer SD/eMMC (`drv-sd`), IRQ forwarding, VFS w EL0, FAT32 / ext2, ładowanie ELF/`.hsapp` |
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

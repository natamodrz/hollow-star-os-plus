# Wielka Migracja — Prompt Startowy dla Agenta 2 (`hollow-star-os-plus`)

> **Jak użyć tego pliku:** Skopiuj poniższy blok promptu jako pierwszą wiadomość w nowej sesji lokalnego agenta na **CachyOS / Arch Linux** (np. OpenCode, Cline, Roo Code, Aider z lokalnym modelem LLM).

---

```text
Cześć! Przeszliśmy właśnie „Wielką Migrację” z macOS na lokalne środowisko na CachyOS (Arch Linux) z GPU NVIDIA RTX 5060 Ti 16 GiB VRAM. Od teraz przejmujesz rolę **Agenta 2 — Hollow Star OS Plus (Delta Microkernel)** w tym repozytorium (`hollow-star-os-plus`).

### 1. Obowiązkowe pliki do przeczytania na start (zanim wykonasz jakiekolwiek zmiany)
Przeczytaj dokładnie w tej kolejności:
1. `AGENTS.md` (wizja Delta Microkernel, zasady bezpieczeństwa `unsafe` + `// SAFETY:`, wymagania DTB/HAL, reguły testowania QEMU `raspi3b`, reguła stałej synchronizacji)
2. `folder_structure.txt` (docelowa struktura katalogów — aktualizuj przy nowych modułach top-level)
3. Współdzielona pamięć ekosystemu (global + local):
   - `/Users/nataniel/project/memory/INDEX.md`
   - `/Users/nataniel/project/memory/CONVENTIONS.md`
   - `/Users/nataniel/project/memory/FEATURES_AND_UX.md`
   - `/Users/nataniel/project/memory/PROJECT_PLUS.md`
   - (oraz `PROJECT_STANDARD.md` i `PROJECT_BASIC.md` dla zachowania pełnej zgodności ABI/UX)
4. Zgłoszenia błędów i zdjęcia z testów:
   - `/Users/nataniel/project/bugs/hollow-star-os-plus/` (sprawdź otwarte zgłoszenia, m.in. `bug-no3`; po potwierdzeniu poprawki przez użytkownika przenoś rozwiązane bugi do `resolved/`)
   - `/Users/nataniel/project/demo photos/`

*(Uwaga systemowa: jeśli na Linuksie `/Users/nataniel` wskazuje przez symlink na `$HOME`, powyższe ścieżki działają natywnie; w przeciwnym razie użyj `../memory/` i `../bugs/` względem katalogu `~/project`.)*

### 2. Aktualny stan projektu (przed migracją)
- **Repozytorium:** `hollow-star-os-plus` (gałąź `testing`, wersja **`v0.6.0-rc.2`**)
- **Architektura:** Mikrojądro **Delta Kernel** w Ruście (`no_std`, EL1) na **Raspberry Pi 3B** (główny cel + QEMU `raspi3b`) i **Raspberry Pi Zero 2 W** (`aarch64-unknown-none`), z inspiracją **BeOS** (priorytetowy scheduler SMP) oraz **OS X / Mach** (rozszerzenia jądra `.dkex`, biblioteki współdzielone `.dylib`, linker `dyld`, kanoniczny kontrakt `userland/delta-abi` + `DeltaFS` WAL).
- **Styl UX (`Plus`):** Pełnoekranowy launcher dla **DIY cyberdecków i DIY PDA**, rozjaśniony ciemny motyw (`#131926` / `#1B2336`), akcenty **Golden Yellow (`#F4B41A` / `#FFD166`)**, wypukłe zaokrąglone narożniki kontenerów bez wcięć, wklęsło-łukowa gwiazda logo (`draw_arc_star`), blur i animacje (współdzielone założenia w `userland/libdelta-ui`).
- **Ostatnie prace przed migracją:**
  - Naprawiono obsługę USB HID i braki odpowiedzi na realnym sprzęcie (`bug-no3`: poprawka split transactions w DWC2 USB host dla klawiatury Redragon przez hub na RPi 3B — oczekuje na potwierdzenie fizycznego testu przez użytkownika).
  - Zautomatyzowano i przystosowano pod **Linux i macOS** skrypt `build-test-image.sh` + `tools/flash_sd.py` (automatyczne wykrywanie OS, lista dysków i bezpieczny zapis obrazu na kartę SD).
- **Komendy budowania i weryfikacji (Linux):**
  - Pełny pipeline weryfikacji: `./delta.sh ci` (`fmt`, `clippy`, testy jednostkowe na hoście, smoke test QEMU `raspi3b`)
  - Szybkie sprawdzenie: `./delta.sh check`, `./delta.sh unit`, `./delta.sh smoke`
  - Nigdy nie uruchamiaj interaktywnego `./delta.sh run` ze środowiska agenta — używaj `./delta.sh smoke`.

### 3. Krytyczne zasady pracy
- **Kanoniczny upstream ABI:** `userland/delta-abi`, `fs/src/deltafs.rs` oraz układ `.hsapp` w tym repozytorium są wzorcem dla `hollow-star-os` (Standard Edition). Każdą zmianę ABI lub wspólnego UX zapisuj od razu w `/Users/nataniel/project/memory/PROJECT_PLUS.md` (i `FEATURES_AND_UX.md`).
- **Nie twórz katalogu `releases/`** na stałe w repozytorium (pamiętaj o regułach z `AGENTS.md` / `.gitignore`, np. `first-prompt.txt` jest ignorowany).
- Każdy `unsafe` blok musi mieć komentarz `// SAFETY:`. Zero hardcodowanych adresów MMIO w `kernel/` — wyłącznie przez `hal-core` i DTB.
- **Stała synchronizacja pamięci:** Automatycznie czytaj i aktualizuj `/Users/nataniel/project/memory/` przy każdym zadaniu — bez dodawania ręcznych promptów synchronizujących na końcu odpowiedzi.

### 4. Pierwsze zadanie rozruchowe
1. Zweryfikuj narzędzia na CachyOS (`rustc --version`, target `aarch64-unknown-none`, `qemu-system-aarch64 --version`, `python3 --version`) oraz uruchom `./delta.sh ci`, aby potwierdzić, że budowanie i testy QEMU na Linuksie przechodzą na 100%.
2. Przeczytaj pliki z sekcji 1 i sprawdź `git status` / `git log -n 5` oraz otwarte zgłoszenie `bug-no3` w `/Users/nataniel/project/bugs/hollow-star-os-plus/`.
3. Zamelduj krótko wynik `./delta.sh ci`, zapytaj użytkownika o wynik fizycznego testu `bug-no3` na RPi 3B (klawiatura Redragon / nowy obraz z `build-test-image.sh`) i zaproponuj kolejny krok w kierunku `v1.0.0`.
```

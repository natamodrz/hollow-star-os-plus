# Projekt planisty Delta Scheduler (inspirowany BeOS)

## 1. Założenia projektowe

System **Hollow Star OS+** kładzie nacisk na płynną wielozadaniowość i responsywność multimediów w czasie rzeczywistym na wielordzeniowych układach ARM64 (4x Cortex-A53 w Raspberry Pi 3B oraz przyszłym RPi Zero 2 W).

Zamiast ciężkiego modelu z gruboziarnistymi procesami, Delta Kernel traktuje **wątek (Thread)** jako podstawową jednostkę planowania na wszystkich dostępnych rdzeniach CPU (SMP — *pervasive multithreading*).

---

## 2. Skala i klasy priorytetów (`1..=120`)

Planista wykorzystuje dyskretną skalę priorytetów od `1` (najniższy) do `120` (najwyższy), wzorowaną bezpośrednio na systemie BeOS:

| Zakres / Wartość | Nazwa warstwy | Przeznaczenie |
|---|---|---|
| `1` | **Idle** | Wątki bezczynności przypięte per-CPU (`wfi` / `wfe`) |
| `2..=9` | **Low / Background** | Indeksowanie, zadania w tle, kompresja, logowanie |
| `10` | **Normal / Interactive** | Domyślny priorytet aplikacji powłoki i procesów CLI |
| `15` | **Display / UI** | Kompozytor okien, obsługa zdarzeń wejścia (klawiatura, mysz) |
| `20` | **Urgent** | Krytyczne usługi systemowe (np. serwery VFS/IPC wymagające krótkiego czasu reakcji) |
| `21..=99` | **Elevated Time-Sharing** | Zarezerwowane dla dynamicznych podbić priorytetu lub zadań o podwyższonej ważności |
| `100..=120` | **Real-Time (RT)** | Strumienie audio, dekodowanie wideo, serwery sterowników o twardych wymaganiach czasowych |

---

## 3. Polityka szeregowania i wywłaszczania

### 3.1. Warstwa Time-Sharing (`2..=99`)
- Wątki są zorganizowane w kolejki priorytetowe (*ready queues*).
- Zawsze wybierany jest wątek o najwyższym priorytecie; w obrębie tego samego priorytetu stosowany jest algorytm **Round-Robin** z krótkim kwantem czasu (docelowo ok. `2–5 ms`), aby interfejs użytkownika oraz powłoka pozostały responsywne nawet pod dużym obciążeniem.
- Wywłaszczenie następuje natychmiastowo (*low-latency preemption*), gdy przerwanie zegara lub komunikat IPC obudzi wątek o wyższym priorytecie.

### 3.2. Warstwa Real-Time (`100..=120`)
- Wątki RT mają ścisłe pierwszeństwo przed wszystkimi wątkami z warstwy Time-Sharing.
- Wątek RT wykonuje się do momentu dobrowolnego oddania CPU (np. oczekiwanie na bufor audio / kolejny sygnał timera) lub pojawienia się wątku RT o jeszcze wyższym priorytecie.
- Budżetowanie / limity bezpieczeństwa (planowane w M3/M6) zapobiegną całkowitemu zagłodzeniu systemu przez błędny wątek RT.

---

## 4. Dziedziczenie priorytetów (Priority Inheritance)

Aby uniknąć **inwersji priorytetów** (np. gdy wątek audio RT `110` wysyła synchroniczny komunikat IPC lub czeka na muteks utrzymywany przez serwer o priorytecie `10`, a w tym czasie średni wątek `20` wywłaszcza serwer):

- Gdy wątek $T_{high}$ blokuje się na synchronicznym IPC lub blokadzie posiadanej przez wątek $T_{low}$, jądro tymczasowo podnosi efektywny priorytet $T_{low}$ do $\max(\text{prio}(T_{low}), \text{prio}(T_{high}))$.
- Po zakończeniu sekcji krytycznej lub wysłaniu odpowiedzi IPC priorytet efektywny wątku $T_{low}$ wraca do poziomu bazowego (lub kolejnego najwyższego czekającego wątku).

---

## 5. Wsparcie SMP (4x Cortex-A53)

- **Stan obecny (M3 zaimplementowane):**
  - Wybudzanie rdzeni `1..=3` przez warstwę HAL (`Board::wake_secondary_core`, spin-table `0xE0/0xE8/0xF0` + czyszczenie linii cache `dc civac` + `sev` dla `board-raspi3b`) oraz wejście `_start_secondary` (zejście `EL2 -> EL1`, wspólne tablice stron `TTBR0_EL1`/`TTBR1_EL1`, lokalne wektory `VBAR_EL1` i zegar `100 Hz` per rdzeń).
  - Kontekst AArch64 (`CpuContext`: `x19..x30`, `sp`) i bezpieczne przełączanie `switch_context` z odroczonym zwrotem wątku wychodzącego (`pending_requeue[core]`) chroniącym stosy przed wyścigiem SMP.
  - Wsparcie dla powinowactwa CPU (`affinity: Option<u8>`), wywłaszczania timerem, `boost_priority` / `clear_priority_boost` (fundament Priority Inheritance) oraz automatycznego zwalniania stosów zakończonych wątków.
- **Dalszy rozwój (M4+):**
  - Osobne kolejki per-CPU (*work stealing*) oraz pełna integracja PI z blokowaniem na portach IPC.

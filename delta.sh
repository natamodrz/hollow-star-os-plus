#!/bin/bash
set -euo pipefail

# =========================================================
# Konfiguracja Hollow Star OS+ (Delta Kernel)
# =========================================================
TARGET="${TARGET:-aarch64-unknown-none}"
KERNEL_PKG="${KERNEL_PKG:-kernel}"
PROFILE="${PROFILE:-debug}"                 # debug | release
UART_MODE="${UART_MODE:-pl011}"             # pl011 (-serial stdio) | mini (-serial null -serial stdio)
SMOKE_TIMEOUT="${SMOKE_TIMEOUT:-5}"         # maks. czas oczekiwania w sekundach
EXPECT_MARKER="${EXPECT_MARKER:-Hollow Star OS+}"

# Lista crate'ów niezależnych od sprzętu, dla których odpalamy unit testy na hoście (macOS/Linux)
HOST_TEST_PKGS="${HOST_TEST_PKGS:-hal-core}"

BUILD_DIR="target/$TARGET/$PROFILE"
KERNEL_ELF="$BUILD_DIR/$KERNEL_PKG"
KERNEL_IMG="target/kernel8.img"
LOG_FILE="target/qemu-smoke.log"

# Kolory w terminalu
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

info() { echo -e "${YELLOW}[delta]${NC} $*"; }
ok()   { echo -e "${GREEN}[OK]${NC} $*"; }
err()  { echo -e "${RED}[BLAD]${NC} $*" >&2; }

find_objcopy() {
  if command -v rust-objcopy >/dev/null 2>&1; then
    command -v rust-objcopy
    return
  fi
  local sysroot
  sysroot="$(rustc --print sysroot)"
  find "$sysroot" -name "llvm-objcopy" -type f 2>/dev/null | head -n 1
}

qemu_uart_args() {
  if [ "$UART_MODE" = "mini" ]; then
    echo "-serial null -serial stdio"
  else
    echo "-serial stdio"
  fi
}

cmd_build() {
  info "Kompilacja $KERNEL_PKG ($TARGET, tryb: $PROFILE)..."
  local flags=(--package "$KERNEL_PKG" --target "$TARGET")
  if [ "$PROFILE" = "release" ]; then
    flags+=(--release)
  fi

  cargo build "${flags[@]}"

  if [ ! -f "$KERNEL_ELF" ]; then
    err "Nie znaleziono pliku ELF: $KERNEL_ELF"
    exit 1
  fi

  local objcopy
  objcopy="$(find_objcopy)"
  if [ -z "$objcopy" ]; then
    err "Brak llvm-objcopy. Zainstaluj: rustup component add llvm-tools-preview"
    exit 1
  fi

  mkdir -p target
  "$objcopy" --strip-all -O binary "$KERNEL_ELF" "$KERNEL_IMG"
  local size_bytes
  size_bytes=$(wc -c < "$KERNEL_IMG" | tr -d ' ')
  ok "Zbudowano $KERNEL_IMG ($size_bytes B)"
}

cmd_check() {
  info "Sprawdzanie formatowania (rustfmt)..."
  cargo fmt --all -- --check

  info "Analiza statyczna (clippy) dla $KERNEL_PKG..."
  cargo clippy --package "$KERNEL_PKG" --target "$TARGET" -- -D warnings
  ok "Check zakonczony bez bledow"
}

cmd_unit() {
  local host_target
  host_target="$(rustc -vV | awk '/^host:/ {print $2}')"
  info "Uruchamianie testow jednostkowych na hoscie ($host_target)..."

  for pkg in $HOST_TEST_PKGS; do
    if cargo metadata --no-deps --format-version 1 2>/dev/null | grep -q "\"name\":\"$pkg\""; then
      info "Testuje crate: $pkg"
      cargo test --package "$pkg" --target "$host_target"
    else
      info "Pomijam nieistniejacy jeszcze crate: $pkg"
    fi
  done
  ok "Testy jednostkowe zaliczone"
}

cmd_run() {
  cmd_build
  if ! command -v qemu-system-aarch64 >/dev/null 2>&1; then
    err "Brak qemu-system-aarch64. Zainstaluj: brew install qemu"
    exit 1
  fi

  info "Uruchamianie QEMU (raspi3b, UART=$UART_MODE). Wyjscie: Ctrl+A potem X"
  # shellcheck disable=SC2046
  qemu-system-aarch64 \
    -M raspi3b \
    -display none \
    $(qemu_uart_args) \
    -kernel "$KERNEL_IMG"
}

cmd_gdb() {
  cmd_build
  info "QEMU wstrzymane na starcie (port GDB :1234). Podlacz debugger do $KERNEL_ELF"
  # shellcheck disable=SC2046
  qemu-system-aarch64 \
    -M raspi3b \
    -display none \
    $(qemu_uart_args) \
    -kernel "$KERNEL_IMG" \
    -s -S
}

cmd_smoke() {
  cmd_build
  if ! command -v qemu-system-aarch64 >/dev/null 2>&1; then
    err "Brak qemu-system-aarch64. Zainstaluj: brew install qemu"
    exit 1
  fi

  info "Test startowy QEMU (timeout=${SMOKE_TIMEOUT}s, szukam: '$EXPECT_MARKER')..."
  rm -f "$LOG_FILE"
  touch "$LOG_FILE"

  # shellcheck disable=SC2046
  qemu-system-aarch64 \
    -M raspi3b \
    -display none \
    $(qemu_uart_args) \
    -kernel "$KERNEL_IMG" \
    > "$LOG_FILE" 2>&1 &
  local qemu_pid=$!

  local ticks=0
  local max_ticks=$((SMOKE_TIMEOUT * 5)) # 0.2 s na krok
  local found_marker=0
  local found_panic=0

  while [ "$ticks" -lt "$max_ticks" ]; do
    if grep -qiE "KERNEL PANIC|panicked at|Unrecoverable" "$LOG_FILE" 2>/dev/null; then
      found_panic=1
      break
    fi

    if grep -qF "$EXPECT_MARKER" "$LOG_FILE" 2>/dev/null; then
      found_marker=1
      sleep 0.4 # Krótka chwila na wykrycie ewentualnej paniki zaraz po banerze
      if grep -qiE "KERNEL PANIC|panicked at|Unrecoverable" "$LOG_FILE" 2>/dev/null; then
        found_panic=1
      fi
      break
    fi

    if ! kill -0 "$qemu_pid" 2>/dev/null; then
      break
    fi

    sleep 0.2
    ticks=$((ticks + 1))
  done

  kill -TERM "$qemu_pid" 2>/dev/null || true
  sleep 0.1
  kill -KILL "$qemu_pid" 2>/dev/null || true
  wait "$qemu_pid" 2>/dev/null || true

  echo "---------- UART OUTPUT ($LOG_FILE) ----------"
  cat "$LOG_FILE"
  echo "---------------------------------------------"

  if [ "$found_panic" -eq 1 ]; then
    err "Wykryto PANIC podczas bootowania!"
    exit 1
  fi

  if [ "$found_marker" -eq 1 ]; then
    ok "Smoke test ZALICZONY (wykryto '$EXPECT_MARKER')"
    exit 0
  else
    err "Smoke test NIEZALICZONY (brak '$EXPECT_MARKER' w ciagu ${SMOKE_TIMEOUT}s)"
    exit 1
  fi
}

cmd_ci() {
  cmd_check
  cmd_unit
  cmd_smoke
  ok "Pelny pakiet testow CI zaliczony!"
}

cmd_clean() {
  rm -f "$KERNEL_IMG" "$LOG_FILE"
  cargo clean
  ok "Wyczyszczono artefakty kompilacji"
}

case "${1:-}" in
  build) cmd_build ;;
  check) cmd_check ;;
  unit)  cmd_unit ;;
  run)   cmd_run ;;
  smoke) cmd_smoke ;;
  gdb)   cmd_gdb ;;
  ci)    cmd_ci ;;
  clean) cmd_clean ;;
  *)
    echo "Uzycie: ./delta.sh {build|check|unit|run|smoke|gdb|ci|clean}"
    echo ""
    echo "Zmienne opcjonalne:"
    echo "  PROFILE=release         kompilacja z optymalizacjami"
    echo "  UART_MODE=mini          przelacza QEMU z PL011 na Mini-UART"
    echo "  EXPECT_MARKER='...'     tekst oczekiwany w tescie smoke"
    echo "  SMOKE_TIMEOUT=10        limit czasu testu smoke w sekundach"
    exit 1
    ;;
esac
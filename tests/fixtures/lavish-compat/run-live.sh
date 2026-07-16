#!/usr/bin/env bash
set -euo pipefail

root=$(git rev-parse --show-toplevel)
evidence=${1:-target/compatibility/production}
port=${LAVISH_WEBDRIVER_PORT:-9594}
manual_display=${LAVISH_MANUAL_DISPLAY:-${DISPLAY:-:1}}
manual_args=()
if [[ ${LAVISH_MANUAL_EXCALIDRAW:-0} == 1 ]]; then
  manual_args=(--manual-excalidraw --manual-display "$manual_display")
fi

session_conf=$(find /nix/store -path '*/share/dbus-1/session.conf' | head -1)
atspi_conf=$(find /nix/store -path '*at-spi2-core-*/share/defaults/at-spi2/accessibility.conf' | head -1)
registry=$(find /nix/store -path '*at-spi2-core-*/libexec/at-spi2-registryd' | head -1)
mapfile -t session_bus < <(dbus-daemon --config-file="$session_conf" --fork --print-address=1 --print-pid=1)
mapfile -t accessibility_bus < <(dbus-daemon --config-file="$atspi_conf" --fork --print-address=1 --print-pid=1)
export DBUS_SESSION_BUS_ADDRESS=${session_bus[0]}
export AT_SPI_BUS_ADDRESS=${accessibility_bus[0]}

DBUS_SESSION_BUS_ADDRESS=$AT_SPI_BUS_ADDRESS "$registry" >target/compatibility/atspi-registry.log 2>&1 &
registry_pid=$!
Xvfb :99 -screen 0 1600x1000x24 -nolisten tcp >target/compatibility/xvfb.log 2>&1 &
xvfb_pid=$!
cleanup() {
  kill "$registry_pid" "$xvfb_pid" "${accessibility_bus[1]}" "${session_bus[1]}" 2>/dev/null || true
  wait "$registry_pid" "$xvfb_pid" 2>/dev/null || true
}
trap cleanup EXIT INT TERM
sleep 2

DISPLAY=:99 python -u "$root/tests/fixtures/lavish-compat/run.py" \
  --browser "$root/target/debug/lavish-browser" \
  --launcher "$root/target/debug/lavish-open" \
  --evidence "$root/$evidence" \
  --port "$port" \
  "${manual_args[@]}"

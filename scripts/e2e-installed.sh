#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
run_id=${LAVISH_E2E_RUN_ID:-$(date -u +%Y%m%dT%H%M%SZ)-$$}
evidence=${LAVISH_E2E_EVIDENCE:-$root/target/e2e/$run_id}
prefix=$evidence/prefix
profile=${LAVISH_E2E_PROFILE:-release}
display=${LAVISH_E2E_DISPLAY:-:98}
mkdir -p "$evidence/logs"

required=(cargo dbus-daemon Xvfb WebKitWebDriver npx zellij python script xprop sha256sum)
for command in "${required[@]}"; do
  command -v "$command" >/dev/null || {
    echo "FAIL: required live E2E command is unavailable: $command" >&2
    exit 1
  }
done
[[ -n ${DISPLAY:-} ]] || echo "INFO: using isolated Xvfb; no ambient display inherited" >&2

"$root/packaging/install.sh" --prefix "$prefix" --profile "$profile"
desktop-file-validate "$prefix/share/applications/works.from-nibly.LavishBrowser.desktop"

session_conf=$(find /nix/store -path '*/share/dbus-1/session.conf' | head -1)
atspi_conf=$(find /nix/store -path '*at-spi2-core-*/share/defaults/at-spi2/accessibility.conf' | head -1)
registry=$(find /nix/store -path '*at-spi2-core-*/libexec/at-spi2-registryd' | head -1)
[[ -n $session_conf && -n $atspi_conf && -x $registry ]] || {
  echo "FAIL: isolated D-Bus/AT-SPI configuration is unavailable" >&2
  exit 1
}
mapfile -t session_bus < <(dbus-daemon --config-file="$session_conf" --fork --print-address=1 --print-pid=1)
mapfile -t accessibility_bus < <(dbus-daemon --config-file="$atspi_conf" --fork --print-address=1 --print-pid=1)
export DBUS_SESSION_BUS_ADDRESS=${session_bus[0]}
export AT_SPI_BUS_ADDRESS=${accessibility_bus[0]}
export DISPLAY=$display
unset GTK_A11Y NO_AT_BRIDGE || true

DBUS_SESSION_BUS_ADDRESS=$AT_SPI_BUS_ADDRESS "$registry" >"$evidence/logs/atspi-registry.log" 2>&1 &
registry_pid=$!
Xvfb "$display" -screen 0 1600x1000x24 -nolisten tcp >"$evidence/logs/xvfb.log" 2>&1 &
xvfb_pid=$!
cleanup() {
  kill "$registry_pid" "$xvfb_pid" "${accessibility_bus[1]}" "${session_bus[1]}" 2>/dev/null || true
  wait "$registry_pid" "$xvfb_pid" 2>/dev/null || true
}
trap cleanup EXIT INT TERM
sleep 2
xdpyinfo -display "$display" >/dev/null || { echo "FAIL: isolated display did not start" >&2; exit 1; }
python - <<'PY'
import pyatspi
assert pyatspi.Registry.getDesktopCount() > 0
print("isolated AT-SPI registry is reachable")
PY

manual=()
if [[ ${LAVISH_MANUAL_EXCALIDRAW:-0} == 1 ]]; then
  manual=(--manual-excalidraw --manual-display "${LAVISH_MANUAL_DISPLAY:-$DISPLAY}")
fi

# Re-run the full unchanged-Lavish compatibility gate against installed paths.
set +e
python -u "$root/tests/fixtures/lavish-compat/run.py" \
  --browser "$prefix/bin/lavish-browser" \
  --launcher "$prefix/bin/lavish-open" \
  --evidence "$evidence/compatibility" \
  --port "${LAVISH_WEBDRIVER_PORT:-9595}" \
  "${manual[@]}" 2>&1 | tee "$evidence/logs/compatibility-harness.log"
compatibility_status=${PIPESTATUS[0]}

# GApplication and AT-SPI names are session-bus scoped. Use a fresh pair of
# buses for the integration phase so a WebDriver-owned application's deferred
# teardown cannot make the installed browser look like a second activation.
kill "$registry_pid" "${accessibility_bus[1]}" "${session_bus[1]}" 2>/dev/null || true
wait "$registry_pid" 2>/dev/null || true
mapfile -t session_bus < <(dbus-daemon --config-file="$session_conf" --fork --print-address=1 --print-pid=1)
mapfile -t accessibility_bus < <(dbus-daemon --config-file="$atspi_conf" --fork --print-address=1 --print-pid=1)
export DBUS_SESSION_BUS_ADDRESS=${session_bus[0]}
export AT_SPI_BUS_ADDRESS=${accessibility_bus[0]}
DBUS_SESSION_BUS_ADDRESS=$AT_SPI_BUS_ADDRESS "$registry" >"$evidence/logs/atspi-registry-integration.log" 2>&1 &
registry_pid=$!
sleep 1

integration_manual=()
[[ ${LAVISH_MANUAL_EXCALIDRAW:-0} == 1 ]] && integration_manual=(--manual-excalidraw)
python -u "$root/tests/e2e/run.py" \
  --prefix "$prefix" --evidence "$evidence/integration" \
  "${integration_manual[@]}" 2>&1 | tee "$evidence/logs/installed-harness.log"
integration_status=${PIPESTATUS[0]}
set -e

cat >"$evidence/manifest.json" <<EOF
{
  "git_revision": "$(git -C "$root" rev-parse HEAD)",
  "prefix": "$prefix",
  "profile": "$profile",
  "display": "$display",
  "compatibility_status": $compatibility_status,
  "integration_status": $integration_status,
  "compatibility_evidence": "$evidence/compatibility",
  "integration_evidence": "$evidence/integration"
}
EOF
if ((compatibility_status || integration_status)); then
  echo "FAIL: installed E2E has blockers; evidence: $evidence" >&2
  exit 1
fi
printf 'Installed E2E passed. Evidence: %s\n' "$evidence"

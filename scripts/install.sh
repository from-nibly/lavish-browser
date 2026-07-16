#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
prefix=${PREFIX:-/usr/local}
destdir=${DESTDIR:-}
profile=release
build=1

usage() {
  cat <<'EOF'
Usage: scripts/install.sh [--prefix PATH] [--destdir PATH] [--profile debug|release] [--no-build]

Installs Lavish Browser's native binaries, desktop metadata, and optional
Zellij plugin. DESTDIR is honored for package staging; installed desktop and
plugin paths continue to use PREFIX.
EOF
}

while (($#)); do
  case $1 in
    --prefix) prefix=$2; shift 2 ;;
    --destdir) destdir=$2; shift 2 ;;
    --profile) profile=$2; shift 2 ;;
    --no-build) build=0; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "install.sh: unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

case $profile in
  debug) cargo_profile=debug; cargo_flags=() ;;
  release) cargo_profile=release; cargo_flags=(--release) ;;
  *) echo "install.sh: profile must be debug or release" >&2; exit 2 ;;
esac

if ((build)); then
  cargo build --locked "${cargo_flags[@]}" -p lavish-browser -p lavish-browser-cli
  cargo build --locked "${cargo_flags[@]}" -p lavish-browser-zellij --target wasm32-wasip1
fi

native_dir=${CARGO_TARGET_DIR:-$root/target}/$cargo_profile
wasm_dir=${CARGO_TARGET_DIR:-$root/target}/wasm32-wasip1/$cargo_profile
stage=${destdir%/}${prefix}

install -d "$stage/bin" "$stage/share/applications" "$stage/share/icons/hicolor/scalable/apps" \
  "$stage/share/lavish-browser/zellij"
install -m 0755 "$native_dir/lavish-browser" "$stage/bin/lavish-browser"
install -m 0755 "$native_dir/lavish-open" "$stage/bin/lavish-open"
install -m 0755 "$native_dir/lavish-browser-ctl" "$stage/bin/lavish-browser-ctl"
install -m 0644 "$wasm_dir/lavish_browser_zellij.wasm" \
  "$stage/share/lavish-browser/zellij/lavish-browser-zellij.wasm"
install -m 0644 "$root/data/works.from-nibly.LavishBrowser.desktop" "$stage/share/applications/"
install -m 0644 "$root/data/works.from-nibly.LavishBrowser.svg" \
  "$stage/share/icons/hicolor/scalable/apps/works.from-nibly.LavishBrowser.svg"

printf 'Installed Lavish Browser under %s\n' "$stage"
printf 'Optional Zellij plugin: %s/share/lavish-browser/zellij/lavish-browser-zellij.wasm\n' "$prefix"

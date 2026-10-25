#!/usr/bin/env bash
# Build the release binary and assemble a portable Linux bundle.
#
#   ./scripts/package-linux.sh [output-dir]
#
# The bundle is a plain tarball: the stripped binary, the desktop entry and the icon. Nothing
# is installed, so it can be unpacked anywhere and run from there; `--install` copies the
# files into ~/.local instead.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
root="$(cd "$here/.." && pwd)"
out="${1:-$here/target/package}"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$here/Cargo.toml" | head -1)"
name="wassh-$version-linux-x86_64"

echo "==> building release binary"
CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-8}" cargo build --manifest-path "$here/Cargo.toml" \
    --release -p wassh

stage="$out/$name"
rm -rf "$stage"
mkdir -p "$stage"
install -m 755 "$here/target/release/wassh" "$stage/wassh"
install -m 644 "$here/packaging/wassh.desktop" "$stage/wassh.desktop"

if [[ -f "$root/res/WaSSH program icon.png" ]]; then
    install -m 644 "$root/res/WaSSH program icon.png" "$stage/wassh.png"
fi
install -m 644 "$here/../README.md" "$stage/README.md" 2>/dev/null || true

cat >"$stage/INSTALL.txt" <<'TXT'
WaSSH — portable Linux build

Run it from this folder:
    ./wassh

Install for the current user:
    install -Dm755 wassh ~/.local/bin/wassh
    install -Dm644 wassh.desktop ~/.local/share/applications/wassh.desktop
    install -Dm644 wassh.png ~/.local/share/icons/hicolor/512x512/apps/wassh.png

Configuration lives in ~/.config/wassh-native (hosts, settings, encrypted secrets).
TXT

mkdir -p "$out"
tar -C "$out" -czf "$out/$name.tar.gz" "$name"
echo "==> $out/$name.tar.gz"
ls -lh "$out/$name.tar.gz" | awk '{print "    size:", $5}'

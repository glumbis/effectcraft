#!/usr/bin/env bash
# Package the fork's Linux x86_64 downloads from binaries that are already built:
#
#   <out>/EffectCraft-OFX-<label>-linux-x86_64.tar.gz     folder with the binaries, install.sh, ...
#   <out>/EffectCraft-OFX-<label>-linux-x86_64.AppImage   single file; carries the test plug-in and
#                                                         aep-import/ next to its binary, where
#                                                         the app looks for them
#
# Usage: package-linux.sh <label> [<out-dir>]      (label: e.g. 0.10.0-ofx.1; out-dir: <repo>/dist/fork)
#
# Needs, in <repo>/target: release/effectcraft and release/effectcraft-cli (built with
# --features "openfx aep") and ofx-test/ec-test-ofx.ofx.bundle (build-test-bundle.sh).
# The AppImage reuses packaging/linux/package.sh (which downloads appimagetool); build on an old
# distro (CI: Ubuntu 22.04) so the binaries run on newer ones.
set -euo pipefail

if [ $# -lt 1 ]; then
  echo "usage: $0 <label> [<out-dir>]" >&2
  exit 2
fi
LABEL="$1"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
OUT="${2:-$ROOT/dist/fork}"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
BIN="$TARGET_DIR/release"
BUNDLE="$TARGET_DIR/ofx-test/ec-test-ofx.ofx.bundle"
BASENAME="EffectCraft-OFX-$LABEL-linux-x86_64"
WORK="$TARGET_DIR/fork-linux-package"

for f in "$BIN/effectcraft" "$BIN/effectcraft-cli" "$BUNDLE"; do
  [ -e "$f" ] || { echo "error: $f not found (build first)" >&2; exit 1; }
done
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
rm -rf "$WORK"
mkdir -p "$WORK"

# What the app finds next to its executable: OFX/ (plug-in scan) and aep-import/ (the .aep converter).
stage_extras() {
  local dest="$1"
  mkdir -p "$dest/OFX" "$dest/aep-import"
  cp -R "$BUNDLE" "$dest/OFX/"
  cp "$ROOT"/tools/aep-import/*.py "$ROOT/tools/aep-import/README.md" "$ROOT/tools/aep-import/requirements.txt" "$dest/aep-import/"
}

# ---- AppImage -----------------------------------------------------------------------------------
# EFFECTCRAFT_STAGE_EXTRA is the hook in packaging/linux/package.sh: its contents are copied next to
# the binaries (usr/bin), and inside the AppImage the app's executable is usr/bin/effectcraft.
# EFFECTCRAFT_APPIMAGE_UPDATE_INFO=none is another: upstream's AppImage embeds update information that
# points at a .zsync next to its releases, and this fork never publishes one (a build run locally
# would even point at upstream's releases). The workflow sets it too; running this script by hand
# gets the same default.
export EFFECTCRAFT_APPIMAGE_UPDATE_INFO="${EFFECTCRAFT_APPIMAGE_UPDATE_INFO:-none}"
echo "==> AppImage"
mkdir -p "$WORK/appimage-extra"
stage_extras "$WORK/appimage-extra"
EFFECTCRAFT_VERSION="$LABEL" EFFECTCRAFT_STAGE_EXTRA="$WORK/appimage-extra" DIST="$WORK/appimage-out" \
  bash "$ROOT/packaging/linux/package.sh" --skip-build --formats appimage
mv "$WORK/appimage-out/effectcraft-$LABEL-linux-x86_64.AppImage" "$OUT/$BASENAME.AppImage"
chmod +x "$OUT/$BASENAME.AppImage"

# ---- tar.gz -------------------------------------------------------------------------------------
echo "==> tar.gz"
T="$WORK/tar/$BASENAME"
mkdir -p "$T"
for exe in effectcraft effectcraft-cli; do
  install -m 0755 "$BIN/$exe" "$T/$exe"
  strip "$T/$exe" 2>/dev/null || true
done
stage_extras "$T"
cp "$ROOT/FORK.md" "$T/README.md"
for f in LICENSE-MIT LICENSE-APACHE; do cp "$ROOT/$f" "$T/"; done
cp "$ROOT/assets/app-icon/hicolor/256x256/apps/ai.storyteller.effectcraft.png" "$T/effectcraft.png"
cp "$ROOT/assets/app-icon/hicolor/scalable/apps/ai.storyteller.effectcraft.svg" "$T/effectcraft.svg"
LINUX_DIR="$ROOT/tools/fork-installer/linux"
install -m 0755 "$LINUX_DIR/install.sh" "$T/install.sh"
install -m 0755 "$LINUX_DIR/uninstall.sh" "$T/uninstall.sh"
install -m 0644 "$LINUX_DIR/effectcraft-ofx.desktop.in" "$T/effectcraft-ofx.desktop.in"
# Scripts written on Windows may carry CRLF line endings, which bash can't run.
sed -i 's/\r$//' "$T/install.sh" "$T/uninstall.sh" "$T/effectcraft-ofx.desktop.in"
tar -C "$WORK/tar" --sort=name --owner=0 --group=0 --numeric-owner -czf "$OUT/$BASENAME.tar.gz" "$BASENAME"

echo "==> done"
ls -lh "$OUT"

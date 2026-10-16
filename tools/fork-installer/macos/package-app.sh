#!/usr/bin/env bash
# Package the fork's macOS download from binaries that are already built: EffectCraft.app (ad-hoc
# signed, not notarized: the fork has no Apple developer certificate) in a zip with a README.
#
#   <out>/EffectCraft-OFX-<label>-macos-<arm64|x86_64>.zip
#       EffectCraft-OFX-<label>-macos-<arch>/
#           EffectCraft.app      Contents/MacOS/{EffectCraft, effectcraft-cli, OFX/, aep-import/}
#           READ ME FIRST.txt    how to open an app that isn't notarized
#           Set up After Effects import.command
#           README.md, LICENSE-*
#
# OFX/ (the EC Test plug-in) and aep-import/ (the .aep converter) sit beside the executable because
# that is where the app looks (crates/ofx/src/loader.rs, crates/host/src/aep.rs).
#
# Usage: package-app.sh --label <label> --arch <arm64|x86_64> [--out <dir>] [--bin-dir <dir>] [--bundle <dir>]
#   --bin-dir  folder with the built `effectcraft` and `effectcraft-cli`
#              (default: target/<rust target>/release)
#   --bundle   the ec-test-ofx.ofx.bundle folder (default: target/ofx-test/ec-test-ofx.ofx.bundle)
# Needs macOS (codesign, plutil, ditto). Plug-ins are loaded without the hardened runtime on purpose:
# a hardened app refuses third-party OFX binaries unless it has the disable-library-validation
# entitlement.
set -euo pipefail

# The comment block at the top of this file is the help text.
help() { awk 'NR > 1 { if (/^#/) { sub(/^# ?/, ""); print } else exit }' "$0"; }

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
HERE="$ROOT/tools/fork-installer/macos"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
LABEL=""
ARCH=""
OUT="$ROOT/dist/fork"
BIN_DIR=""
BUNDLE="$TARGET_DIR/ofx-test/ec-test-ofx.ofx.bundle"
while [ $# -gt 0 ]; do
  case "$1" in
    --label) LABEL="$2"; shift 2 ;;
    --arch) ARCH="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --bin-dir) BIN_DIR="$2"; shift 2 ;;
    --bundle) BUNDLE="$2"; shift 2 ;;
    -h | --help) help; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
[ -n "$LABEL" ] || { echo "error: --label is required" >&2; exit 2; }
case "$ARCH" in
  arm64) RUST_TARGET=aarch64-apple-darwin ;;
  x86_64) RUST_TARGET=x86_64-apple-darwin ;;
  *) echo "error: --arch must be arm64 or x86_64" >&2; exit 2 ;;
esac
[ -n "$BIN_DIR" ] || BIN_DIR="$TARGET_DIR/$RUST_TARGET/release"
for f in "$BIN_DIR/effectcraft" "$BIN_DIR/effectcraft-cli" "$BUNDLE"; do
  [ -e "$f" ] || { echo "error: $f not found (build first)" >&2; exit 1; }
done

SHORT_VERSION="${LABEL%%-*}"
BUILD_SHA="${GITHUB_SHA:-$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo unknown)}"
NAME="EffectCraft-OFX-$LABEL-macos-$ARCH"
WORK="$TARGET_DIR/fork-macos-package-$ARCH"
STAGE="$WORK/$NAME"
APP="$STAGE/EffectCraft.app"
MACOS="$APP/Contents/MacOS"
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
rm -rf "$WORK"
mkdir -p "$MACOS" "$APP/Contents/Resources"

echo "==> assembling $APP"
# The executable and icon carry the display name (CFBundleExecutable / CFBundleIconFile).
cp "$BIN_DIR/effectcraft" "$MACOS/EffectCraft"
cp "$BIN_DIR/effectcraft-cli" "$MACOS/effectcraft-cli"
chmod 0755 "$MACOS/EffectCraft" "$MACOS/effectcraft-cli"
cp "$ROOT/assets/app-icon/effectcraft.icns" "$APP/Contents/Resources/EffectCraft.icns"
sed -e "s/@VERSION@/$LABEL/g" -e "s/@SHORT_VERSION@/$SHORT_VERSION/g" -e "s/@BUILD_SHA@/$BUILD_SHA/g" \
  "$ROOT/packaging/macos/Info.plist.in" >"$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist"
printf 'APPL????' >"$APP/Contents/PkgInfo"
mkdir -p "$MACOS/OFX" "$MACOS/aep-import"
cp -R "$BUNDLE" "$MACOS/OFX/"
cp "$ROOT"/tools/aep-import/*.py "$ROOT/tools/aep-import/README.md" "$ROOT/tools/aep-import/requirements.txt" "$MACOS/aep-import/"

# Ad-hoc signature, inside out: nested code first, then the app. Apple Silicon refuses to run
# unsigned code, so every Mach-O needs at least this.
sign() { codesign --force --sign - --timestamp=none "$@"; }
find "$MACOS/OFX" -type f -name '*.ofx' | while IFS= read -r f; do sign "$f"; done
sign "$MACOS/effectcraft-cli"
sign "$MACOS/EffectCraft"
sign "$APP"
if ! codesign --verify --deep --strict --verbose=2 "$APP"; then
  echo "::warning::codesign --verify --deep --strict failed for the ad-hoc signed app (see above)"
fi
codesign --display --verbose=2 "$APP" 2>&1 | sed 's/^/    /' || true

echo "==> writing the zip"
sed -e "s/@LABEL@/$LABEL/g" -e "s/@ARCH@/$ARCH/g" "$HERE/READ ME FIRST.txt.in" >"$STAGE/READ ME FIRST.txt"
install -m 0755 "$HERE/Set up After Effects import.command" "$STAGE/Set up After Effects import.command"
cp "$ROOT/FORK.md" "$STAGE/README.md"
for f in LICENSE-MIT LICENSE-APACHE; do cp "$ROOT/$f" "$STAGE/"; done
# Scripts written on Windows may carry CRLF line endings, which macOS can't run.
sed -i '' $'s/\r$//' "$STAGE/Set up After Effects import.command" "$STAGE/READ ME FIRST.txt"
rm -f "$OUT/$NAME.zip"
ditto -c -k --keepParent "$STAGE" "$OUT/$NAME.zip"
echo "==> done"
ls -lh "$OUT/$NAME.zip"

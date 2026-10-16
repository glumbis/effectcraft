#!/usr/bin/env bash
# Build the test OpenFX plug-in (examples/ofx/ec-test-ofx, its own cargo workspace) and lay it out
# as an OFX bundle, for the OFX integration tests (EC_TEST_OFX_BUNDLE) and for the fork's downloads:
#
#   <out>/ec-test-ofx.ofx.bundle/Contents/<Win64|MacOS|Linux-x86-64|Linux-aarch64>/ec-test-ofx.ofx
#
# The folder names are the ones crates/ofx/src/loader.rs looks for (PLATFORM_DIR).
#
# Usage: build-test-bundle.sh [--target <rust target triple>] [--out <dir>]
#   --target  cross-compile (default: the host); picks the bundle folder and library name
#   --out     where the .ofx.bundle folder goes (default: <repo>/target/ofx-test)
set -euo pipefail

# The comment block at the top of this file is the help text.
help() { awk 'NR > 1 { if (/^#/) { sub(/^# ?/, ""); print } else exit }' "$0"; }

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
TARGET=""
OUT="$ROOT/target/ofx-test"
while [ $# -gt 0 ]; do
  case "$1" in
    --target) TARGET="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    -h | --help) help; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

# Which OS and CPU the bundle is for: from the target triple when given, else from this machine.
if [ -n "$TARGET" ]; then
  triple="$TARGET"
else
  case "$(uname -s)" in
    MINGW* | MSYS* | CYGWIN*) triple="x86_64-pc-windows-msvc" ;;
    Darwin) triple="$(uname -m)-apple-darwin" ;;
    *) triple="$(uname -m)-unknown-linux-gnu" ;;
  esac
fi
case "$triple" in
  *-windows-*) dir=Win64; lib=ec_test_ofx.dll ;;
  *-apple-darwin) dir=MacOS; lib=libec_test_ofx.dylib ;;
  x86_64-*linux*) dir=Linux-x86-64; lib=libec_test_ofx.so ;;
  *linux*) dir=Linux-aarch64; lib=libec_test_ofx.so ;;
  *) echo "error: don't know the OFX bundle folder for $triple" >&2; exit 1 ;;
esac

# Relative paths for cargo: on Windows this runs in Git Bash, which would turn an absolute /d/a/...
# argument into a Windows path for a native program, but only reliably for plain arguments.
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
cd "$ROOT"
manifest=examples/ofx/ec-test-ofx/Cargo.toml
if [ -n "$TARGET" ]; then
  cargo build --release --locked --manifest-path "$manifest" --target "$TARGET"
  built="examples/ofx/ec-test-ofx/target/$TARGET/release/$lib"
else
  cargo build --release --locked --manifest-path "$manifest"
  built="examples/ofx/ec-test-ofx/target/release/$lib"
fi

bundle="$OUT/ec-test-ofx.ofx.bundle"
rm -rf "$bundle"
mkdir -p "$bundle/Contents/$dir"
cp "$built" "$bundle/Contents/$dir/ec-test-ofx.ofx"
echo "OFX test bundle ($triple): $bundle/Contents/$dir/ec-test-ofx.ofx"

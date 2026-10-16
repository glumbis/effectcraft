#!/usr/bin/env bash
# Removes EffectCraft (OFX) installed by install.sh: the app folder, its commands, its app menu
# entry and icon, and the private Python environment for .aep import it may have made
# (~/.config/effectcraft/aep-python). Your projects and settings (~/.config/effectcraft), OpenFX
# plug-ins you installed yourself (also those in the app's OFX folder, ~/.local/share/OFX/Plugins and
# ~/.config/effectcraft/OFX), and the system's Python and py-aep are left alone.
#
# Usage: uninstall.sh [--yes]
set -euo pipefail

# The comment block at the top of this file is the help text.
help() { awk 'NR > 1 { if (/^#/) { sub(/^# ?/, ""); print } else exit }' "$0"; }

DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/effectcraft"
BIN_DIR="$HOME/.local/bin"
DEST="$DATA_HOME/effectcraft-ofx"
ASSUME_YES=0

while [ $# -gt 0 ]; do
  case "$1" in
    -y | --yes) ASSUME_YES=1 ;;
    -h | --help) help; exit 0 ;;
    *) echo "unknown argument: $1 (try --help)" >&2; exit 2 ;;
  esac
  shift
done

say() { printf '%s\n' "$*"; }
ok() { printf '[ok] %s\n' "$*"; }
warn() { printf '[!]  %s\n' "$*" >&2; }

if [ "$ASSUME_YES" != 1 ] && [ -t 0 ]; then
  read -r -p "Remove EffectCraft (OFX) from $DEST ? [y/N] " answer || answer=n
  case "$answer" in y | Y | yes | YES) ;; *) exit 0 ;; esac
fi
# Is the installed app running? (The process list says which file each one was started from.)
app_running() {
  local exe
  for exe in /proc/[0-9]*/exe; do
    if [ "$(readlink "$exe" 2>/dev/null)" = "$DEST/effectcraft" ]; then return 0; fi
  done
  return 1
}
if app_running; then
  echo "EffectCraft (OFX) is running: close it first." >&2
  exit 1
fi

# Commands: only the ones that are ours (a symlink into DEST, or the launcher script an older
# install.sh wrote when py-aep lived in a private environment).
for cmd in effectcraft-ofx effectcraft-cli-ofx; do
  path="$BIN_DIR/$cmd"
  if [ -L "$path" ]; then
    case "$(readlink "$path")" in "$DEST"/*) rm -f "$path" ;; esac
  elif [ -f "$path" ] && grep -q "$DEST" "$path" 2>/dev/null; then
    rm -f "$path"
  fi
done
rm -f "$DATA_HOME/applications/effectcraft-ofx.desktop" \
  "$DATA_HOME/icons/hicolor/256x256/apps/effectcraft-ofx.png" \
  "$DATA_HOME/icons/hicolor/scalable/apps/effectcraft-ofx.svg"
if command -v update-desktop-database >/dev/null 2>&1; then update-desktop-database "$DATA_HOME/applications" >/dev/null 2>&1 || true; fi

# The private Python environment for .aep import. It is only ours when it is a virtual environment
# (pyvenv.cfg), so a folder somebody else put there is not touched.
if [ -f "$CONFIG_DIR/aep-python/pyvenv.cfg" ]; then
  rm -rf "$CONFIG_DIR/aep-python"
  ok "Removed the private Python environment for .aep import"
fi

# The app folder. Plug-ins the user copied into OFX/ stay (only our test plug-in goes).
if [ -d "$DEST/OFX" ]; then
  rm -rf "$DEST/OFX/ec-test-ofx.ofx.bundle"
  if [ -n "$(ls -A "$DEST/OFX" 2>/dev/null)" ]; then
    warn "Kept $DEST/OFX: it holds plug-ins you added. Delete it yourself if you don't need them."
    for f in effectcraft effectcraft-cli effectcraft.new effectcraft-cli.new README.md uninstall.sh LICENSE-MIT LICENSE-APACHE; do rm -f "$DEST/$f"; done
    rm -rf "$DEST/aep-import" "$DEST/venv"  # (venv: from an older installer)
    ok "EffectCraft (OFX) removed (except your plug-ins)"
    exit 0
  fi
fi
# This script may be running from inside the folder; the shell has read it already, so this is fine.
rm -rf "$DEST"
ok "EffectCraft (OFX) removed"
say "Settings and projects in $CONFIG_DIR were left alone."

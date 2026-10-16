#!/usr/bin/env bash
# Installs EffectCraft (OpenFX plug-ins + After Effects project import) for the current user.
# Run it from the unpacked download. It never needs root:
#
#   * the app goes to  ${XDG_DATA_HOME:-~/.local/share}/effectcraft-ofx
#   * commands:        ~/.local/bin/effectcraft-ofx  and  ~/.local/bin/effectcraft-cli-ofx
#   * app menu entry:  "EffectCraft (OFX)" (+ its icon) under ${XDG_DATA_HOME:-~/.local/share}
#   * the test plug-in "EC Test" next to the app, in OFX/ (EffectCraft scans that folder)
#   * for opening .aep / .aepx: py-aep, installed with `python3 -m pip install --user`, or, where
#     the system Python refuses that (Debian 12+, Ubuntu 23.04+, Arch...), into a private virtual
#     environment at ${XDG_CONFIG_HOME:-~/.config}/effectcraft/aep-python, which the app finds by itself
#
# Usage: ./install.sh [--yes] [--no-python]
#   --yes         answer yes to every question (also what happens when there is no terminal)
#   --no-python   skip the Python / py-aep step (After Effects project import)
set -euo pipefail

# The comment block at the top of this file is the help text.
help() { awk 'NR > 1 { if (/^#/) { sub(/^# ?/, ""); print } else exit }' "$0"; }

SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
# EffectCraft's config folder (crates/host/src/app_dirs.rs): settings, the OFX scan report, and the
# private Python environment for .aep import ("aep-python", found by the app without any setting).
CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/effectcraft"
BIN_DIR="$HOME/.local/bin"
DEST="$DATA_HOME/effectcraft-ofx"
ASSUME_YES=0
WITH_PYTHON=1

while [ $# -gt 0 ]; do
  case "$1" in
    -y | --yes) ASSUME_YES=1 ;;
    --no-python) WITH_PYTHON=0 ;;
    -h | --help) help; exit 0 ;;
    *) echo "unknown argument: $1 (try --help)" >&2; exit 2 ;;
  esac
  shift
done

say() { printf '%s\n' "$*"; }
ok() { printf '[ok] %s\n' "$*"; }
warn() { printf '[!]  %s\n' "$*" >&2; }

ask() {
  if [ "$ASSUME_YES" = 1 ] || [ ! -t 0 ]; then return 0; fi
  local answer
  read -r -p "$1 [Y/n] " answer || return 0
  case "$answer" in '' | y | Y | yes | YES) return 0 ;; *) return 1 ;; esac
}

say ""
say "EffectCraft with OpenFX plug-ins and After Effects project import"
say "Installing to $DEST"
say ""

if [ ! -f "$SRC/effectcraft" ] || [ ! -f "$SRC/effectcraft-cli" ]; then
  echo "effectcraft is not next to this installer: unpack the whole download first, then run it again." >&2
  exit 1
fi
if [ "$(uname -s)" != Linux ]; then
  echo "This installer is for Linux." >&2
  exit 1
fi
if [ "$(uname -m)" != x86_64 ]; then
  echo "This download is for x86_64 (64-bit Intel/AMD) Linux; this machine is $(uname -m)." >&2
  exit 1
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
  warn "EffectCraft (OFX) is running. It is replaced anyway; restart it afterwards."
fi

# 1. The app. Binaries are swapped with a rename, which is safe while the old one is running.
mkdir -p "$DEST" "$BIN_DIR"
for exe in effectcraft effectcraft-cli; do
  install -m 0755 "$SRC/$exe" "$DEST/$exe.new"
  mv -f "$DEST/$exe.new" "$DEST/$exe"
done
for f in README.md uninstall.sh LICENSE-MIT LICENSE-APACHE; do
  if [ -f "$SRC/$f" ]; then install -m 0644 "$SRC/$f" "$DEST/$f"; fi
done
chmod 0755 "$DEST/uninstall.sh" 2>/dev/null || true
if [ -d "$SRC/aep-import" ]; then
  rm -rf "$DEST/aep-import"
  cp -R "$SRC/aep-import" "$DEST/aep-import"
fi
ok "App installed"

# 2. The test plug-in, in OFX/ next to the app (EffectCraft scans it; other OFX hosts don't).
if [ -d "$SRC/OFX/ec-test-ofx.ofx.bundle" ]; then
  mkdir -p "$DEST/OFX"
  rm -rf "$DEST/OFX/ec-test-ofx.ofx.bundle"
  cp -R "$SRC/OFX/ec-test-ofx.ofx.bundle" "$DEST/OFX/"
  ok 'Test plug-in installed ("EC Test" effects)'
fi
say "     Your own OpenFX plug-ins (RSMB, Twixtor, Sapphire...): install their Linux OFX versions,"
say "     normally into /usr/OFX/Plugins, or, without root, copy the .ofx.bundle folder into"
say "     $DATA_HOME/OFX/Plugins (or $CONFIG_DIR/OFX). Both stay when EffectCraft is uninstalled."

# 3. Commands in ~/.local/bin. Named *-ofx so they never replace a distribution's own effectcraft.
#    (An older installer wrote launcher scripts here when py-aep lived in a private environment; the
#    app finds that environment itself now, so these are plain links again.)
ln -sfn "$DEST/effectcraft" "$BIN_DIR/effectcraft-ofx"
ln -sfn "$DEST/effectcraft-cli" "$BIN_DIR/effectcraft-cli-ofx"
ok "Commands: effectcraft-ofx, effectcraft-cli-ofx (in $BIN_DIR)"
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) warn "$BIN_DIR is not on your PATH. Add it (for example in ~/.profile) to type the commands in a terminal." ;;
esac

# 4. App menu entry and icon.
APPS_DIR="$DATA_HOME/applications"
ICON_DIR="$DATA_HOME/icons/hicolor"
mkdir -p "$APPS_DIR" "$ICON_DIR/256x256/apps" "$ICON_DIR/scalable/apps"
if [ -f "$SRC/effectcraft.png" ]; then install -m 0644 "$SRC/effectcraft.png" "$ICON_DIR/256x256/apps/effectcraft-ofx.png"; fi
if [ -f "$SRC/effectcraft.svg" ]; then install -m 0644 "$SRC/effectcraft.svg" "$ICON_DIR/scalable/apps/effectcraft-ofx.svg"; fi

# A path as one desktop-entry Exec argument: quoted, with a backslash before `"`, backtick, `$` and
# `\` (the key file's own string syntax doubles each backslash again), and `%` doubled.
exec_quote() {
  local s
  s="$(printf '%s' "$1" | sed -e 's/\\/\\\\\\\\/g' -e 's/"/\\\\"/g' -e 's/`/\\\\`/g' -e 's/\$/\\\\$/g' -e 's/%/%%/g')"
  printf '"%s"' "$s"
}
render_desktop() {
  local exec_q try line
  exec_q="$(exec_quote "$BIN_DIR/effectcraft-ofx")"
  try="$BIN_DIR/effectcraft-ofx"
  while IFS= read -r line || [ -n "$line" ]; do
    # Quoted replacements: an unquoted `&` would be taken as "the matched text" (bash 5.2).
    line="${line//@EXEC@/"$exec_q"}"
    line="${line//@TRYEXEC@/"$try"}"
    printf '%s\n' "$line"
  done <"$SRC/effectcraft-ofx.desktop.in"
}
if [ -f "$SRC/effectcraft-ofx.desktop.in" ]; then
  render_desktop >"$APPS_DIR/effectcraft-ofx.desktop"
  chmod 0644 "$APPS_DIR/effectcraft-ofx.desktop"
  if command -v update-desktop-database >/dev/null 2>&1; then update-desktop-database "$APPS_DIR" >/dev/null 2>&1 || true; fi
  if command -v gtk-update-icon-cache >/dev/null 2>&1; then gtk-update-icon-cache -q -t -f "$ICON_DIR" >/dev/null 2>&1 || true; fi
  ok 'App menu entry: "EffectCraft (OFX)"'
fi

# 5. Libraries the app needs from the system. ldd lists what the dynamic linker needs at start-up
#    (libasound; libc and friends); the libraries opened later are checked further down.
lib_hint() {
  case "$1" in
    libasound.so.2) echo "libasound2 (Debian/Ubuntu 24.04+: libasound2t64) | alsa-lib (Fedora)" ;;
    libxkbcommon.so.0) echo "libxkbcommon0 (Debian/Ubuntu) | libxkbcommon (Fedora)" ;;
    libxkbcommon-x11.so.0) echo "libxkbcommon-x11-0 (Debian/Ubuntu) | libxkbcommon-x11 (Fedora)" ;;
    libwayland-client.so.0) echo "libwayland-client0 (Debian/Ubuntu) | libwayland-client (Fedora)" ;;
    libwayland-cursor.so.0) echo "libwayland-cursor0 (Debian/Ubuntu) | libwayland-cursor (Fedora)" ;;
    libwayland-egl.so.1) echo "libwayland-egl1 (Debian/Ubuntu) | libwayland-egl (Fedora)" ;;
    libX11.so.6) echo "libx11-6 (Debian/Ubuntu) | libX11 (Fedora)" ;;
    libxcb.so.1) echo "libxcb1 (Debian/Ubuntu) | libxcb (Fedora)" ;;
    libGL.so.1) echo "libgl1 (Debian/Ubuntu) | mesa-libGL (Fedora)" ;;
    libEGL.so.1) echo "libegl1 (Debian/Ubuntu) | mesa-libEGL (Fedora)" ;;
    libvulkan.so.1) echo "libvulkan1 (Debian/Ubuntu) | vulkan-loader (Fedora)" ;;
    libdbus-1.so.3) echo "libdbus-1-3 (Debian/Ubuntu) | dbus-libs (Fedora)" ;;
    *) echo "" ;;
  esac
}
if command -v ldd >/dev/null 2>&1; then
  missing="$(ldd "$DEST/effectcraft" 2>/dev/null | awk '/not found/ { print $1 }' | sort -u || true)"
  if [ -n "$missing" ]; then
    warn "These system libraries are missing, so EffectCraft will not start until they are installed:"
    for lib in $missing; do
      hint="$(lib_hint "$lib")"
      if [ -n "$hint" ]; then warn "  $lib  ->  $hint"; else warn "  $lib"; fi
    done
  else
    ok "All required system libraries found"
  fi
fi
# The libraries the app opens when it needs them (found with `strings` in the CI build): graphics
# (EGL), keyboard (xkbcommon), the Wayland ones in a Wayland session, D-Bus for desktop portals.
# Which are really needed depends on the session, so this only warns.
LDCONFIG="$(command -v ldconfig 2>/dev/null || true)"
if [ -z "$LDCONFIG" ] && [ -x /sbin/ldconfig ]; then LDCONFIG=/sbin/ldconfig; fi
if [ -n "$LDCONFIG" ]; then
  cache="$("$LDCONFIG" -p 2>/dev/null || true)"
  if [ -n "$cache" ]; then
    absent=""
    for lib in libEGL.so.1 libxkbcommon.so.0 libxkbcommon-x11.so.0 libwayland-client.so.0 libwayland-egl.so.1 libdbus-1.so.3; do
      case "$cache" in *"$lib "*) ;; *) absent="$absent $lib" ;; esac
    done
    if [ -n "$absent" ]; then
      warn "Not found (EffectCraft loads these when it runs; you may only need some, depending on Wayland or X11):"
      for lib in $absent; do
        hint="$(lib_hint "$lib")"
        if [ -n "$hint" ]; then warn "  $lib  ->  $hint"; else warn "  $lib"; fi
      done
    fi
  fi
fi

# 6. Python + py-aep for .aep / .aepx import.
#    The app looks for Python in this order: EFFECTCRAFT_PYTHON, $CONFIG_DIR/aep-python/bin/python3
#    (the private environment below), then python3 on the PATH (crates/host/src/aep.rs). So an
#    environment made here is used without a launcher script or an environment variable.
VENV="$CONFIG_DIR/aep-python"
# Made by an older installer next to the app (with launcher scripts in ~/.local/bin, replaced above).
if [ -d "$DEST/venv" ]; then rm -rf "$DEST/venv"; fi
if [ "$WITH_PYTHON" = 1 ]; then
  say ""
  if [ -x "$VENV/bin/python3" ] && "$VENV/bin/python3" -c 'import py_aep' >/dev/null 2>&1; then
    ok "After Effects project import ready (py-aep is in the private environment $VENV)"
  elif ! command -v python3 >/dev/null 2>&1; then
    warn "Python 3 is needed only to open After Effects projects (.aep / .aepx). It was not found."
    say "     Install it (Debian/Ubuntu: sudo apt install python3 python3-pip; Fedora: sudo dnf install python3 python3-pip),"
    say "     then run:  python3 -m pip install --user py-aep"
  elif python3 -c 'import py_aep' >/dev/null 2>&1; then
    ok "After Effects project import ready (py-aep is already installed)"
  elif ask "Install py-aep (reads After Effects projects) for your user with pip?"; then
    log="$(mktemp)"
    if python3 -m pip install --user --upgrade --disable-pip-version-check py-aep >"$log" 2>&1 && python3 -c 'import py_aep' >/dev/null 2>&1; then
      ok "After Effects project import ready: File > Import > After Effects Project..."
    elif grep -q 'externally-managed-environment' "$log"; then
      # Debian 12+, Ubuntu 23.04+, Arch and others refuse `pip --user` for the system Python:
      # keep py-aep in a private virtual environment in EffectCraft's config folder instead, where
      # the app looks for one.
      say "     This Python is managed by your distribution, so py-aep goes into a private environment: $VENV"
      made_venv=0
      if [ ! -e "$VENV" ]; then made_venv=1; fi
      mkdir -p "$CONFIG_DIR"
      if { [ -x "$VENV/bin/python3" ] || python3 -m venv "$VENV" >/dev/null 2>&1; } \
        && "$VENV/bin/python3" -m pip install --upgrade --disable-pip-version-check py-aep >"$log" 2>&1 \
        && "$VENV/bin/python3" -c 'import py_aep' >/dev/null 2>&1; then
        ok "After Effects project import ready (EffectCraft finds the private environment by itself)"
      else
        # Do not leave a half-made environment (the app would try it first); one that was there before stays.
        if [ "$made_venv" = 1 ]; then rm -rf "$VENV"; fi
        warn "Could not create the environment (Debian/Ubuntu: sudo apt install python3-venv). Or use pipx:  pipx install py-aep"
        tail -n 5 "$log" >&2 || true
      fi
    else
      warn "py-aep did not install. Try later in a terminal:  python3 -m pip install --user py-aep"
      tail -n 5 "$log" >&2 || true
    fi
    rm -f "$log"
  else
    say "[--] Skipped .aep import setup. Later:  python3 -m pip install --user py-aep"
  fi
fi

say ""
say 'Done. Start EffectCraft from your app menu ("EffectCraft (OFX)") or with:  effectcraft-ofx'
say "Settings and projects stay in $CONFIG_DIR. Uninstall with:  $DEST/uninstall.sh"

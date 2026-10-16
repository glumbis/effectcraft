#!/bin/bash
# Installs py-aep, the Python library EffectCraft uses to open After Effects projects (.aep / .aepx).
# Double-click this file in Finder (or run it in Terminal). It never needs administrator rights.
#
# py-aep goes into a private Python environment in EffectCraft's config folder,
#     ~/Library/Application Support/EffectCraft/aep-python
# which EffectCraft looks in first when it opens an After Effects project, so nothing else needs
# setting up. A virtual environment works where `pip install --user` may not: recent Pythons (Homebrew's,
# python.org's 3.12 and newer) refuse it ("externally-managed-environment", PEP 668). Only if the
# environment cannot be made does this fall back to `pip install --user`.
#
# EffectCraft is started by macOS with a short PATH, so it finds a Python by itself in these places:
# the environment above, python3 on that PATH (Apple's /usr/bin/python3, which comes with the Xcode
# Command Line Tools), /opt/homebrew/bin/python3 and /usr/local/bin/python3. This tries the same ones.
set -u

CONFIG_DIR="$HOME/Library/Application Support/EffectCraft"
VENV="$CONFIG_DIR/aep-python"

pause() { read -r -p "Press Enter to close. " _; }

echo "Setting up After Effects project import for EffectCraft"
echo

# A Python 3 that works: Apple's (it needs the Command Line Tools), then Homebrew's, then python.org's.
CLT=1
xcode-select -p >/dev/null 2>&1 || CLT=0
PY=""
for candidate in /usr/bin/python3 /opt/homebrew/bin/python3 /usr/local/bin/python3; do
  # Without the Command Line Tools /usr/bin/python3 is a stub that opens an install dialog: leave it alone.
  if [ "$candidate" = /usr/bin/python3 ] && [ "$CLT" = 0 ]; then continue; fi
  if [ -x "$candidate" ] && "$candidate" --version >/dev/null 2>&1; then
    PY="$candidate"
    break
  fi
done
if [ -z "$PY" ]; then
  if [ "$CLT" = 0 ]; then
    echo "Python 3 comes with Apple's Command Line Tools, which are not installed yet."
    echo "A window will ask to install them. When that has finished, run this file again."
    xcode-select --install
  else
    echo "No working Python 3 was found. Install it from https://www.python.org/downloads/ and run this file again."
  fi
  pause
  exit 1
fi

echo "Using $("$PY" --version) at $PY"
echo

if [ -x "$VENV/bin/python3" ] && "$VENV/bin/python3" -c 'import py_aep' >/dev/null 2>&1; then
  echo "Done: py-aep is already set up in $VENV."
  echo "File > Import > After Effects Project... works in EffectCraft."
elif "$PY" -c 'import py_aep' >/dev/null 2>&1; then
  echo "Done: py-aep is already installed for $PY."
  echo "File > Import > After Effects Project... works in EffectCraft."
else
  log="$(mktemp)"
  # An environment that was already there stays, whatever happens; one made now is removed if it fails.
  made=0
  [ -e "$VENV" ] || made=1
  mkdir -p "$CONFIG_DIR"
  if { [ -x "$VENV/bin/python3" ] || "$PY" -m venv "$VENV" >"$log" 2>&1; } \
    && "$VENV/bin/python3" -m pip install --upgrade --disable-pip-version-check py-aep >>"$log" 2>&1 \
    && "$VENV/bin/python3" -c 'import py_aep' >/dev/null 2>&1; then
    echo "Done: py-aep is installed in $VENV."
    echo "File > Import > After Effects Project... now works in EffectCraft."
  else
    if [ "$made" = 1 ]; then rm -rf "${VENV:?}"; fi
    echo "The private environment could not be set up:"
    tail -n 3 "$log"
    echo
    echo "Trying pip --user instead..."
    if "$PY" -m pip install --user --upgrade --disable-pip-version-check py-aep >"$log" 2>&1 && "$PY" -c 'import py_aep' >/dev/null 2>&1; then
      echo "Done: py-aep is installed for $PY."
      echo "File > Import > After Effects Project... now works in EffectCraft."
    else
      echo "py-aep did not install:"
      tail -n 5 "$log"
      echo
      echo "Try in Terminal:  $PY -m venv \"$VENV\"  and then  \"$VENV/bin/pip\" install py-aep"
    fi
  fi
  rm -f "$log"
fi
echo
pause

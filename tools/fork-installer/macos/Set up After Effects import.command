#!/bin/bash
# Installs py-aep, the Python library EffectCraft uses to open After Effects projects (.aep / .aepx).
# Double-click this file in Finder (or run it in Terminal). It never needs administrator rights.
#
# EffectCraft is started by macOS with a short PATH, where `python3` is Apple's /usr/bin/python3
# (it comes with the Xcode Command Line Tools). That is the Python this installs py-aep for.
set -u

echo "Setting up After Effects project import for EffectCraft"
echo

if ! xcode-select -p >/dev/null 2>&1; then
  echo "Python 3 comes with Apple's Command Line Tools, which are not installed yet."
  echo "A window will ask to install them. When that has finished, run this file again."
  xcode-select --install
  read -r -p "Press Enter to close. " _
  exit 1
fi

PY=/usr/bin/python3
if ! "$PY" --version >/dev/null 2>&1; then
  echo "$PY does not work. Install Python 3 from https://www.python.org/downloads/ and run:"
  echo "    python3 -m pip install --user py-aep"
  read -r -p "Press Enter to close. " _
  exit 1
fi

echo "Using $("$PY" --version) at $PY"
if "$PY" -m pip install --user --upgrade --disable-pip-version-check py-aep && "$PY" -c 'import py_aep'; then
  echo
  echo "Done: File > Import > After Effects Project... now works in EffectCraft."
else
  echo
  echo "py-aep did not install. Try in Terminal:  $PY -m pip install --user py-aep"
fi
echo
read -r -p "Press Enter to close. " _

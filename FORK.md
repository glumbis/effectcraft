# EffectCraft with OpenFX plug-ins and After Effects import (unofficial)

This is an **unofficial** version of [EffectCraft](https://github.com/storytold/effectcraft), the
free, open-source After Effects alternative. It adds two things the official version doesn't have:

- **OpenFX (OFX) plug-ins.** OFX is the plug-in standard of DaVinci Resolve, Nuke, Vegas and
  Natron. In this fork its plug-ins run as ordinary effects: they show up in Effects & Presets,
  their settings appear in Effect Controls, and they take keyframes and expressions.
- **Opening After Effects projects** (`.aep` and `.aepx`). File ▸ Import ▸ After Effects
  Project… rebuilds the comps, layers, keyframes, expressions, masks and effects.

It runs on **Windows, Linux and macOS**.

> **Please don't report problems with this fork to the EffectCraft maintainers or to plug-in
> vendors.** They don't support it. Use [this fork's Issues](https://github.com/glumbis/effectcraft/issues).

## Contents

1. [What you need (and what you don't)](#what-you-need-and-what-you-dont)
2. [Download](#download)
3. [Install on Windows](#install-on-windows)
4. [Install on Linux](#install-on-linux)
5. [Install on macOS](#install-on-macos)
6. [Using OpenFX plug-ins](#using-openfx-plug-ins)
7. [Opening After Effects projects](#opening-after-effects-projects)
8. [Troubleshooting](#troubleshooting)
9. [Reporting a problem](#reporting-a-problem)
10. [Settings and files reference](#settings-and-files-reference)
11. [Building it yourself](#building-it-yourself)
12. [How this fork is organised](#how-this-fork-is-organised)

## What you need (and what you don't)

EffectCraft is one program. Nothing else needs installing to run it, apart from the small items in
the first row below (nothing at all on macOS). Opening `.aep` files needs one optional extra.
Installing py-aep needs an internet connection.

| | Windows | Linux | macOS |
|---|---|---|---|
| **To run EffectCraft** | The Microsoft Visual C++ 2015–2022 (x64) Redistributable. Most PCs already have it. | A few system libraries. Most desktop installs already have them. | Nothing |
| **To open `.aep` / `.aepx` files** (optional) | Python 3 and the free py-aep library. The installer sets both up (it asks before installing Python). | Python 3 (from your distribution) and py-aep. The installer sets py-aep up. | Python 3 and py-aep. A small script sets py-aep up. |
| **To use OpenFX plug-ins** | The plug-ins themselves. EffectCraft includes none. | Same | Same |

You do **not** need After Effects, DaVinci Resolve or any other host installed. Plug-ins come from
their vendors and have their own licences (see [Licences](#licences)).

**How well tested is each system?** The Windows build is the one used day to day with RSMB, Twixtor
and Sapphire. The Linux and macOS builds are new in this release. They are built and tested
automatically (the OpenFX tests run on Windows, Linux and an Apple Silicon Mac; the Intel Mac app is
built from the same code and given a basic start-up check). Nobody has tried the macOS builds
hands-on on a Mac yet, and no commercial plug-in has been confirmed on Linux or macOS. Please
[tell us how it goes](#reporting-a-problem).

## Download

1. Open the [**Releases page**](https://github.com/glumbis/effectcraft/releases) and pick the newest
   release (the title starts with "EffectCraft + OpenFX"). Don't download "Source code".
2. Under **Assets**, download the file for your system:

| Your system | Download |
|---|---|
| Windows 10 or 11, 64-bit | `EffectCraft-OFX-<label>-windows-x64.zip` |
| Linux, 64-bit Intel/AMD (x86_64), installer | `EffectCraft-OFX-<label>-linux-x86_64.tar.gz` |
| Linux, 64-bit Intel/AMD (x86_64), single file | `EffectCraft-OFX-<label>-linux-x86_64.AppImage` |
| Mac with Apple Silicon (M1 or later) | `EffectCraft-OFX-<label>-macos-arm64.zip` |
| Mac with an Intel processor | `EffectCraft-OFX-<label>-macos-x86_64.zip` |

`<label>` is the release name, for example `0.10.0-ofx.1`: EffectCraft 0.10.0, fork build 1.
`SHA256SUMS.txt` on the same page lets you check a download.

**Not sure which Mac you have?** Apple menu ▸ About This Mac. "Chip: Apple M…" means Apple
Silicon; "Processor: Intel…" means Intel.

## Install on Windows

**Requirements:** Windows 10 or 11, 64-bit. No administrator rights needed.
If EffectCraft won't start and Windows complains about `VCRUNTIME140.dll`, install Microsoft's
[Visual C++ Redistributable (x64)](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist),
then try again.

1. **Unzip** `EffectCraft-OFX-<label>-windows-x64.zip`: right-click it ▸ *Extract All…* You get a
   folder called `EffectCraft-OFX`. (Don't run the installer from inside the zip.)
2. **Double-click `Install EffectCraft.bat`** in that folder. A black window opens. Windows may say
   "Windows protected your PC" because the files aren't code-signed: click *More info* ▸ *Run
   anyway*.
3. **Answer its question, if it asks one.** If Python 3 isn't found, the installer offers to
   install it with winget. Say `n` to skip: Python is only needed for `.aep` files, and you can add
   it later. Press a key when it says it's done.
4. **Start "EffectCraft (OFX)"** from the Start menu or the desktop. If Windows warns again:
   *More info* ▸ *Run anyway*.

What the installer does:

- Copies the app to `%LOCALAPPDATA%\Programs\EffectCraft-OFX` (paste that into File Explorer's
  address bar to open it).
- Adds Start menu and desktop shortcuts named **EffectCraft (OFX)**.
- Adds **EC Test**, a tiny test plug-in, in the `OFX` folder next to the app, so you can check that
  OpenFX works before you install any real plug-in.
- Finds Python 3 and installs the py-aep library for your user (for `.aep` files).
- Tells you that you can copy plug-ins into `%APPDATA%\EffectCraft\OFX` without administrator rights.

It doesn't need administrator rights and doesn't list itself under Settings ▸ Apps.

**First start.** If you have large plug-in suites installed, the window can take a few seconds to
appear while they load. Your plug-ins appear in **Effects & Presets**. See
[Using OpenFX plug-ins](#using-openfx-plug-ins).

**Updating.** Unzip the new download and run `Install EffectCraft.bat` again. Close EffectCraft
first. Your settings, projects and plug-ins stay.

**Uninstalling.** Run `Uninstall EffectCraft.bat` in `%LOCALAPPDATA%\Programs\EffectCraft-OFX`. It
removes the app, the shortcuts and the EC Test plug-in. Your projects, your settings
(`%APPDATA%\EffectCraft`), Python and py-aep stay. Plug-ins that vendors installed in
`C:\Program Files\Common Files\OFX\Plugins` stay too. So do plug-ins you added to the app's own `OFX`
folder (only the bundled EC Test plug-in is removed from it; if other bundles remain, that folder is
kept and the uninstaller prints its path) and everything in `%APPDATA%\EffectCraft\OFX`, which it
never touches. Installing over an existing install also keeps them.

## Install on Linux

**Requirements:** 64-bit Intel/AMD (x86_64), glibc 2.35 or newer. That means Ubuntu 22.04 or newer,
Debian 12 or newer, Fedora 36 or newer, or anything of the same age or newer. You also need a
working graphics driver (Mesa for AMD and Intel, NVIDIA's own driver for NVIDIA).

**System libraries.** EffectCraft needs a few libraries that nearly every desktop install already
has. If yours is missing one, the installer tells you which. To install them all at once:

```sh
# Ubuntu 22.04, Debian 12
sudo apt install libasound2 libegl1 libxkbcommon0 libxkbcommon-x11-0 libwayland-client0 libwayland-egl1 libdbus-1-3
# Ubuntu 24.04 and newer: the same, but libasound2t64 instead of libasound2
# Fedora
sudo dnf install alsa-lib mesa-libEGL libxkbcommon libxkbcommon-x11 libwayland-client libwayland-egl dbus-libs
```

### Option A: the installer (`.tar.gz`)

1. **Unpack** the download and go into the folder:

   ```sh
   tar xzf EffectCraft-OFX-<label>-linux-x86_64.tar.gz
   cd EffectCraft-OFX-<label>-linux-x86_64
   ```

2. **Run the installer.** It never needs root. Add `--yes` to answer every question with yes, or
   `--no-python` to skip the `.aep` setup.

   ```sh
   ./install.sh
   ```

3. **Start it** from your app menu ("EffectCraft (OFX)") or with `effectcraft-ofx`.

What the installer does:

- Puts the app in `~/.local/share/effectcraft-ofx`.
- Adds the commands `effectcraft-ofx` and `effectcraft-cli-ofx` in `~/.local/bin`. If that folder
  isn't on your `PATH`, the installer says so; add it in `~/.profile` to use the commands in a
  terminal. The menu entry works either way.
- Adds an app-menu entry named **EffectCraft (OFX)** with an icon.
- Adds the **EC Test** test plug-in in the `OFX` folder inside the app folder.
- Warns about missing system libraries (see above).
- Offers to install py-aep for `.aep` files. Newer Debian, Ubuntu and Arch refuse `pip install --user`
  for the system Python; in that case the installer puts py-aep in a private environment,
  `~/.config/effectcraft/aep-python`, which EffectCraft finds by itself. If Python 3 isn't installed,
  it tells you what to install.
- Tells you where to put plug-ins without root: `~/.local/share/OFX/Plugins` or
  `~/.config/effectcraft/OFX` (see [Where plug-ins go](#where-plug-ins-go)).

You can also run `./effectcraft` straight from the unpacked folder without installing.

### Option B: the AppImage (single file)

1. Make the file executable, then run it:

   ```sh
   chmod +x EffectCraft-OFX-<label>-linux-x86_64.AppImage
   ./EffectCraft-OFX-<label>-linux-x86_64.AppImage
   ```

It doesn't need FUSE. It does need the system libraries above, and it has no installer: for `.aep`
files install py-aep yourself (see [Opening After Effects projects](#opening-after-effects-projects)).
Put plug-ins in `/usr/OFX/Plugins`, or, without root, in `~/.local/share/OFX/Plugins` or
`~/.config/effectcraft/OFX` (see [Where plug-ins go](#where-plug-ins-go)).

**Wayland.** On a native Wayland session, dragging files into the window doesn't work. Use File ▸
Import ▸ File…, or start with `env -u WAYLAND_DISPLAY effectcraft-ofx` to run through XWayland.

**Updating.** Option A: unpack the new download and run `./install.sh` again. The app is replaced;
settings and plug-ins stay. Option B: download the new AppImage and delete the old one.

**Uninstalling (Option A).**

```sh
~/.local/share/effectcraft-ofx/uninstall.sh
```

This removes the app, the two commands, the menu entry, the icon and the private py-aep environment
(`~/.config/effectcraft/aep-python`, if the installer made one). Your projects and settings
(`~/.config/effectcraft`), the system's Python and py-aep stay, and so do all your plug-ins: those in
the app's `OFX` folder, in `~/.local/share/OFX/Plugins` and in `~/.config/effectcraft/OFX`.

## Install on macOS

**Requirements:** macOS 11 (Big Sur) or newer, on an Apple Silicon or Intel Mac. Use the download
that matches your Mac. Nothing else needs installing to run EffectCraft.

> **Status:** these builds are built and tested automatically (on Apple Silicon and, for the start-up
> check, Intel), but nobody has tried them hands-on on a Mac yet. If something doesn't work, please
> [report it](#reporting-a-problem).

1. **Unzip** the download and drag **EffectCraft.app** into your Applications folder.
2. **Open it the first time.** The app isn't notarized by Apple (the project has no Apple developer
   certificate), so macOS blocks it at first. Do **one** of these:
   - Open Terminal and run this once:

     ```sh
     xattr -dr com.apple.quarantine /Applications/EffectCraft.app
     ```

   - Or try to open EffectCraft, then go to System Settings ▸ Privacy & Security, scroll down to
     "EffectCraft was blocked" and click **Open Anyway**. (On macOS 14 and older you can also
     right-click the app and choose Open.)
3. **For `.aep` files:** run the same `xattr` command on the unzipped folder
   (`xattr -dr com.apple.quarantine "<the folder>"`), then double-click
   **`Set up After Effects import.command`**. See [Opening After Effects
   projects](#opening-after-effects-projects).

The download also has `READ ME FIRST.txt` with these steps.

**Plug-ins.** Vendors' installers put them in `/Library/OFX/Plugins`. To add one by hand without an
administrator password, use `~/Library/OFX/Plugins` or `~/Library/Application Support/EffectCraft/OFX`
(see [Where plug-ins go](#where-plug-ins-go)).

**Command line.** The CLI is inside the app:
`/Applications/EffectCraft.app/Contents/MacOS/effectcraft-cli`.

**Updating.** Quit EffectCraft, replace the app in Applications with the new one, and run the
`xattr` command again.

**Uninstalling.** Drag EffectCraft.app to the Bin. Your settings, projects, per-user plug-ins and the
py-aep environment stay in `~/Library/Application Support/EffectCraft`; delete that folder too to
remove everything.

## Using OpenFX plug-ins

### In short

1. **Install the plug-in** with its own installer, and choose the **OpenFX (OFX)** version (not the
   After Effects one).
2. **Restart EffectCraft.** It looks for plug-ins every time it starts.
3. **Find it** in Effects & Presets, under the plug-in's own category (for example *REVision
   Effects*, *Sapphire Stylize*), or in the Effect menu. Apply it like any effect; its settings are
   in Effect Controls and take keyframes and expressions.

To check that OpenFX works before you install anything, look for the **EC Test** category in
Effects & Presets: it's a small test plug-in that comes with the download.

If your plug-in doesn't show up, see [Getting plug-ins to work](#getting-plug-ins-to-work).

### Is it an OpenFX plug-in at all?

EffectCraft runs **OpenFX (OFX) image effects only**, the kind DaVinci Resolve, Nuke, Vegas and
Natron use. These do **not** work, whatever the vendor:

| Kind | Examples | Why |
|---|---|---|
| After Effects plug-ins (`.aex`) | Element 3D, Optical Flares, Deep Glow, the AE builds of Trapcode, Sapphire, RSMB, Twixtor | Adobe's own plug-in API, which EffectCraft doesn't implement |
| After Effects extensions and panels (CEP/UXP, `.zxp`) | **GEOlayers 3**, Motion Tools, Overlord, Flow, AEJuice | HTML panels that run inside Adobe's extension runtime |
| Premiere-only, Resolve-only (DCTL, Fusion macros) or Vegas-only formats | | Different plug-in APIs |
| OFX plug-ins with no CPU renderer | some GPU-only effects | EffectCraft renders OFX on the CPU only |

Plain ExtendScript scripts (`.jsx`) are a separate thing: EffectCraft can run many of them with
**File ▸ Scripts ▸ Run Script File…** (no `.jsxbin`, no CEP panels).

Many vendors sell the OFX version separately or as a different download: look for "OFX",
"Resolve / Vegas / Nuke" or "OpenFX" on the vendor's download page. If the installer lets you pick
hosts, tick **OFX** (or DaVinci Resolve, which installs the OFX version).

### Where plug-ins go

An OpenFX plug-in is a folder called `Something.ofx.bundle`. Vendors' installers put it in the
standard folder for your system, and EffectCraft looks there automatically:

| System | Standard folder (needs administrator rights, or `sudo`, to write to) |
|---|---|
| Windows | `C:\Program Files\Common Files\OFX\Plugins` |
| macOS | `/Library/OFX/Plugins` |
| Linux | `/usr/OFX/Plugins` (and `/usr/local/OFX/Plugins`) |

EffectCraft also looks in folders that **don't** need administrator rights:

| Where | Notes |
|---|---|
| **The `OFX` folder in the [settings folder](#where-the-settings-folder-is)** (every system) | Windows `%APPDATA%\EffectCraft\OFX`, macOS `~/Library/Application Support/EffectCraft/OFX`, Linux `~/.config/effectcraft/OFX`. **This is the place for plug-ins you copy by hand.** The uninstallers and installers never touch it, so your plug-ins survive an update or a reinstall. Portable mode moves it along with the settings. |
| macOS: `~/Library/OFX/Plugins` | The per-user version of `/Library/OFX/Plugins`. |
| Linux: `~/.local/share/OFX/Plugins` | The per-user version of `/usr/OFX/Plugins`. If `XDG_DATA_HOME` is set to a full path, it's `$XDG_DATA_HOME/OFX/Plugins`. |
| The `OFX` folder next to the app | Windows: `%LOCALAPPDATA%\Programs\EffectCraft-OFX\OFX`. Linux installer: `~/.local/share/effectcraft-ofx/OFX`. The EC Test plug-in lives here. On macOS this folder is inside the app, which you shouldn't change. The AppImage's own folder is read-only. |
| Every folder in the `OFX_PLUGIN_PATH` environment variable | Separate several folders with `;` on Windows and `:` on Linux and macOS. See [Environment variables](#environment-variables). |

**The easy way to open the per-user folder:** Effect ▸ **Open OpenFX Plug-in Folder**. It creates
`OFX` in the settings folder if it isn't there and opens it. Copy the `.ofx.bundle` folder in and
restart EffectCraft. (Effect ▸ **Open OpenFX Scan Report** opens `ofx_scan.txt`.)

All of these are searched including everything below them, system folders first, then the per-user
ones, the folder next to the app, and `OFX_PLUGIN_PATH`. A folder reached twice is searched once. If
the same plug-in is installed in two places, one copy is used: the newest within one folder tree,
otherwise the one in the folder searched first. The report's **Notes** list the copies it skipped.
A bundle looks like this, and **the platform folder must match your system**:

```
Something.ofx.bundle/
  Contents/
    Win64/          Windows (64-bit)       Something.ofx
    MacOS/          macOS                  Something.ofx
    Linux-x86-64/   Linux (x86_64)         Something.ofx
```

A folder named `Win32`, or a bundle with only a `MacOS` folder on Windows, is not a build for your
system and can't load.

**Loading one by hand.** Effect ▸ **Load OpenFX Plug-in…**, then pick the `.ofx` file inside the
platform folder. This only lasts until you quit. To keep a plug-in, put it in one of the folders
above (the per-user `OFX` folder is the easiest).

### Fast starts: the scan cache

Loading a big suite is slow: Sapphire alone has about 290 effects, and each one used to be loaded
and described at every start. So after a scan, EffectCraft remembers what each plug-in file
contains, in **`ofx_cache.json`** in the [settings folder](#where-the-settings-folder-is). On the
next start:

- A plug-in file that hasn't changed is registered from that memory **without being loaded**. It
  still shows up in Effects & Presets as usual.
- Its library is loaded the first time you use one of its effects. The first render with a plug-in
  after starting can therefore take a moment longer.

What this means for you:

- **Installing, updating or removing a plug-in needs nothing.** The cache notices that the file
  changed (it compares the file's path, size and modification time) and scans it again.
- **If you fix something the cache can't see**, such as activating a licence or replacing a support
  file next to the plug-in without touching its `.ofx` file, force a rescan: with EffectCraft closed,
  delete `ofx_cache.json` (or run `effectcraft-cli exec effect.plugins.rescanOfx --empty`), then
  start EffectCraft.
- **The cache is thrown away and rebuilt by itself** when EffectCraft is updated or the [host
  name](#4-try-a-different-host-name) changes.
- **If a cached plug-in can't be loaded when you first use it**, you see "the OpenFX plug-in X could
  not be loaded: …" and EffectCraft scans that file again at the next start.
- **To turn the cache off**, put `"scan_cache": false` in `ofx_settings.json` (see [Keys in
  ofx_settings.json](#keys-in-ofx_settingsjson)). Every start then loads everything, as before.

The "Scan cache" part of `ofx_scan.txt` says which files came from the cache and which were
scanned, and why.

### Licences

EffectCraft doesn't include, sell or unlock any plug-in.

- Activate commercial plug-ins with the vendor's own tool (RE:Vision Activate, the Boris FX Hub, the
  Topaz app and so on). Make sure your licence covers the **OFX** version: After Effects licences
  often don't.
- Without a licence, plug-ins run as trials, usually with a watermark.
- EffectCraft is not a host the vendors support. Don't ask them for help with it; use [this
  fork's Issues](https://github.com/glumbis/effectcraft/issues).

### What works

| Plug-in | Status |
|---|---|
| RE:Vision **ReelSmart Motion Blur** (RSMB, RSMB Pro, RSMB Pro Vectors) | Works. Tested on Windows with RSMB 7. |
| RE:Vision **Twixtor** (Twixtor, Twixtor Pro, Vectors In, Motion Vectors Create) | Works: slow motion and speed ramps. Tested on Windows with Twixtor 8. |
| Boris FX **Sapphire** (about 290 effects) | Works. Tested on Windows with Sapphire 2026.5. |
| **Topaz Video** (OFX) | Had problems in 0.8.0. This release fixes the most likely cause (see below), but it hasn't been confirmed with the plug-in yet. |
| **Frischluft** (Lenscare, Flair) | Same as Topaz: had problems in 0.8.0; this release should help; not confirmed yet. |
| Other OFX plug-ins | Should work if they have a CPU render path. [Report how it goes](https://github.com/glumbis/effectcraft/issues). |

If you try Topaz or Frischluft, please attach `ofx_scan.txt` to your report (see [Reporting a
problem](#reporting-a-problem)). What changed in this release that may matter for them:

- When a plug-in refuses to load, the report now says why: what it answered, what it said and what
  it asked for that EffectCraft doesn't offer.
- If several copies of the same plug-in are installed (an alpha, a beta and a release folder, as
  Topaz leaves behind), only the newest is used.
- Plug-ins that only start in hosts they know can be shown a different host name (see [Try a
  different host name](#4-try-a-different-host-name)).

Topaz says to install only the newest bundle and to have its main app installed and signed in.

### What to expect

- **Watermark or red X:** the plug-in is running as a trial. Activate the **OFX** licence with the
  vendor's tool.
- **Nothing happens on a shape or text layer:** RSMB and Twixtor need the layer's own pixels to move;
  use them on footage or a precomp. A shape moved only with Position looks still to them (as in
  After Effects).
- **Twixtor starts at Speed 100 %**, which changes nothing: set Speed to, say, 50 for half-speed
  slow motion, or keyframe it for speed ramps.
- **Sapphire glows need something opaque behind them** on the same layer (footage, or a precomp
  with a background). On a transparent layer the halo disappears.
- **No on-screen handles:** OFX overlays (tracking boxes, draggable points) aren't shown; use the
  numbers in Effect Controls. Buttons that open the vendor's own windows (Sapphire's preset browser,
  Mocha) do nothing.
- **Slow:** everything renders on the CPU. Work at Half or Quarter resolution while you edit and
  render at Full.
- **Curves can't be edited.** Plug-ins that have curve parameters (a colour curve, say) load and
  work, but EffectCraft has no curve editor yet. The curves don't appear in Effect Controls, and
  they aren't saved with the project: they start from the plug-in's own default every time.
- **Effects that need the GPU don't load:** a plug-in that only renders on the GPU is reported in
  `ofx_scan.txt` as "needs GPU rendering". Plug-ins that only work with 16-bit float ("half-float")
  images do load.
- **A plug-in is native code.** It runs inside EffectCraft with full access to your computer. Install
  only plug-ins you trust.

## Opening After Effects projects

1. Choose **File ▸ Import ▸ After Effects Project…** and pick an `.aep` or `.aepx` file.
2. The main comp opens when it's done. A report lists anything that couldn't come over
   (third-party effects, layer styles, Curves and the like).
3. Footage is looked for at its saved path and next to the project. Missing files become
   placeholders you can relink.

**What it needs:** Python 3 and the py-aep library. EffectCraft finds Python by itself, trying these
in order and using the first one that has py-aep:

1. The Python named in the `EFFECTCRAFT_PYTHON` environment variable (a full path; see
   [Environment variables](#environment-variables)). If it runs but lacks py-aep, you get that
   error instead of another Python being tried.
2. A private environment in the [settings folder](#where-the-settings-folder-is):
   `aep-python\Scripts\python.exe` on Windows, `aep-python/bin/python3` on Linux and macOS. The Linux
   installer and the macOS script make it where the system Python refuses `pip install --user`.
3. Windows only: `py -3`.
4. `python3`.
5. macOS only: `/opt/homebrew/bin/python3`, then `/usr/local/bin/python3` (Homebrew's and
   python.org's). An app opened from Finder gets a short `PATH` that doesn't include them, so
   EffectCraft looks there itself.
6. `python`.

A Python that runs but has no py-aep is skipped in favour of the next one.

| System | Set-up |
|---|---|
| **Windows** | `Install EffectCraft.bat` does it: it finds Python 3 (or offers to install it with winget) and installs py-aep for your user. By hand: `py -m pip install py-aep`. |
| **Linux** (installer) | `./install.sh` offers to install py-aep with `pip install --user`. If your distribution refuses, it makes the private environment `~/.config/effectcraft/aep-python`, which EffectCraft finds by itself (no wrapper scripts, no variables to set). The uninstaller removes that environment. |
| **Linux** (AppImage, or by hand) | `python3 -m pip install --user py-aep`. If that says "externally-managed-environment", make the private environment yourself: `python3 -m venv ~/.config/effectcraft/aep-python` and then `~/.config/effectcraft/aep-python/bin/pip install py-aep`. Or use `pipx`. |
| **macOS** | Double-click `Set up After Effects import.command` (after the `xattr` step in [Install on macOS](#install-on-macos)). It makes the private environment `~/Library/Application Support/EffectCraft/aep-python` with the first working Python 3 of Apple's (which comes with the Command Line Tools; it offers to install those if they're missing), Homebrew's or python.org's, and falls back to `pip install --user`. By hand, in Terminal: `/usr/bin/python3 -m venv "$HOME/Library/Application Support/EffectCraft/aep-python"` and then `"$HOME/Library/Application Support/EffectCraft/aep-python/bin/pip" install py-aep`. |

What carries over and what doesn't, and how to run the steps yourself, is in the [.aep import
notes](https://github.com/glumbis/effectcraft/blob/openfx/tools/aep-import/README.md).

## Troubleshooting

### Getting plug-ins to work

Work through these in order. Most "my plug-in doesn't work" reports end at step 1, 2 or 3.

#### 1. Read the scan report

Every start writes a plain-text report, **`ofx_scan.txt`**, in EffectCraft's [settings
folder](#where-the-settings-folder-is) (on Windows, paste `%APPDATA%\EffectCraft` into File
Explorer's address bar), or open it with Effect ▸ **Open OpenFX Scan Report**. When something
fails, EffectCraft also shows a message at start-up that points to it. The report has these parts,
in this order:

| Part | What it tells you |
|---|---|
| Host name shown to plug-ins | What EffectCraft calls itself when plug-ins ask. See [step 4](#4-try-a-different-host-name). |
| Folders searched | Each folder, marked `found` or `missing`. |
| Plug-ins that crashed EffectCraft in the previous session | Only there if one did: which plug-in and action, when, and on Windows the error code and the file it happened in. See [EffectCraft crashes](#effectcraft-crashes). |
| Scan cache | Which plug-in files were registered from the [cache](#fast-starts-the-scan-cache) without being loaded, which were loaded now, and why. |
| Effects registered | Every effect that loaded, with its category. "0" means nothing loaded. |
| Problems | Plug-ins that failed to load, and why. |
| Notes | Plug-ins skipped on purpose, and why (see the table below). |
| Load transcript | One block per plug-in file: what it answered at each step. |

A plug-in that loads cleanly takes one line in the transcript. One that fails shows everything it
said. For example:

```
C:\Program Files\Common Files\OFX\Plugins\X.ofx.bundle\Contents\Win64\X.ofx
  host shown to the plug-in: "EffectCraft" 0.10.0 (OFX API 1.4)
  OfxSetHost -> kOfxStatOK (0)
  OfxGetNumberOfPlugins -> 2
  com.x.Y v1.2
    Load -> kOfxStatOK (0) (12 ms)
    Describe -> kOfxStatErrMissingHostFeature (4) (3 ms)
      plug-in said (error): "..."
      asked for property OfxFoo of the host, which this host does not provide
    host properties read: OfxPropName
    not registered
```

Two kinds of line matter most. "plug-in said" is the plug-in's own message (often a licence or host
complaint). "asked for … which this host does not provide" is something EffectCraft lacks. Both are
what to send in a report.

What the common messages mean:

| Message in the report | What to do |
|---|---|
| `cannot load the library: ... The specified module could not be found` (Windows) | The plug-in needs a DLL it can't find. Install the latest **Microsoft Visual C++ Redistributable (x64)**; reinstall the plug-in (its installer brings its own runtime files). |
| `cannot load the library: ... %1 is not a valid Win32 application` (Windows) | A 32-bit or wrong-platform build. Get the 64-bit Windows OFX version. |
| `cannot load the library: ... cannot open shared object file` (Linux) | The plug-in needs a system library you don't have. In a terminal, `ldd <the .ofx file> \| grep "not found"` names it. |
| `cannot load the library: ...` mentioning an incompatible architecture (macOS) | The plug-in is for the other kind of Mac processor. Get the build for the processor your EffectCraft download is for (Apple Silicon or Intel). |
| `no Win64 binary in this bundle` (or `MacOS`, `Linux-x86-64`) | The bundle has no build for this system. |
| `not an OpenFX plug-in` | The file isn't an OFX binary (an AE `.aex` renamed, or a support library). |
| `skipped: it crashed EffectCraft while loading 2 times in a row` | It crashed EffectCraft twice while starting, so it's skipped. See [EffectCraft crashes](#effectcraft-crashes). |
| `skipped: it crashed EffectCraft while rendering 2 times in a row` | It crashed EffectCraft twice while rendering, so it's skipped. See [EffectCraft crashes](#effectcraft-crashes). |
| `declined this host` (Notes) or `exports no OpenFX plug-ins` (Notes) | The plug-in looked at the host name and said it isn't for it. Try [step 4](#4-try-a-different-host-name). |
| `Load failed: ... declined to load` (Problems) | Often a licence check or a host check. Run the vendor's activation tool, then try [step 4](#4-try-a-different-host-name). |
| `Describe failed: ...` (Problems) | The plug-in couldn't describe itself. Read what "the plug-in said" and "asked for something this host does not provide" at the end of the line, and [report it](#reporting-a-problem). |
| `needs GPU rendering` | It only renders on the GPU, which EffectCraft doesn't use. No fix on your side. |
| `no supported context` / `needs an unsupported pixel depth` | It's a kind of OFX effect EffectCraft can't host yet. [Open an issue](https://github.com/glumbis/effectcraft/issues) with the report. |
| `N duplicate plug-in(s) skipped, the newest copy is kept` (Notes) | You have several copies of one plug-in installed. The newest runs. See [step 2](#2-keep-only-the-newest-copy-of-each-plug-in). |
| `skipped, it is not an OpenFX image effect` (Notes) | The file belongs to another kind of OFX plug-in, which EffectCraft doesn't run. |
| `... the general context is used instead` (Notes) | The plug-in's first way of describing itself failed, and another worked. Informational. |
| `its first required clip, ..., is used as the main input` (Notes) | The plug-in doesn't call its main input "Source". EffectCraft guessed. If the effect looks wrong, report it. |
| Effects registered: 0, no problems, every folder `missing` | Nothing is installed in the searched folders. See [step 3](#3-check-the-folder-and-the-platform-binary). |

You can also run the same scan from a terminal; it prints the result as JSON, including the
transcript:

```sh
effectcraft-cli exec effect.plugins.scanOfx --empty
```

`effectcraft-cli` is `effectcraft-cli.exe` in the install folder on Windows, `effectcraft-cli-ofx`
on Linux (installer), and `/Applications/EffectCraft.app/Contents/MacOS/effectcraft-cli` on macOS.

#### 2. Keep only the newest copy of each plug-in

Some vendors leave older versions installed beside the new one (alpha, beta and release folders).
EffectCraft uses only one copy of a plug-in (within a folder, the newest; if the copies are in
different folders, the one in the folder searched first) and lists the others under Notes, but old
copies are still opened, and some have known problems. Delete the old `.ofx.bundle` folders and
restart. If you use Topaz, keep only the newest bundle.

#### 3. Check the folder and the platform binary

Look at **Folders searched** in the report. Is the folder your plug-in is in marked `found`? Does
the `.ofx.bundle` contain the right platform folder (`Win64`, `MacOS` or `Linux-x86-64`)? If your
plug-in is somewhere else (a portable install, a Resolve-only folder), copy the whole `.ofx.bundle`
folder into one of the [folders EffectCraft looks in](#where-plug-ins-go), or add its parent folder
to `OFX_PLUGIN_PATH` and restart EffectCraft. If you fixed something that doesn't change the
plug-in's `.ofx` file (a licence, a support file), the [scan cache](#fast-starts-the-scan-cache) may
still remember the old state: delete `ofx_cache.json` and restart.

#### 4. Try a different host name

Some plug-ins only turn themselves on in hosts they know, and decline anything else. All they can
check is the host's name. EffectCraft calls itself `EffectCraft`. You can make it use another
name. **Treat this as an experiment:**

- These names are common ones but are **unverified** as to whether they help: `DaVinciResolve`,
  `DaVinciResolveLite`, `uk.co.thefoundry.nuke`, `fr.inria.Natron`.
- A plug-in that loads under another name may still fail when it renders, because it can switch to
  features that host has and EffectCraft doesn't.
- Your licence still decides which hosts you may use the plug-in in.
- The name is read when EffectCraft starts, so **restart after a change**.

To set it:

1. Close EffectCraft. Open the [settings folder](#where-the-settings-folder-is).
2. Create a text file there called `ofx_settings.json` (or edit it) containing:

   ```json
   {"host_name": "DaVinciResolve"}
   ```

3. Start EffectCraft and read the first line of the new `ofx_scan.txt`: it shows the name now in
   use. To go back to the default, delete the file, or set `"host_name": ""`.

From a terminal on Linux or macOS you can do the same with a command (on Windows, edit the file:
typing JSON on the Windows command line needs awkward quoting):

```sh
effectcraft-cli exec effect.plugins.setOfxHostName '{"name": "DaVinciResolve"}' --empty
```

The environment variable `EFFECTCRAFT_OFX_HOST_NAME` overrides the file. A file that can't be read
is ignored, and the report says so.

#### 5. Report it

If it still doesn't work, [report a problem](#reporting-a-problem) and attach `ofx_scan.txt`.

### It loads, but doesn't look right

See [What to expect](#what-to-expect) for watermarks, shape layers, Twixtor, Sapphire glows and
slow renders. One more case: **an effect from an `.aep` didn't come over.** The `.aep` importer
lists third-party effects in its report; apply the OFX version by hand from Effects & Presets.

### EffectCraft crashes

- **While starting:** a plug-in that crashes EffectCraft while it loads is remembered. EffectCraft
  keeps a strike count per plug-in file in `ofx_blocklist.txt`, and **skips a plug-in that crashed
  twice in a row**. The report says `skipped: it crashed EffectCraft while loading 2 times in a
  row`. Updating or reinstalling the plug-in un-skips it automatically. Otherwise delete its line
  from `ofx_blocklist.txt` (or the whole file) and restart.
- **To start without any plug-ins:** set the environment variable `EFFECTCRAFT_NO_OFX=1`
  (see [Environment variables](#environment-variables)), start EffectCraft, and remove the variable
  again later. This is also the way to check whether a plug-in is the cause of a problem.
- **While rendering:** EffectCraft can't catch the crash, because OFX plug-ins run inside the app.
  But while it runs it notes which plug-in action is in progress (in `ofx_inflight.<number>.bin` in
  the settings folder; it's removed when you quit normally), and reads that note at the next start.
  Your work is auto-saved; reopen EffectCraft and it offers to recover it. Then:
  - You see a message "*plug-in* crashed EffectCraft last time", and `ofx_scan.txt` has a section
    about it: the action, the time and, on Windows, the error code and the file it happened in.
  - A plug-in that crashed EffectCraft **twice in a row** while rendering is **skipped** from then
    on. The report says `skipped: it crashed EffectCraft while rendering 2 times in a row`. The
    strikes are kept in `ofx_render_crashes.txt`.
  - To bring a skipped plug-in back: update or reinstall it, or delete its line from
    `ofx_render_crashes.txt`, or on Linux or macOS run
    `effectcraft-cli exec effect.plugins.scanOfx '{"retry": true}' --empty` (this also clears the
    list of plug-ins that crashed while loading).
  - A session in which the plug-in renders without a crash resets its count.

  Please report such a crash with the plug-in name and version, `ofx_scan.txt` and
  `Logs/effectcraft.log`.
- **EffectCraft doesn't start at all.** Windows: install the Visual C++ Redistributable (see
  [Install on Windows](#install-on-windows)). Linux: run `effectcraft-ofx` in a terminal; a missing
  library is named in the error (see [Install on Linux](#install-on-linux)). macOS: see step 2 of
  [Install on macOS](#install-on-macos). If the window opens and draws nothing, Help ▸ System
  Compatibility Report… collects your graphics details for a report.
- **The log:** `Logs/effectcraft.log` in the settings folder is the log of the current run (the
  two before it are kept as `effectcraft.1.log` and `effectcraft.2.log`, so the log of a run that
  crashed survives the next start). Help ▸ Reveal Logging File opens its folder.

### Opening an `.aep` fails

| Message | What to do |
|---|---|
| `Python 3 was not found` | Install Python 3 from [python.org](https://www.python.org/downloads/), then install py-aep (see [Opening After Effects projects](#opening-after-effects-projects)). |
| `py-aep is not installed for the Python found here` | No Python EffectCraft tried has py-aep (the message names them). Install it for one of them with the command the message shows, or run the installer / `Set up After Effects import.command` again. If you set `EFFECTCRAFT_PYTHON`, the message is about that Python. |
| `the converter aep2effectcraft.py was not found` | The `aep-import` folder next to the app is missing. Reinstall from the download. |

## Reporting a problem

1. **Open an issue on this fork:** [github.com/glumbis/effectcraft/issues](https://github.com/glumbis/effectcraft/issues).
   Not upstream, and not the plug-in vendor.
2. **Say what you did and what happened.** Include your operating system and version, and the
   plug-in's name and version (or the `.aep` you opened).
3. **Say which release of the fork you use.** Give the name of the file you downloaded, which ends
   like `0.10.0-ofx.1-…`. Help ▸ About shows only `0.10.0`, not the fork build number. On macOS,
   About is in the EffectCraft menu.
4. **Attach these files** from the [settings folder](#where-the-settings-folder-is):
   - `ofx_scan.txt` (for anything about plug-ins; Effect ▸ Open OpenFX Scan Report opens it), and
   - `Logs/effectcraft.log` (for crashes and anything else).

   Open them in a text editor first. They contain file paths, which may include your user name;
   remove anything you don't want to share.

## Settings and files reference

### Where the settings folder is

EffectCraft keeps its settings, logs and the plug-in report in one folder:

| System | Folder |
|---|---|
| Windows | `%APPDATA%\EffectCraft` (usually `C:\Users\<you>\AppData\Roaming\EffectCraft`). Paste it into File Explorer's address bar. |
| macOS | `~/Library/Application Support/EffectCraft`. In Finder: Go ▸ Go to Folder… (Shift+Cmd+G) and paste it. |
| Linux | `$XDG_CONFIG_HOME/effectcraft`, usually `~/.config/effectcraft` |

**Portable mode** (Windows and Linux): put an empty file called `portable.txt` (or
`EffectCraft.portable`) next to the program (`effectcraft.exe` or `effectcraft`). Everything is
then kept in a folder called `EffectCraftData` next to it, so a copy on a USB stick leaves nothing
on the computer. If that folder can't be written to, EffectCraft logs a warning and uses the normal
folder instead.

The environment variable `EFFECTCRAFT_CONFIG_DIR` overrides both and names the folder to use.

### Files in the settings folder

| File | What it is |
|---|---|
| `ofx_scan.txt` | The plug-in report, rewritten at every start of the desktop app. See [Read the scan report](#1-read-the-scan-report). |
| `ofx_settings.json` | Optional. You create it. Holds the [host name](#4-try-a-different-host-name) (see the keys below). |
| `ofx_blocklist.txt` | Plug-in files that crashed EffectCraft while loading. One line each: the number of crashes in a row, a file stamp and the path. A plug-in with a count of 2 or more is skipped. |
| `ofx_loading.<number>.txt` | A short-lived marker while a plug-in file is loading. If one is left behind, EffectCraft crashed while loading that file, and the next start counts it as a crash. |
| `ofx_cache.json` | The [scan cache](#fast-starts-the-scan-cache): what each plug-in file contains, so that starting is faster. Safe to delete; it's rebuilt at the next start. |
| `ofx_inflight.<number>.bin` | Exists while EffectCraft runs: a note of which plug-in action is in progress. Removed at a normal quit. If one is left behind, EffectCraft crashed, and the next start reads it. |
| `ofx_render_crashes.txt` | Plug-ins that crashed EffectCraft while it was running them. One line each: the number of crashes in a row, a file stamp, the plug-in's id and the action. A plug-in with a count of 2 or more is skipped. |
| `Logs/effectcraft.log` | The log of the current run. `effectcraft.1.log` and `effectcraft.2.log` are the two runs before. |
| `OFX/` | Your per-user OpenFX plug-ins: copy `.ofx.bundle` folders here (no administrator rights). Searched at every start; installers and uninstallers never touch it. Effect ▸ Open OpenFX Plug-in Folder opens it. |
| `aep-python/` | The private Python environment with py-aep for `.aep` import, made by the Linux installer and the macOS setup script. EffectCraft looks in it first (after `EFFECTCRAFT_PYTHON`). |
| `Plug-ins/` | EffectCraft's own WebAssembly plug-ins. Not OpenFX. |

### Keys in `ofx_settings.json`

| Key | Value | Meaning |
|---|---|---|
| `host_name` | text, 1 to 128 characters, no control characters | The name OpenFX plug-ins are told the host has. Default `EffectCraft`. Other keys in the file are kept. |
| `scan_cache` | `true` or `false` | Whether starting may use the [scan cache](#fast-starts-the-scan-cache). Default `true`. Anything else is ignored (the cache stays on) and the report says so. |

### Environment variables

| Variable | Meaning |
|---|---|
| `OFX_PLUGIN_PATH` | Extra folders to search for plug-ins. Separate several with `;` on Windows and `:` on Linux and macOS. |
| `EFFECTCRAFT_NO_OFX` | Set to `1` to start the app without scanning for OpenFX plug-ins. Only `1` counts. |
| `EFFECTCRAFT_OFX_HOST_NAME` | The host name shown to plug-ins. Overrides `ofx_settings.json`. |
| `EFFECTCRAFT_CONFIG_DIR` | Use this folder as the settings folder. |
| `EFFECTCRAFT_PYTHON` | The Python used to open `.aep` files (a full path). Tried first, and if it runs but lacks py-aep no other Python is tried. See [Opening After Effects projects](#opening-after-effects-projects). |
| `EFFECTCRAFT_AEP_SCRIPT` | The `.aep` converter script `aep2effectcraft.py`, if it isn't in the `aep-import` folder next to the app. You normally never need this. |

**How to set one:**

- **Windows:** Start ▸ type "Edit environment variables for your account" ▸ *New…*. Restart
  EffectCraft afterwards.
- **Linux** (terminal): `OFX_PLUGIN_PATH=$HOME/ofx effectcraft-ofx`. For the app-menu entry, add
  `export OFX_PLUGIN_PATH=$HOME/ofx` to `~/.profile` and log in again.
- **macOS:** apps opened from Finder don't see variables set in a terminal. Start EffectCraft from a
  terminal instead:
  `OFX_PLUGIN_PATH=/path/to/plugins /Applications/EffectCraft.app/Contents/MacOS/EffectCraft`.

### Commands

These are for the command line (`effectcraft-cli exec <command> --empty`), scripts and agents. Three
also have entries in the Effect menu: Load OpenFX Plug-in…, Open OpenFX Plug-in Folder and Open
OpenFX Scan Report.

| Command | What it does |
|---|---|
| `effect.plugins.scanOfx` | Scans the plug-in folders and returns `{paths, loaded, errors, notes, transcript, cache, crashes}`. With `{"retry": true}` it first forgets the crash lists (plug-ins skipped for crashing while loading or rendering). With `{"cache": false}` it doesn't use the scan cache. |
| `effect.plugins.rescanOfx` | Deletes the scan cache, so the next start loads and describes every plug-in again. It takes effect at the next start. |
| `effect.plugins.loadOfx {"path": "…"}` | Loads an `.ofx` file, an `.ofx.bundle` folder, or every bundle in a folder. It always loads the plug-in fresh (it doesn't use the cache). |
| `effect.plugins.openOfxUserFolder` | Creates the per-user plug-in folder (`OFX` in the settings folder) and opens it in the file manager. In the Effect menu as "Open OpenFX Plug-in Folder". |
| `effect.plugins.openOfxReport` | Opens `ofx_scan.txt` with your default text application. In the Effect menu as "Open OpenFX Scan Report". |
| `effect.plugins.ofxHostName` | Shows the host name in use, where it comes from, what `ofx_settings.json` holds, and whether a change is waiting for a restart. |
| `effect.plugins.setOfxHostName {"name": "…"}` | Stores the host name in `ofx_settings.json` (an empty name resets it). It applies from the next start. |

The technical reference is [docs/ofx.md](https://github.com/glumbis/effectcraft/blob/openfx/docs/ofx.md).

## Building it yourself

You need [Rust](https://rustup.rs/) (stable, 1.95 or newer). Both additions are opt-in features:

```sh
cargo build --release -p effectcraft -p effectcraft-cli --features "openfx aep"
```

The programs end up in `target/release`. To run the app straight away, use
`cargo run --release -p effectcraft --features "openfx aep"`.

- **Windows:** the Visual Studio C++ build tools (the MSVC toolchain: "Desktop development with
  C++").
- **Linux:** development packages. On Ubuntu 22.04, the ones the project's build uses:

  ```sh
  sudo apt install libxkbcommon-dev libwayland-dev libx11-dev libxrandr-dev libxi-dev libgl1-mesa-dev libasound2-dev pkg-config
  ```

- **macOS:** the Xcode Command Line Tools (`xcode-select --install`).
- **To open `.aep` files from a build you made:** `python -m pip install -r tools/aep-import/requirements.txt`.
  EffectCraft finds the converter in the repository's `tools/aep-import` folder.
- **The EC Test plug-in:** `bash tools/fork-installer/ci/build-test-bundle.sh` builds it and lays
  out its bundle in `target/ofx-test`.

## How this fork is organised

| What | Where |
|---|---|
| **Downloads** (Windows, Linux and macOS, with OpenFX and `.aep` import) | [Releases](https://github.com/glumbis/effectcraft/releases). They are built by the `OFX host` workflow when a tag like `v0.10.0-ofx.1` is pushed |
| `openfx` | The fork you download: EffectCraft with the in-process OpenFX host and `.aep` import. The default branch, and the one releases are cut from |
| `openfx-aep` | Same commit as `openfx` (kept so older links keep working) |
| `feat/openfx-host` | The branch of the first upstream proposal, [storytold/effectcraft#508](https://github.com/storytold/effectcraft/pull/508) |
| `claude/openfx-external-helper-*` | The new upstream proposal: a safe **external effect helper** interface (no OpenFX code inside EffectCraft) |
| `ofx-host` | Development history of the OpenFX host before it was squashed into `feat/openfx-host` |
| `sync/*`, `ci/*`, `docs/*` | Working branches: merges of new upstream releases, packaging, and documentation, merged into `openfx` when done |
| `main` | An older copy of upstream EffectCraft; not used for builds |
| [ec-extension-ofx](https://github.com/glumbis/ec-extension-ofx) | The OpenFX host as a separate helper program, for EffectCraft builds with external helper support |

**Where it's going.** The EffectCraft maintainers asked for OpenFX not to run inside EffectCraft
itself (a crashing plug-in would take the app and your work down with it). So upstream gets a
small, safe interface for *external effect helpers*, separate programs that EffectCraft talks to,
and the OpenFX host moves into one of them,
[ec-extension-ofx](https://github.com/glumbis/ec-extension-ofx). If a plug-in crashes there,
EffectCraft shows an error on the effect and keeps running. Until that lands upstream, this fork's
releases stay the way to use OpenFX plug-ins.

## Credits

Built by [@glumbis](https://github.com/glumbis), mostly written with Claude Code. EffectCraft
itself is by the [EffectCraft contributors](https://github.com/storytold/effectcraft). The OpenFX API
headers are © the OpenFX contributors (BSD-3-Clause). After Effects projects are read with
[py-aep](https://github.com/forticheprod/py-aep) © Fortiche Production (MIT), installed separately.

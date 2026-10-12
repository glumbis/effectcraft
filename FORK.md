# EffectCraft with OpenFX plug-ins and After Effects project import (unofficial)

This fork of [EffectCraft](https://github.com/storytold/effectcraft) adds two things:

- **OpenFX (OFX) plug-ins**, the plug-in standard of DaVinci Resolve, Nuke, Vegas and Natron, run as
  ordinary effects: they show up in Effects & Presets, their settings appear in Effect Controls, and
  they take keyframes and expressions.
- **Opening After Effects projects** (`.aep` and `.aepx`): File ▸ Import ▸ After Effects Project…
  rebuilds the comps, layers, keyframes, expressions, masks and effects.

| Plug-in | Status |
|---|---|
| RE:Vision **ReelSmart Motion Blur** (RSMB, RSMB Pro, RSMB Pro Vectors) | ✅ loads and renders |
| RE:Vision **Twixtor** (Twixtor, Twixtor Pro, Vectors In, Motion Vectors Create) | ✅ slow-motion and speed ramps |
| Boris FX **Sapphire** (290 effects) | ✅ loads and renders |
| Other OFX plug-ins | Should work if they have a CPU render path; [report how it goes](https://github.com/glumbis/effectcraft/issues) |

This fork is **unofficial**: please don't report problems with it to the EffectCraft maintainers or
to the plug-in vendors. Use [this fork's Issues](https://github.com/glumbis/effectcraft/issues).

## Install (Windows)

1. **Download** `EffectCraft-OFX-windows.zip` from the
   [latest release](https://github.com/glumbis/effectcraft/releases/latest) and unzip it.
2. **Double-click `Install EffectCraft.bat`.** It installs the app for your user (no administrator
   rights), adds **"EffectCraft (OFX)"** to the Start menu and desktop, and sets up After Effects
   project import: it finds Python 3 and installs the free py-aep library, or offers to install
   Python for you if it's missing.
3. **Install your plug-ins** with their own installers, choosing the **OFX** version (not the After
   Effects one). They go into `C:\Program Files\Common Files\OFX\Plugins`, where EffectCraft finds
   them automatically.
4. **Start "EffectCraft (OFX)".** It isn't code-signed, so Windows may warn the first time: *More
   info* ▸ *Run anyway*. Plug-ins appear in **Effects & Presets** under their own categories (for
   example *REVision Effects*, *Sapphire Stylize*), plus **EC Test**, a tiny test plug-in that comes
   with the download. If your plug-ins don't show up, see
   [Getting plug-ins to work](#getting-plug-ins-to-work) below.

To remove it: *Uninstall EffectCraft.bat* in the install folder
(`%LOCALAPPDATA%\Programs\EffectCraft-OFX`). Your projects, settings and plug-ins stay.

**Licences:** EffectCraft doesn't include or unlock any plug-in. Without a licence they run as
trials with a watermark; activate them with the vendor's own tools (RE:Vision Activate, the Boris FX
Hub).

## Opening After Effects projects

**File ▸ Import ▸ After Effects Project…**, then pick an `.aep` or `.aepx`. The main comp opens when
it's done, and a report lists anything that couldn't come over (third-party effects, layer styles,
Curves and the like). Footage is looked for at its saved path and next to the project; missing files
become placeholders you can relink.

It needs Python 3 and py-aep, which the installer sets up. Without the installer:
`py -m pip install py-aep`. Details and limits: [tools/aep-import/README.md](tools/aep-import/README.md).

## Getting plug-ins to work

Most "my plug-in doesn't work" reports come down to one of the cases below. Work through them in
order.

### 1. Is it an OpenFX plug-in at all?

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
"Resolve / Vegas / Nuke" or "OpenFX" on the vendor's download page. The installer usually lets you
pick hosts; tick **OFX** (or DaVinci Resolve, which installs the OFX version).

### 2. Is it installed where EffectCraft looks?

At every start EffectCraft scans these folders (and everything below them):

| Where | Notes |
|---|---|
| `C:\Program Files\Common Files\OFX\Plugins` | Where OFX installers put plug-ins on Windows (macOS: `/Library/OFX/Plugins`, Linux: `/usr/OFX/Plugins`) |
| `OFX` next to `effectcraft.exe` (`%LOCALAPPDATA%\Programs\EffectCraft-OFX\OFX`) | The EC Test plug-in lives here; you can copy `.ofx.bundle` folders here too |
| Every folder in the `OFX_PLUGIN_PATH` environment variable | Separate several folders with `;` |

A plug-in is a folder named `Something.ofx.bundle` containing `Contents\Win64\Something.ofx`. Look
for it in File Explorer. If the folder is called `Win32` or there is only a `MacOS` folder, it's not
a 64-bit Windows build and can't load. If your plug-in is somewhere else (a portable install, a
Resolve-only folder), either copy the whole `.ofx.bundle` folder into one of the folders above or
add its parent folder to `OFX_PLUGIN_PATH`:

*Start ▸ "Edit environment variables for your account" ▸ New…*, name `OFX_PLUGIN_PATH`, value the
folder (for example `D:\Plugins\OFX`). Restart EffectCraft afterwards.

You can also load one by hand: **Effect ▸ Load OpenFX Plug-in…** and pick the `.ofx` file inside
`Contents\Win64`. That only lasts until you quit; put it in a scanned folder to keep it.

### 3. Read the scan report

Every start writes **`%APPDATA%\EffectCraft\ofx_scan.txt`** (paste that into File Explorer's
address bar). It lists the folders searched (`found` / `missing`), every effect that loaded with its
category, and every problem. When something fails EffectCraft also shows a message at start-up
that points there.

What the common problems mean:

| Message in the report | What to do |
|---|---|
| `cannot load the library: ... The specified module could not be found` | The plug-in needs a DLL it can't find. Install the latest **Microsoft Visual C++ Redistributable (x64)**; reinstall the plug-in (its installer brings its own runtime files) |
| `cannot load the library: ... %1 is not a valid Win32 application` | A 32-bit or wrong-platform build; get the 64-bit Windows OFX version |
| `not an OpenFX plug-in` | The file isn't an OFX binary (an AE `.aex` renamed, or a support library) |
| `... skipped: it crashed EffectCraft while loading 2 times in a row` | It crashed EffectCraft twice while starting, so it's skipped. Updating or reinstalling it un-skips it automatically; otherwise delete its line from `%APPDATA%\EffectCraft\ofx_blocklist.txt` (or the whole file) and restart |
| `Load failed (OFX status ...)` / `Describe failed` | The plug-in refused this host, often a licence check or a missing installation step. Run the vendor's activation tool, then try again |
| `no supported context` / `unsupported pixel depth` | It's a kind of OFX effect EffectCraft can't host yet; please [open an issue](https://github.com/glumbis/effectcraft/issues) with the report |
| `no Win64 binary in this bundle` | The bundle has no 64-bit Windows build |
| Effects registered: 0, no problems, every folder `missing` | Nothing is installed in the scanned folders: see step 2 |

The same scan can be run from a terminal, which prints the result as JSON:

```bat
cd %LOCALAPPDATA%\Programs\EffectCraft-OFX
effectcraft-cli exec effect.plugins.scanOfx --empty
```

### 4. It loads, but doesn't look right

- **Watermark or red X:** the plug-in is running as a trial. Activate the **OFX** licence with the
  vendor's tool (RE:Vision Activate, the Boris FX Hub); AE licences often don't cover OFX.
- **Nothing happens on a shape or text layer:** RSMB and Twixtor need the layer's own pixels to move;
  use them on footage or a precomp. A shape moved only with Position looks still to them (as in After
  Effects).
- **Twixtor starts at Speed 100 %**, which changes nothing: set Speed to e.g. 50 for half-speed
  slow motion, or keyframe it for speed ramps.
- **Sapphire glows need something opaque behind them** on the same layer (footage, or a precomp
  with a background); on a transparent layer the halo disappears.
- **No on-screen handles:** OFX overlays (tracking boxes, draggable points) aren't shown; use the
  numbers in Effect Controls. Buttons that open the vendor's own windows (Sapphire's preset browser,
  Mocha) do nothing.
- **Slow:** everything renders on the CPU. Work at Half or Quarter resolution while you edit and
  render at Full.
- **An effect from an `.aep` didn't come over:** the `.aep` importer lists third-party effects in its
  report; apply the OFX version by hand from Effects & Presets.

### 5. EffectCraft crashes

- **While starting:** a plug-in that crashes while loading is skipped automatically on the next
  start. To start without any OFX plug-ins, set the environment variable `EFFECTCRAFT_NO_OFX=1`
  (same place as `OFX_PLUGIN_PATH` above) and remove it again later.
- **While rendering:** the crash isn't caught (OFX plug-ins run inside the app). Your work is
  auto-saved; reopen and EffectCraft offers to recover it. Please report it with the plug-in name and
  version and `ofx_scan.txt`.

### Reporting a problem

[Open an issue on this fork](https://github.com/glumbis/effectcraft/issues) (not upstream, not the
plug-in vendor) and attach `%APPDATA%\EffectCraft\ofx_scan.txt`, the plug-in's name and version, and
which release of this fork you use (Help ▸ About).

More detail: [docs/ofx.md](docs/ofx.md) (OpenFX) and [tools/aep-import/README.md](tools/aep-import/README.md)
(After Effects projects).

## Building it yourself

You need [Rust](https://rustup.rs/) 1.95 or newer (and on Windows the Visual Studio C++ build tools).
Both additions are opt-in features:

```sh
cargo run --release -p effectcraft --features "openfx aep"
```

## How this fork is organised

| What | Where |
|---|---|
| **Download** (Windows, OpenFX + `.aep` import) | [Latest release](https://github.com/glumbis/effectcraft/releases/latest), built from the `openfx` branch |
| `openfx` | The fork you download: EffectCraft 0.7 with the in-process OpenFX host and `.aep` import. Default for the releases |
| `openfx-aep` | Same commit as `openfx` (kept so older links keep working) |
| `feat/openfx-host` | The branch of the first upstream proposal, [storytold/effectcraft#508](https://github.com/storytold/effectcraft/pull/508) |
| `claude/openfx-external-helper-*` | The new upstream proposal: a safe **external effect helper** interface (no OpenFX code inside EffectCraft) |
| `ofx-host` | Development history of the OpenFX host before it was squashed into `feat/openfx-host` |
| `main` | An older copy of upstream EffectCraft; not used for builds |
| [ec-extension-ofx](https://github.com/glumbis/ec-extension-ofx) | The OpenFX host as a separate helper program, for EffectCraft builds with external helper support |

**Where it's going.** The EffectCraft maintainers asked for OpenFX not to run inside EffectCraft
itself (a crashing plug-in would take the app and your work down with it). So upstream gets a
small, safe interface for *external effect helpers*, separate programs that EffectCraft talks to,
and the OpenFX host moves into one of them,
[ec-extension-ofx](https://github.com/glumbis/ec-extension-ofx). If a plug-in crashes there,
EffectCraft shows an error on the effect and keeps running. Until that lands upstream, the release
above stays the way to use OpenFX plug-ins.

## Credits

Built by [@glumbis](https://github.com/glumbis), mostly written with Claude Code. EffectCraft
itself is by the [EffectCraft contributors](https://github.com/storytold/effectcraft). The OpenFX API
headers are © the OpenFX contributors (BSD-3-Clause). After Effects projects are read with
[py-aep](https://github.com/forticheprod/py-aep) © Fortiche Production (MIT), installed separately.

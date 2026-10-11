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
   with the download.

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

## Tips

- **Twixtor starts at Speed 100 %**, which changes nothing: set Speed to e.g. 50 for half-speed
  slow motion, or keyframe it for speed ramps.
- **RSMB and Twixtor need the layer's own pixels to move**: use them on footage or a precomp. A
  shape that's only moved with Position looks still to them (as in After Effects).
- **Sapphire glows need something opaque behind them** on the same layer (footage, or a precomp
  with a background); on a transparent layer the halo disappears.
- Everything renders on the **CPU**, so heavy effects are slow at high resolution.
- If a plug-in crashes EffectCraft while loading, it's skipped automatically next launch. To start
  without any OFX plug-ins, set the environment variable `EFFECTCRAFT_NO_OFX=1`.

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

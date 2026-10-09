# EffectCraft with OpenFX plug-ins

This fork of [EffectCraft](https://github.com/storytold/effectcraft) can run **OpenFX (OFX)
plug-ins**, the plug-in standard used by DaVinci Resolve, Nuke, Vegas and Natron, as ordinary
effects: they show up in Effects & Presets, their settings appear in Effect Controls, and they take
keyframes and expressions.

| Plug-in | Status |
|---|---|
| RE:Vision **ReelSmart Motion Blur** (RSMB, RSMB Pro, RSMB Pro Vectors) | ✅ loads and renders |
| RE:Vision **Twixtor** (Twixtor, Twixtor Pro, Vectors In, Motion Vectors Create) | ✅ slow-motion and speed ramps |
| Boris FX **Sapphire** (290 effects) | ✅ loads and renders |
| Other OFX plug-ins | Should work if they have a CPU render path; [report how it goes](https://github.com/glumbis/effectcraft/issues) |

The OFX support is proposed for the official EffectCraft in
[storytold/effectcraft#508](https://github.com/storytold/effectcraft/pull/508). Until then
it lives here. This fork is **unofficial**: please don't report problems with it to the EffectCraft
maintainers or to the plug-in vendors.

## Getting started (Windows)

1. **Download** `EffectCraft-OFX-windows.zip` from the
   [latest release](https://github.com/glumbis/effectcraft/releases/latest) and unzip it anywhere.
2. **Install your plug-ins** with their own installers, choosing the **OFX** version (not the After
   Effects one). They install into `C:\Program Files\Common Files\OFX\Plugins`, where EffectCraft
   finds them automatically.
3. **Run `effectcraft.exe`.** It isn't code-signed, so Windows SmartScreen may warn the first time:
   click *More info* ▸ *Run anyway*. Your plug-ins appear in **Effects & Presets** under their own
   categories (for example *REVision Effects*, *Sapphire Stylize*).

**Licences:** EffectCraft doesn't include or unlock any plug-in. Without a licence they run as
trials with a watermark; activate them with the vendor's own tools (RE:Vision Activate, the Boris FX
Hub).

**Want to check it works without a commercial plug-in?** The zip also contains
`ec-test-ofx.ofx.bundle`, a tiny test plug-in. Put it in the OFX folder above (or choose
**Effect ▸ Load OpenFX Plug-in…** and pick the `.ofx` file inside its `Contents\Win64` folder) and
apply one of the **EC Test** effects.

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

More detail (supported features, limits, how it works): [docs/ofx.md](docs/ofx.md).

## Building it yourself

You need [Rust](https://rustup.rs/) 1.95 or newer (and on Windows the Visual Studio C++ build tools).
OFX support is an opt-in feature:

```sh
cargo run --release -p effectcraft --features openfx
```

## Credits

Built by [@glumbis](https://github.com/glumbis), mostly written with Claude Code. EffectCraft
itself is by the [EffectCraft contributors](https://github.com/storytold/effectcraft). The OpenFX API
headers are © the OpenFX contributors (BSD-3-Clause).

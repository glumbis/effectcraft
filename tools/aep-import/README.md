# Opening After Effects projects (.aep / .aepx)

**File ▸ Import ▸ After Effects Project…** rebuilds an After Effects project in EffectCraft: its
folders, footage, compositions, layers (solids, nulls, footage, precomps, text, shapes, cameras,
lights), timing, switches, parenting, track mattes, keyframes with their eases, expressions, masks,
effects and markers. It's part of this fork's builds with the `aep` feature.

## What you need

- **Python 3** (from [python.org](https://www.python.org/downloads/), or the Microsoft Store).
- **py-aep**, the Python library that reads `.aep` files:

  ```sh
  py -m pip install py-aep
  ```

That's all: EffectCraft finds Python (`py -3`, `python3` or `python`) and this folder by itself.

## How it works

1. `aep2effectcraft.py` reads the project with [py-aep](https://github.com/forticheprod/py-aep)
   (MIT, by Fortiche Production). An `.aepx` (After Effects' XML format) is first repacked into
   the binary layout by `aepx.py`.
2. It writes an After Effects-style script (`.jsx`) that rebuilds everything through EffectCraft's
   scripting API, the same one After Effects scripts use (`app.project.items.addComp`,
   `layer.property(...).setValuesAtTimes`, ...).
3. EffectCraft runs that script. Each step is guarded, so anything EffectCraft can't do yet is
   skipped and listed afterwards instead of stopping the import.

You can also run the steps yourself:

```sh
py -3 aep2effectcraft.py MyProject.aep -o MyProject.jsx
effectcraft-cli script MyProject.jsx --save-as MyProject.ecproj
effectcraft-cli exec file.importAep '{"path": "MyProject.aep"}' --empty --save-as MyProject.ecproj
```

`check_import.py MyProject.aep MyProject.ecproj --cli effectcraft-cli` compares the two (layers,
timing, parenting, transform values over time, effects, masks) and prints every difference.

## What carries over, and what doesn't

Carries over: comps and their settings, folders, footage (found at its saved path or relative to the
project; missing files become placeholders you can relink), solids, nulls, precomps, text with its
styling and its animators (selectors and animated properties), shape layers, cameras, lights, layer timing / stretch / switches / blend modes / quality,
parenting, track mattes, keyframes (values, interpolation, temporal eases, spatial tangents),
expressions, masks, built-in effects and their settings, comp and layer markers.

Not (yet): third-party effects (not installed in EffectCraft: listed by name), layer styles, effects
EffectCraft doesn't have, "custom" property values (Curves, gradient ramps and the like), Essential
Graphics, the render queue, 3D model layers. The import report lists each one.

Checked on 17 sample projects from py-aep's test set (AE 2018 to 2026) and 5 public `.aepx`
files: in the comparison above, layers, timing, parenting, effects, masks and animated transform
values match. Known differences: a Hold keyframe right after a non-Hold one (After Effects' rule is
ambiguous here), layers with a negative stretch, and default values py-aep has to guess (it says
some synthesised defaults may be inaccurate).

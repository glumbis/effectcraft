# OpenFX plug-ins

This fork of EffectCraft can load [OpenFX](https://openfx.readthedocs.io/) (OFX) image-effect plug-ins and
use them as ordinary effects. The goal is to run commercial plug-ins that ship as OFX, such as
**RE:Vision ReelSmart Motion Blur (RSMB)** and **Boris FX Sapphire**, alongside the built-in effects.

An OFX effect behaves like any other EffectCraft effect: it appears in Effects & Presets under the
category the plug-in declares, its parameters show up in Effect Controls, they take keyframes and
expressions, and it can be applied by id from the control protocol
(`effect.apply {"effect": "<id>"}`). The id is the OFX identifier, lower-cased (for example
`org.effectcraft.test.invert`).

## Installing OFX plug-ins

Install the plug-in with its own installer. OFX plug-ins live in `.ofx.bundle` folders in standard
locations, and EffectCraft looks in the same places as other hosts:

| OS | Standard folder |
|---|---|
| Windows | `C:\Program Files\Common Files\OFX\Plugins` |
| macOS | `/Library/OFX/Plugins` |
| Linux | `/usr/OFX/Plugins` |

In addition, every folder listed in the `OFX_PLUGIN_PATH` environment variable is searched
(separated by `;` on Windows, `:` elsewhere). Folders are scanned recursively for bundles.

Nothing needs configuring: at start-up the desktop app scans these folders (the
`effect.plugins.scanOfx` command) and registers everything it finds. Large suites register many
effects, Sapphire alone around 270, so the first start-up can take a moment.

## Loading a plug-in by hand

* **Effect ▸ Load OpenFX Plug-in…** asks for a path: an `.ofx` binary, an `.ofx.bundle` folder, or a
  folder to scan.
* Control protocol: `effect.plugins.loadOfx {"path": "..."}` loads one path and returns the loaded
  effects and any errors; `effect.plugins.scanOfx {}` rescans every default location and
  `OFX_PLUGIN_PATH` and returns `{paths, loaded, errors}`.
* To skip the start-up scan (for example to rule OFX out when something misbehaves) set the
  environment variable `EFFECTCRAFT_NO_OFX=1`.

## How OFX maps onto EffectCraft

* Contexts: Filter is preferred, then General, Transition and Generator. A General effect's extra
  input clips (every clip except `Source`) become **Layer** parameters named after the clip: pick the
  layer to use as that input. A Transition's `SourceTo` clip works the same way.
* Parameters: doubles and integers become sliders, booleans checkboxes, choices popups, RGBA colours,
  2D doubles points, 3D doubles a 3-value point, strings text fields; custom parameters carry
  opaque plug-in data and are hidden. Groups and pages are folded into parameter names.
* The effect's output may be larger than its input (glows, blurs, borders); EffectCraft pads the
  layer buffer by the plug-in's region of definition before rendering.
* Pixels are float RGBA, premultiplied. Plug-ins that only support 8- or 16-bit are converted.
* Temporal plug-ins (motion blur, echo, retiming) can ask for the input at other times and for other
  layers; EffectCraft renders those on demand.

## Licensing

EffectCraft does not include, license or activate any third-party plug-in.

* **RSMB** and **Sapphire** need their own OFX licences. Without one they render a watermark or
  report an "unlicensed" status; both vendors sell OFX licences separately from their After Effects
  ones (some bundles include both, check yours). Licence activation is done with the vendor's own
  tools; copy any licence file to where the vendor says OFX hosts look.
* The host identifies itself to plug-ins as "EffectCraft". It is **not a supported host** for any
  vendor: do not ask the vendor for support, and a plug-in whose licence is tied to a list of known
  hosts may refuse to run.

## Known limitations

* **No GPU.** Plug-ins are rendered on the CPU through the OFX CPU API. Plug-ins that only render
  with OpenGL, CUDA or Metal fall back to their CPU path, or fail to load if they have none.
* **No on-screen overlays.** The plug-ins' viewer widgets (handles, tracking boxes) do not appear;
  use the parameters instead. Push buttons that open dialogs do nothing.
* **Native code runs in-process.** An OFX plug-in is a native library with full access to the
  machine. A bug in one can crash EffectCraft, and a malicious one can do anything the app can.
  Only install plug-ins you trust. (This is the main reason it is not part of upstream EffectCraft,
  whose own plug-ins are sandboxed WebAssembly.)
* Desktop only: there is no OFX on the web build. The binaries in this fork's CI artifacts are
  Windows x64 only for now.
* Plug-in messages (errors, licence notices) are formatted with the common printf conversions (`%s`,
  `%d`, `%u`, `%x`, `%c`, `%p`, `%f`, `%e`, `%g`, with width and precision) and at most 8 arguments;
  anything else is shown as written. On Apple Silicon the arguments cannot be read and the raw
  format string is shown.
* Some plug-ins rely on host features that are not implemented (analysis passes, multi-threaded
  frame rendering, custom interpolation, host-side clip caching). Those plug-ins may load with
  reduced functionality or report errors in the loader's `errors` list.

## Testing the host

`examples/ofx/ec-test-ofx` is a small dependency-free OFX plug-in (Invert, Echo, Border, Mix and an
effect with every parameter type) that exercises the host. The CI workflow
(`.github/workflows/ofx-host.yml`) builds it into a bundle and runs the integration tests in
`crates/ofx/tests`. To do the same locally:

```sh
cargo build --release --manifest-path examples/ofx/ec-test-ofx/Cargo.toml
# lay out the bundle (Windows shown; use Contents/Linux-x86-64 and libec_test_ofx.so on Linux)
mkdir -p target/ofx-test/ec-test-ofx.ofx.bundle/Contents/Win64
cp examples/ofx/ec-test-ofx/target/release/ec_test_ofx.dll target/ofx-test/ec-test-ofx.ofx.bundle/Contents/Win64/ec-test-ofx.ofx
EC_TEST_OFX_BUNDLE=$PWD/target/ofx-test/ec-test-ofx.ofx.bundle cargo test -p effectcraft-ofx
```

Without `EC_TEST_OFX_BUNDLE` the integration tests print a note and pass without testing anything.
You can also copy the bundle into a standard OFX folder to see the "EC Test" effects in the app.

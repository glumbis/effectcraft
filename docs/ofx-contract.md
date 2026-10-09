# OpenFX host: the shared contract (fork branch `ofx-host`)

This fork adds an **OpenFX (OFX) plug-in host** to EffectCraft so commercial and free OFX
plug-ins (target: RE:Vision ReelSmart Motion Blur and Boris FX Sapphire, both sold as OFX) can run
as ordinary EffectCraft effects. It is not intended to be merged upstream: it loads native code,
which upstream's sandboxed plug-in design deliberately avoids.

Three pieces, built in parallel against the interfaces below. **Do not change these interfaces
without updating this file.**

| Piece | Owner | Files |
|---|---|---|
| A. Plug-in API v2 + engine/host wiring | agent A | `crates/effects/src/plugin.rs`, `crates/plugin/**`, `crates/engine/**`, `crates/host/**`, `apps/effectcraft/src/main.rs`, `docs/plugins.md` |
| B. The OFX host crate | agent B | `crates/ofx/**`, root `Cargo.toml` (workspace dep entries only) |
| C. Test OFX plug-in, integration test, CI, docs | agent C | `examples/ofx/**`, `crates/ofx/tests/**`, `.github/workflows/ofx-host.yml`, `docs/ofx.md`, `FORK.md` |

## A. Plug-in API v2 (`effectcraft_effects::plugin`)

Everything in API v1 stays and keeps working (WebAssembly plug-ins, `register_plugin`). Additions:

```rust
pub const PLUGIN_API_VERSION: u32 = 2;          // manifests with api 1 or 2 are accepted
pub const MAX_PLUGINS: usize = 1024;            // Sapphire alone registers ~270 effects

// New PluginParamKind variants (serde tag names in brackets):
//   Point3 { default: [f64; 3] }        ["point3"]  flattens to 3 f64s (x, y in buffer px; z as is)
//   Layer                               ["layer"]   flattens to 1 f64: layer id, or -1 for none
//   Text { default: String }            ["text"]    flattens to 0 f64s; read with PluginParams::text
//   Hidden { default: String }          ["hidden"]  flattens to 0 f64s; not shown in the UI; for
//                                                   opaque plug-in data (OFX custom params)
// Slider gains `#[serde(default)] integer: bool` (OFX int params; UI shows 0 decimals).
// Every PluginParam gains optional `#[serde(default)] group: String` (OFX group/page path,
// e.g. "Advanced/Edges"); the UI may ignore it (names can be prefixed instead).

pub struct PluginParams<'a> {
    pub manifest: &'a PluginManifest,
    pub flat: &'a [f64],
    pub texts: &'a [String],   // one per Text/Hidden param, declaration order
}
impl PluginParams<'_> { pub fn text(&self, id: &str) -> &str; pub fn point3(&self, id: &str) -> [f64; 3]; pub fn layer(&self, id: &str) -> Option<u64>; /* + v1 accessors */ }

pub struct PluginFrame<'a> {
    pub width: u32,
    pub height: u32,
    pub pixels: &'a mut [[f32; 4]],   // premultiplied RGBA f32, row-major, row 0 = TOP
    pub scale: f64,                   // buffer pixels per layer pixel
    pub origin: [f64; 2],             // buffer-pixel position of layer pixel (0,0) (= Buf::offset)
    pub layer_size: [f64; 2],         // layer bounds in layer pixels
}

/// An image the host hands a v2 plug-in (another time or another layer). Same layout as
/// PluginFrame: premultiplied RGBA f32, row 0 = top, `origin` = buffer px of layer (0,0).
pub struct PluginImage { pub width: u32, pub height: u32, pub pixels: Vec<[f32; 4]>, pub origin: [f64; 2], pub scale: f64 }

/// Services available during a v2 render. Implemented by plugin.rs on top of EffectCtx/EffectHost.
pub trait PluginHost {
    /// This effect's input (its layer with masks and the effects ABOVE this one) at another
    /// LAYER time in seconds. None when unavailable (e.g. adjustment layers, no host).
    fn input_at(&self, layer_time: f64) -> Option<PluginImage>;
    /// The layer chosen in Layer parameter `param_id`, at LAYER time `layer_time`
    /// (converted to comp time by the host), effects and masks applied, mapped into this
    /// layer's buffer space at the current scale. None when unset/unavailable.
    fn layer_input(&self, param_id: &str, layer_time: f64) -> Option<PluginImage>;
    /// All flattened parameter values (+ texts) at another LAYER time (OFX paramGetValueAtTime).
    fn params_at(&self, layer_time: f64) -> Option<(Vec<f64>, Vec<String>)>;
    /// Composition frame rate (30 when unknown).
    fn frame_rate(&self) -> f64;
    /// Layer time of the current render, seconds.
    fn time(&self) -> f64;
    /// The layer's source time range in layer-time seconds as (first, end): the first source frame
    /// starts at `first`, the source ends at `end`. None when unknown (stills, solids, text,
    /// shapes, or a host that does not say); retimers then assume an unbounded range.
    fn frame_range(&self) -> Option<(f64, f64)> { None }   // default method, added for retimers
}

pub trait EffectPlugin: Send + Sync {
    fn manifest(&self) -> &PluginManifest;
    fn render(&self, frame: &mut PluginFrame, params: &PluginParams, time: f64) -> Result<(), String>;
    fn source(&self) -> String { "built-in".into() }
    /// v2: transparent pixels to add on EVERY side of the buffer before rendering (glows,
    /// blurs, OFX regions of definition larger than the input). In buffer pixels.
    fn padding(&self, _params: &PluginParams, _time: f64, _scale: f64) -> u32 { 0 }
    /// v2: render with host services. Default calls `render`.
    fn render_v2(&self, frame: &mut PluginFrame, params: &PluginParams, time: f64, _host: &dyn PluginHost) -> Result<(), String> { self.render(frame, params, time) }
}
```

`frame_range` is backed by `EffectEnv::layer_span: Option<(f64, f64)>` (`crates/effects`, layer-time
seconds, same meaning), which the renderer fills in `run_effect_stack` (`crates/render`,
`eval::layer_span`): footage and nested comps last their item's duration from layer time 0; a layer
with Time Remap reports its visible span (in to out point) in layer time instead, because layer
time is then no longer source time; layers without a duration get `None`. The OFX crate turns the
range into frames (`first * fps` to `end * fps - 1`) for the clips' `FrameRange` /
`UnmappedFrameRange`, the effect duration and the timeline suite, keeps the old 0..99999 fallback
when it is `None`, and fails `clipGetImage` on the source clip outside the range.

`run_slot` order: flatten params → `padding()` → `Buf::pad` → build `PluginFrame` → `render_v2`.
On error the frame is restored (as in v1). `render_v2` may be called from several threads at once.

### Engine / host wiring (agent A)

* `effectcraft_engine::Session` gains `pub ofx_loader: Option<OfxLoader>` where
  `pub type OfxLoader = fn(&str) -> Result<serde_json::Value, String>;` (argument: a path to an
  `.ofx` file, an `.ofx.bundle` folder, or a folder to scan recursively).
* Commands: `effect.plugins.loadOfx {path}` → the loader's JSON; `effect.plugins.scanOfx {}` →
  loads every default search path (see B) and returns `{paths:[…], loaded:[…], errors:[…]}`.
  Menu: Effect ▸ Load OpenFX Plug-in… (`effect.plugins.loadOfx`).
* `crates/host` sets `ofx_loader: Some(effectcraft_ofx::loader)` and the default paths on native
  (not wasm32). The desktop app runs `effect.plugins.scanOfx` at start-up unless the environment
  variable `EFFECTCRAFT_NO_OFX=1` is set.

## B. The OFX host crate (`crates/ofx`, package `effectcraft-ofx`)

Public API (agent C's tests use exactly this):

```rust
pub struct LoadedEffect { pub id: String, pub name: String, pub category: String, pub ofx_id: String, pub context: String, pub bundle: String }
pub struct LoadReport { pub loaded: Vec<LoadedEffect>, pub errors: Vec<String> }
/// `.ofx` binary, `.ofx.bundle` directory, or any folder (scanned recursively for bundles).
pub fn load_path(path: &std::path::Path) -> LoadReport;
/// Engine hook: load_path → {"loaded":[{id,name,category,ofxId,context,bundle}], "errors":[…]}.
/// Err only when nothing could be loaded AND there were errors.
pub fn loader(path: &str) -> Result<serde_json::Value, String>;
/// Standard OFX locations for this OS plus every entry of OFX_PLUGIN_PATH.
pub fn default_search_paths() -> Vec<std::path::PathBuf>;
```

Each OFX plug-in becomes one `EffectPlugin` registered with `register_plugin`. Effect id = the OFX
identifier (lower-cased, invalid characters → `_`), prefixed `ofx.` if it would start with `ec.`.
Category = first component of `kOfxImageEffectPluginPropGrouping` (fallback "OpenFX").

Mapping rules: context preference Filter > General > Transition > Generator. Extra input clips
(General: every clip except `Source`; Transition: `SourceTo`) become **Layer** params named after
the clip. OFX y-up ↔ our y-down is handled inside the crate (e.g. data pointer at the bottom row
with negative row bytes). Host supports float RGBA premultiplied images; byte/short-only plug-ins
are converted. No GPU, no OpenGL overlay interacts (stubs returning kOfxStatErrMissingHostFeature
/ kOfxStatReplyDefault).

## C. Test plug-in (`examples/ofx/ec-test-ofx`, standalone cargo project, cdylib)

One binary exporting several OFX effects that exercise the host: `org.effectcraft.test.Invert`
(filter, double param "amount"), `org.effectcraft.test.Echo` (filter; averages Source at t and
t − 1 frame via clipGetImage at another time), `org.effectcraft.test.Border` (filter; RoD grown by
int param "size" px, paints opaque red outside the source rect), `org.effectcraft.test.Mix`
(general context; second clip "Matte" multiplies alpha), and one effect declaring every param type
(double, int, bool, choice, rgba, double2D, double3D, string, custom, group, page, pushbutton).
The CI job builds it into `target/ofx-test/ec-test-ofx.ofx.bundle/Contents/Win64/ec-test-ofx.ofx`
(Linux: `Linux-x86-64`) and runs `crates/ofx/tests/*` with `EC_TEST_OFX_BUNDLE` set to the bundle.

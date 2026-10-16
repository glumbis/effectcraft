# OpenFX plug-ins

EffectCraft can load [OpenFX](https://openfx.readthedocs.io/) (OFX) image-effect plug-ins and use
them as ordinary effects: commercial suites that ship as OFX (for example RE:Vision ReelSmart Motion
Blur and Twixtor, Boris FX Sapphire) and the free OFX collections run alongside the built-in
effects. OFX is the open plug-in standard of Nuke, DaVinci Resolve, Vegas, Natron and others; its
API headers are BSD-3-Clause.

An OFX effect behaves like any other EffectCraft effect: it appears in Effects & Presets under the
category the plug-in declares, its parameters show up in Effect Controls, they take keyframes and
expressions, and it can be applied by id from the control protocol
(`effect.apply {"effect": "<id>"}`). The id is the OFX identifier, lower-cased (for example
`org.effectcraft.test.invert`).

Users: installing, finding and troubleshooting plug-ins is covered, step by step and per operating
system, in [FORK.md](../FORK.md). This page is the technical reference.

## Opt-in: the `openfx` feature

OFX plug-ins are native libraries that run inside the app's process, **without a sandbox**, unlike
EffectCraft's own WebAssembly plug-ins ([plugins.md](plugins.md)). Hosting them also needs `unsafe`
Rust to call the C interface, which the rest of the workspace forbids. So the host is a separate
crate (`crates/ofx`, `effectcraft-ofx`), the only one allowed `unsafe`, and it is **off by
default**: builds only include it with the `openfx` feature. The fork's downloads are built with
`openfx` and `aep` (After Effects project import, see
[tools/aep-import](../tools/aep-import/README.md)):

```sh
cargo run -p effectcraft --features openfx           # the desktop app
cargo build --release -p effectcraft -p effectcraft-cli --features "openfx aep"
```

Without the feature the OFX commands are disabled ("OpenFX plug-ins are not available in this
build") and nothing native is loaded. There is no OFX on the web build.

The fork ships for Windows x64, Linux x86_64 and macOS (Apple Silicon and Intel). The loader looks
for the bundle binary in `Contents/Win64`, `Contents/MacOS` or `Contents/Linux-x86-64`; on Linux
arm64 it tries `Linux-aarch64` and `Linux-arm-64` (no fork download is built for it).

## Installing OFX plug-ins

Install the plug-in with its own installer. OFX plug-ins live in `.ofx.bundle` folders in standard
locations, and EffectCraft looks in the same places as other hosts:

| OS | Standard folder |
|---|---|
| Windows | `C:\Program Files\Common Files\OFX\Plugins` (and `%CommonProgramFiles%\OFX\Plugins` when it differs) |
| macOS | `/Library/OFX/Plugins` |
| Linux | `/usr/OFX/Plugins` |

In addition, an `OFX` folder next to the executable and every folder listed in the
`OFX_PLUGIN_PATH` environment variable are searched (separated by `;` on Windows, `:` elsewhere).
Duplicate entries (compared case-insensitively) are searched once. Folders are scanned recursively
(up to eight levels) for bundles; links and junctions are followed.

The `OFX` folder next to the executable is where the fork's downloads keep the **EC Test** plug-in,
so other OFX hosts never see it:

| Download | The `OFX` folder |
|---|---|
| Windows | `OFX\` beside `effectcraft.exe` (`%LOCALAPPDATA%\Programs\EffectCraft-OFX\OFX` once installed) |
| Linux tar.gz | `OFX/` beside `effectcraft` (`~/.local/share/effectcraft-ofx/OFX` once installed) |
| Linux AppImage | `usr/bin/OFX` inside the image (read-only) |
| macOS | `EffectCraft.app/Contents/MacOS/OFX`, a symlink to `Contents/Resources/OFX`: `codesign` refuses data files in `Contents/MacOS`, and the lookup follows the link. Don't add files inside the signed app; use `/Library/OFX/Plugins` |

<!-- PENDING(versatility): per-user plug-in folders on Linux and macOS (for example under ~/.local/share and ~/Library) go in the table above and in the paragraph before it. -->

Nothing needs configuring: at start-up the desktop app scans these folders (the
`effect.plugins.scanOfx` command) and registers everything it finds. Large suites register many
effects (Sapphire around 290); loading them takes a few seconds. `EFFECTCRAFT_NO_OFX=1` skips the
start-up scan of the desktop app (only the exact value `1`; the CLI's `scanOfx` still works).

<!-- PENDING(speed): the start-up scan cache (ofx_cache.json in the config folder, settings key scan_cache, command effect.plugins.rescanOfx): what is cached, when it is invalidated, and how to force a rescan. -->

## The scan report

The desktop app writes what the scan found to `ofx_scan.txt` in its config folder
(`%APPDATA%\EffectCraft` on Windows, `~/Library/Application Support/EffectCraft` on macOS,
`$XDG_CONFIG_HOME/effectcraft` or `~/.config/effectcraft` on Linux; `EFFECTCRAFT_CONFIG_DIR` and
portable mode move it). The file is rewritten at every start. The release app has no console on
Windows, so this file is where load errors end up. When a scan found problems (or nothing loaded
but something was skipped) the app also shows a message at start-up that names the file. The CLI
doesn't write the file: `effectcraft-cli exec effect.plugins.scanOfx --empty` prints the same data
as JSON. Troubleshooting steps for users are in [FORK.md](../FORK.md#getting-plug-ins-to-work).

The file has these parts, in order:

1. A title line, then **`Host name shown to plug-ins: <name> (<source>)`**, with the settings file's
   path, how to change the name, a pending change that waits for a restart and any problem with the
   settings file ([Host name](#host-name)).
2. **Folders searched**: every search path, marked `found` or `missing`.
3. **Effects registered**: one line per effect (name, category, id, bundle).
4. **Problems**: plug-ins that could not become effects, one line each, naming the binary.
5. **Notes (plug-ins skipped on purpose)**: duplicates, plug-ins that declined the host, binaries
   that export no plug-ins, plug-ins of another OFX API, a context that fell back, a guessed main
   input. These are not failures.
6. **Load transcript**: one block per plug-in binary (see below).

If nothing was found at all, or files were found but none became an effect, the file ends with a
paragraph that says so.

### The load transcript

Each block starts with the binary's path and the host as the plug-in was shown it
(`"EffectCraft" 0.10.0 (OFX API 1.4)`: the name and the version from `Cargo.toml`), then lists what
the binary answered, with the OFX status named (`kOfxStatErrMissingHostFeature (4)`, not `4`;
unknown numbers are `unknown OFX status N`) and the time each action took:

```
<binary path>
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

A plug-in that registered cleanly (every action answered OK, nothing posted, nothing missing)
collapses to one line, so a suite of hundreds of effects stays readable:
`<id> v1.2: registered as effect <id> (filter context) (Load, Describe and DescribeInContext: OK)`.
Anything else keeps the full lines: the actions tried, each context attempted
(`DescribeInContext(filter)`, `DescribeInContext(general)`), the messages, the unmet requests, the
host properties it read and the outcome.

## What the host learns about a plug-in

`diag.rs` keeps a `PluginDiag` per plug-in. The suites reach it through a thread-local call context
that every action sets for the plug-in code it runs (`multiThread` workers inherit it), so what a
plug-in asks for is attributed to it and to the action. It records:

* **Messages** the plug-in posts through the message suite (`error`, `fatal`, `warning`,
  `message`, `log`, `question`): usually the text its other hosts' users see ("licence not found",
  "needs DaVinci Resolve"). The last 32 are kept, each cut at 600 characters.
* **Unmet requests**: properties (named with the kind of set: host, effect descriptor, instance,
  clip, parameter, image, action arguments) and suites the plug-in asked for that this host does
  not provide. A plug-in that gets `kOfxStatErrUnknown` for a property it needs usually fails with
  `kOfxStatErrMissingHostFeature` without saying which one, so these are the useful part of a
  report. Each distinct request is recorded once per plug-in (at most 64) and logged at `warn`
  once per process, so it reaches `Logs/effectcraft.log` at the default log level.
* **Host properties it read**, sorted (at most 128): what the plug-in was told about the host,
  above all its name.

A failed action's error line carries the status name, the plug-in's messages (not `log`-kind ones)
and the unmet requests, for example `Describe failed: kOfxStatErrMissingHostFeature (4); the
plug-in said "..."; it asked for something this host does not provide: property ... of the host`.
Create-instance and render failures carry the same detail. A `Load` that answers `kOfxStatFailed`
is explained as "the plug-in declined to load (a licence or host check?)" with the host name it was
shown; `kOfxStatReplyDefault` from `Load` is accepted.

## How a scan works

Loading a path is two phases, so that copies of one plug-in can be told apart before any of their
code is started.

1. **Probe**, per binary: open the library, call `OfxSetHost` if it exports it, call
   `OfxGetNumberOfPlugins`, then `OfxGetPlugin` for each. Nothing is described yet (no `Load`). A
   binary that answers a failure status to `OfxSetHost` ("not for this host"), or exports no
   plug-ins, is skipped with a note that names the host name it was shown. Only a failure status
   counts: an older plug-in that declares no return value leaves whatever was in the register.
2. **Duplicates**: an identifier offered more than once keeps its **highest version** (equal
   versions: the copy found first); the others are skipped before their `Load`, with one note per
   binary (`N duplicate plug-in(s) skipped, the newest copy is kept: ...`) and a transcript line
   per plug-in. Vendors such as Topaz leave alpha, beta and release bundles side by side in the
   OFX folder and document crashes from the older ones. An identifier that an earlier scan in the
   same process already registered keeps its place: registrations are permanent, so even a newer
   copy loses.
3. **Describe**, per plug-in that was kept: `setHost`, `Load`, `Describe`, then the checks and the
   context attempts below, and the registration as an effect.

What can stop a plug-in at the describe phase (each is an entry under Problems unless noted):

* Not an OpenFX image effect (another plug-in API): a note, not a problem. API version other than 1,
  or no `setHost` / `mainEntry`: a problem.
* `Load` or `Describe` fails (see above).
* **GPU only.** `OfxImageEffectPropCPURenderSupported` is `"false"` (OFX 1.5.1), or OpenGL, CUDA,
  Metal or OpenCL render support is `"needed"`: reported after `Describe` as "needs GPU rendering,
  which this host does not offer" with the reasons, instead of registering and failing every render.
* None of the contexts the plug-in lists is one this host runs, or `DescribeInContext` fails in
  every one of them.
* None of the pixel depths the plug-in lists is supported ([How OFX maps onto
  EffectCraft](#how-ofx-maps-onto-effectcraft)).
* It describes no `Output` clip.

## Host name

Some commercial plug-ins only enable themselves in hosts they know and decline everything else
(Frischluft's and Topaz's documentation name DaVinci Resolve). The only thing a plug-in can check is
the host's name, `kOfxPropName` (the host's `kOfxPropLabel` is set to the same text). It is
`EffectCraft` unless the user changes it in one of two places:

* `host_name` in `ofx_settings.json` in the config folder, next to `ofx_scan.txt`:
  `{"host_name": "SomeHost"}`. Other keys in the file are kept.
* The environment variable `EFFECTCRAFT_OFX_HOST_NAME`, which wins over the file.

Both are read **once**, when the host block is built. `set_state_dir`, which
`effectcraft_host::session()` calls as soon as it knows the config folder, decides it at start-up,
so a change applies from the **next start**. The name is trimmed and must be 1 to 128 characters without control
characters. A missing file is normal; a blank or `null` `host_name` means "default". A file that
cannot be read or understood (bad JSON, not an object, `host_name` not a string, an unusable name)
never stops a load: the default name is used and the problem is logged at `warn` and shown in
`ofx_scan.txt`.

The name is a claim, not a capability: a plug-in that recognises another host's name may switch to
code paths that host supports (GPU buffers, vendor suites) and fail for it at render time. Names
other hosts use, which have **not** been verified to help with any particular plug-in:
`DaVinciResolve`, `DaVinciResolveLite`, `uk.co.thefoundry.nuke`, `fr.inria.Natron`.

## Loading a plug-in by hand

* **Effect ▸ Load OpenFX Plug-in…** asks for an `.ofx` binary (inside `X.ofx.bundle/Contents/<platform>`).
  It lasts for the session; a plug-in kept in a scanned folder is loaded at every start.
* Control protocol: see [Commands](#commands).
* To skip the start-up scan (for example to rule OFX out when something misbehaves) set the
  environment variable `EFFECTCRAFT_NO_OFX=1`.

## A plug-in that crashes while loading

A native crash inside a plug-in (in its library start-up code, `OfxSetHost`, `OfxGetPlugin`, `Load`,
`Describe` or `DescribeInContext`) takes the whole app down, and since the standard folders are
scanned at every start-up, one bad plug-in would make EffectCraft crash on every launch. So, like
other OFX hosts, EffectCraft keeps a **crash blocklist** in its config folder (see
[The scan report](#the-scan-report) for where that is):

* Before it opens a plug-in binary the process writes the binary's path to `ofx_loading.<pid>.txt`,
  keeps that file locked, and deletes it when the phase is over (successfully or with an ordinary
  error). The probe and the describe phase each hold the marker.
* If the process dies in between, the file is still there and its lock is gone. The next process
  that loads plug-ins records a **strike** against that binary in `ofx_blocklist.txt` (one line
  per binary: strikes, a file stamp, the path, separated by tabs). A marker that is still locked
  belongs to another EffectCraft process (the CLI next to the app) that is loading right now, and
  is left alone.
* One strike doesn't block anything: the app may have been closed or killed while a big suite was
  loading (the window isn't up yet, so it can look stuck), or the plug-in had a one-off problem.
  A binary is **skipped** only after it crashed **two loads in a row**. The skip shows up in the
  loader's `errors` ("... skipped: it crashed EffectCraft while loading 2 times in a row ..."), in
  `ofx_scan.txt` and in the start-up message. The rest of the plug-ins load normally.
* A binary's strikes are cleared when its load finishes without a crash: that is, when its describe
  phase ends, or when the probe ends with nothing to describe (declined host, no plug-ins). A
  crash in the describe phase after a clean probe still counts. A binary whose file changed
  (updated or reinstalled) starts again from zero. To try a skipped plug-in again without updating
  it, run `effect.plugins.scanOfx {"retry": true}`, or delete its line (or the whole
  `ofx_blocklist.txt`) and restart.
* Lists written by earlier versions of the fork (a bare path per line, written after a single
  crash) count as one strike, so those plug-ins are tried again.

This only covers loading. A crash while an effect is rendering is not caught and is not
blocklisted; if that keeps happening, start with `EFFECTCRAFT_NO_OFX=1` or uninstall the plug-in.

<!-- PENDING(speed): protection against crashes during rendering: a plug-in that crashes twice while rendering is skipped, like one that crashes while loading. Replace the last paragraph above once confirmed. -->

## How OFX maps onto EffectCraft

* **Contexts:** every context the host can run and the plug-in lists is tried in the order Filter,
  General, Transition, Generator, until one describes. Each attempt starts from what `Describe`
  left: clips, parameters and property changes of a failed attempt are dropped. The reasons the
  other contexts failed become a note ("... the general context is used instead").
* **Clips:** a General effect's extra input clips (every clip except the main input) become
  **Layer** parameters named after the clip: pick the layer to use as that input. A Transition's
  `SourceTo` clip works the same way. The effect's own pixels come in on `Source` (`SourceFrom` in
  a Transition); a plug-in that has no clip with that name gets its first required clip that is
  neither `Output` nor a mask (noted), and a generator with none has no input.
* **Parameters:** doubles and integers become sliders, booleans checkboxes, choices popups, RGBA
  colours, strings text fields; custom parameters carry opaque plug-in data and are hidden. Groups
  and pages are folded into parameter names. Curve (parametric) parameters are not supported: a
  plug-in that defines one gets `kOfxStatErrUnknown`.
* 2D and 3D doubles follow their OFX double type. Only position values (type XY or XY absolute)
  become an on-screen point: a 2D position a point, a 3D position a 3-value point. Every other 2D or
  3D double, including the plain default, becomes one slider per component, named `<name>_x`,
  `_y` and `_z`. A plug-in that wants a viewer point for a value must declare it as a position.
* The effect's output may be larger than its input (glows, blurs, borders); EffectCraft pads the
  layer buffer by the plug-in's region of definition before rendering, measured on the real layer.
* **Pixels** are float RGBA, premultiplied. The pixel depth used is `float` if the plug-in lists it,
  else `short` (16-bit), else `byte`; byte and short plug-ins are converted. A plug-in whose list
  has none of the three (half-float only, for example) is reported as "needs an unsupported pixel
  depth". Clip components are RGBA, else RGB, else Alpha.
* **Parameter values** are applied to a new instance before `CreateInstance`, and the clip
  preferences are asked for again whenever a parameter named in
  `kOfxImageEffectPropClipPreferencesSlaveParam` changes, so plug-ins that read their parameters in
  `CreateInstance` (AI plug-ins load their models there) see the real values, and the depth,
  components and premultiplication the host uses follow the parameter.
* **Threads:** plug-in code runs on threads with 16 MiB stacks (the stack size Linux and other
  plug-ins are built for; Twixtor's optical flow overflowed the 2 MiB of the renderer's worker
  threads): the multithread suite's workers, and the `Render` and `CreateInstance` actions on
  **every OS**. Those two run on pooled `ofx-plugin` threads: an idle worker (the most recently
  used first, up to eight kept waiting) takes the job, so a plug-in that keeps per-thread state
  (GPU contexts, runtimes, licence checks) isn't handed a new thread for every frame. A call
  from a plug-in-stack thread runs inline, and if no thread can be started the job runs on the
  caller. A job that panics is a failed action; the worker survives.
* **Host properties** the plug-ins read without a fallback are served: no GPU of any kind
  (OpenGL, CUDA, Metal and OpenCL render support `"false"`, CPU render support `"true"`), no
  overlays, tiles or multi-resolution, temporal clip access yes, sequential render off, no
  parametric animation, a null window handle, render quality draft off. Setting the colour
  management properties is accepted and ignored: the host doesn't publish colour management.
  The descriptor starts with the specification's defaults for its list properties (the OFX C++
  Support library appends to them at the index `propGetDimension` returns, and fails Describe if
  that query fails).
* Temporal plug-ins (motion blur, echo, retiming) can ask for the input at other times and for other
  layers; EffectCraft renders those on demand. Each applied effect gets its own pool of OFX
  instances, so plug-ins that keep state between frames never mix two layers up.

## Retimers (Twixtor)

Time-remapping plug-ins such as RE:Vision **Twixtor** (OFX version) work. Twixtor offers the Filter
and General contexts; in General its optional extra inputs (motion vectors, foreground matte) are
Layer parameters like any other extra clip.

* `paramGetIntegral` and `paramGetDerivative` work for double, 2D / 3D double and colour parameters,
  so a plug-in can integrate an animated Speed % curve to find the source frame. They sample the
  animated value through the host: the derivative is a central difference over one frame, the
  integral composite Simpson with steps of at most one frame (at most 4096 steps per call, so ranges
  longer than that are coarser). Every sample evaluates the effect's parameters, so integrating from
  frame 0 late in a long clip costs time on every render. Outside a render the value is constant.
  Integer, choice and string parameters return `kOfxStatErrUnsupported`.
* The source clip's `FrameRange` / `UnmappedFrameRange`, the effect duration and the timeline
  suite's bounds are the layer's real source range (footage and nested comps: their duration; Time
  Remap layers: the visible span), converted with the comp frame rate. Layers without a duration
  (stills, solids, text, shapes) keep the fallback of frames 0 to 99999.
* `clipGetImage` on the source clip accepts fractional times (frame 12.5 renders the layer at that
  time; footage shows the frame the media layer picks for it, blended if the layer's Frame Blending
  is on) and fails with `kOfxStatFailed` outside the frame range instead of inventing a frame.
* Limits: no GPU, no on-screen overlays (the speed graph and motion-vector tools are unavailable), no
  host-supplied motion vectors (Twixtor's own optical flow runs on the CPU and is slow at high
  resolution). The extra clips' frame ranges are assumed to equal the source's.

## Licensing

EffectCraft does not include, license or activate any third-party plug-in.

* Commercial plug-ins need their own OFX licences. Without one they render a watermark or report an
  "unlicensed" status; vendors often sell OFX licences separately from their After Effects ones.
  Licence activation is done with the vendor's own tools.
* The host identifies itself to plug-ins as "EffectCraft" unless the user changes the name
  ([Host name](#host-name)). It is **not a supported host** for any vendor: do not ask the vendor
  for support, and a plug-in whose licence is tied to a list of known hosts may refuse to run. Whether
  a licence allows running a plug-in under another host's name is for the licence to say.

## Known limitations

* **No GPU.** Plug-ins are rendered on the CPU through the OFX CPU API. Plug-ins that only render
  with OpenGL, CUDA or Metal fall back to their CPU path, or are reported "needs GPU rendering"
  and not registered if they have none.
* **No on-screen overlays.** The plug-ins' viewer widgets (handles, tracking boxes) do not appear;
  use the parameters instead. Push buttons that open dialogs do nothing.
* **Native code runs in-process.** An OFX plug-in is a native library with full access to the
  machine. A bug in one can crash EffectCraft, and a malicious one can do anything the app can.
  Only install plug-ins you trust. (This is why the host is opt-in; EffectCraft's own plug-ins are
  sandboxed WebAssembly.)
* **Glows on transparent layers.** Some plug-ins (Sapphire's glows) add light without adding alpha,
  so on a layer with transparent areas the glow outside the opaque pixels disappears. On footage, or
  a precomp with a background, it shows as in other hosts.
* **Curve (parametric) parameters and half-float images** are not supported (see the notes under
  [How OFX maps onto EffectCraft](#how-ofx-maps-onto-effectcraft)).

  <!-- PENDING(versatility): curve (parametric) parameters and half-float plug-ins are supported: remove this bullet, and the matching sentences under "How OFX maps onto EffectCraft", once confirmed. -->

* **Platforms.** Desktop only: there is no OFX on the web build. **Windows x64** is tested with
  RSMB 7, Twixtor 8 and Sapphire 2026.5. **Linux x86_64** and **macOS** are built and run the host's
  test suite in CI (macOS only on Apple Silicon, where a failing test is tolerated so the packages
  are still built), but no commercial plug-in has been confirmed on them. The Intel Mac build is
  cross-compiled.
* **Apple Silicon.** The parameter and message suites are C-variadic (`paramGetValue(handle, name,
  ...)`, `message(handle, type, id, format, ...)`). The Apple arm64 ABI passes every variadic
  argument on the stack in an 8-byte slot while named arguments use registers, which differs from
  the System V and Windows slot models; the host declares those functions accordingly on
  `aarch64-apple-darwin` (the unused argument registers as dummy parameters, then one `u64` per
  stack slot). Before this, parameter reads wrote through garbage pointers there. The message suite
  now expands printf arguments on Apple Silicon too. `tests/variadic_params.rs` calls every
  parameter function through real C-variadic pointers on whatever platform runs the tests, but the
  Apple declarations were only type-checked locally and have not been confirmed on a Mac by a
  person.
* Plug-in messages (errors, licence notices) are formatted with the common printf conversions (`%s`,
  `%d`, `%u`, `%x`, `%c`, `%p`, `%f`, `%e`, `%g`, with width and precision) and at most 8 arguments;
  anything else is shown as written.
* Some plug-ins rely on host features that are not implemented (analysis passes, multi-threaded
  frame rendering, custom interpolation, host-side clip caching). Those plug-ins may load with
  reduced functionality or report errors in the loader's `errors` list.

## Commands

| Command | Parameters | Result |
|---|---|---|
| `effect.plugins.scanOfx` | `{retry?}` | `{paths, loaded, errors, notes, transcript}`: loads every plug-in in the search paths (folders that don't exist are skipped; one bad plug-in doesn't stop the rest). `retry: true` empties the crash blocklist first. |
| `effect.plugins.loadOfx` | `{path}`: an `.ofx` binary, an `.ofx.bundle` folder or a folder (scanned recursively) | `{loaded: [{id, name, category, ofxId, context, bundle}], errors, notes, transcript}`. It fails, with the reasons from `errors` and `notes`, when nothing loaded and there is something to say. |
| `effect.plugins.ofxHostName` | none | `{name, source, configured, environmentOverride, settingsFile, problem, restartRequired, next?, note?}`: the host name in use, where it comes from (`default`, `settings file` or `environment variable`), what `ofx_settings.json` holds, and whether a stored change waits for a restart. |
| `effect.plugins.setOfxHostName` | `{name}`; an empty name goes back to `EffectCraft` | The same as `ofxHostName`. Stores the name in `ofx_settings.json` (through a temporary file, so a failure leaves the old file); it applies at the next start. Fails when there is no config folder or the name is unusable. |

`loadOfx` is the only one in a menu (Effect ▸ Load OpenFX Plug-in…). Under file-access
confinement (`--automation-read-root` and `--automation-write-root`, see [mcp.md](mcp.md)) the
agent can't use `loadOfx`, `scanOfx` or `setOfxHostName`: native, unsandboxed code and what it is
told about its host are not an agent's call. `ofxHostName` only reads.

<!-- PENDING(speed): effect.plugins.rescanOfx joins this table with the scan cache. -->

## How it fits together

* **Plug-in API 2** (`effectcraft_effects::plugin`, [plugins.md](plugins.md#plug-in-api-2-native-plug-ins))
  is what the host is built on: more parameter kinds (3D points, layers, text, hidden data), padding
  for regions of definition larger than the layer, and a `PluginHost` with the effect's input at other
  times (`input_at`), other layers (`layer_input`), parameters at other times (`params_at`), the frame
  rate, the source's frame range and a stable per-instance key. WebAssembly plug-ins keep ABI 1.
* **`crates/ofx`**: `ffi` (the C ABI transcribed from the OpenFX headers), `props` (property sets),
  `params` (parameter storage and the mapping above), `clips` (images and pixel conversion; OFX is
  y-up, rows are flipped on the way in and out), `instance` (effect handles, geometry, the
  per-render context, actions and the plug-in thread pool), `effect` (the `EffectPlugin`
  implementation), `suites` (property, parameter, image effect, memory, multithread, message,
  progress, timeline, interact), `loader` (bundle discovery, the probe and describe phases,
  duplicates, registration), `blocklist`, `settings` (the host name) and `diag` (messages, unmet
  requests and the load transcript).
* **Engine / hosts**: `Session::ofx_loader`, `ofx_search_paths`, `ofx_clear_blocklist` and
  `ofx_host_name` are set by `effectcraft-host` with the `openfx` feature, which also tells the
  crate where the config folder is (`set_state_dir`: the blocklist and `ofx_settings.json` live
  there). The commands are in [Commands](#commands); the desktop app runs the scan at start-up and
  writes `ofx_scan.txt`.

## Testing the host

`examples/ofx/ec-test-ofx` is a small dependency-free OFX plug-in that exercises the host. It
exports nine effects: Invert, Echo, Border, Mix, an effect with every parameter type (All Params),
and one each that mimics a situation real plug-ins create: **Support Lib** (describes itself the
way the OFX C++ Support library does), **Create Time** (reads a parameter in `CreateInstance` and
answers clip preferences from a slave parameter), **Context Fallback** (its Filter context fails,
General works) and **No Source** (the main input isn't called `Source`). The environment variable
`EC_TEST_OFX_MODE` makes it misbehave for the diagnostics tests: `decline-host`, `no-plugins`,
`load-fails`, `describe-fails`, and `extras` (exports only `GpuOnly` and `NotAnImageEffect`, which
cannot become effects). The integration tests in `crates/ofx/tests` (`test_plugin`, `diagnostics`,
`duplicates` and `variadic_params`) load it. Build it into a bundle first; the script picks the
platform folder and library name from the Rust target:

```sh
bash tools/fork-installer/ci/build-test-bundle.sh            # writes target/ofx-test/ec-test-ofx.ofx.bundle
EC_TEST_OFX_BUNDLE=$PWD/target/ofx-test/ec-test-ofx.ofx.bundle cargo test -p effectcraft-ofx
```

Without `EC_TEST_OFX_BUNDLE` the integration tests print a note and pass without testing anything
(the unit tests always run). Registrations are permanent for a process, so tests that load copies
of the same plug-ins live in their own test binaries. You can also put the bundle in a standard OFX
folder or on `OFX_PLUGIN_PATH` to see the "EC Test" effects in an `openfx` build of the app; the
fork's downloads already carry it.

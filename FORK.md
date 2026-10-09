# About this fork

This is an **unofficial, personal fork** of EffectCraft (the upstream project is linked from the
README). It adds one thing: an **OpenFX plug-in host**, so that OFX plug-ins such as RE:Vision
**ReelSmart Motion Blur** and **Boris FX Sapphire** can run inside EffectCraft as normal effects.

* It is **not for merging upstream.** Loading arbitrary native plug-in code in-process is the
  opposite of upstream's sandboxed WebAssembly plug-in design, so nothing here is meant to be
  proposed as a pull request. The branch with the work is `ofx-host`.
* It is **unofficial and unsupported.** Don't report problems with this fork to the upstream
  project or to the plug-in vendors.
* Everything else in the app is unchanged. The plug-in API gained a second version (more parameter
  kinds, padding, access to other times and layers); version 1 WebAssembly plug-ins keep working.

How it works, what is supported and the licensing caveats are in [docs/ofx.md](docs/ofx.md); the
design contract between the pieces is [docs/ofx-contract.md](docs/ofx-contract.md).

## Getting a build without compiling

GitHub Actions builds the fork on every push to `ofx-host` (workflow `OFX host`):

1. **Enable Actions on your fork first.** Forks have workflows turned off by default: open the
   fork's **Actions** tab on GitHub and click "I understand my workflows, go ahead and enable
   them".
2. Push to `ofx-host`, or open Actions > **OFX host** > Run workflow and pick the `ofx-host`
   branch.
3. When the run finishes, open it and download the artifact **`effectcraft-ofx-windows`** (kept for
   30 days; you need to be signed in to GitHub to download artifacts). Unzip it. It contains
   `effectcraft.exe` (the app), `effectcraft-cli.exe` and `ec-test-ofx.ofx.bundle.zip` (the test
   plug-in).

The Windows binaries are unsigned, so SmartScreen will warn on first run ("More info", then "Run
anyway"). The same workflow also runs the tests on Linux.

## Trying RSMB or Sapphire on another PC

1. Install the plug-in with the vendor's installer on that PC; it puts the OFX bundle in
   `C:\Program Files\Common Files\OFX\Plugins`. Activate your **OFX** licence (RSMB and Sapphire
   sell and activate OFX separately from After Effects).
2. Download and unzip the `effectcraft-ofx-windows` artifact somewhere, and run `effectcraft.exe`.
   The OFX folders are scanned at start-up; the plug-ins appear in Effects & Presets under their own
   categories (Sapphire registers about 270 effects).
3. Not showing up? Choose **Effect > Load OpenFX Plug-in...** and point it at the `.ofx.bundle`
   folder; any errors it reports say what the plug-in objected to. Set `OFX_PLUGIN_PATH` if the
   bundle is somewhere unusual.
4. To check the host itself with no commercial plug-in involved, unzip `ec-test-ofx.ofx.bundle.zip`
   into the same OFX folder (or load it by hand) and apply the "EC Test" effects.
5. If something crashes the app, start it with `EFFECTCRAFT_NO_OFX=1` to run without OFX, and note
   which plug-in and effect it was. Native plug-in bugs can take the whole app down.
6. A plug-in that crashes *while loading* does not need `EFFECTCRAFT_NO_OFX`: the next start-up
   remembers it in `ofx_blocklist.txt` (in the config folder, `%APPDATA%\EffectCraft` on Windows) and
   skips it, with a message on stderr, so the app comes up with everything else. To try it again
   run the control command `effect.plugins.scanOfx {"retry": true}` or delete its line from that
   file. See [docs/ofx.md](docs/ofx.md).

//! An OpenFX (OFX 1.4) image-effect host for EffectCraft.
//!
//! This crate loads native OFX plug-in bundles (RE:Vision ReelSmart Motion Blur, Boris FX
//! Sapphire, the free OFX collections, ...) and registers every plug-in as an ordinary
//! EffectCraft effect through the plug-in API v2 (`effectcraft_effects::plugin`). It is a CPU
//! host: float premultiplied RGBA images (byte / short / RGB / alpha plug-ins are converted),
//! no GPU or OpenGL, no overlay interacts. See `docs/ofx.md` for usage and how it fits together.
//!
//! **This crate runs native code in-process, without a sandbox**, unlike EffectCraft's own
//! plug-ins, which are sandboxed WebAssembly. It is opt-in: only hosts built with the `openfx`
//! feature (`effectcraft-host/openfx`) link it. It needs `unsafe` to speak the C ABI, so it does
//! not opt into the workspace's `unsafe_code = "forbid"` lint; every host callback is wrapped in
//! `catch_unwind`, null-checks its pointers and validates handles by magic number. On wasm32 the
//! crate is empty.
//!
//! Modules: [`ffi`] (the C ABI), [`props`] (property sets), [`params`] (parameter storage and the
//! mapping to EffectCraft parameters), [`curve`] (the curves of parametric parameters), [`clips`] (clips, images and pixel conversion),
//! [`instance`] (effect handles, geometry, per-render context, actions), [`effect`] (the
//! `EffectPlugin` implementation), [`suites`] (the host suites), [`loader`] (bundle discovery
//! and registration), [`blocklist`] (skips plug-ins that crashed the app while loading),
//! [`settings`] (the host name plug-ins are told, configurable) and [`diag`] (what the host learns about a plug-in while it runs: its messages, the properties and
//! suites it asked for that this host lacks; the load transcript is built from it).

// Native code: nothing to host on the web build.
#![cfg(not(target_arch = "wasm32"))]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod blocklist;
pub mod cache;
pub mod clips;
pub mod curve;
pub mod diag;
pub mod effect;
pub mod ffi;
pub mod inflight;
pub mod instance;
pub mod loader;
pub mod lock;
pub mod open;
pub mod params;
pub mod pool;
pub mod props;
pub mod settings;
pub mod suites;

use std::path::PathBuf;

pub use loader::{default_search_paths, library_loads, load_path, load_path_with};
pub use open::open_command;
pub use settings::host_name_command;

/// Set the directory that holds the crash blocklist and `ofx_settings.json` (the host's config
/// directory) and decide the host name plug-ins will be told ([`settings`]) right away, so the
/// start-up scan and the commands agree on it. Until this is called the blocklist is disabled and
/// the host name is the default or the environment variable's.
pub fn set_state_dir(dir: PathBuf) {
    blocklist::set_state_dir(dir.clone());
    cache::reset();
    let _ = settings::identity();
    // Where the record of what plug-ins are doing goes, and where a crashed session's is found (when
    // the first plug-in is loaded).
    inflight::set_dir(dir);
}

/// One OFX plug-in that was registered as an effect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedEffect {
    /// The EffectCraft effect id: the OFX identifier lower-cased (`ofx.` is prefixed if it
    /// would start with `ec.`).
    pub id: String,
    /// Display name (the plug-in's label).
    pub name: String,
    /// Effects & Presets category: the first component of the plug-in's grouping.
    pub category: String,
    /// The OFX plug-in identifier.
    pub ofx_id: String,
    /// The OFX context the effect runs in: `filter`, `general`, `transition` or `generator`.
    pub context: String,
    /// The bundle folder (or binary) it came from.
    pub bundle: String,
}

/// The outcome of [`load_path`].
#[derive(Clone, Debug, Default)]
pub struct LoadReport {
    pub loaded: Vec<LoadedEffect>,
    /// What stopped a plug-in from becoming an effect (one line each, naming the binary).
    pub errors: Vec<String>,
    /// Plug-ins that were skipped on purpose, and why: not an image effect, declined this host,
    /// exports no plug-ins, a duplicate of another copy (not failures).
    pub notes: Vec<String>,
    /// One block of text per binary: what each action answered, what the plug-in said, which
    /// properties and suites it asked for that this host lacks (see [`diag`]).
    pub transcript: Vec<String>,
    /// What the scan cache did: which binaries registered from it without being loaded, which were
    /// scanned (and why), problems with the cache file itself.
    pub cache: cache::CacheReport,
}

/// Engine hook: [`load_path_with`] as `{"loaded": [{id, name, category, ofxId, context, bundle}],
/// "errors": [...], "notes": [...], "transcript": [...], "cache": {enabled, file, hits, scanned,
/// lines, problems}, "crashes": [...]}` (`crashes`: the plug-ins that crashed the previous session). `use_cache`: binaries that did not change register from the scan cache
/// without being loaded (the start-up scan); `false` loads everything (an explicit load). Always
/// `Ok`: a path with nothing to load is an empty `loaded` and `errors` that say why (the engine
/// turns that into an error for a single `loadOfx`, and a scan wants the notes and the transcript
/// either way).
pub fn loader(path: &str, use_cache: bool) -> Result<serde_json::Value, String> {
    let report = load_path_with(&PathBuf::from(path), use_cache);
    let loaded: Vec<serde_json::Value> = report
        .loaded
        .iter()
        .map(|e| serde_json::json!({"id": e.id, "name": e.name, "category": e.category, "ofxId": e.ofx_id, "context": e.context, "bundle": e.bundle}))
        .collect();
    Ok(serde_json::json!({
        "loaded": loaded,
        "errors": report.errors,
        "notes": report.notes,
        "transcript": report.transcript,
        "cache": report.cache,
        "crashes": inflight::recovered_lines()
    }))
}

/// Engine hook behind `effect.plugins.rescanOfx`: delete the scan cache, so that the next start
/// loads and describes every plug-in again. (Effects already registered in this process stay as
/// they are: a registration cannot be replaced.)
pub fn rescan_command() -> Result<serde_json::Value, String> {
    let file = cache::clear()?;
    Ok(serde_json::json!({
        "cleared": true,
        "file": file,
        "restartRequired": true,
        "note": "the scan cache was deleted: the next start loads and describes every OpenFX plug-in again (this takes effect when EffectCraft restarts)"
    }))
}

/// Helper programs plug-ins start and leave running after the host quits (Boris FX Sapphire
/// pre-launches its preset browser on load).
const HELPERS: &[&str] = &["preset-browser.exe"];

/// Call when the app quits: sends `DestroyInstance` to the pooled plug-in instances and `Unload` to
/// the loaded plug-ins, and stops the plug-in helper programs ([`HELPERS`]) this process started,
/// which would otherwise keep running (and keep a CLI's output pipe open). Only direct child
/// processes with those names are touched.
pub fn shutdown() {
    // Plug-ins that rendered all session without a crash lose their strikes; the in-flight record is
    // removed (nothing after this is blamed on a plug-in).
    inflight::close(&effect::rendered_ids());
    // Every pooled instance is destroyed and every loaded plug-in unloaded (each call guarded, the
    // whole bounded in time); then the helper programs they started are stopped.
    effect::shutdown_all();
    #[cfg(windows)]
    win::stop_children(HELPERS);
}

#[cfg(windows)]
mod win {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
    use windows_sys::Win32::System::Threading::{GetCurrentProcessId, OpenProcess, PROCESS_TERMINATE, TerminateProcess};

    /// Terminate this process's direct children whose executable is one of `names`, together with
    /// everything they started (packaged helpers relaunch themselves as a child).
    pub fn stop_children(names: &[&str]) {
        let procs = snapshot();
        let me = unsafe { GetCurrentProcessId() };
        let mut doomed: Vec<u32> = procs.iter().filter(|p| p.1 == me && names.iter().any(|n| p.2.eq_ignore_ascii_case(n))).map(|p| p.0).collect();
        let mut i = 0;
        while let Some(&pid) = doomed.get(i) {
            for p in procs.iter().filter(|p| p.1 == pid && p.0 != pid) {
                if !doomed.contains(&p.0) && doomed.len() < 256 {
                    doomed.push(p.0);
                }
            }
            i += 1;
        }
        for pid in doomed {
            // SAFETY: a handle opened for termination only, and closed.
            unsafe {
                let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
                if !h.is_null() {
                    TerminateProcess(h, 0);
                    CloseHandle(h);
                }
            }
        }
    }

    /// (pid, parent pid, executable name) of every process.
    fn snapshot() -> Vec<(u32, u32, String)> {
        let mut out = Vec::new();
        // SAFETY: plain Win32 calls on a process snapshot; the entry struct is sized as the API
        // requires and the snapshot handle is closed.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE_VALUE {
                return out;
            }
            let mut e: PROCESSENTRY32W = std::mem::zeroed();
            e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut more = Process32FirstW(snap, &mut e) != 0;
            while more {
                let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                out.push((e.th32ProcessID, e.th32ParentProcessID, String::from_utf16_lossy(e.szExeFile.get(..len).unwrap_or_default())));
                more = Process32NextW(snap, &mut e) != 0;
            }
            CloseHandle(snap);
        }
        out
    }
}

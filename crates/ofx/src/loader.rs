//! Finding, loading and describing OFX plug-in binaries, and registering them as effects.
//!
//! Bundle layout: `X.ofx.bundle/Contents/{Win64,MacOS,Linux-x86-64}/X.ofx`. A binary exports
//! `OfxGetNumberOfPlugins` and `OfxGetPlugin` (and optionally `OfxSetHost`). For each plug-in the
//! loader runs `Load`, `Describe` and `DescribeInContext` for the preferred context
//! (Filter, General, Transition, Generator), maps the parameters to a manifest and registers an
//! [`OfxEffect`]. Libraries stay loaded for the life of the process.

use std::collections::HashMap;
use std::ffi::{c_int, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use effectcraft_effects::plugin::{PLUGIN_API_VERSION, PluginManifest, register_plugin};

use crate::clips::{Comps, Depth};
use crate::diag::{CallCtx, PluginDiag, is_failure, one_line, status_name};
use crate::effect::OfxEffect;
use crate::ffi::*;
use crate::instance::{ClipCfg, ClipRole, EffectCfg, EffectObj, PluginEntry, Safety};
use crate::params::map_params;
use crate::props::PropSet;
use crate::suites::host_ptr;
use crate::{LoadReport, LoadedEffect};

/// The directory inside a bundle's `Contents` that holds this platform's binary.
#[cfg(all(windows, target_arch = "x86_64"))]
const PLATFORM_DIR: &str = "Win64";
#[cfg(all(windows, not(target_arch = "x86_64")))]
const PLATFORM_DIR: &str = "Win32";
#[cfg(target_os = "macos")]
const PLATFORM_DIR: &str = "MacOS";
#[cfg(all(not(windows), not(target_os = "macos"), target_arch = "x86_64"))]
const PLATFORM_DIR: &str = "Linux-x86-64";
#[cfg(all(not(windows), not(target_os = "macos"), not(target_arch = "x86_64")))]
const PLATFORM_DIR: &str = "Linux-aarch64";

const MAX_DEPTH: usize = 8;

fn load_lock() -> &'static Mutex<()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
}

/// Effects already loaded, by canonical binary path (re-loading a path reports them again).
fn loaded_cache() -> &'static Mutex<HashMap<PathBuf, Vec<LoadedEffect>>> {
    static L: OnceLock<Mutex<HashMap<PathBuf, Vec<LoadedEffect>>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Standard OFX locations for this OS, an `OFX` folder next to the executable (where the
/// fork's download keeps its test plug-in, so other OFX hosts don't see it) and every entry of
/// `OFX_PLUGIN_PATH`.
pub fn default_search_paths() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = Vec::new();
    #[cfg(windows)]
    {
        let common = std::env::var_os("CommonProgramFiles").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Program Files\Common Files"));
        v.push(common.join("OFX").join("Plugins"));
        v.push(PathBuf::from(r"C:\Program Files\Common Files\OFX\Plugins"));
    }
    #[cfg(target_os = "macos")]
    v.push(PathBuf::from("/Library/OFX/Plugins"));
    #[cfg(all(not(windows), not(target_os = "macos")))]
    v.push(PathBuf::from("/usr/OFX/Plugins"));
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("OFX"))) {
        v.push(dir);
    }
    if let Some(p) = std::env::var_os("OFX_PLUGIN_PATH") {
        v.extend(std::env::split_paths(&p).filter(|p| !p.as_os_str().is_empty()));
    }
    let mut seen = std::collections::HashSet::new();
    v.retain(|p| seen.insert(p.to_string_lossy().to_lowercase()));
    v
}

/// Load an `.ofx` binary, an `.ofx.bundle` folder, or every bundle below a folder.
pub fn load_path(path: &Path) -> LoadReport {
    let _guard = load_lock().lock().unwrap_or_else(PoisonError::into_inner);
    let mut report = LoadReport::default();
    let mut binaries: Vec<(PathBuf, String)> = Vec::new();
    if !path.exists() {
        report.errors.push(format!("{}: no such file or folder", path.display()));
        return report;
    }
    find_binaries(path, 0, &mut binaries, &mut report.errors);
    if binaries.is_empty() && report.errors.is_empty() {
        report.errors.push(format!("{}: no OpenFX plug-ins found", path.display()));
    }
    // The same binary can be reached twice (through a link, or a folder listed twice).
    let mut seen = std::collections::HashSet::new();
    binaries.retain(|(bin, _)| seen.insert(std::fs::canonicalize(bin).unwrap_or_else(|_| bin.clone())));
    for (bin, bundle) in binaries {
        load_binary(&bin, &bundle, &mut report);
    }
    report
}

fn is_bundle(p: &Path) -> bool {
    p.file_name().and_then(|n| n.to_str()).map(|n| n.to_lowercase().ends_with(".ofx.bundle")).unwrap_or(false)
}

fn is_ofx_file(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("ofx")).unwrap_or(false)
}

/// The binary inside a bundle.
fn bundle_binary(bundle: &Path) -> Option<PathBuf> {
    let name = bundle.file_name()?.to_str()?;
    let stem = name.get(..name.len().checked_sub(".ofx.bundle".len())?)?;
    let dir = bundle.join("Contents").join(PLATFORM_DIR);
    let exact = dir.join(format!("{stem}.ofx"));
    if exact.is_file() {
        return Some(exact);
    }
    std::fs::read_dir(&dir).ok()?.filter_map(|e| e.ok()).map(|e| e.path()).find(|p| p.is_file() && is_ofx_file(p))
}

/// The `.ofx.bundle` folder a bare binary belongs to: `X.ofx.bundle/Contents/<platform>/X.ofx`.
/// Pure path logic (the platform directory may have any name, nothing is read from disk).
fn bundle_of(bin: &Path) -> Option<PathBuf> {
    let platform = bin.parent()?;
    let contents = platform.parent()?;
    if !contents.file_name()?.to_str()?.eq_ignore_ascii_case("Contents") {
        return None;
    }
    let bundle = contents.parent()?;
    is_bundle(bundle).then(|| bundle.to_path_buf())
}

fn find_binaries(path: &Path, depth: usize, out: &mut Vec<(PathBuf, String)>, errors: &mut Vec<String>) {
    if path.is_file() {
        // A bare binary inside a bundle reports (and describes) its bundle folder; otherwise the binary itself.
        let bundle = bundle_of(path).unwrap_or_else(|| path.to_path_buf());
        out.push((path.to_path_buf(), bundle.to_string_lossy().into_owned()));
        return;
    }
    if is_bundle(path) {
        match bundle_binary(path) {
            Some(b) => out.push((b, path.to_string_lossy().into_owned())),
            None => errors.push(format!("{}: no {PLATFORM_DIR} binary in this bundle", path.display())),
        }
        return;
    }
    if depth >= MAX_DEPTH {
        return;
    }
    let Ok(rd) = std::fs::read_dir(path) else {
        errors.push(format!("{}: cannot read folder", path.display()));
        return;
    };
    let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        // Links and junctions are followed (plug-ins are often linked in from elsewhere); a link
        // cycle stops at MAX_DEPTH and `load_path` loads each binary once.
        let Ok(meta) = std::fs::metadata(&p) else { continue };
        if meta.is_dir() {
            find_binaries(&p, depth + 1, out, errors);
        } else if meta.is_file() && is_ofx_file(&p) {
            let bundle = bundle_of(&p).unwrap_or_else(|| p.clone());
            out.push((p.clone(), bundle.to_string_lossy().into_owned()));
        }
    }
}

type CountFn = unsafe extern "C" fn() -> c_int;
type GetPluginFn = unsafe extern "C" fn(c_int) -> *mut OfxPlugin;
/// `OfxStatus OfxSetHost(const OfxHost *host)` (OFX 1.5). A binary that exports it and answers
/// `kOfxStatFailed` is "not for this host" and is skipped.
type SetHostFn = unsafe extern "C" fn(*mut OfxHost) -> OfxStatus;

fn open_library(path: &Path) -> Result<libloading::Library, String> {
    // SAFETY: loading a native library runs its initialisers; that is what loading a plug-in is.
    #[cfg(windows)]
    {
        use libloading::os::windows::{LOAD_WITH_ALTERED_SEARCH_PATH, Library};
        // LOAD_WITH_ALTERED_SEARCH_PATH needs an absolute path with backslashes (forward slashes
        // make the loader fail).
        let native = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let native = PathBuf::from(native.to_string_lossy().replace('/', "\\"));
        unsafe { Library::load_with_flags(&native, LOAD_WITH_ALTERED_SEARCH_PATH) }.map(libloading::Library::from).map_err(|e| lib_error(&e))
    }
    #[cfg(not(windows))]
    {
        unsafe { libloading::Library::new(path) }.map_err(|e| lib_error(&e))
    }
}

/// A loader error with the OS reason it wraps (libloading's own message omits it on Windows).
fn lib_error(e: &libloading::Error) -> String {
    match std::error::Error::source(e) {
        Some(src) => format!("{e}: {src}"),
        None => e.to_string(),
    }
}

/// Run `f` as `action` of `plugin`, so the suites attribute what the plug-in asks for to it.
fn in_scope<R>(diag: &Arc<PluginDiag>, plugin: &str, action: &str, f: impl FnOnce() -> R) -> R {
    let ctx = CallCtx { diag: diag.clone(), plugin: plugin.into(), action: action.into() };
    let _scope = ctx.enter();
    f()
}

fn load_binary(bin: &Path, bundle: &str, report: &mut LoadReport) {
    let key = std::fs::canonicalize(bin).unwrap_or_else(|_| bin.to_path_buf());
    if let Some(done) = loaded_cache().lock().unwrap_or_else(PoisonError::into_inner).get(&key) {
        report.loaded.extend(done.iter().cloned());
        return;
    }
    let label = bin.display().to_string();
    // A binary that crashed the process last time is skipped; the crash marker stays on disk
    // until this binary has been loaded and described (the guard removes it when dropped).
    let _marker = match crate::blocklist::begin(&key, &label) {
        Ok(g) => g,
        Err(e) => {
            report.errors.push(e);
            return;
        }
    };
    let mut tr = vec![label.clone(), format!("  host shown to the plug-in: {}", crate::suites::host_summary())];
    load_open(bin, bundle, &label, &key, report, &mut tr);
    report.transcript.push(tr.join("\n"));
}

/// The part of [`load_binary`] that runs plug-in code; what happens goes to `tr`.
fn load_open(bin: &Path, bundle: &str, label: &str, key: &Path, report: &mut LoadReport, tr: &mut Vec<String>) {
    let lib = match open_library(bin) {
        Ok(l) => l,
        Err(e) => {
            tr.push(format!("  cannot load the library: {e}"));
            report.errors.push(format!("{label}: cannot load the library: {e}"));
            return;
        }
    };
    // The library stays loaded for the life of the process: plug-ins keep code and state alive.
    let lib: &'static libloading::Library = Box::leak(Box::new(lib));
    // SAFETY: these are the entry points the OFX specification requires of every binary.
    let (count, get): (CountFn, GetPluginFn) = unsafe {
        match (lib.get::<CountFn>(b"OfxGetNumberOfPlugins\0"), lib.get::<GetPluginFn>(b"OfxGetPlugin\0")) {
            (Ok(c), Ok(g)) => (*c, *g),
            _ => {
                tr.push("  OfxGetNumberOfPlugins / OfxGetPlugin are not exported".to_string());
                report.errors.push(format!("{label}: not an OpenFX plug-in (OfxGetNumberOfPlugins / OfxGetPlugin not exported)"));
                return;
            }
        }
    };
    let host = crate::suites::host_name();
    let bin_name = bin.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| label.to_string());
    let bin_diag = PluginDiag::new();
    // `OfxSetHost` is optional (OFX 1.5) and comes first: the header's order is OfxSetHost,
    // OfxGetNumberOfPlugins, OfxGetPlugin. A plug-in that is not for this host answers
    // kOfxStatFailed and the host skips it silently (here: with a note, so the user can tell).
    // SAFETY: optional entry point with the documented shape.
    if let Ok(sh) = unsafe { lib.get::<SetHostFn>(b"OfxSetHost\0") } {
        let sh: SetHostFn = *sh;
        let answer = in_scope(&bin_diag, &bin_name, "OfxSetHost", || std::panic::catch_unwind(|| unsafe { sh(host_ptr()) }));
        match answer {
            Err(_) => tr.push("  OfxSetHost -> the plug-in panicked (ignored)".to_string()),
            // Only a status that says "failed" counts: an older plug-in that declares no return
            // value leaves whatever was in the register.
            Ok(st) if is_failure(st) => {
                tr.push(format!("  OfxSetHost -> {}", status_name(st)));
                report.notes.push(format!(
                    "{label}: declined this host: OfxSetHost returned {} (the plug-in says it is not for this host; the host name it was shown is \"{host}\")",
                    status_name(st)
                ));
                return;
            }
            Ok(K_STAT_OK) => tr.push("  OfxSetHost -> kOfxStatOK (0)".to_string()),
            Ok(st) => tr.push(format!("  OfxSetHost -> {} (not a failure: ignored)", status_name(st))),
        }
    }
    // SAFETY: as above.
    let n_raw = in_scope(&bin_diag, &bin_name, "OfxGetNumberOfPlugins", || std::panic::catch_unwind(|| unsafe { count() })).unwrap_or(0);
    tr.push(format!("  OfxGetNumberOfPlugins -> {n_raw}"));
    if n_raw <= 0 {
        report.notes.push(format!(
            "{label}: exports no OpenFX plug-ins (OfxGetNumberOfPlugins returned {n_raw}); it is not a plug-in for this host, or it declined the host name \"{host}\""
        ));
        return;
    }
    let n = n_raw.clamp(0, 4096);
    let lock = Arc::new(Mutex::new(()));
    let mut here: Vec<LoadedEffect> = Vec::new();
    for i in 0..n {
        // SAFETY: as above.
        let p = in_scope(&bin_diag, &bin_name, "OfxGetPlugin", || std::panic::catch_unwind(|| unsafe { get(i) })).unwrap_or(std::ptr::null_mut());
        if p.is_null() {
            tr.push(format!("  OfxGetPlugin({i}) -> null"));
            report.errors.push(format!("{label}: OfxGetPlugin({i}) returned null"));
            continue;
        }
        match describe_plugin(p, bin, bundle, lock.clone(), tr) {
            Outcome::Registered(e) => here.push(e),
            Outcome::Skipped(note) => report.notes.push(format!("{label}: {note}")),
            Outcome::Failed(e) => report.errors.push(format!("{label}: {e}")),
        }
    }
    report.loaded.extend(here.iter().cloned());
    if !here.is_empty() {
        loaded_cache().lock().unwrap_or_else(PoisonError::into_inner).insert(key.to_path_buf(), here);
    }
}

/// What became of one plug-in.
enum Outcome {
    Registered(LoadedEffect),
    /// Left out on purpose: the note says why.
    Skipped(String),
    /// Could not be made into an effect: the error says why.
    Failed(String),
}

fn short_context(ctx: &str) -> &'static str {
    match ctx {
        CTX_FILTER => "filter",
        CTX_GENERAL => "general",
        CTX_TRANSITION => "transition",
        CTX_GENERATOR => "generator",
        _ => "other",
    }
}

/// The effect id: the OFX identifier lower-cased, invalid characters replaced by `_`.
fn effect_id(ofx_id: &str) -> String {
    let mut id: String = ofx_id.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' }).collect();
    if id.is_empty() {
        id = "unnamed".into();
    }
    if id.starts_with("ec.") {
        id = format!("ofx.{id}");
    }
    id
}

fn pick_comps(listed: &[String]) -> Comps {
    let has = |c: Comps| listed.iter().any(|s| s == c.name());
    if listed.is_empty() || has(Comps::Rgba) {
        Comps::Rgba
    } else if has(Comps::Rgb) {
        Comps::Rgb
    } else if has(Comps::Alpha) {
        Comps::Alpha
    } else {
        Comps::Rgba
    }
}

/// One plug-in's lines in the load transcript. A plug-in that registered cleanly (every action
/// answered OK, nothing posted, nothing missing) collapses to one line, so a suite of hundreds of
/// effects stays readable; anything else keeps the full detail.
#[derive(Default)]
struct Block {
    /// The plug-in identifier (empty until it is known).
    id: String,
    /// `id vMAJOR.MINOR`.
    head: String,
    lines: Vec<String>,
    /// Something happened that the reader may need to see.
    noteworthy: bool,
    /// The last action answered with a failure status (its line already says why).
    action_failed: bool,
}

impl Block {
    /// Add the block to the transcript, closing it with `outcome`. `clean` = the plug-in registered.
    fn finish(self, tr: &mut Vec<String>, diag: &PluginDiag, outcome: &str, clean: bool) {
        if self.head.is_empty() {
            tr.push(format!("  {outcome}"));
        } else if clean && !self.noteworthy {
            tr.push(format!("  {}: {outcome} (Load, Describe and DescribeInContext: OK)", self.head));
        } else {
            tr.push(format!("  {}", self.head));
            tr.extend(self.lines);
            let reads = diag.host_reads();
            if !reads.is_empty() {
                tr.push(format!("    host properties read: {}", reads.join(", ")));
            }
            tr.push(format!("    {outcome}"));
        }
    }
}

/// Run one action of the load sequence for `entry`, adding its status, the plug-in's messages and
/// what it asked for that this host lacks to the block. Returns the status and the
/// [`PluginDiag::mark`] from before the action (for [`PluginEntry::failure`]).
fn run_action(entry: &PluginEntry, block: &mut Block, name: &str, action: &str, handle: *const c_void, inp: Option<&PropSet>) -> (OfxStatus, u64) {
    let mark = entry.diag.mark();
    let t0 = std::time::Instant::now();
    let st = entry.call(action, handle, inp, None);
    block.lines.push(format!("    {name} -> {} ({} ms)", status_name(st), t0.elapsed().as_millis()));
    block.action_failed = !matches!(st, K_STAT_OK | K_STAT_REPLY_DEFAULT);
    block.noteworthy |= block.action_failed;
    for m in entry.diag.messages_since(mark) {
        block.lines.push(format!("      plug-in said ({}): \"{}\"", m.kind, one_line(&m.text, 400)));
        block.noteworthy = true;
    }
    for u in entry.diag.unmet_since(mark) {
        block.lines.push(format!("      asked for {}, which this host does not provide", u.request));
        block.noteworthy = true;
    }
    (st, mark)
}

/// What [`describe_steps`] made of a plug-in.
enum Step {
    Registered(LoadedEffect),
    /// Left out on purpose (a note, without the plug-in's id).
    Skip(String),
    /// Could not be made into an effect (the reason, without the plug-in's id).
    Fail(String),
}

/// Load, describe and register one plug-in; its transcript lines go to `tr`.
fn describe_plugin(ptr: *mut OfxPlugin, bin: &Path, bundle: &str, lock: Arc<Mutex<()>>, tr: &mut Vec<String>) -> Outcome {
    let diag = PluginDiag::new();
    let mut block = Block::default();
    let step = describe_steps(ptr, bin, bundle, lock, &diag, &mut block);
    let id = block.id.clone();
    let prefixed = |s: String| if id.is_empty() { s } else { format!("{id}: {s}") };
    match step {
        Step::Registered(e) => {
            let outcome = format!("registered as effect {} ({} context)", e.id, e.context);
            block.finish(tr, &diag, &outcome, true);
            Outcome::Registered(e)
        }
        Step::Skip(note) => {
            block.finish(tr, &diag, &format!("skipped: {note}"), false);
            Outcome::Skipped(prefixed(format!("skipped, {note}")))
        }
        Step::Fail(why) => {
            // An action that failed already says so in its own line.
            let outcome = if block.action_failed { "not registered".to_string() } else { format!("not registered: {why}") };
            block.finish(tr, &diag, &outcome, false);
            Outcome::Failed(prefixed(why))
        }
    }
}

fn describe_steps(ptr: *mut OfxPlugin, bin: &Path, bundle: &str, lock: Arc<Mutex<()>>, diag: &Arc<PluginDiag>, block: &mut Block) -> Step {
    // SAFETY: a non-null OfxPlugin* from the binary; the struct is static data in the plug-in.
    let p = unsafe { &*ptr };
    let ofx_id = unsafe { str_arg(p.plugin_identifier) }.unwrap_or("").to_string();
    if ofx_id.is_empty() {
        return Step::Fail("a plug-in has no identifier".into());
    }
    block.id = ofx_id.clone();
    block.head = format!("{ofx_id} v{}.{}", p.plugin_version_major, p.plugin_version_minor);
    let api = unsafe { str_arg(p.plugin_api) };
    if api != Some(IMAGE_EFFECT_API) {
        return Step::Skip(format!("it is not an OpenFX image effect (plug-in API \"{}\")", api.unwrap_or("?")));
    }
    if p.api_version != 1 {
        return Step::Fail(format!("unsupported image-effect API version {}", p.api_version));
    }
    let (Some(set_host), Some(main)) = (p.set_host, p.main_entry) else {
        return Step::Fail("missing setHost or mainEntry".into());
    };
    // SAFETY: the plug-in's setHost with our process-lifetime host block.
    let set = in_scope(diag, &ofx_id, "setHost", || std::panic::catch_unwind(|| unsafe { set_host(host_ptr()) }));
    if set.is_err() {
        return Step::Fail("setHost panicked".into());
    }
    let entry = Arc::new(PluginEntry { main, ofx_id: ofx_id.clone(), lock, diag: diag.clone() });
    let host = crate::suites::host_name();

    let (st, mark) = run_action(&entry, block, "Load", ACT_LOAD, std::ptr::null(), None);
    match st {
        K_STAT_OK | K_STAT_REPLY_DEFAULT => {}
        K_STAT_FAILED => {
            let hint = format!("the plug-in declined to load (a licence or host check? it was shown the host name \"{host}\")");
            return Step::Fail(entry.failure_hint("Load", st, mark, Some(&hint)));
        }
        s => return Step::Fail(entry.failure("Load", s, mark)),
    }

    let desc = EffectObj::new_descriptor();
    desc.props.put_ptr(IE_PLUGIN_HANDLE, ptr as usize);
    // The bundle folder itself (plug-ins append `Contents/Resources/...`); unset when no bundle is known.
    if !bundle.is_empty() && bundle != bin.to_string_lossy() {
        desc.props.put_str(P_PLUGIN_FILE_PATH, bundle);
    }
    let (st, mark) = run_action(&entry, block, "Describe", ACT_DESCRIBE, desc.handle(), None);
    if st != K_STAT_OK {
        return Step::Fail(entry.failure("Describe", st, mark));
    }

    let contexts = desc.props.strs(IE_SUPPORTED_CONTEXTS);
    let Some(context) = [CTX_FILTER, CTX_GENERAL, CTX_TRANSITION, CTX_GENERATOR].into_iter().find(|c| contexts.iter().any(|s| s == c)) else {
        return Step::Fail(format!("no supported context (needs filter, general, transition or generator; it lists {contexts:?})"));
    };

    let depths = desc.props.strs(IE_SUPPORTED_PIXEL_DEPTHS);
    let depth = if depths.is_empty() {
        Depth::Float
    } else {
        match [Depth::Float, Depth::Short, Depth::Byte].into_iter().find(|d| depths.iter().any(|s| s == d.name())) {
            Some(d) => d,
            None => {
                return Step::Fail(format!("needs an unsupported pixel depth {depths:?} (this host offers {DEPTH_FLOAT}, {DEPTH_SHORT} and {DEPTH_BYTE})"));
            }
        }
    };
    let safety = match desc.props.str(IE_RENDER_THREAD_SAFETY).as_deref() {
        Some(IE_RENDER_UNSAFE) => Safety::Unsafe,
        Some(IE_RENDER_FULLY_SAFE) => Safety::FullySafe,
        _ => Safety::InstanceSafe,
    };

    let inp = PropSet::new();
    inp.put_str(IE_CONTEXT, context);
    let (st, mark) = run_action(&entry, block, "DescribeInContext", ACT_DESCRIBE_IN_CONTEXT, desc.handle(), Some(&inp));
    if st != K_STAT_OK {
        return Step::Fail(entry.failure(&format!("DescribeInContext ({})", short_context(context)), st, mark));
    }

    // Clips: Output; the effect's own layer; everything else becomes a Layer parameter.
    let input_name = if context == CTX_TRANSITION { CLIP_SOURCE_FROM } else { CLIP_SOURCE };
    let mut clip_infos: Vec<(String, Comps, bool)> = Vec::new();
    desc.each_clip(|c| clip_infos.push((c.name.clone(), pick_comps(&c.props.strs(IE_SUPPORTED_COMPONENTS)), c.props.flag(CLIP_OPTIONAL, false))));
    if !clip_infos.iter().any(|(n, ..)| n == CLIP_OUTPUT) {
        return Step::Fail("describes no Output clip".into());
    }
    let layer_clips: Vec<String> = clip_infos.iter().filter(|(n, ..)| n != CLIP_OUTPUT && n != input_name).map(|(n, ..)| n.clone()).collect();
    let page_order = desc.props.strs(IE_PARAM_PAGE_ORDER);
    let mapped = map_params(&desc.params, &page_order, &layer_clips);

    let clips: Vec<ClipCfg> = clip_infos
        .into_iter()
        .map(|(name, comps, optional)| {
            let role = if name == CLIP_OUTPUT {
                ClipRole::Output
            } else if name == input_name {
                ClipRole::Input
            } else {
                let pid = mapped.bindings.layers.iter().find(|l| l.clip == name).map(|l| l.param_id.clone()).unwrap_or_else(|| name.clone());
                ClipRole::Layer(pid)
            };
            ClipCfg { name, role, comps, optional }
        })
        .collect();
    let cfg = Arc::new(EffectCfg { context: context.to_string(), clips, depth, safety, slave_params: desc.props.strs(IE_CLIP_PREFS_SLAVE_PARAM) });

    let name = desc.props.str(P_LABEL).filter(|s| !s.trim().is_empty()).unwrap_or_else(|| ofx_id.clone());
    let category = desc
        .props
        .str(IE_GROUPING)
        .and_then(|g| g.split('/').map(str::trim).find(|s| !s.is_empty()).map(str::to_string))
        .unwrap_or_else(|| "OpenFX".to_string());
    let id = effect_id(&ofx_id);
    let manifest = PluginManifest {
        api: PLUGIN_API_VERSION,
        id: id.clone(),
        name: name.clone(),
        category: category.clone(),
        version: format!("{}.{}", p.plugin_version_major, p.plugin_version_minor),
        author: String::new(),
        description: desc.props.str(P_PLUGIN_DESCRIPTION).unwrap_or_default(),
        params: mapped.params,
    };
    let source = bin.to_string_lossy().into_owned();
    let effect = OfxEffect::new(manifest, entry, cfg, desc, mapped.bindings, source);
    if let Err(e) = register_plugin(Arc::new(effect)) {
        return Step::Fail(e);
    }
    Step::Registered(LoadedEffect { id, name, category, ofx_id, context: short_context(context).to_string(), bundle: bundle.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_of_finds_the_bundle_folder_of_a_bundled_binary() {
        // The platform directory can have any name, and the bundle name is matched case-insensitively.
        for (bin, bundle) in [
            ("C:/OFX/Plugins/Foo.ofx.bundle/Contents/Win64/Foo.ofx", "C:/OFX/Plugins/Foo.ofx.bundle"),
            ("/Library/OFX/Plugins/Foo.ofx.bundle/Contents/MacOS/Foo.ofx", "/Library/OFX/Plugins/Foo.ofx.bundle"),
            ("/usr/OFX/Plugins/Foo.ofx.bundle/Contents/Linux-x86-64/Foo.ofx", "/usr/OFX/Plugins/Foo.ofx.bundle"),
            ("/x/Foo.OFX.BUNDLE/contents/anything/Foo.ofx", "/x/Foo.OFX.BUNDLE"),
        ] {
            assert_eq!(bundle_of(Path::new(bin)), Some(PathBuf::from(bundle)), "{bin}");
        }
    }

    #[test]
    fn bundle_of_is_none_outside_a_bundle() {
        // A loose binary, a binary in a plain folder, and a binary whose parents are not the bundle layout.
        assert_eq!(bundle_of(Path::new("Foo.ofx")), None);
        assert_eq!(bundle_of(Path::new("/opt/plugins/Foo.ofx")), None);
        assert_eq!(bundle_of(Path::new("/opt/Foo/Contents/Win64/Foo.ofx")), None);
        assert_eq!(bundle_of(Path::new("/opt/Foo.ofx.bundle/Win64/Foo.ofx")), None);
        assert_eq!(bundle_of(Path::new("/opt/Foo.ofx.bundle/Contents/Foo.ofx")), None);
        assert_eq!(bundle_of(Path::new("/opt/Foo.ofx.bundle/Contents/Win64/Sub/Foo.ofx")), None);
        assert_eq!(bundle_of(Path::new("")), None);
    }

    #[test]
    fn effect_id_is_the_lower_cased_ofx_id() {
        assert_eq!(effect_id("org.effectcraft.test.Invert"), "org.effectcraft.test.invert");
        assert_eq!(effect_id("com.Boris-FX.Sapphire_Blur"), "com.boris-fx.sapphire_blur");
    }

    #[test]
    fn effect_id_replaces_invalid_characters() {
        assert_eq!(effect_id("a b/c:d"), "a_b_c_d");
        assert_eq!(effect_id("x\u{e9}y"), "x_y");
    }

    #[test]
    fn effect_id_prefixes_ec_namespace_and_names_empty_ids() {
        assert_eq!(effect_id("ec.blur"), "ofx.ec.blur");
        assert_eq!(effect_id("EC.Blur"), "ofx.ec.blur");
        assert_eq!(effect_id("ecx.blur"), "ecx.blur");
        assert_eq!(effect_id(""), "unnamed");
    }
}

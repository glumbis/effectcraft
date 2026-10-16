//! Finding, loading and describing OFX plug-in binaries, and registering them as effects.
//!
//! Bundle layout: `X.ofx.bundle/Contents/{Win64,MacOS,Linux-x86-64}/X.ofx`. A binary exports
//! `OfxGetNumberOfPlugins` and `OfxGetPlugin` (and optionally `OfxSetHost`). For each plug-in the
//! loader runs `Load`, `Describe` and `DescribeInContext` for the preferred context
//! (Filter, General, Transition, Generator), maps the parameters to a manifest and registers an
//! [`OfxEffect`]. Libraries stay loaded for the life of the process.
//!
//! With the scan cache ([`crate::cache`]) a binary whose file did not change is not loaded at all:
//! its plug-ins register from the stored description as *lazy* effects, and [`LazyBinary`] loads
//! the library, runs the load sequence again and compares the result to the stored description the
//! first time one of them needs an instance.

use std::collections::{BTreeMap, HashMap};
use std::ffi::{c_int, c_void};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use effectcraft_effects::plugin::{PLUGIN_API_VERSION, PluginManifest};

use crate::cache::{self, BinaryEntry, PluginRecord, Registration};
use crate::clips::{Comps, Depth};
use crate::diag::{CallCtx, PluginDiag, is_failure, one_line, status_name};
use crate::effect::{Live, OfxEffect};
use crate::ffi::*;
use crate::instance::{ClipCfg, ClipRole, EffectCfg, EffectObj, PluginEntry, Safety};
use crate::params::map_params;
use crate::props::PropSet;
use crate::suites::host_ptr;
use crate::{LoadReport, LoadedEffect};

/// The directories inside a bundle's `Contents` that may hold this platform's binary, best first.
#[cfg(all(windows, target_arch = "x86_64"))]
const PLATFORM_DIRS: &[&str] = &["Win64"];
#[cfg(all(windows, not(target_arch = "x86_64")))]
const PLATFORM_DIRS: &[&str] = &["Win32"];
#[cfg(target_os = "macos")]
const PLATFORM_DIRS: &[&str] = &["MacOS"];
#[cfg(all(not(windows), not(target_os = "macos"), target_arch = "x86_64"))]
const PLATFORM_DIRS: &[&str] = &["Linux-x86-64"];
// The OFX packaging reference only names Linux arm64 informally (`Linux-$(uname -m)`, i.e.
// `Linux-aarch64`); bundles are also seen with `Linux-arm-64`. Both are tried.
#[cfg(all(not(windows), not(target_os = "macos"), not(target_arch = "x86_64")))]
const PLATFORM_DIRS: &[&str] = &["Linux-aarch64", "Linux-arm-64"];

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

/// How many plug-in libraries this process has opened (each binary at most once): a start from the
/// scan cache opens none until an effect is used.
static LIBRARY_LOADS: AtomicUsize = AtomicUsize::new(0);

/// The number of plug-in libraries this process has loaded so far.
pub fn library_loads() -> usize {
    LIBRARY_LOADS.load(Ordering::Relaxed)
}

/// Load an `.ofx` binary, an `.ofx.bundle` folder, or every bundle below a folder. Binaries that
/// did not change since the last scan register from the scan cache (see [`crate::cache`]) when
/// there is one; use [`load_path_with`] to load everything.
pub fn load_path(path: &Path) -> LoadReport {
    load_path_with(path, true)
}

/// [`load_path`]; `use_cache = false` loads every binary even when the scan cache knows it (an
/// explicit load of a plug-in the user just fixed), still updating the cache.
pub fn load_path_with(path: &Path, use_cache: bool) -> LoadReport {
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
    let mut cache = cache::Run::begin(use_cache, &crate::suites::host_name());
    // Phase one lists every binary's plug-ins (a binary in the scan cache is not loaded: its
    // listing is the cached one), so that copies of one plug-in can be told apart before any of
    // them is described; phase two describes what is kept.
    let mut units: Vec<Unit> = Vec::new();
    for (bin, bundle) in &binaries {
        let key = std::fs::canonicalize(bin).unwrap_or_else(|_| bin.clone());
        let label = bin.display().to_string();
        let known = loaded_cache().lock().unwrap_or_else(PoisonError::into_inner).get(&key).cloned();
        if let Some(done) = known {
            report.loaded.extend(done);
            continue;
        }
        if cache.enabled()
            && let Some(entry) = cache.lookup(&key, &label)
        {
            // A cached binary is skipped for the same reason a loaded one would be.
            if let Err(e) = crate::blocklist::check(&key, &label) {
                report.errors.push(e);
                continue;
            }
            let skips = vec![None; entry.plugins.len()];
            units.push(Unit::Cached(Cached { bin: bin.clone(), bundle: bundle.clone(), label, key, entry, skips }));
            continue;
        }
        if let Some(p) = probe_binary(bin, bundle, &mut report) {
            units.push(Unit::Probed(p));
        }
    }
    mark_duplicates(&mut units, &mut report.notes);
    for u in units {
        match u {
            Unit::Probed(p) => describe_binary(p, &mut report, &mut cache),
            Unit::Cached(c) => register_cached(c, &mut report),
        }
    }
    report.cache = cache.finish();
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
    bundle_binary_in(bundle, PLATFORM_DIRS)
}

/// The binary inside a bundle: `<stem>.ofx` in the first of `dirs` (under `Contents`) that has
/// one, else any `.ofx` file in the first that has one.
fn bundle_binary_in(bundle: &Path, dirs: &[&str]) -> Option<PathBuf> {
    let name = bundle.file_name()?.to_str()?;
    let stem = name.get(..name.len().checked_sub(".ofx.bundle".len())?)?;
    dirs.iter().find_map(|d| {
        let dir = bundle.join("Contents").join(d);
        let exact = dir.join(format!("{stem}.ofx"));
        if exact.is_file() {
            return Some(exact);
        }
        std::fs::read_dir(&dir).ok()?.filter_map(|e| e.ok()).map(|e| e.path()).find(|p| p.is_file() && is_ofx_file(p))
    })
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
            None => errors.push(format!("{}: no {} binary in this bundle", path.display(), PLATFORM_DIRS.join(" or "))),
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
    LIBRARY_LOADS.fetch_add(1, Ordering::Relaxed);
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

/// A binary after the probe: loaded, offered the host, its plug-ins listed, nothing described yet.
struct Probed {
    bin: PathBuf,
    bundle: String,
    label: String,
    key: PathBuf,
    /// The binary's load transcript so far (the describe phase adds the plug-ins).
    tr: Vec<String>,
    /// Shared by the plug-ins of this library that declare themselves render-thread-unsafe.
    lock: Arc<Mutex<()>>,
    plugins: Vec<Listed>,
}

/// A binary the scan cache knows: its plug-ins register from the stored description without the
/// library being loaded.
struct Cached {
    bin: PathBuf,
    bundle: String,
    label: String,
    key: PathBuf,
    entry: Arc<BinaryEntry>,
    /// Per plug-in of the entry: why it is not registered (another copy of it is kept).
    skips: Vec<Option<String>>,
}

/// A binary of one scan: freshly probed, or known to the scan cache.
enum Unit {
    Probed(Probed),
    Cached(Cached),
}

impl Unit {
    fn label(&self) -> &str {
        match self {
            Unit::Probed(p) => &p.label,
            Unit::Cached(c) => &c.label,
        }
    }

    /// The plug-ins the binary offers, in the order it lists them.
    fn offers(&self) -> Vec<Offer> {
        match self {
            Unit::Probed(p) => p.plugins.iter().map(|l| Offer { id: l.id.clone(), version: l.version }).collect(),
            Unit::Cached(c) => c.entry.plugins.iter().map(|r| Offer { id: r.id().to_string(), version: r.version() }).collect(),
        }
    }

    /// Mark plug-in `j` as not to be registered, because of `why`.
    fn skip(&mut self, j: usize, why: String) {
        match self {
            Unit::Probed(p) => {
                if let Some(l) = p.plugins.get_mut(j) {
                    l.skip = Some(why);
                }
            }
            Unit::Cached(c) => {
                if let Some(s) = c.skips.get_mut(j) {
                    *s = Some(why);
                }
            }
        }
    }
}

/// One plug-in a binary lists through `OfxGetPlugin`.
struct Listed {
    ptr: *mut OfxPlugin,
    id: String,
    version: (u32, u32),
    /// Why it is not described: another copy of the plug-in is kept.
    skip: Option<String>,
}

/// The plug-in identifiers registered so far in this process, with their versions: a later
/// `load_path` call cannot replace one that is already registered.
fn registered_ids() -> &'static Mutex<HashMap<String, (u32, u32)>> {
    static R: OnceLock<Mutex<HashMap<String, (u32, u32)>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Phase one of loading a binary: open it, offer it the host (`OfxSetHost`), ask how many plug-ins
/// it has and list them. Nothing is described (no `Load`), so copies of one plug-in (the alpha,
/// beta and release folders some vendors leave behind) can be told apart before any of their code
/// runs. `None`: nothing to describe (already loaded, declined the host, failed, no plug-ins).
fn probe_binary(bin: &Path, bundle: &str, report: &mut LoadReport) -> Option<Probed> {
    let key = std::fs::canonicalize(bin).unwrap_or_else(|_| bin.to_path_buf());
    if let Some(done) = loaded_cache().lock().unwrap_or_else(PoisonError::into_inner).get(&key) {
        report.loaded.extend(done.iter().cloned());
        return None;
    }
    let label = bin.display().to_string();
    // A binary that crashed the process last time is skipped; the crash marker stays on disk
    // until this binary has been loaded and described (the guard removes it when dropped). The
    // probe keeps the binary's earlier strikes: only a finished describe phase clears them.
    let mut marker = match crate::blocklist::begin_phase(&key, &label, false) {
        Ok(g) => g,
        Err(e) => {
            report.errors.push(e);
            return None;
        }
    };
    let mut tr = vec![label.clone(), format!("  host shown to the plug-in: {}", crate::suites::host_summary())];
    match probe_open(bin, &label, report, &mut tr) {
        Some(plugins) => Some(Probed { bin: bin.to_path_buf(), bundle: bundle.to_string(), label, key, tr, lock: Arc::new(Mutex::new(())), plugins }),
        None => {
            // Over without a crash: nothing more will be described for this binary.
            marker.forgive();
            report.transcript.push(tr.join("\n"));
            None
        }
    }
}

/// The part of [`probe_binary`] that runs plug-in code; what happens goes to `tr`.
fn probe_open(bin: &Path, label: &str, report: &mut LoadReport, tr: &mut Vec<String>) -> Option<Vec<Listed>> {
    let lib = match open_library(bin) {
        Ok(l) => l,
        Err(e) => {
            tr.push(format!("  cannot load the library: {e}"));
            report.errors.push(format!("{label}: cannot load the library: {e}"));
            return None;
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
                return None;
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
                return None;
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
        return None;
    }
    let mut plugins = Vec::new();
    for i in 0..n_raw.clamp(0, 4096) {
        // SAFETY: as above.
        let p = in_scope(&bin_diag, &bin_name, "OfxGetPlugin", || std::panic::catch_unwind(|| unsafe { get(i) })).unwrap_or(std::ptr::null_mut());
        if p.is_null() {
            tr.push(format!("  OfxGetPlugin({i}) -> null"));
            report.errors.push(format!("{label}: OfxGetPlugin({i}) returned null"));
            continue;
        }
        // SAFETY: a non-null OfxPlugin* from the binary; the struct is static data in the plug-in.
        let pl = unsafe { &*p };
        let id = unsafe { str_arg(pl.plugin_identifier) }.unwrap_or("").to_string();
        plugins.push(Listed { ptr: p, id, version: (pl.plugin_version_major, pl.plugin_version_minor), skip: None });
    }
    Some(plugins)
}

/// A plug-in offered by a binary, for [`pick_duplicates`].
#[derive(Clone, Debug, PartialEq, Eq)]
struct Offer {
    id: String,
    version: (u32, u32),
}

/// Who keeps a plug-in identifier that several copies offer.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Winner {
    /// Another binary of this scan (its index and the version it offers).
    Binary(usize, (u32, u32)),
    /// An earlier scan registered it already (and registrations cannot be replaced).
    Registered((u32, u32)),
}

/// Which offers lose because the same plug-in identifier is offered more than once. `offers[i]`
/// are the offers of binary `i` in scan order; `claimed` are the identifiers registered by earlier
/// scans. The highest version wins; among equal versions the copy found first. Result: (binary
/// index, offer index) of every loser, with the winner.
fn pick_duplicates(offers: &[Vec<Offer>], claimed: &HashMap<String, (u32, u32)>) -> BTreeMap<(usize, usize), Winner> {
    // (binary index, offer index, version) of every offer of an identifier, in scan order.
    type Candidate = (usize, usize, (u32, u32));
    let mut by_id: BTreeMap<&str, Vec<Candidate>> = BTreeMap::new();
    for (i, list) in offers.iter().enumerate() {
        for (j, o) in list.iter().enumerate() {
            by_id.entry(o.id.as_str()).or_default().push((i, j, o.version));
        }
    }
    let mut losers = BTreeMap::new();
    for (id, candidates) in by_id {
        if let Some(&v) = claimed.get(id) {
            for (i, j, _) in candidates {
                losers.insert((i, j), Winner::Registered(v));
            }
            continue;
        }
        if candidates.len() < 2 {
            continue;
        }
        // `reduce` keeps the earlier candidate unless a later one has a higher version.
        let Some(best) = candidates.iter().copied().reduce(|a, b| if b.2 > a.2 { b } else { a }) else { continue };
        for (i, j, _) in candidates {
            if (i, j) != (best.0, best.1) {
                losers.insert((i, j), Winner::Binary(best.0, best.2));
            }
        }
    }
    losers
}

/// Mark the plug-ins that lose to another copy of themselves, and say so in a note per binary.
/// Users end up with alpha, beta and release bundles of one product side by side in the OFX
/// folder (Topaz documents crashes from the older ones in Nuke); the newest is the one that runs.
fn mark_duplicates(probed: &mut [Unit], notes: &mut Vec<String>) {
    let offers: Vec<Vec<Offer>> = probed.iter().map(Unit::offers).collect();
    let claimed = registered_ids().lock().unwrap_or_else(PoisonError::into_inner).clone();
    let losers = pick_duplicates(&offers, &claimed);
    let version = |v: (u32, u32)| format!("{}.{}", v.0, v.1);
    let mut per_binary: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for (&(i, j), winner) in &losers {
        let why = match winner {
            Winner::Binary(b, v) => {
                let winner_label = probed.get(*b).map(Unit::label).unwrap_or("another copy");
                (
                    format!("duplicate: {winner_label} offers version {} of it", version(*v)),
                    format!("{} (this copy {}, kept {} in {winner_label})", offers[i][j].id, version(offers[i][j].version), version(*v)),
                )
            }
            Winner::Registered(v) => (
                format!("duplicate: it is already registered (version {})", version(*v)),
                format!("{} (this copy {}, already registered {})", offers[i][j].id, version(offers[i][j].version), version(*v)),
            ),
        };
        if let Some(u) = probed.get_mut(i) {
            u.skip(j, why.0);
        }
        per_binary.entry(i).or_default().push(why.1);
    }
    for (i, list) in per_binary {
        let Some(p) = probed.get(i) else { continue };
        let shown: Vec<&str> = list.iter().take(3).map(String::as_str).collect();
        let more = if list.len() > 3 { format!(" and {} more", list.len() - 3) } else { String::new() };
        notes.push(format!("{}: {} duplicate plug-in(s) skipped, the newest copy is kept: {}{more}", p.label(), list.len(), shown.join("; ")));
    }
}

/// The effect the report lists for a registered plug-in.
fn loaded_effect(reg: &Registration, bundle: &str) -> LoadedEffect {
    LoadedEffect {
        id: reg.manifest.id.clone(),
        name: reg.manifest.name.clone(),
        category: reg.manifest.category.clone(),
        ofx_id: reg.ofx_id.clone(),
        context: reg.context.clone(),
        bundle: bundle.to_string(),
    }
}

/// Phase two: describe and register the plug-ins of a probed binary that were not marked as
/// duplicates, and remember the result in the scan cache.
fn describe_binary(p: Probed, report: &mut LoadReport, cache: &mut cache::Run) {
    let Probed { bin, bundle, label, key, mut tr, lock, plugins } = p;
    let _marker = match crate::blocklist::begin_phase(&key, &label, true) {
        Ok(g) => g,
        Err(e) => {
            report.errors.push(e);
            report.transcript.push(tr.join("\n"));
            return;
        }
    };
    let mut here: Vec<LoadedEffect> = Vec::new();
    let mut records: Vec<PluginRecord> = Vec::new();
    // A binary with a plug-in skipped as a duplicate is not cached: that plug-in was never described.
    let mut cacheable = true;
    let (mut withheld, crash_stamp) = (0usize, crate::inflight::binary_stamp(&bin));
    for l in &plugins {
        if let Some(why) = &l.skip {
            tr.push(format!("  {} v{}.{}: skipped, {why}", l.id, l.version.0, l.version.1));
            cacheable = false;
            continue;
        }
        match describe_plugin(l.ptr, &bin, &bundle, lock.clone(), crash_stamp, &mut tr) {
            Outcome::Registered(reg) => {
                registered_ids().lock().unwrap_or_else(PoisonError::into_inner).insert(l.id.clone(), l.version);
                report.notes.extend(reg.notes.iter().map(|n| format!("{label}: {n}")));
                here.push(loaded_effect(&reg, &bundle));
                records.push(PluginRecord::Registered { reg });
            }
            Outcome::Skipped(note) => {
                report.notes.push(format!("{label}: {note}"));
                records.push(PluginRecord::Skipped { id: l.id.clone(), version: l.version, note });
            }
            Outcome::Failed(error) => {
                report.errors.push(format!("{label}: {error}"));
                records.push(PluginRecord::Failed { id: l.id.clone(), version: l.version, error });
            }
            Outcome::Withheld(reg, why) => {
                report.errors.push(format!("{label}: {}: {why}", l.id));
                withheld += 1;
                records.push(PluginRecord::Registered { reg });
            }
        }
    }
    report.loaded.extend(here.iter().cloned());
    if cacheable && (!here.is_empty() || withheld > 0) {
        match cache::file_stamp(&bin) {
            Some((size, mtime)) => cache.store(BinaryEntry {
                path: key.to_string_lossy().into_owned(),
                bundle: bundle.clone(),
                size,
                mtime,
                stale: None,
                transcript: tr.iter().skip(1).cloned().collect(),
                plugins: records,
            }),
            None => cache.forget(&key),
        }
    } else {
        cache.forget(&key);
    }
    if !here.is_empty() {
        loaded_cache().lock().unwrap_or_else(PoisonError::into_inner).insert(key, here);
    }
    report.transcript.push(tr.join("\n"));
}

/// Register the plug-ins of a binary the scan cache knows, as lazy effects: the library is not
/// loaded until one of them needs an instance ([`LazyBinary`]).
fn register_cached(c: Cached, report: &mut LoadReport) {
    let Cached { bin, bundle, label, key, entry, skips } = c;
    let lazy = Arc::new(LazyBinary::new(&bin, &bundle, &label, &key));
    let source = bin.to_string_lossy().into_owned();
    let mut tr = vec![label.clone(), "  from the scan cache: the library is not loaded until one of its effects is first used".to_string()];
    tr.extend(entry.transcript.iter().cloned());
    let mut here: Vec<LoadedEffect> = Vec::new();
    let crash_stamp = crate::inflight::binary_stamp(&bin);
    for (rec, skip) in entry.plugins.iter().zip(&skips) {
        if let Some(why) = skip {
            tr.push(format!("  {} v{}.{}: skipped, {why}", rec.id(), rec.version().0, rec.version().1));
            continue;
        }
        match rec {
            PluginRecord::Registered { reg } if crate::inflight::skip_reason(&reg.ofx_id, crash_stamp).is_some() => {
                let why = crate::inflight::skip_reason(&reg.ofx_id, crash_stamp).unwrap_or_default();
                tr.push(format!("  {} v{}.{}: not registered: {why}", reg.ofx_id, reg.version.0, reg.version.1));
                report.errors.push(format!("{label}: {}: {why}", reg.ofx_id));
            }
            PluginRecord::Registered { reg } => match OfxEffect::register(OfxEffect::lazy((**reg).clone(), lazy.clone(), source.clone())) {
                Ok(()) => {
                    registered_ids().lock().unwrap_or_else(PoisonError::into_inner).insert(reg.ofx_id.clone(), reg.version);
                    report.notes.extend(reg.notes.iter().map(|n| format!("{label}: {n}")));
                    here.push(loaded_effect(reg, &bundle));
                }
                Err(e) => {
                    tr.push(format!("  {}: not registered: {e}", reg.ofx_id));
                    report.errors.push(format!("{label}: {}: {e}", reg.ofx_id));
                }
            },
            PluginRecord::Failed { error, .. } => report.errors.push(format!("{label}: {error}")),
            PluginRecord::Skipped { note, .. } => report.notes.push(format!("{label}: {note}")),
        }
    }
    report.loaded.extend(here.iter().cloned());
    if !here.is_empty() {
        loaded_cache().lock().unwrap_or_else(PoisonError::into_inner).insert(key, here);
    }
    report.transcript.push(tr.join("\n"));
}

/// A plug-in binary that registered from the scan cache and has not been loaded yet. It is shared
/// by the effects of the binary: the first one that needs an instance opens the library, and each
/// effect then runs its own `Load`, `Describe` and `DescribeInContext`.
pub(crate) struct LazyBinary {
    bin: PathBuf,
    bundle: String,
    label: String,
    key: PathBuf,
    /// Shared by the plug-ins of this library that declare themselves render-thread-unsafe.
    lock: Arc<Mutex<()>>,
    /// The library once opened: its plug-ins' identifiers and `OfxPlugin` addresses, or why it
    /// could not be (`None` until the first lazy load).
    opened: Mutex<Option<Result<Vec<(String, usize)>, String>>>,
}

impl LazyBinary {
    fn new(bin: &Path, bundle: &str, label: &str, key: &Path) -> LazyBinary {
        LazyBinary {
            bin: bin.to_path_buf(),
            bundle: bundle.to_string(),
            label: label.to_string(),
            key: key.to_path_buf(),
            lock: Arc::new(Mutex::new(())),
            opened: Mutex::new(None),
        }
    }

    /// Load the plug-in `reg` describes: open the library (the first time), run the load sequence
    /// and check the plug-in still describes itself as the cache said. Crashes while doing it count
    /// against the binary like any load crash (the marker is written before and removed after).
    /// A failure marks the cache entry out of date, so the next start scans the binary again.
    pub(crate) fn load(&self, reg: &Registration) -> Result<Live, String> {
        // One load at a time in the process (the crash marker is one file per process).
        let _guard = load_lock().lock().unwrap_or_else(PoisonError::into_inner);
        let _marker = crate::blocklist::begin_phase(&self.key, &self.label, true)?;
        let t0 = std::time::Instant::now();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.load_inner(reg))).unwrap_or_else(|_| Err("loading it panicked".to_string()));
        match &r {
            Ok(_) => log::info!("openfx: loaded {} on first use ({} ms)", reg.ofx_id, t0.elapsed().as_millis()),
            Err(why) => {
                log::warn!("openfx: {}: {why}; its scan cache entry is marked out of date and the binary is scanned again at the next start", reg.ofx_id);
                cache::mark_stale(&self.key, &format!("{}: {why}", reg.ofx_id));
            }
        }
        r
    }

    fn load_inner(&self, reg: &Registration) -> Result<Live, String> {
        let ptr = self.plugin_ptr(&reg.ofx_id)?;
        let diag = PluginDiag::new();
        let mut block = Block::default();
        // The address of an `OfxPlugin` the library listed: static data in the library, which stays
        // loaded for the life of the process.
        match describe_core(ptr as *mut OfxPlugin, &self.bin, &self.bundle, self.lock.clone(), &diag, &mut block) {
            Step::Registered(d) => match reg.difference(&d.reg) {
                None => Ok(Live { entry: d.entry, desc: d.desc }),
                Some(diff) => Err(format!("it no longer matches its description in the scan cache ({diff})")),
            },
            Step::Skip(note) => Err(format!("it no longer describes an effect ({note})")),
            Step::Fail(why) => Err(why),
        }
    }

    /// The address of the `OfxPlugin` of `id`, opening the library if that has not happened yet.
    fn plugin_ptr(&self, id: &str) -> Result<usize, String> {
        let mut opened = self.opened.lock().unwrap_or_else(PoisonError::into_inner);
        if opened.is_none() {
            let (mut scratch, mut tr) = (LoadReport::default(), Vec::new());
            *opened = Some(match probe_open(&self.bin, &self.label, &mut scratch, &mut tr) {
                Some(list) => Ok(list.into_iter().map(|l| (l.id, l.ptr as usize)).collect()),
                None => {
                    Err(scratch.errors.into_iter().chain(scratch.notes).next().unwrap_or_else(|| format!("{}: the library could not be opened", self.label)))
                }
            });
        }
        match opened.as_ref() {
            Some(Ok(list)) => list.iter().find(|(i, _)| i == id).map(|(_, p)| *p).ok_or_else(|| format!("the library no longer lists the plug-in {id}")),
            Some(Err(e)) => Err(e.clone()),
            None => Err("the library is not open".to_string()),
        }
    }
}

/// What became of one plug-in.
enum Outcome {
    /// Registered; the registration notes say how it was described (a fallback context, a
    /// guessed input), prefixed with the plug-in's identifier.
    Registered(Box<Registration>),
    /// Left out on purpose: the note says why.
    Skipped(String),
    /// Could not be made into an effect: the error says why.
    Failed(String),
    /// Described, but not registered because it crashed EffectCraft while rendering twice in a row
    /// (the reason is the error for the report). The cache still keeps the description, so the
    /// plug-in returns as soon as its strikes are gone.
    Withheld(Box<Registration>, String),
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

/// A plug-in after `Load`, `Describe` and `DescribeInContext`: what registers (and the cache
/// keeps) and the live parts a render needs.
struct Described {
    reg: Registration,
    entry: Arc<PluginEntry>,
    desc: Box<EffectObj>,
}

/// What [`describe_core`] made of a plug-in.
enum Step {
    /// Described.
    Registered(Box<Described>),
    /// Left out on purpose (a note, without the plug-in's id).
    Skip(String),
    /// Could not be made into an effect (the reason, without the plug-in's id).
    Fail(String),
}

/// Load, describe and register one plug-in; its transcript lines go to `tr`.
fn describe_plugin(ptr: *mut OfxPlugin, bin: &Path, bundle: &str, lock: Arc<Mutex<()>>, crash_stamp: u64, tr: &mut Vec<String>) -> Outcome {
    /// The result once the effect is registered (a plug-in loaded here is loaded eagerly: it was just described).
    enum Done {
        Registered(Registration),
        Withheld(Registration, String),
        Skip(String),
        Fail(String),
    }
    let diag = PluginDiag::new();
    let mut block = Block::default();
    let done = match describe_core(ptr, bin, bundle, lock, &diag, &mut block) {
        Step::Registered(d) => {
            let Described { reg, entry, desc } = *d;
            // Crashed while rendering twice in a row: described (for the cache) but not registered.
            if let Some(why) = crate::inflight::skip_reason(&reg.ofx_id, crash_stamp) {
                Done::Withheld(reg, why)
            } else {
                let effect = OfxEffect::eager(reg.clone(), Live { entry, desc }, bin.to_string_lossy().into_owned());
                match OfxEffect::register(effect) {
                    Ok(()) => Done::Registered(reg),
                    Err(e) => Done::Fail(e),
                }
            }
        }
        Step::Skip(note) => Done::Skip(note),
        Step::Fail(why) => Done::Fail(why),
    };
    let id = block.id.clone();
    let prefixed = |s: String| if id.is_empty() { s } else { format!("{id}: {s}") };
    match done {
        Done::Registered(mut reg) => {
            let outcome = format!("registered as effect {} ({} context)", reg.manifest.id, reg.context);
            // A fallback or a guess is worth the full lines (a failed attempt's status and message).
            block.noteworthy |= !reg.notes.is_empty();
            block.finish(tr, &diag, &outcome, true);
            reg.notes = std::mem::take(&mut reg.notes).into_iter().map(prefixed).collect();
            Outcome::Registered(Box::new(reg))
        }
        Done::Withheld(mut reg, why) => {
            block.finish(tr, &diag, &format!("not registered: {why}"), false);
            reg.notes = std::mem::take(&mut reg.notes).into_iter().map(prefixed).collect();
            Outcome::Withheld(Box::new(reg), why)
        }
        Done::Skip(note) => {
            block.finish(tr, &diag, &format!("skipped: {note}"), false);
            Outcome::Skipped(prefixed(format!("skipped, {note}")))
        }
        Done::Fail(why) => {
            // An action that failed already says so in its own line.
            let outcome = if block.action_failed { "not registered".to_string() } else { format!("not registered: {why}") };
            block.finish(tr, &diag, &outcome, false);
            Outcome::Failed(prefixed(why))
        }
    }
}

/// `Load`, `Describe` and `DescribeInContext` of one plug-in, and everything registering it needs
/// from the answers. The `OfxEffect` is made (and registered) by the caller.
fn describe_core(ptr: *mut OfxPlugin, bin: &Path, bundle: &str, lock: Arc<Mutex<()>>, diag: &Arc<PluginDiag>, block: &mut Block) -> Step {
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
    let entry = Arc::new(PluginEntry::new(main, ofx_id.clone(), lock, diag.clone(), crate::inflight::binary_stamp(bin)));
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

    // A plug-in that only renders on the GPU would load and then fail every render.
    if let Some(why) = gpu_only(&desc.props) {
        return Step::Fail(format!("needs GPU rendering, which this host does not offer ({why})"));
    }

    // The contexts this host can run, in preference order, among those the plug-in lists.
    let contexts = desc.props.strs(IE_SUPPORTED_CONTEXTS);
    let candidates: Vec<&str> = [CTX_FILTER, CTX_GENERAL, CTX_TRANSITION, CTX_GENERATOR].into_iter().filter(|c| contexts.iter().any(|s| s == c)).collect();
    if candidates.is_empty() {
        return Step::Fail(format!("no supported context (needs filter, general, transition or generator; it lists {contexts:?})"));
    }

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

    // The first context that describes wins; the reasons the others did not are kept for the notes.
    // An attempt may have defined clips and parameters and extended lists before it failed, so each
    // one starts from what Describe left.
    let described = desc.props.clone();
    let mut notes: Vec<String> = Vec::new();
    let mut failed_contexts: Vec<String> = Vec::new();
    let mut chosen: Option<&str> = None;
    for ctx in candidates {
        desc.reset_to(&described);
        let inp = PropSet::new();
        inp.put_str(IE_CONTEXT, ctx);
        let short = short_context(ctx);
        let (st, mark) = run_action(&entry, block, &format!("DescribeInContext({short})"), ACT_DESCRIBE_IN_CONTEXT, desc.handle(), Some(&inp));
        if st == K_STAT_OK {
            chosen = Some(ctx);
            break;
        }
        failed_contexts.push(entry.failure(&format!("DescribeInContext ({short})"), st, mark));
    }
    let Some(context) = chosen else {
        return Step::Fail(failed_contexts.join("; "));
    };
    if !failed_contexts.is_empty() {
        notes.push(format!("{}; the {} context is used instead", failed_contexts.join("; "), short_context(context)));
    }

    // Clips: Output; the effect's own layer; everything else becomes a Layer parameter.
    let standard_input = if context == CTX_TRANSITION { CLIP_SOURCE_FROM } else { CLIP_SOURCE };
    let mut clip_infos: Vec<ClipInfo> = Vec::new();
    desc.each_clip(|c| {
        clip_infos.push(ClipInfo {
            name: c.name.clone(),
            comps: pick_comps(&c.props.strs(IE_SUPPORTED_COMPONENTS)),
            optional: c.props.flag(CLIP_OPTIONAL, false),
            mask: c.props.flag(CLIP_IS_MASK, false),
        })
    });
    if !clip_infos.iter().any(|c| c.name == CLIP_OUTPUT) {
        return Step::Fail("describes no Output clip".into());
    }
    // The effect's own pixels come in on `Source` (`SourceFrom` in a transition). A plug-in that
    // calls its main input something else gets its first required, non-mask clip instead of a
    // Layer parameter with nothing behind it.
    let input_name = main_input(&clip_infos, standard_input).unwrap_or_default();
    if !input_name.is_empty() && input_name != standard_input {
        notes.push(format!("it has no clip named {standard_input}; its first required clip, {input_name}, is used as the main input"));
    }
    let layer_clips: Vec<String> = clip_infos.iter().filter(|c| c.name != CLIP_OUTPUT && c.name != input_name).map(|c| c.name.clone()).collect();
    let page_order = desc.props.strs(IE_PARAM_PAGE_ORDER);
    let mapped = map_params(&desc.params, &page_order, &layer_clips);

    let clips: Vec<ClipCfg> = clip_infos
        .into_iter()
        .map(|ClipInfo { name, comps, optional, .. }| {
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
    let cfg = Arc::new(EffectCfg {
        context: context.to_string(),
        clips,
        depth,
        safety,
        slave_params: desc.props.strs(IE_CLIP_PREFS_SLAVE_PARAM),
        single_instance: desc.props.flag(IE_SINGLE_INSTANCE, false),
        sequential_render: desc.props.int(IE_SEQUENTIAL_RENDER).unwrap_or(0),
    });

    let name = desc.props.str(P_LABEL).filter(|s| !s.trim().is_empty()).unwrap_or_else(|| ofx_id.clone());
    let category = desc
        .props
        .str(IE_GROUPING)
        .and_then(|g| g.split('/').map(str::trim).find(|s| !s.is_empty()).map(str::to_string))
        .unwrap_or_else(|| "OpenFX".to_string());
    let manifest = PluginManifest {
        api: PLUGIN_API_VERSION,
        id: effect_id(&ofx_id),
        name,
        category,
        version: format!("{}.{}", p.plugin_version_major, p.plugin_version_minor),
        author: String::new(),
        description: desc.props.str(P_PLUGIN_DESCRIPTION).unwrap_or_default(),
        params: mapped.params,
    };
    let reg = Registration {
        ofx_id,
        version: (p.plugin_version_major, p.plugin_version_minor),
        context: short_context(context).to_string(),
        manifest,
        bindings: Arc::new(mapped.bindings),
        cfg,
        notes,
    };
    Step::Registered(Box::new(Described { reg, entry, desc }))
}

/// What the loader needs to know about a described clip.
struct ClipInfo {
    name: String,
    comps: Comps,
    optional: bool,
    mask: bool,
}

/// The clip that carries the effect's own pixels: the standard name if the plug-in defines it,
/// else the first clip that is not `Output`, not optional and not a mask; `None` when there is
/// none (generators).
fn main_input(clips: &[ClipInfo], standard: &str) -> Option<String> {
    if clips.iter().any(|c| c.name == standard) {
        return Some(standard.to_string());
    }
    clips.iter().find(|c| c.name != CLIP_OUTPUT && !c.optional && !c.mask).map(|c| c.name.clone())
}

/// Why a plug-in can only render on the GPU, if it can: it says it does not support CPU rendering
/// (`OfxImageEffectPropCPURenderSupported` is "false", OFX 1.5.1) or that it needs OpenGL, CUDA,
/// Metal or OpenCL (`"needed"`).
fn gpu_only(props: &PropSet) -> Option<String> {
    let mut why = Vec::new();
    if props.str(IE_CPU_RENDER_SUPPORTED).as_deref() == Some("false") {
        why.push(format!("{IE_CPU_RENDER_SUPPORTED} is \"false\""));
    }
    for (name, api) in [
        (IE_OPENGL_RENDER_SUPPORTED, "OpenGL"),
        (IE_CUDA_RENDER_SUPPORTED, "CUDA"),
        (IE_METAL_RENDER_SUPPORTED, "Metal"),
        (IE_OPENCL_RENDER_SUPPORTED, "OpenCL"),
    ] {
        if props.str(name).as_deref() == Some("needed") {
            why.push(format!("{name} is \"needed\": it requires {api}"));
        }
    }
    (!why.is_empty()).then(|| why.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(id: &str, major: u32, minor: u32) -> Offer {
        Offer { id: id.into(), version: (major, minor) }
    }

    #[test]
    fn the_newest_copy_of_a_plugin_is_kept() {
        let claimed = HashMap::new();
        // Release 3.1 is found between an older and a newer copy: only the newest stays.
        let offers = vec![
            vec![offer("com.topaz.enhance", 3, 0), offer("com.topaz.other", 1, 0)],
            vec![offer("com.topaz.enhance", 3, 1)],
            vec![offer("com.topaz.enhance", 2, 9)],
        ];
        let losers = pick_duplicates(&offers, &claimed);
        assert_eq!(losers.len(), 2);
        assert_eq!(losers.get(&(0, 0)), Some(&Winner::Binary(1, (3, 1))));
        assert_eq!(losers.get(&(2, 0)), Some(&Winner::Binary(1, (3, 1))));
        // The other identifier is only offered once.
        assert!(!losers.contains_key(&(0, 1)));
    }

    #[test]
    fn equal_versions_keep_the_copy_found_first() {
        let offers = vec![vec![offer("a", 1, 0)], vec![offer("a", 1, 0)], vec![offer("a", 1, 0)]];
        let losers = pick_duplicates(&offers, &HashMap::new());
        assert_eq!(losers.keys().copied().collect::<Vec<_>>(), [(1, 0), (2, 0)]);
        assert!(losers.values().all(|w| *w == Winner::Binary(0, (1, 0))));
    }

    #[test]
    fn a_plugin_registered_by_an_earlier_scan_cannot_be_replaced() {
        // Even a newer copy loses: registrations are permanent.
        let claimed = HashMap::from([("a".to_string(), (1, 0))]);
        let losers = pick_duplicates(&[vec![offer("a", 2, 0), offer("b", 1, 0)]], &claimed);
        assert_eq!(losers.len(), 1);
        assert_eq!(losers.get(&(0, 0)), Some(&Winner::Registered((1, 0))));
    }

    fn clip(name: &str, optional: bool, mask: bool) -> ClipInfo {
        ClipInfo { name: name.into(), comps: Comps::Rgba, optional, mask }
    }

    #[test]
    fn the_main_input_is_source_or_the_first_required_clip() {
        // The standard name wins, wherever it is.
        let clips = [clip("Output", false, false), clip("Depth", false, false), clip("Source", false, false)];
        assert_eq!(main_input(&clips, "Source").as_deref(), Some("Source"));
        // Without it: the first clip that is not Output, optional or a mask.
        let clips =
            [clip("Output", false, false), clip("Mask", false, true), clip("Overlay", true, false), clip("Image", false, false), clip("Other", false, false)];
        assert_eq!(main_input(&clips, "Source").as_deref(), Some("Image"));
        // A generator has no input.
        assert_eq!(main_input(&[clip("Output", false, false), clip("Matte", true, false)], "Source"), None);
    }

    #[test]
    fn gpu_only_plugins_are_recognised() {
        let p = PropSet::new();
        assert_eq!(gpu_only(&p), None);
        p.put_str(IE_OPENGL_RENDER_SUPPORTED, "true");
        p.put_str(IE_CPU_RENDER_SUPPORTED, "true");
        assert_eq!(gpu_only(&p), None, "supporting a GPU API next to the CPU is fine");
        p.put_str(IE_CPU_RENDER_SUPPORTED, "false");
        assert!(gpu_only(&p).is_some_and(|w| w.contains("CPURenderSupported")));
        p.put_str(IE_CPU_RENDER_SUPPORTED, "true");
        p.put_str(IE_CUDA_RENDER_SUPPORTED, "needed");
        assert!(gpu_only(&p).is_some_and(|w| w.contains("CUDA")));
    }

    #[test]
    fn a_bundle_binary_is_found_in_any_of_the_platform_directories() {
        let root = std::env::temp_dir().join(format!("effectcraft-ofx-bundle-dirs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let bundle = root.join("Foo.ofx.bundle");
        let touch = |dir: &str, file: &str| {
            let d = bundle.join("Contents").join(dir);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(file), b"x").unwrap();
            d.join(file)
        };
        let arm = ["Linux-aarch64", "Linux-arm-64"];
        assert_eq!(bundle_binary_in(&bundle, &arm), None);
        // The informal name and the one some vendors ship are both accepted.
        let alt = touch("Linux-arm-64", "Foo.ofx");
        assert_eq!(bundle_binary_in(&bundle, &arm), Some(alt));
        // The first directory wins when both exist; an exact name beats any other binary.
        let first = touch("Linux-aarch64", "Other.ofx");
        assert_eq!(bundle_binary_in(&bundle, &arm), Some(first));
        let exact = touch("Linux-aarch64", "Foo.ofx");
        assert_eq!(bundle_binary_in(&bundle, &arm), Some(exact));
        // Another platform's directory is not this platform's binary.
        assert_eq!(bundle_binary_in(&bundle, &["Win64"]), None);
        let _ = std::fs::remove_dir_all(&root);
    }

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

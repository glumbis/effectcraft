//! The scan cache: what registering OpenFX plug-ins needs, remembered between runs.
//!
//! Starting up used to load every plug-in library and run `Load`, `Describe` and
//! `DescribeInContext` for every plug-in in it (Boris FX Sapphire alone has about 290), and kept
//! all of them loaded. `ofx_cache.json`, in the config folder next to `ofx_scan.txt`, stores per
//! binary what registration needs: the plug-in identifiers and versions, the manifest (name,
//! category, parameters), the clips, the pixel depth, the render thread safety and the other facts
//! of [`EffectCfg`], the parameter bindings, and the notes and transcript lines of the scan. A
//! binary whose file did not change registers from there without being loaded; the library is
//! loaded (and the plug-in described again) the first time one of its effects needs an instance
//! (see [`crate::effect`]).
//!
//! An entry is keyed by the canonical path of the binary plus its size and modification time. The
//! whole file is ignored (and rebuilt) when its format version, the EffectCraft version or the
//! effective host name differ from the running process's: plug-ins describe themselves differently
//! to different hosts, and a new EffectCraft may read more from a description.
//!
//! What is cached: only binaries with at least one plug-in that registered, and only when no
//! plug-in of the binary was skipped as a duplicate of another copy (such a binary is rare, and the
//! next start works out the duplicates again). A plug-in that failed to describe is cached as the
//! failure, so a binary with a few failing effects (GPU-only ones, say) still starts from the cache;
//! `effect.plugins.rescanOfx` clears the cache after a licence or an install was fixed.
//!
//! Everything here is best-effort and never fails a load: a missing, unreadable, corrupt or
//! half-valid file is reported (in `ofx_scan.txt`) and the affected binaries are scanned the long way.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use effectcraft_effects::plugin::PluginManifest;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::instance::EffectCfg;
use crate::params::Bindings;

/// The cache file inside the config folder.
pub const FILE: &str = "ofx_cache.json";
/// Bumped whenever the stored shape changes (an older file is then ignored and rebuilt).
pub const FORMAT: u32 = 1;
/// The largest entry (one binary) that is cached; a larger description is scanned every start.
const MAX_ENTRY_BYTES: usize = 24 << 20;
/// The largest cache file that is read or written.
const MAX_FILE_BYTES: u64 = 256 << 20;

/// Everything registering one plug-in as an effect needs, as the scan found it (and the cache
/// keeps it).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Registration {
    /// The OFX plug-in identifier.
    pub ofx_id: String,
    pub version: (u32, u32),
    /// The context the effect runs in (`filter`, `general`, `transition`, `generator`).
    pub context: String,
    pub manifest: PluginManifest,
    pub bindings: Arc<Bindings>,
    pub cfg: Arc<EffectCfg>,
    /// How it was described (a fallback context, a guessed input).
    #[serde(default)]
    pub notes: Vec<String>,
}

impl Registration {
    /// What differs between this description and `other` (a fresh `Describe` of the same plug-in),
    /// if anything. The notes are not part of the description.
    pub fn difference(&self, other: &Registration) -> Option<String> {
        if self.ofx_id != other.ofx_id {
            return Some(format!("the identifier is now {}", other.ofx_id));
        }
        if self.version != other.version {
            return Some(format!("the version is now {}.{} (was {}.{})", other.version.0, other.version.1, self.version.0, self.version.1));
        }
        if self.context != other.context {
            return Some(format!("the context is now {} (was {})", other.context, self.context));
        }
        if self.cfg != other.cfg {
            return Some("its clips or settings changed".to_string());
        }
        if self.manifest != other.manifest {
            return Some("its name or parameters changed".to_string());
        }
        if self.bindings != other.bindings {
            return Some("its parameter layout changed".to_string());
        }
        None
    }
}

/// What the scan made of one plug-in a binary lists.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PluginRecord {
    /// Registered as an effect.
    Registered { reg: Box<Registration> },
    /// Could not become an effect; `error` is the reason (without the binary's name).
    Failed { id: String, version: (u32, u32), error: String },
    /// Left out on purpose; `note` says why (without the binary's name).
    Skipped { id: String, version: (u32, u32), note: String },
}

impl PluginRecord {
    pub fn id(&self) -> &str {
        match self {
            PluginRecord::Registered { reg } => &reg.ofx_id,
            PluginRecord::Failed { id, .. } | PluginRecord::Skipped { id, .. } => id,
        }
    }

    pub fn version(&self) -> (u32, u32) {
        match self {
            PluginRecord::Registered { reg } => reg.version,
            PluginRecord::Failed { version, .. } | PluginRecord::Skipped { version, .. } => *version,
        }
    }
}

/// One binary in the cache.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BinaryEntry {
    /// The canonical path of the binary.
    pub path: String,
    /// The bundle folder it belongs to (or the binary again).
    pub bundle: String,
    /// File size in bytes.
    pub size: u64,
    /// Modification time, nanoseconds since the Unix epoch.
    pub mtime: u64,
    /// Set when a lazy load found the entry out of date (why): the binary is scanned again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale: Option<String>,
    /// The scan's transcript lines for the binary (without its name line).
    #[serde(default)]
    pub transcript: Vec<String>,
    pub plugins: Vec<PluginRecord>,
}

/// Size and modification time (nanoseconds since the epoch) of a file, `None` if unreadable.
pub fn file_stamp(path: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    let nanos = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
    Some((meta.len(), u64::try_from(nanos).ok()?))
}

/// How cache keys compare paths: case-insensitively on Windows.
fn norm(path: &str) -> String {
    if cfg!(windows) { path.to_lowercase() } else { path.to_string() }
}

/// The cache key of a binary's canonical path.
pub fn key_of(path: &Path) -> String {
    norm(&path.to_string_lossy())
}

/// The contents of `ofx_cache.json`.
#[derive(Clone, Debug, Default)]
pub struct CacheFile {
    /// The EffectCraft version that wrote it.
    pub app: String,
    /// The host name the plug-ins were described for.
    pub host: String,
    pub entries: BTreeMap<String, Arc<BinaryEntry>>,
}

/// The file as written.
#[derive(Serialize)]
struct OnDisk<'a> {
    format: u32,
    app: &'a str,
    host: &'a str,
    binaries: BTreeMap<&'a str, &'a BinaryEntry>,
}

/// What reading the cache file found.
#[derive(Debug, Default)]
pub struct ReadOutcome {
    pub file: CacheFile,
    /// Problems and reasons the file (or parts of it) was not used, for the scan report.
    pub notes: Vec<String>,
}

/// Read `dir`'s cache file for a process of EffectCraft `app` showing plug-ins the host name `host`.
/// Never fails: what cannot be used is left out and described in [`ReadOutcome::notes`].
pub fn read(dir: &Path, app: &str, host: &str) -> ReadOutcome {
    let path = dir.join(FILE);
    let mut out = ReadOutcome { file: CacheFile { app: app.to_string(), host: host.to_string(), entries: BTreeMap::new() }, notes: vec![] };
    match std::fs::metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return out,
        Err(e) => {
            out.notes.push(format!("{} could not be read ({e}); every plug-in is scanned and the file is rebuilt", path.display()));
            return out;
        }
        Ok(m) if m.len() > MAX_FILE_BYTES => {
            out.notes.push(format!("{} is larger than {} MiB; it is ignored and rebuilt", path.display(), MAX_FILE_BYTES >> 20));
            return out;
        }
        Ok(_) => {}
    }
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            out.notes.push(format!("{} could not be read ({e}); every plug-in is scanned and the file is rebuilt", path.display()));
            return out;
        }
    };
    let root: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            out.notes.push(format!("{} is not valid JSON ({e}); it is ignored and rebuilt", path.display()));
            return out;
        }
    };
    let Some(obj) = root.as_object() else {
        out.notes.push(format!("{} is not a JSON object; it is ignored and rebuilt", path.display()));
        return out;
    };
    let format = obj.get("format").and_then(Value::as_u64);
    if format != Some(u64::from(FORMAT)) {
        let was = format.map_or_else(|| "none".to_string(), |f| f.to_string());
        out.notes.push(format!("{} has format {was}, this EffectCraft uses {FORMAT}; every plug-in is scanned again", path.display()));
        return out;
    }
    let (was_app, was_host) = (obj.get("app").and_then(Value::as_str).unwrap_or(""), obj.get("host").and_then(Value::as_str).unwrap_or(""));
    if was_app != app {
        out.notes.push(format!("the scan cache was written by EffectCraft {was_app}, this is {app}; every plug-in is scanned again"));
        return out;
    }
    if was_host != host {
        out.notes.push(format!("the scan cache was written for the host name \"{was_host}\", the host name is now \"{host}\"; every plug-in is scanned again"));
        return out;
    }
    let Some(bins) = obj.get("binaries").and_then(Value::as_object) else {
        out.notes.push(format!("{} has no list of binaries; it is ignored and rebuilt", path.display()));
        return out;
    };
    let mut dropped = 0usize;
    for (key, v) in bins {
        match serde_json::from_value::<BinaryEntry>(v.clone()) {
            Ok(e) if norm(&e.path) == *key => {
                out.file.entries.insert(key.clone(), Arc::new(e));
            }
            _ => dropped += 1,
        }
    }
    if dropped > 0 {
        out.notes.push(format!(
            "{dropped} unreadable entr{} of {} dropped; those binaries are scanned again",
            if dropped == 1 { "y" } else { "ies" },
            path.display()
        ));
    }
    out
}

/// Write `file` as `dir`'s cache file (through a temporary file, so a failure leaves the old file).
pub fn write(dir: &Path, file: &CacheFile) -> Result<(), String> {
    let path = dir.join(FILE);
    let doc = OnDisk { format: FORMAT, app: &file.app, host: &file.host, binaries: file.entries.iter().map(|(k, v)| (k.as_str(), &**v)).collect() };
    let text = serde_json::to_vec(&doc).map_err(|e| format!("cannot serialise the scan cache: {e}"))?;
    if text.len() as u64 > MAX_FILE_BYTES {
        return Err(format!("the scan cache would be larger than {} MiB, so it was not written", MAX_FILE_BYTES >> 20));
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let tmp = dir.join(format!("{FILE}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, &text).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("cannot update {}: {e}", path.display())
    })
}

/// Why a binary is not served from the cache (or that it is).
#[derive(Debug)]
pub enum Lookup {
    Hit(Arc<BinaryEntry>),
    Miss(String),
}

/// Look `bin` up in `file`: a hit needs an entry whose size and modification time still match the
/// file and that no lazy load marked out of date.
pub fn lookup(file: &CacheFile, bin: &Path) -> Lookup {
    let Some(e) = file.entries.get(&key_of(bin)) else { return Lookup::Miss("not in the scan cache yet".into()) };
    if let Some(why) = &e.stale {
        return Lookup::Miss(format!("its cache entry was marked out of date: {why}"));
    }
    let Some((size, mtime)) = file_stamp(bin) else { return Lookup::Miss("its size and modification time cannot be read".into()) };
    if size != e.size {
        return Lookup::Miss(format!("the file changed since it was cached (size {} -> {size} bytes)", e.size));
    }
    if mtime != e.mtime {
        return Lookup::Miss("the file changed since it was cached (modification time)".into());
    }
    Lookup::Hit(e.clone())
}

/// What the scan cache did during one `load_path` call (reported in `ofx_scan.txt`).
#[derive(Clone, Debug, Default, Serialize)]
pub struct CacheReport {
    /// The cache is in use (a config folder, and `scan_cache` is not `false`).
    pub enabled: bool,
    /// The cache file, when there is a config folder.
    pub file: Option<String>,
    /// Binaries registered from the cache (their libraries were not loaded).
    pub hits: usize,
    /// Binaries that were loaded and described.
    pub scanned: usize,
    /// One line per binary: from the cache, or scanned and why.
    pub lines: Vec<String>,
    /// Problems with the cache itself (corrupt file, written for another host name, ...).
    pub problems: Vec<String>,
}

impl CacheReport {
    /// Add what another call reported (a scan loads one folder at a time).
    pub fn merge(&mut self, other: &CacheReport) {
        self.enabled |= other.enabled;
        if self.file.is_none() {
            self.file = other.file.clone();
        }
        self.hits += other.hits;
        self.scanned += other.scanned;
        self.lines.extend(other.lines.iter().cloned());
        for p in &other.problems {
            if !self.problems.contains(p) {
                self.problems.push(p.clone());
            }
        }
    }

    pub fn from_json(v: &Value) -> CacheReport {
        let strings = |k: &str| v[k].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>();
        CacheReport {
            enabled: v["enabled"].as_bool().unwrap_or(false),
            file: v["file"].as_str().map(str::to_string),
            hits: v["hits"].as_u64().unwrap_or(0) as usize,
            scanned: v["scanned"].as_u64().unwrap_or(0) as usize,
            lines: strings("lines"),
            problems: strings("problems"),
        }
    }
}

/// The cache of this process: the file as read, and what changed since.
#[derive(Default)]
struct State {
    /// The folder the file was read from (`None`: not read yet).
    dir: Option<PathBuf>,
    file: CacheFile,
    /// Notes from reading the file that no scan has reported yet.
    pending: Vec<String>,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(State::default()))
}

/// Forget what was read: the next scan reads the file again (the config folder was set or changed).
pub fn reset() {
    *state().lock().unwrap_or_else(PoisonError::into_inner) = State::default();
}

/// Delete the cache file and forget the entries: every plug-in is scanned the long way at the next
/// start. Returns the file's path.
pub fn clear() -> Result<String, String> {
    let dir = crate::blocklist::state_dir().ok_or("EffectCraft has no config folder here, so there is no scan cache")?;
    let path = dir.join(FILE);
    {
        let mut st = state().lock().unwrap_or_else(PoisonError::into_inner);
        st.file.entries.clear();
        st.pending.clear();
    }
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(path.to_string_lossy().into_owned()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(path.to_string_lossy().into_owned()),
        Err(e) => Err(format!("cannot delete {}: {e}", path.display())),
    }
}

/// A lazy load found the entry of `bin` out of date: mark it (and write the file now, so a crash
/// or a kill later does not lose it) and the binary is scanned again at the next start.
pub fn mark_stale(bin: &Path, why: &str) {
    let Some(dir) = crate::blocklist::state_dir() else { return };
    let mut st = state().lock().unwrap_or_else(PoisonError::into_inner);
    let key = key_of(bin);
    let Some(old) = st.file.entries.get(&key).cloned() else { return };
    let mut e = (*old).clone();
    e.stale = Some(why.to_string());
    st.file.entries.insert(key, Arc::new(e));
    if let Err(err) = write(&dir, &st.file) {
        log::warn!("openfx: {err}");
    }
}

/// The scan cache during one `load_path` call.
pub struct Run {
    enabled: bool,
    /// Hits are served (an explicit load of a plug-in leaves this off).
    read: bool,
    dir: Option<PathBuf>,
    changed: bool,
    report: CacheReport,
}

impl Run {
    /// Start a scan. `use_cache`: serve unchanged binaries from the cache (otherwise every binary
    /// is loaded and the cache is only updated). `host` is the host name plug-ins are shown.
    pub fn begin(use_cache: bool, host: &str) -> Run {
        let dir = crate::blocklist::state_dir();
        let setting = crate::settings::scan_cache(dir.as_deref());
        let enabled = setting.enabled && dir.is_some();
        let mut report = CacheReport { enabled, file: dir.as_ref().map(|d| d.join(FILE).to_string_lossy().into_owned()), ..Default::default() };
        report.problems.extend(setting.problem);
        let mut run = Run { enabled, read: enabled && use_cache, dir: dir.clone(), changed: false, report };
        let Some(dir) = dir.filter(|_| enabled) else { return run };
        let mut st = state().lock().unwrap_or_else(PoisonError::into_inner);
        if st.dir.as_deref() != Some(dir.as_path()) {
            let out = read(&dir, env!("CARGO_PKG_VERSION"), host);
            *st = State { dir: Some(dir), file: out.file, pending: out.notes };
        }
        let pending = std::mem::take(&mut st.pending);
        run.report.problems.extend(pending);
        run
    }

    /// Whether the cache is in use at all.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// The cache entry for `bin` (canonical path), if the binary can be registered from it.
    /// `label` names the binary in the report.
    pub fn lookup(&mut self, bin: &Path, label: &str) -> Option<Arc<BinaryEntry>> {
        if !self.enabled {
            return None;
        }
        if !self.read {
            self.report.lines.push(format!("scanned: {label} (an explicit load does not use the scan cache)"));
            self.report.scanned += 1;
            return None;
        }
        let found = lookup(&state().lock().unwrap_or_else(PoisonError::into_inner).file, bin);
        match found {
            Lookup::Hit(e) => {
                let n = e.plugins.iter().filter(|p| matches!(p, PluginRecord::Registered { .. })).count();
                self.report.lines.push(format!("from the cache: {label} ({n} plug-in(s), library not loaded)"));
                self.report.hits += 1;
                Some(e)
            }
            Lookup::Miss(why) => {
                self.report.lines.push(format!("scanned: {label} ({why})"));
                self.report.scanned += 1;
                None
            }
        }
    }

    /// Remember a freshly scanned binary (replacing its old entry).
    pub fn store(&mut self, entry: BinaryEntry) {
        if !self.enabled {
            return;
        }
        let key = key_of(Path::new(&entry.path));
        match serde_json::to_vec(&entry) {
            Ok(b) if b.len() <= MAX_ENTRY_BYTES => {}
            Ok(b) => {
                self.report.problems.push(format!(
                    "{}: its description is {} MiB, too large to cache; it is scanned at every start",
                    entry.path,
                    b.len() >> 20
                ));
                self.forget(Path::new(&entry.path));
                return;
            }
            Err(e) => {
                self.report.problems.push(format!("{}: cannot be cached ({e})", entry.path));
                self.forget(Path::new(&entry.path));
                return;
            }
        }
        let mut st = state().lock().unwrap_or_else(PoisonError::into_inner);
        st.file.entries.insert(key, Arc::new(entry));
        self.changed = true;
    }

    /// Drop the entry of `bin` (it cannot be cached as it is now).
    pub fn forget(&mut self, bin: &Path) {
        if !self.enabled {
            return;
        }
        let mut st = state().lock().unwrap_or_else(PoisonError::into_inner);
        if st.file.entries.remove(&key_of(bin)).is_some() {
            self.changed = true;
        }
    }

    /// Write the file if anything changed (entries of binaries that no longer exist go) and hand
    /// back the report.
    pub fn finish(mut self) -> CacheReport {
        if let (true, true, Some(dir)) = (self.enabled, self.changed, self.dir.clone()) {
            let mut st = state().lock().unwrap_or_else(PoisonError::into_inner);
            st.file.entries.retain(|_, e| Path::new(&e.path).exists());
            if let Err(e) = write(&dir, &st.file) {
                self.report.problems.push(e);
            }
        }
        self.report
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clips::{Comps, Depth};
    use crate::instance::{ClipCfg, ClipRole, Safety};
    use crate::params::{Binding, PKind, Src};
    use effectcraft_effects::plugin::{PluginParam, PluginParamKind};

    /// A fresh, empty folder under the system temp folder; removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("effectcraft-ofx-cache-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub(crate) fn sample_registration(id: &str) -> Registration {
        let manifest = PluginManifest {
            api: 2,
            id: id.to_lowercase(),
            name: "Sample".into(),
            category: "Test".into(),
            version: "1.2".into(),
            author: String::new(),
            description: "d".into(),
            params: vec![
                PluginParam {
                    id: "amount".into(),
                    name: "Amount".into(),
                    group: "Main/Sub".into(),
                    kind: PluginParamKind::Slider {
                        default: 0.1,
                        min: 0.0,
                        max: 1.0,
                        slider_min: Some(0.0),
                        slider_max: Some(1.0),
                        decimals: 2,
                        integer: false,
                    },
                },
                PluginParam { id: "Mask".into(), name: "Mask".into(), group: String::new(), kind: PluginParamKind::Layer },
                PluginParam {
                    id: "mode".into(),
                    name: "Mode".into(),
                    group: String::new(),
                    kind: PluginParamKind::Popup { options: vec!["A".into(), "B".into()], default: 1 },
                },
            ],
        };
        let bindings = Bindings {
            items: vec![Binding { name: "amount".into(), kind: PKind::Double, src: Src::Flat { at: 1, n: 1 }, choices: vec![] }],
            layers: vec![crate::params::LayerBinding { clip: "Mask".into(), param_id: "Mask".into(), at: 0 }],
        };
        let cfg = EffectCfg {
            context: "OfxImageEffectContextFilter".into(),
            clips: vec![
                ClipCfg { name: "Output".into(), role: ClipRole::Output, comps: Comps::Rgba, optional: false },
                ClipCfg { name: "Mask".into(), role: ClipRole::Layer("Mask".into()), comps: Comps::Alpha, optional: true },
            ],
            depth: Depth::Float,
            safety: Safety::Unsafe,
            slave_params: vec!["mode".into()],
            single_instance: true,
            sequential_render: 1,
        };
        Registration {
            ofx_id: id.into(),
            version: (1, 2),
            context: "filter".into(),
            manifest,
            bindings: Arc::new(bindings),
            cfg: Arc::new(cfg),
            notes: vec!["a note".into()],
        }
    }

    fn entry(path: &Path, bundle: &str) -> BinaryEntry {
        let (size, mtime) = file_stamp(path).unwrap();
        BinaryEntry {
            path: path.to_string_lossy().into_owned(),
            bundle: bundle.into(),
            size,
            mtime,
            stale: None,
            transcript: vec!["  one line".into()],
            plugins: vec![
                PluginRecord::Registered { reg: Box::new(sample_registration("org.example.Sample")) },
                PluginRecord::Failed { id: "org.example.Gpu".into(), version: (2, 0), error: "needs GPU rendering".into() },
                PluginRecord::Skipped { id: "org.example.Other".into(), version: (1, 0), note: "not an image effect".into() },
            ],
        }
    }

    fn put(file: &mut CacheFile, e: BinaryEntry) {
        file.entries.insert(key_of(Path::new(&e.path)), Arc::new(e));
    }

    #[test]
    fn a_description_survives_the_round_trip() {
        let t = TempDir::new("roundtrip");
        let bin = t.0.join("Plug.ofx");
        std::fs::write(&bin, b"binary").unwrap();
        let mut file = CacheFile { app: "1.0.0".into(), host: "EffectCraft".into(), entries: BTreeMap::new() };
        put(&mut file, entry(&bin, "Plug.ofx.bundle"));
        write(&t.0, &file).unwrap();
        // No temporary file is left behind.
        assert_eq!(std::fs::read_dir(&t.0).unwrap().count(), 2);
        let out = read(&t.0, "1.0.0", "EffectCraft");
        assert!(out.notes.is_empty(), "{:?}", out.notes);
        let Lookup::Hit(e) = lookup(&out.file, &bin) else { panic!("a miss") };
        assert_eq!((e.bundle.as_str(), e.transcript.len(), e.plugins.len()), ("Plug.ofx.bundle", 1, 3));
        let PluginRecord::Registered { reg } = &e.plugins[0] else { panic!("not registered") };
        // Nothing that registration needs is lost: the fresh description equals the stored one.
        assert_eq!(reg.difference(&sample_registration("org.example.Sample")), None);
        assert_eq!(reg.notes, ["a note"]);
        assert!(reg.cfg.single_instance && reg.cfg.sequential_render == 1 && reg.cfg.safety == Safety::Unsafe);
        assert!(matches!(&e.plugins[1], PluginRecord::Failed { error, .. } if error == "needs GPU rendering"));
        assert!(matches!(&e.plugins[2], PluginRecord::Skipped { id, .. } if id == "org.example.Other"));
        assert_eq!(e.plugins[1].id(), "org.example.Gpu");
        assert_eq!(e.plugins[1].version(), (2, 0));
    }

    #[test]
    fn a_changed_description_is_told_apart() {
        let a = sample_registration("org.example.Sample");
        let mut b = a.clone();
        assert_eq!(a.difference(&b), None);
        b.notes.push("notes are not part of the description".into());
        assert_eq!(a.difference(&b), None);
        b.manifest.params.pop();
        assert!(a.difference(&b).is_some_and(|d| d.contains("parameters")));
        let mut c = a.clone();
        c.version = (1, 3);
        assert!(a.difference(&c).is_some_and(|d| d.contains("1.3")));
        let mut d = a.clone();
        Arc::make_mut(&mut d.cfg).slave_params.clear();
        assert!(a.difference(&d).is_some());
        let mut e = a.clone();
        Arc::make_mut(&mut e.bindings).items.clear();
        assert!(a.difference(&e).is_some_and(|d| d.contains("layout")));
    }

    #[test]
    fn a_changed_file_is_a_miss() {
        let t = TempDir::new("stamp");
        let bin = t.0.join("Plug.ofx");
        std::fs::write(&bin, b"binary").unwrap();
        let mut file = CacheFile::default();
        put(&mut file, entry(&bin, "b"));
        assert!(matches!(lookup(&file, &bin), Lookup::Hit(_)));
        // A different size.
        std::fs::write(&bin, b"a longer binary").unwrap();
        assert!(matches!(lookup(&file, &bin), Lookup::Miss(w) if w.contains("size 6 -> 15")));
        // The same size with another modification time.
        std::fs::write(&bin, b"binary").unwrap();
        let (size, mtime) = file_stamp(&bin).unwrap();
        let mut e = (**file.entries.values().next().unwrap()).clone();
        e.size = size;
        e.mtime = mtime + 1_000_000_000;
        put(&mut file, e);
        assert!(matches!(lookup(&file, &bin), Lookup::Miss(w) if w.contains("modification time")));
        // Unknown binaries and removed files.
        assert!(matches!(lookup(&file, &t.0.join("Other.ofx")), Lookup::Miss(w) if w.contains("not in the scan cache")));
        std::fs::remove_file(&bin).unwrap();
        assert!(matches!(lookup(&file, &bin), Lookup::Miss(_)));
    }

    #[test]
    fn a_stale_entry_is_a_miss() {
        let t = TempDir::new("stale");
        let bin = t.0.join("Plug.ofx");
        std::fs::write(&bin, b"binary").unwrap();
        let mut file = CacheFile::default();
        let mut e = entry(&bin, "b");
        e.stale = Some("its parameters changed".into());
        put(&mut file, e);
        assert!(matches!(lookup(&file, &bin), Lookup::Miss(w) if w.contains("out of date") && w.contains("parameters")));
    }

    #[test]
    fn another_format_version_host_name_or_app_invalidates_everything() {
        let t = TempDir::new("keys");
        let bin = t.0.join("Plug.ofx");
        std::fs::write(&bin, b"binary").unwrap();
        let mut file = CacheFile { app: "1.0.0".into(), host: "EffectCraft".into(), entries: BTreeMap::new() };
        put(&mut file, entry(&bin, "b"));
        write(&t.0, &file).unwrap();
        // The host name plug-ins are shown changed.
        let out = read(&t.0, "1.0.0", "DaVinciResolve");
        assert!(out.file.entries.is_empty() && out.notes.iter().any(|n| n.contains("host name") && n.contains("DaVinciResolve")), "{:?}", out.notes);
        // A different EffectCraft version.
        let out = read(&t.0, "1.1.0", "EffectCraft");
        assert!(out.file.entries.is_empty() && out.notes.iter().any(|n| n.contains("written by EffectCraft 1.0.0")), "{:?}", out.notes);
        // A different format version.
        let path = t.0.join(FILE);
        let text = std::fs::read_to_string(&path).unwrap().replacen(&format!("\"format\":{FORMAT}"), "\"format\":999", 1);
        std::fs::write(&path, text).unwrap();
        let out = read(&t.0, "1.0.0", "EffectCraft");
        assert!(out.file.entries.is_empty() && out.notes.iter().any(|n| n.contains("format 999")), "{:?}", out.notes);
    }

    #[test]
    fn a_corrupt_file_is_reported_and_ignored() {
        let t = TempDir::new("corrupt");
        let path = t.0.join(FILE);
        // Missing: nothing to report.
        let out = read(&t.0, "1.0.0", "EffectCraft");
        assert!(out.file.entries.is_empty() && out.notes.is_empty());
        for (text, needle) in [
            ("{not json", "not valid JSON"),
            ("", "not valid JSON"),
            ("[1, 2]", "not a JSON object"),
            ("{\"format\": \"x\"}", "format none"),
            (&format!("{{\"format\": {FORMAT}, \"app\": \"1.0.0\", \"host\": \"EffectCraft\"}}"), "no list of binaries"),
            ("\u{0}\u{1}\u{2}", "not valid JSON"),
        ] {
            std::fs::write(&path, text).unwrap();
            let out = read(&t.0, "1.0.0", "EffectCraft");
            assert!(out.file.entries.is_empty(), "{text}");
            assert!(out.notes.iter().any(|n| n.contains(needle)), "{text}: {:?}", out.notes);
        }
        // A binary file that is not UTF-8.
        std::fs::write(&path, [0xffu8, 0xfe, 0x00, 0x80]).unwrap();
        assert!(read(&t.0, "1.0.0", "EffectCraft").notes.iter().any(|n| n.contains("could not be read")));
    }

    #[test]
    fn one_bad_entry_does_not_lose_the_others() {
        let t = TempDir::new("partial");
        let good = t.0.join("Good.ofx");
        std::fs::write(&good, b"binary").unwrap();
        let mut file = CacheFile { app: "1.0.0".into(), host: "EffectCraft".into(), entries: BTreeMap::new() };
        put(&mut file, entry(&good, "b"));
        write(&t.0, &file).unwrap();
        // Add an entry with a plug-in record the reader cannot understand, and one under the wrong key.
        let path = t.0.join(FILE);
        let mut doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let bins = doc["binaries"].as_object_mut().unwrap();
        bins.insert("bad".into(), serde_json::json!({"path": "bad", "bundle": "", "size": 1, "mtime": 1, "plugins": [{"state": "registered", "reg": 5}]}));
        bins.insert("wrong-key".into(), serde_json::json!({"path": "other", "bundle": "", "size": 1, "mtime": 1, "plugins": []}));
        std::fs::write(&path, doc.to_string()).unwrap();
        let out = read(&t.0, "1.0.0", "EffectCraft");
        assert_eq!(out.file.entries.len(), 1);
        assert!(matches!(lookup(&out.file, &good), Lookup::Hit(_)));
        assert!(out.notes.iter().any(|n| n.contains("2 unreadable entries")), "{:?}", out.notes);
    }

    #[test]
    fn writing_to_an_unusable_folder_fails_politely() {
        let t = TempDir::new("io");
        let file_as_dir = t.0.join("not-a-dir");
        std::fs::write(&file_as_dir, b"x").unwrap();
        let err = write(&file_as_dir, &CacheFile::default()).unwrap_err();
        assert!(err.contains("cannot"), "{err}");
    }

    #[test]
    fn reports_merge() {
        let mut a = CacheReport { enabled: true, hits: 2, scanned: 1, lines: vec!["a".into()], problems: vec!["p".into()], file: Some("f".into()) };
        let b = CacheReport { enabled: true, hits: 1, scanned: 3, lines: vec!["b".into()], problems: vec!["p".into(), "q".into()], file: None };
        a.merge(&b);
        assert_eq!((a.hits, a.scanned, a.lines.len(), a.problems.len()), (3, 4, 2, 2));
        let v = serde_json::to_value(&a).unwrap();
        let back = CacheReport::from_json(&v);
        assert_eq!(
            (back.enabled, back.hits, back.scanned, back.file.as_deref(), back.lines, back.problems),
            (true, 3, 4, Some("f"), vec!["a".to_string(), "b".into()], vec!["p".to_string(), "q".into()])
        );
    }
}

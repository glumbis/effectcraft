//! The scan cache and lazy loading, against the test plug-in (`examples/ofx/ec-test-ofx`).
//!
//! Registering effects is permanent for a process, so every start-up in these scenarios is a
//! process of its own: this test binary runs itself with the `child_start` test selected and the
//! `EC_SCAN_CACHE_*` variables naming a config folder (the state folder: scan cache, settings) and a
//! folder with a copy of the test bundle. The child scans, optionally renders one effect, and prints
//! a `RESULT {json}` line the scenario reads. Set `EC_TEST_OFX_BUNDLE` to the built
//! `ec-test-ofx.ofx.bundle`; without it the scenarios return.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::{defaults, test_binary};
use effectcraft_effects::plugin::{PluginFrame, PluginParams, plugin};
use effectcraft_ofx::{library_loads, load_path_with};
use serde_json::{Value, json};

/// The effects the test bundle registers.
const EFFECTS: usize = 13;

/// The child process: one start-up of EffectCraft's OpenFX host. A normal run of this test binary
/// does nothing here.
#[test]
fn child_start() {
    let Some(cfg) = std::env::var_os("EC_SCAN_CACHE_CFG") else { return };
    let plugins = PathBuf::from(std::env::var_os("EC_SCAN_CACHE_PLUGINS").expect("EC_SCAN_CACHE_PLUGINS"));
    effectcraft_ofx::set_state_dir(PathBuf::from(cfg));
    let use_cache = std::env::var_os("EC_SCAN_CACHE_EXPLICIT").is_none();
    let report = load_path_with(&plugins, use_cache);
    let loads_after_scan = library_loads();
    // Optionally use one effect, which is what makes a cached plug-in load.
    let render = std::env::var("EC_SCAN_CACHE_RENDER").ok().map(|id| {
        let p = plugin(&id).unwrap_or_else(|| panic!("{id} is not registered"));
        let (flat, texts) = defaults(&*p, &[]);
        let params = PluginParams { manifest: p.manifest(), flat: &flat, texts: &texts };
        let mut pixels = vec![[0.2f32, 0.4, 0.6, 1.0]; 4];
        let mut frame = PluginFrame { width: 2, height: 2, pixels: &mut pixels, scale: 1.0, origin: [0.0; 2], layer_size: [2.0, 2.0] };
        match p.render(&mut frame, &params, 1.0) {
            Ok(()) => json!({"ok": true, "pixel": pixels[0]}),
            Err(e) => json!({"ok": false, "error": e}),
        }
    });
    let result = json!({
        "loaded": report.loaded.len(),
        "errors": report.errors,
        "notes": report.notes,
        "hits": report.cache.hits,
        "scanned": report.cache.scanned,
        "enabled": report.cache.enabled,
        "lines": report.cache.lines,
        "problems": report.cache.problems,
        "transcript": report.transcript,
        "loads_after_scan": loads_after_scan,
        "loads_after_render": library_loads(),
        "render": render,
    });
    println!("RESULT {result}");
}

/// A config folder and a plug-in folder holding a copy of the test bundle's binary.
struct Fixture {
    root: PathBuf,
    cfg: PathBuf,
    plugins: PathBuf,
    bin: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Option<Fixture> {
        let Some(src) = test_binary() else {
            println!("note: EC_TEST_OFX_BUNDLE is not set; skipping the scan cache tests");
            return None;
        };
        let root = std::env::temp_dir().join(format!("effectcraft-ofx-scan-cache-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let platform = src.parent().and_then(Path::file_name).map(|n| n.to_owned()).unwrap_or_default();
        let dir = root.join("plugins").join("cachetest.ofx.bundle").join("Contents").join(&platform);
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("cachetest.ofx");
        std::fs::copy(&src, &bin).unwrap();
        Some(Fixture { cfg: root.join("config"), plugins: root.join("plugins"), root, bin })
    }

    /// One start-up in a process of its own; `env` adds variables to it.
    fn start(&self, env: &[(&str, &str)]) -> Value {
        let out = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "child_start", "--nocapture", "--test-threads=1"])
            .env("EC_SCAN_CACHE_CFG", &self.cfg)
            .env("EC_SCAN_CACHE_PLUGINS", &self.plugins)
            .env_remove("EFFECTCRAFT_OFX_HOST_NAME")
            .env_remove("EC_TEST_OFX_MODE")
            .envs(env.iter().copied())
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "the child failed ({}):\n{stdout}\n{}", out.status, String::from_utf8_lossy(&out.stderr));
        let line = stdout.lines().find_map(|l| l.split_once("RESULT ").map(|(_, json)| json)).unwrap_or_else(|| panic!("no RESULT line in:\n{stdout}"));
        serde_json::from_str(line).unwrap()
    }

    fn cache_text(&self) -> String {
        std::fs::read_to_string(self.cfg.join("ofx_cache.json")).unwrap_or_default()
    }

    fn touch(&self) {
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(120);
        std::fs::File::options().write(true).open(&self.bin).unwrap().set_modified(later).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn n(v: &Value, k: &str) -> u64 {
    v[k].as_u64().unwrap_or_else(|| panic!("{k} in {v}"))
}

fn lines(v: &Value, k: &str) -> String {
    v[k].as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>().join("\n")
}

const INVERT: &str = "org.effectcraft.test.invert";

/// The first start loads and describes everything (and writes the cache); the next one registers
/// every effect without loading the library, which only happens when an effect is first used.
#[test]
fn a_warm_start_registers_from_the_cache_without_loading_any_library() {
    let Some(f) = Fixture::new("warm") else { return };
    let cold = f.start(&[("EC_SCAN_CACHE_RENDER", INVERT)]);
    assert!(cold["errors"].as_array().is_some_and(Vec::is_empty), "{cold}");
    assert_eq!((n(&cold, "loaded") as usize, n(&cold, "hits"), n(&cold, "scanned")), (EFFECTS, 0, 1), "{cold}");
    assert_eq!(n(&cold, "loads_after_scan"), 1, "a cold start loads the one binary");
    assert!(lines(&cold, "lines").contains("not in the scan cache yet"), "{cold}");
    let text = f.cache_text();
    assert!(
        text.contains("\"format\":1") && text.contains("org.effectcraft.test.Invert") && text.contains("\"host\":\"EffectCraft\""),
        "{}",
        &text[..text.len().min(300)]
    );

    let warm = f.start(&[("EC_SCAN_CACHE_RENDER", INVERT)]);
    assert!(warm["errors"].as_array().is_some_and(Vec::is_empty), "{warm}");
    // The same effects, without a library load: one binary came from the cache.
    assert_eq!((n(&warm, "loaded") as usize, n(&warm, "hits"), n(&warm, "scanned")), (EFFECTS, 1, 0), "{warm}");
    assert_eq!(n(&warm, "loads_after_scan"), 0, "a warm start must not load the library: {warm}");
    assert!(lines(&warm, "lines").contains("from the cache:") && lines(&warm, "lines").contains("library not loaded"), "{warm}");
    assert!(lines(&warm, "transcript").contains("from the scan cache"), "the report says so too: {warm}");
    // Using an effect loads the library, once, and the effect works.
    assert_eq!(n(&warm, "loads_after_render"), 1, "{warm}");
    assert_eq!(warm["render"]["ok"], true, "{warm}");
    let px: Vec<f64> = warm["render"]["pixel"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    for (got, want) in px.iter().zip([0.8, 0.6, 0.4, 1.0]) {
        assert!((got - want).abs() < 1e-4, "{px:?}");
    }
    // A cached start that uses nothing never loads anything, and an explicit load ignores the cache.
    let idle = f.start(&[]);
    assert_eq!((n(&idle, "hits"), n(&idle, "loads_after_render")), (1, 0), "{idle}");
    let explicit = f.start(&[("EC_SCAN_CACHE_EXPLICIT", "1")]);
    assert_eq!((n(&explicit, "hits"), n(&explicit, "scanned"), n(&explicit, "loads_after_scan")), (0, 1, 1), "{explicit}");
    assert!(lines(&explicit, "lines").contains("explicit load"), "{explicit}");
}

/// A binary that changed, another host name, a corrupt file and the `scan_cache` setting each send
/// the next start the long way; the cache never makes a start fail.
#[test]
fn a_changed_binary_host_name_or_cache_file_is_scanned_again() {
    let Some(f) = Fixture::new("invalidate") else { return };
    let _ = f.start(&[]);
    assert_eq!(n(&f.start(&[]), "hits"), 1);

    // The binary was replaced (same size, new modification time).
    f.touch();
    let v = f.start(&[]);
    assert_eq!((n(&v, "hits"), n(&v, "scanned"), n(&v, "loaded") as usize), (0, 1, EFFECTS), "{v}");
    assert!(lines(&v, "lines").contains("modification time"), "{v}");
    assert_eq!(n(&f.start(&[]), "hits"), 1, "the rescan refreshed the entry");

    // Another host name: plug-ins describe themselves differently to different hosts.
    let v = f.start(&[("EFFECTCRAFT_OFX_HOST_NAME", "SomeHost")]);
    assert_eq!((n(&v, "hits"), n(&v, "scanned")), (0, 1), "{v}");
    assert!(lines(&v, "problems").contains("host name") && lines(&v, "problems").contains("SomeHost"), "{v}");
    // ... and back: the file now says SomeHost.
    let v = f.start(&[]);
    assert_eq!((n(&v, "hits"), n(&v, "scanned")), (0, 1), "{v}");
    assert!(lines(&v, "problems").contains("written for the host name \"SomeHost\""), "{v}");
    assert_eq!(n(&f.start(&[]), "hits"), 1);

    // A corrupt file is reported, ignored and rebuilt.
    std::fs::write(f.cfg.join("ofx_cache.json"), b"{\"format\": 1, \"binaries\": [not json").unwrap();
    let v = f.start(&[]);
    assert_eq!((n(&v, "hits"), n(&v, "scanned"), n(&v, "loaded") as usize), (0, 1, EFFECTS), "{v}");
    assert!(lines(&v, "problems").contains("not valid JSON"), "{v}");
    assert!(v["errors"].as_array().is_some_and(Vec::is_empty), "{v}");
    assert_eq!(n(&f.start(&[]), "hits"), 1, "the file was rebuilt");
    // An empty file and a file full of garbage bytes too.
    for junk in [&b""[..], &[0xff, 0xfe, 0x00, 0x01][..]] {
        std::fs::write(f.cfg.join("ofx_cache.json"), junk).unwrap();
        let v = f.start(&[]);
        assert_eq!((n(&v, "hits"), n(&v, "loaded") as usize), (0, EFFECTS), "{v}");
        assert_eq!(n(&f.start(&[]), "hits"), 1);
    }

    // "scan_cache": false turns it off: everything is loaded, and the file is left alone.
    let before = f.cache_text();
    std::fs::write(f.cfg.join("ofx_settings.json"), b"{\"scan_cache\": false}").unwrap();
    let v = f.start(&[]);
    assert_eq!((v["enabled"].clone(), n(&v, "hits"), n(&v, "loads_after_scan")), (json!(false), 0, 1), "{v}");
    assert_eq!(n(&v, "loaded") as usize, EFFECTS);
    assert_eq!(f.cache_text(), before);
    std::fs::write(f.cfg.join("ofx_settings.json"), b"{\"scan_cache\": true}").unwrap();
    assert_eq!(n(&f.start(&[]), "hits"), 1);
}

/// A cached plug-in that cannot be loaded when it is first used, or no longer describes itself as
/// it did, fails that render with a message (no crash), marks its cache entry out of date, and the
/// next start scans the binary again.
#[test]
fn a_lazy_load_that_fails_is_a_render_error_and_marks_the_entry_stale() {
    let Some(f) = Fixture::new("stale") else { return };
    let _ = f.start(&[]);
    for (mode, needle) in [
        // Describe fails now (a licence that ran out, a missing runtime).
        ("describe-fails", "Describe failed"),
        // Load declines.
        ("load-fails", "Load failed"),
        // It describes itself differently than when it was cached.
        ("renamed", "no longer matches its description in the scan cache"),
        // The library no longer lists plug-ins.
        ("no-plugins", "exports no OpenFX plug-ins"),
    ] {
        let v = f.start(&[("EC_TEST_OFX_MODE", mode), ("EC_SCAN_CACHE_RENDER", INVERT)]);
        // Registered from the cache without loading (the plug-in's mode is not even looked at) ...
        assert_eq!((n(&v, "hits"), n(&v, "loads_after_scan"), n(&v, "loaded") as usize), (1, 0, EFFECTS), "{mode}: {v}");
        // ... and the render fails with a reason.
        assert_eq!(v["render"]["ok"], false, "{mode}: {v}");
        let e = v["render"]["error"].as_str().unwrap_or_default();
        assert!(e.contains("could not be loaded") && e.contains(needle), "{mode}: {e}");
        assert!(f.cache_text().contains("\"stale\""), "{mode}: the entry should be marked out of date");
        // The next start scans the binary again (and says why); the plug-in works again.
        let v = f.start(&[("EC_SCAN_CACHE_RENDER", INVERT)]);
        assert_eq!((n(&v, "hits"), n(&v, "scanned")), (0, 1), "{mode}: {v}");
        assert!(lines(&v, "lines").contains("marked out of date"), "{mode}: {v}");
        assert_eq!(v["render"]["ok"], true, "{mode}: {v}");
        assert!(!f.cache_text().contains("\"stale\""), "{mode}");
    }
}

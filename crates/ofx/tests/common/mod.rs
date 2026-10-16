//! Helpers shared by the integration tests that load copies of the test plug-in.

#![allow(dead_code)]

use std::path::PathBuf;

/// The platform binary inside the bundle named by `EC_TEST_OFX_BUNDLE`.
pub fn test_binary() -> Option<PathBuf> {
    let bundle = PathBuf::from(std::env::var_os("EC_TEST_OFX_BUNDLE")?);
    let contents = std::fs::read_dir(bundle.join("Contents")).ok()?;
    contents
        .filter_map(|e| e.ok())
        .flat_map(|d| std::fs::read_dir(d.path()).into_iter().flatten().filter_map(|e| e.ok()))
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "ofx"))
}

/// A scratch folder for this test process.
pub fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("effectcraft-ofx-tests-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Flattened default values and texts of a plug-in's parameters, in declaration order. `set`
/// overrides by parameter name (a suffix match, as the host may prefix ids with a group path).
pub fn defaults(p: &dyn effectcraft_effects::plugin::EffectPlugin, set: &[(&str, &[f64])]) -> (Vec<f64>, Vec<String>) {
    use effectcraft_effects::plugin::PluginParamKind;
    let (mut flat, mut texts) = (vec![], vec![]);
    for q in &p.manifest().params {
        if let Some((_, v)) = set.iter().find(|(n, _)| q.id.to_lowercase().ends_with(&n.to_lowercase())) {
            flat.extend_from_slice(v);
            continue;
        }
        match &q.kind {
            PluginParamKind::Slider { default, .. } => flat.push(*default),
            PluginParamKind::Angle { default } => flat.push(*default),
            PluginParamKind::Checkbox { default } => flat.push(*default as u8 as f64),
            PluginParamKind::Popup { default, .. } => flat.push(*default as f64),
            PluginParamKind::Point { default } => flat.extend(default),
            PluginParamKind::Color { default } => flat.extend(default),
            PluginParamKind::Point3 { default } => flat.extend(default),
            PluginParamKind::Layer => flat.push(-1.0),
            PluginParamKind::Text { default } | PluginParamKind::Hidden { default } => texts.push(default.clone()),
        }
    }
    (flat, texts)
}

/// A config folder (the host's state folder: scan cache, settings, crash records) and a plug-in
/// folder holding a copy of the test bundle's binary, for scenarios that need start-ups of their
/// own: registering effects is permanent for a process, so every start-up is a child process, the
/// test binary itself running one `#[test]` that reads `EC_FIXTURE_CFG` / `EC_FIXTURE_PLUGINS` and
/// prints a `RESULT {json}` line.
pub struct Fixture {
    pub root: PathBuf,
    pub cfg: PathBuf,
    pub plugins: PathBuf,
    pub bin: PathBuf,
}

impl Fixture {
    pub fn new(name: &str) -> Option<Fixture> {
        let Some(src) = test_binary() else {
            println!("note: EC_TEST_OFX_BUNDLE is not set; skipping this OFX scenario");
            return None;
        };
        let root = std::env::temp_dir().join(format!("effectcraft-ofx-fixture-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let platform = src.parent().and_then(std::path::Path::file_name).map(|n| n.to_owned()).unwrap_or_default();
        let dir = root.join("plugins").join("cachetest.ofx.bundle").join("Contents").join(&platform);
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("cachetest.ofx");
        std::fs::copy(&src, &bin).unwrap();
        Some(Fixture { cfg: root.join("config"), plugins: root.join("plugins"), root, bin })
    }

    /// One start-up: this test binary, running only the test `child`, with `env` added.
    pub fn run(&self, child: &str, env: &[(&str, &str)]) -> std::process::Output {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", child, "--nocapture", "--test-threads=1"])
            .env("EC_FIXTURE_CFG", &self.cfg)
            .env("EC_FIXTURE_PLUGINS", &self.plugins)
            .env_remove("EFFECTCRAFT_OFX_HOST_NAME")
            .env_remove("EC_TEST_OFX_MODE")
            .env_remove("EC_TEST_OFX_CRASH")
            .envs(env.iter().copied())
            .output()
            .unwrap()
    }

    /// The `RESULT` line of a child's output.
    pub fn result_of(out: &std::process::Output) -> Option<serde_json::Value> {
        let stdout = String::from_utf8_lossy(&out.stdout);
        let line = stdout.lines().find_map(|l| l.split_once("RESULT ").map(|(_, json)| json))?;
        serde_json::from_str(line).ok()
    }

    /// A start-up that must succeed: its result.
    pub fn start(&self, child: &str, env: &[(&str, &str)]) -> serde_json::Value {
        let out = self.run(child, env);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "the child failed ({}):\n{stdout}\n{}", out.status, String::from_utf8_lossy(&out.stderr));
        Fixture::result_of(&out).unwrap_or_else(|| panic!("no RESULT line in:\n{stdout}"))
    }

    pub fn cache_text(&self) -> String {
        std::fs::read_to_string(self.cfg.join("ofx_cache.json")).unwrap_or_default()
    }

    /// Give the binary another modification time (as an update would).
    pub fn touch(&self) {
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(120);
        std::fs::File::options().write(true).open(&self.bin).unwrap().set_modified(later).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

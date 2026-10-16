//! How the OFX host presents itself to plug-ins, and where that is configured.
//!
//! Some commercial plug-ins only run in hosts they know and decline everything else (Frischluft
//! names DaVinci Resolve and Fusion Studio; Topaz documents Resolve Studio), and the only thing a
//! plug-in can check is the host's name, `kOfxPropName`. The name is "EffectCraft" unless the user
//! changes it, in either of two places:
//!
//! * `ofx_settings.json` in the app's config folder, next to `ofx_scan.txt` (`EFFECTCRAFT_CONFIG_DIR`
//!   and portable mode move it with the rest of the settings): `{"host_name": "SomeHost"}`. The
//!   commands `effect.plugins.ofxHostName` and `effect.plugins.setOfxHostName` read and write it.
//! * the environment variable `EFFECTCRAFT_OFX_HOST_NAME`, which wins over the file.
//!
//! Both are read **once**, when the host block is built for the first plug-in load (the config
//! folder is known by then, see [`crate::set_state_dir`]), so a change applies from the next start.
//! A missing file is normal. A file that cannot be read or understood never stops a load: the
//! default name is used and the problem is reported ([`HostIdentity::problem`], shown in
//! `ofx_scan.txt` and logged).
//!
//! The name is a claim, not a capability: a plug-in that recognises another host's name may switch
//! to code paths that host supports (GPU buffers, vendor suites) and fail for it.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::{Map, Value, json};

/// The settings file inside the config folder.
pub const FILE: &str = "ofx_settings.json";
/// The environment variable that overrides the stored name.
pub const ENV_HOST_NAME: &str = "EFFECTCRAFT_OFX_HOST_NAME";
/// The name plug-ins see unless it is changed.
pub const DEFAULT_HOST_NAME: &str = "EffectCraft";
/// The longest host name accepted.
const MAX_NAME: usize = 128;

/// Where the host name in use came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Default,
    Settings,
    Environment,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Default => "default",
            Source::Settings => "settings file",
            Source::Environment => "environment variable",
        }
    }
}

/// The host name in use for this process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostIdentity {
    pub name: String,
    pub source: Source,
    /// Something wrong with the settings file (the name fell back to the default).
    pub problem: Option<String>,
}

/// What the settings file says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stored {
    /// The configured host name, if the file has a usable one.
    pub host_name: Option<String>,
    /// Why the file (or its `host_name`) was not used, if it was not.
    pub problem: Option<String>,
}

/// A usable host name: trimmed, 1 to [`MAX_NAME`] characters, no control characters.
fn valid_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("it is empty".into());
    }
    if name.chars().count() > MAX_NAME {
        return Err(format!("it is longer than {MAX_NAME} characters"));
    }
    if name.chars().any(char::is_control) {
        return Err("it contains control characters".into());
    }
    Ok(name.to_string())
}

/// The settings file in `dir` as a JSON object (empty when the file does not exist), or why it is
/// not usable.
fn read_object(dir: &Path) -> Result<Map<String, Value>, String> {
    let path = dir.join(FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(e) => return Err(format!("{} could not be read ({e})", path.display())),
    };
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(m)) => Ok(m),
        Ok(_) => Err(format!("{} is not a JSON object", path.display())),
        Err(e) => Err(format!("{} is not valid JSON ({e})", path.display())),
    }
}

/// Read the settings file in `dir` (`None`: no config folder, nothing stored). Never fails: what
/// cannot be used is described in [`Stored::problem`].
pub fn read_stored(dir: Option<&Path>) -> Stored {
    let Some(dir) = dir else { return Stored::default() };
    let obj = match read_object(dir) {
        Ok(o) => o,
        Err(p) => return Stored { host_name: None, problem: Some(format!("{p}; the default host name \"{DEFAULT_HOST_NAME}\" is used")) },
    };
    match obj.get("host_name") {
        None | Some(Value::Null) => Stored::default(),
        Some(Value::String(s)) if s.trim().is_empty() => Stored::default(),
        Some(Value::String(s)) => match valid_name(s) {
            Ok(n) => Stored { host_name: Some(n), problem: None },
            Err(why) => Stored {
                host_name: None,
                problem: Some(format!(
                    "host_name in {} is not usable ({why}); the default host name \"{DEFAULT_HOST_NAME}\" is used",
                    dir.join(FILE).display()
                )),
            },
        },
        Some(_) => Stored {
            host_name: None,
            problem: Some(format!("host_name in {} must be a string; the default host name \"{DEFAULT_HOST_NAME}\" is used", dir.join(FILE).display())),
        },
    }
}

/// The host name for a process: the environment variable `env` if it is set and not blank, else
/// the stored name, else the default.
pub fn resolve(env: Option<&str>, stored: &Stored) -> HostIdentity {
    let problem = stored.problem.clone();
    if let Some(e) = env.map(str::trim).filter(|e| !e.is_empty()) {
        return HostIdentity { name: e.to_string(), source: Source::Environment, problem };
    }
    match &stored.host_name {
        Some(n) => HostIdentity { name: n.clone(), source: Source::Settings, problem },
        None => HostIdentity { name: DEFAULT_HOST_NAME.to_string(), source: Source::Default, problem },
    }
}

fn env_value() -> Option<String> {
    std::env::var(ENV_HOST_NAME).ok().filter(|s| !s.trim().is_empty())
}

static IDENTITY: OnceLock<HostIdentity> = OnceLock::new();

/// The host name in use: decided the first time this is called (environment variable, settings
/// file in the config folder, default), then fixed for the life of the process.
pub fn identity() -> &'static HostIdentity {
    IDENTITY.get_or_init(|| {
        let dir = crate::blocklist::state_dir();
        let id = resolve(env_value().as_deref(), &read_stored(dir.as_deref()));
        log::info!("openfx: host name shown to plug-ins: \"{}\" ({})", id.name, id.source.as_str());
        if let Some(p) = &id.problem {
            log::warn!("openfx: {p}");
        }
        id
    })
}

/// The settings file's path, when there is a config folder.
pub fn settings_path() -> Option<PathBuf> {
    crate::blocklist::state_dir().map(|d| d.join(FILE))
}

/// Store `name` as the host name in `dir`'s settings file (an empty name removes it, so the default
/// applies again), keeping any other keys. Written through a temporary file, so a failure leaves
/// the old file as it was.
pub fn store_host_name(dir: &Path, name: &str) -> Result<(), String> {
    let name = name.trim();
    let name = if name.is_empty() { None } else { Some(valid_name(name).map_err(|why| format!("not a usable host name: {why}"))?) };
    // A file that is not JSON is replaced (its content could not be kept anyway).
    let mut obj = read_object(dir).unwrap_or_default();
    match name {
        Some(n) => {
            obj.insert("host_name".into(), Value::String(n));
        }
        None => {
            obj.remove("host_name");
        }
    }
    let path = dir.join(FILE);
    if obj.is_empty() {
        return match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("cannot update {}: {e}", path.display())),
        };
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let text = serde_json::to_string_pretty(&Value::Object(obj)).map_err(|e| e.to_string())?;
    let tmp = dir.join(format!("{FILE}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, format!("{text}\n")).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("cannot update {}: {e}", path.display())
    })
}

/// The command behind `effect.plugins.ofxHostName` (`set` = `None`) and
/// `effect.plugins.setOfxHostName` (`set` = the new name; empty resets it): the name in use now,
/// where it came from, what the settings file holds, and whether that differs from what is in use
/// (a change applies from the next start).
pub fn host_name_command(set: Option<&str>) -> Result<Value, String> {
    let dir = crate::blocklist::state_dir();
    if let Some(name) = set {
        let dir = dir.as_deref().ok_or("EffectCraft has no config folder here, so the host name cannot be stored")?;
        store_host_name(dir, name)?;
    }
    let now = identity();
    let stored = read_stored(dir.as_deref());
    let env = env_value();
    let next = resolve(env.as_deref(), &stored);
    let restart = next.name != now.name;
    let mut out = json!({
        "name": now.name,
        "source": now.source.as_str(),
        "configured": stored.host_name,
        "environmentOverride": env,
        "settingsFile": dir.as_deref().map(|d| d.join(FILE).to_string_lossy().into_owned()),
        "problem": stored.problem.clone().or_else(|| now.problem.clone()),
        "restartRequired": restart,
    });
    if restart {
        out["next"] = json!(next.name);
        out["note"] = json!("the new host name applies the next time EffectCraft starts (plug-ins are told the host name once, when they load)");
    } else if set.is_some() {
        out["note"] = json!("the host name in use is unchanged");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, empty folder under the system temp folder; removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("effectcraft-ofx-settings-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn write(&self, text: &str) {
            std::fs::write(self.0.join(FILE), text).unwrap();
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_default_is_effectcraft() {
        let id = resolve(None, &read_stored(None));
        assert_eq!((id.name.as_str(), id.source, id.problem), ("EffectCraft", Source::Default, None));
        // A folder without a file is the same.
        let t = TempDir::new("missing");
        assert_eq!(read_stored(Some(&t.0)), Stored::default());
    }

    #[test]
    fn the_file_sets_the_name_and_the_environment_overrides_it() {
        let t = TempDir::new("file");
        t.write(r#"{"host_name": "  DaVinciResolve  ", "other": 1}"#);
        let stored = read_stored(Some(&t.0));
        assert_eq!(stored.host_name.as_deref(), Some("DaVinciResolve"));
        let id = resolve(None, &stored);
        assert_eq!((id.name.as_str(), id.source), ("DaVinciResolve", Source::Settings));
        let id = resolve(Some(" Fusion "), &stored);
        assert_eq!((id.name.as_str(), id.source), ("Fusion", Source::Environment));
        // A blank variable does not override.
        assert_eq!(resolve(Some("  "), &stored).source, Source::Settings);
    }

    #[test]
    fn a_bad_file_never_fails_and_is_reported() {
        let t = TempDir::new("bad");
        for (text, needle) in [
            ("{not json", "not valid JSON"),
            ("[1, 2]", "not a JSON object"),
            (r#"{"host_name": 5}"#, "must be a string"),
            (r#"{"host_name": "a\u0007b"}"#, "control characters"),
            (&format!(r#"{{"host_name": "{}"}}"#, "x".repeat(200)), "longer than"),
        ] {
            t.write(text);
            let stored = read_stored(Some(&t.0));
            assert_eq!(stored.host_name, None, "{text}");
            let problem = stored.problem.as_deref().unwrap_or_default();
            assert!(problem.contains(needle) && problem.contains("EffectCraft"), "{text}: {problem}");
            // The process still gets a name.
            assert_eq!(resolve(None, &stored).name, "EffectCraft");
        }
        // An empty file, a null or an empty name are "not set", not problems.
        for text in ["", "  \n", r#"{"host_name": null}"#, r#"{"host_name": ""}"#, "{}"] {
            t.write(text);
            assert_eq!(read_stored(Some(&t.0)), Stored::default(), "{text}");
        }
    }

    #[test]
    fn storing_a_name_round_trips_keeps_other_keys_and_resets() {
        let t = TempDir::new("store");
        assert!(store_host_name(&t.0, "  ").is_ok(), "resetting nothing is fine");
        assert!(!t.0.join(FILE).exists());
        store_host_name(&t.0, " Natron ").unwrap();
        assert_eq!(read_stored(Some(&t.0)).host_name.as_deref(), Some("Natron"));
        // Other keys survive a change.
        t.write(r#"{"host_name": "A", "keep": [1, 2]}"#);
        store_host_name(&t.0, "B").unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(t.0.join(FILE)).unwrap()).unwrap();
        assert_eq!((v["host_name"].as_str(), v["keep"].clone()), (Some("B"), json!([1, 2])));
        // Resetting removes the key (and the file once it is empty).
        store_host_name(&t.0, "").unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(t.0.join(FILE)).unwrap()).unwrap();
        assert!(v.get("host_name").is_none() && v["keep"] == json!([1, 2]));
        t.write(r#"{"host_name": "A"}"#);
        store_host_name(&t.0, "").unwrap();
        assert!(!t.0.join(FILE).exists());
        // A corrupt file is replaced; a bad name is refused and nothing is written.
        t.write("{broken");
        store_host_name(&t.0, "C").unwrap();
        assert_eq!(read_stored(Some(&t.0)).host_name.as_deref(), Some("C"));
        assert!(store_host_name(&t.0, "bad\nname").unwrap_err().contains("control characters"));
        assert_eq!(read_stored(Some(&t.0)).host_name.as_deref(), Some("C"));
        // No temporary files are left behind.
        assert_eq!(std::fs::read_dir(&t.0).unwrap().count(), 1);
    }
}

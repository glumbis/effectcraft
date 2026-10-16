//! The engine commands around the OpenFX scan cache: `effect.plugins.scanOfx` reports what the cache
//! did and which plug-ins crashed the last session, and `effect.plugins.rescanOfx` deletes the cache
//! file (an agent never gets to run either, see the automation crate's confinement).
//!
//! The config folder is a scratch folder given to the OFX crate directly (it is process-wide, so this
//! is the only test in this file, and the session is built by hand instead of through
//! `effectcraft_host::session()`, which would point it at the user's real folder). Set
//! `EC_TEST_OFX_BUNDLE` to the built `ec-test-ofx.ofx.bundle`; without it the test returns.

#![cfg(feature = "openfx")]

use std::path::PathBuf;

use serde_json::json;

/// The platform binary inside the bundle named by `EC_TEST_OFX_BUNDLE`.
fn test_binary() -> Option<PathBuf> {
    let bundle = PathBuf::from(std::env::var_os("EC_TEST_OFX_BUNDLE")?);
    std::fs::read_dir(bundle.join("Contents"))
        .ok()?
        .filter_map(|e| e.ok())
        .flat_map(|d| std::fs::read_dir(d.path()).into_iter().flatten().filter_map(|e| e.ok()))
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "ofx"))
}

#[test]
fn scanning_reports_the_cache_and_rescanning_deletes_it() {
    let Some(bin) = test_binary() else {
        println!("note: EC_TEST_OFX_BUNDLE is not set; skipping the scan cache command test");
        return;
    };
    let root = std::env::temp_dir().join(format!("effectcraft-host-ofx-cache-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let platform = bin.parent().and_then(std::path::Path::file_name).map(|n| n.to_owned()).unwrap_or_default();
    let dir = root.join("plugins").join("Hosted.ofx.bundle").join("Contents").join(platform);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(&bin, dir.join("Hosted.ofx")).unwrap();
    let cfg = root.join("config");
    effectcraft_ofx::set_state_dir(cfg.clone());

    let mut s = effectcraft_engine::Session {
        ofx_loader: Some(effectcraft_ofx::loader),
        ofx_rescan: Some(effectcraft_ofx::rescan_command),
        ofx_search_paths: vec![root.join("plugins")],
        ..Default::default()
    };
    assert!(s.is_enabled("effect.plugins.scanOfx") && s.is_enabled("effect.plugins.rescanOfx"));

    // The first scan loads the binary and writes the cache; the result says so, and that nothing
    // crashed the session before.
    let r = s.execute("effect.plugins.scanOfx", json!({})).unwrap();
    assert!(r["errors"].as_array().is_some_and(Vec::is_empty), "{r}");
    assert!(r["loaded"].as_array().is_some_and(|l| l.len() >= 9), "{r}");
    let cache = &r["cache"];
    assert_eq!((cache["enabled"].clone(), cache["hits"].clone(), cache["scanned"].clone()), (json!(true), json!(0), json!(1)), "{r}");
    assert_eq!(cache["file"].as_str().map(PathBuf::from), Some(cfg.join("ofx_cache.json")), "{r}");
    assert!(cache["lines"].as_array().is_some_and(|l| l.len() == 1), "{r}");
    assert_eq!(r["crashes"], json!([]), "{r}");
    assert!(cfg.join("ofx_cache.json").is_file());

    // Rescanning deletes the cache (at the next start everything is loaded again) and says that
    // takes effect at the next start; it can be repeated, and it never touches other files.
    std::fs::write(cfg.join("ofx_settings.json"), b"{\"scan_cache\": true}").unwrap();
    let r = s.execute("effect.plugins.rescanOfx", json!({})).unwrap();
    assert_eq!((r["cleared"].clone(), r["restartRequired"].clone()), (json!(true), json!(true)), "{r}");
    assert_eq!(r["file"].as_str().map(PathBuf::from), Some(cfg.join("ofx_cache.json")), "{r}");
    assert!(!cfg.join("ofx_cache.json").exists());
    assert!(cfg.join("ofx_settings.json").is_file());
    assert!(s.execute("effect.plugins.rescanOfx", json!({})).is_ok());

    // `cache: false` asks for a scan that does not read the cache; a build without OpenFX has no command.
    let r = s.execute("effect.plugins.scanOfx", json!({"cache": false})).unwrap();
    assert!(r["errors"].as_array().is_some_and(Vec::is_empty), "{r}");
    let mut none = effectcraft_engine::Session::default();
    assert!(!none.is_enabled("effect.plugins.rescanOfx"));
    assert!(none.execute("effect.plugins.rescanOfx", json!({})).is_err());
    let _ = std::fs::remove_dir_all(&root);
}

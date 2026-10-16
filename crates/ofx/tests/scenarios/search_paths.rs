//! The folders OpenFX plug-ins are searched in, as the host makes them at start-up: the config
//! folder (set with `set_state_dir`, as `effectcraft_host::session` does before it asks for the
//! paths) holds a per-user `OFX` folder, and on Linux and macOS the per-user data folders are there
//! too. This needs a process of its own (the state folder is process-wide): the test binary runs
//! itself with the `child_paths` test selected, as the other scenarios do. No plug-in bundle needed.

use std::path::{Path, PathBuf};

use serde_json::json;

/// The child process: the search paths after `set_state_dir`. A normal run does nothing here.
#[test]
fn child_paths() {
    let Some(cfg) = std::env::var_os("EC_FIXTURE_CFG") else { return };
    effectcraft_ofx::set_state_dir(PathBuf::from(cfg));
    let paths: Vec<String> = effectcraft_ofx::default_search_paths().iter().map(|p| p.display().to_string()).collect();
    println!("RESULT {}", json!({ "paths": paths }));
}

#[test]
fn the_config_folder_and_the_per_user_folders_are_searched() {
    let root = std::env::temp_dir().join(format!("effectcraft-ofx-search-{}", std::process::id()));
    let (cfg, xdg, home) = (root.join("config"), root.join("xdg"), root.join("home"));
    std::fs::create_dir_all(cfg.join("OFX")).unwrap();
    // The config folder's `OFX` is also named by `OFX_PLUGIN_PATH`: it must be listed once.
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "search_paths::child_paths", "--nocapture", "--test-threads=1"])
        .env("EC_FIXTURE_CFG", &cfg)
        .env("OFX_PLUGIN_PATH", cfg.join("OFX"))
        .env("XDG_DATA_HOME", &xdg)
        .env("HOME", &home)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "the child failed ({}):\n{stdout}\n{}", out.status, String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = stdout
        .lines()
        .find_map(|l| l.split_once("RESULT ").map(|(_, j)| j))
        .and_then(|j| serde_json::from_str(j).ok())
        .unwrap_or_else(|| panic!("no RESULT line in:\n{stdout}"));
    let paths: Vec<PathBuf> = json["paths"].as_array().unwrap().iter().map(|p| PathBuf::from(p.as_str().unwrap())).collect();
    let listed = |p: &Path| paths.iter().filter(|q| *q == p).count();

    assert_eq!(listed(&cfg.join("OFX")), 1, "<config>/OFX once, everywhere: {paths:?}");
    #[cfg(target_os = "linux")]
    {
        assert_eq!(listed(&xdg.join("OFX/Plugins")), 1, "{paths:?}");
        assert_eq!(listed(Path::new("/usr/local/OFX/Plugins")), 1, "{paths:?}");
        assert_eq!(listed(Path::new("/usr/OFX/Plugins")), 1, "the existing default stays: {paths:?}");
    }
    #[cfg(target_os = "macos")]
    {
        assert_eq!(listed(&home.join("Library/OFX/Plugins")), 1, "{paths:?}");
        assert_eq!(listed(Path::new("/Library/OFX/Plugins")), 1, "the existing default stays: {paths:?}");
    }
    #[cfg(windows)]
    {
        let _ = (&xdg, &home);
        assert!(paths.iter().any(|p| p.ends_with(r"OFX\Plugins")), "the Common Files default stays: {paths:?}");
    }
    // `OFX` next to the executable stays too.
    let exe_ofx = std::env::current_exe().unwrap().parent().unwrap().join("OFX");
    assert_eq!(listed(&exe_ofx), 1, "{paths:?}");
    let _ = std::fs::remove_dir_all(&root);
}

//! The name OpenFX plug-ins are told the host has is a setting, readable and writable through the
//! engine commands (`effect.plugins.ofxHostName`, `effect.plugins.setOfxHostName`) and kept in
//! `ofx_settings.json` in the config folder.
//!
//! The config folder is a scratch folder given to the OFX crate directly (it is process-wide, so
//! this is the only test in this file, and the session is built by hand instead of through
//! `effectcraft_host::session()`, which would point it at the user's real folder).

#![cfg(feature = "openfx")]

use std::path::PathBuf;

use serde_json::json;

#[test]
fn the_host_name_is_a_setting_the_commands_read_and_write() {
    let dir = std::env::temp_dir().join(format!("effectcraft-host-ofx-name-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    effectcraft_ofx::set_state_dir(dir.clone());
    let mut s = effectcraft_engine::Session { ofx_host_name: Some(effectcraft_ofx::host_name_command), ..Default::default() };
    assert!(s.is_enabled("effect.plugins.setOfxHostName"));
    // (A developer's own EFFECTCRAFT_OFX_HOST_NAME would make the first answers differ.)
    let env = std::env::var_os("EFFECTCRAFT_OFX_HOST_NAME").is_some_and(|v| !v.to_string_lossy().trim().is_empty());

    // Nothing stored: the default, from no file.
    let r = s.execute("effect.plugins.ofxHostName", json!({})).unwrap();
    if !env {
        assert_eq!((r["name"].as_str(), r["source"].as_str()), (Some("EffectCraft"), Some("default")), "{r}");
    }
    assert_eq!(r["settingsFile"].as_str().map(PathBuf::from), Some(dir.join("ofx_settings.json")), "{r}");
    assert!(r["configured"].is_null(), "{r}");
    let in_use = r["name"].as_str().unwrap().to_string();

    // Setting it stores it, but plug-ins keep the name this process told them until the next start.
    let r = s.execute("effect.plugins.setOfxHostName", json!({"name": " DaVinciResolve "})).unwrap();
    assert_eq!(r["name"], in_use.as_str(), "{r}");
    assert_eq!(r["configured"].as_str(), Some("DaVinciResolve"), "{r}");
    if !env {
        assert_eq!((r["restartRequired"].as_bool(), r["next"].as_str()), (Some(true), Some("DaVinciResolve")), "{r}");
        assert!(r["note"].as_str().is_some_and(|n| n.contains("next time EffectCraft starts")), "{r}");
    }
    let file = std::fs::read_to_string(dir.join("ofx_settings.json")).unwrap();
    assert!(file.contains("\"host_name\"") && file.contains("DaVinciResolve"), "{file}");
    // That file is what the next start reads.
    assert_eq!(effectcraft_ofx::settings::read_stored(Some(&dir)).host_name.as_deref(), Some("DaVinciResolve"));
    let r = s.execute("effect.plugins.ofxHostName", json!({})).unwrap();
    assert_eq!(r["configured"].as_str(), Some("DaVinciResolve"), "{r}");

    // An empty name resets it: the file goes, nothing is pending.
    let r = s.execute("effect.plugins.setOfxHostName", json!({"name": ""})).unwrap();
    assert!(r["configured"].is_null() && r["restartRequired"] == false, "{r}");
    assert!(!dir.join("ofx_settings.json").exists());

    // A name that cannot be used, or none, is refused and changes nothing.
    assert!(s.execute("effect.plugins.setOfxHostName", json!({"name": "bad\nname"})).is_err());
    assert!(s.execute("effect.plugins.setOfxHostName", json!({})).is_err());
    assert!(!dir.join("ofx_settings.json").exists());

    // Without the hook (a build without OpenFX) the commands are disabled and refuse.
    let mut none = effectcraft_engine::Session::default();
    assert!(!none.is_enabled("effect.plugins.setOfxHostName"));
    assert!(none.execute("effect.plugins.setOfxHostName", json!({"name": "x"})).is_err());
    assert!(none.execute("effect.plugins.ofxHostName", json!({})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

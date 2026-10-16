//! Copies of one OpenFX plug-in in different search folders (the system's folder and the per-user
//! one, an old release beside a new one) are told apart by version in *one* scan: `effect.plugins.scanOfx`
//! hands the loader every folder at once. It used to load one folder after the other, so whichever copy
//! sat in the folder searched first stayed, whatever its version, and the others were only known as
//! "already registered".
//!
//! Registering plug-ins is permanent for a process, so this is the only test in this file. Set
//! `EC_TEST_OFX_BUNDLE` to the built `ec-test-ofx.ofx.bundle`; without it the test returns. (The test
//! plug-in has one version, so which version wins is covered by the loader's unit tests; here the
//! versions are equal and the copy found first stays.)

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
fn copies_in_different_search_folders_are_resolved_in_one_scan() {
    let Some(bin) = test_binary() else {
        println!("note: EC_TEST_OFX_BUNDLE is not set; skipping the cross-folder duplicate test");
        return;
    };
    let root = std::env::temp_dir().join(format!("effectcraft-host-ofx-dedupe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let platform = bin.parent().and_then(std::path::Path::file_name).map(|n| n.to_owned()).unwrap_or_default();
    for folder in ["first-folder", "second-folder"] {
        let dir = root.join(folder).join("Dup.ofx.bundle").join("Contents").join(&platform);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::copy(&bin, dir.join("Dup.ofx")).unwrap();
    }
    effectcraft_ofx::set_state_dir(root.join("config"));
    // The search order of a real scan: the system's folder, then the user's; a folder that does not
    // exist is skipped.
    let mut s = effectcraft_engine::Session {
        ofx_loader: Some(effectcraft_ofx::loader),
        ofx_search_paths: vec![root.join("first-folder"), root.join("missing"), root.join("second-folder")],
        ..Default::default()
    };
    let r = s.execute("effect.plugins.scanOfx", json!({"cache": false})).unwrap();
    assert!(r["errors"].as_array().is_some_and(Vec::is_empty), "{r}");
    assert_eq!(r["paths"].as_array().map(Vec::len), Some(2), "{r}");
    // Every plug-in comes from the first folder, once.
    let loaded = r["loaded"].as_array().unwrap();
    assert_eq!(loaded.len(), 16, "{r}");
    assert!(loaded.iter().all(|e| e["bundle"].as_str().is_some_and(|b| b.contains("first-folder"))), "{r}");
    // The second folder's copy lost to the first one *in this scan* (it names the copy that is kept
    // and its version), not to a registration an earlier scan left behind.
    let notes: Vec<&str> = r["notes"].as_array().unwrap().iter().filter_map(|n| n.as_str()).collect();
    let note = notes.iter().find(|n| n.contains("duplicate plug-in(s) skipped")).unwrap_or_else(|| panic!("{notes:?}"));
    assert!(note.contains("second-folder") && note.contains("16 duplicate") && note.contains("kept 1.0 in") && !note.contains("already registered"), "{note}");
    let transcript: Vec<&str> = r["transcript"].as_array().unwrap().iter().filter_map(|t| t.as_str()).collect();
    let second = transcript.iter().find(|t| t.contains("second-folder")).unwrap_or_else(|| panic!("{transcript:?}"));
    assert!(second.contains("skipped, duplicate:") && !second.contains("Load ->"), "{second}");
    let _ = std::fs::remove_dir_all(&root);
}

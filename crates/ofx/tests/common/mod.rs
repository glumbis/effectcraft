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

//! After Effects project import (`file.importAep`, the `aep` feature). With the feature, the
//! round trip runs when `EC_TEST_AEP` names an `.aep` / `.aepx` (and Python 3 with py-aep is
//! installed); without it the test only checks the command is unavailable.

use serde_json::json;

#[test]
fn aep_import_follows_the_build_feature() {
    let mut s = effectcraft_host::session();
    if !cfg!(feature = "aep") {
        assert!(!s.is_enabled("file.importAep"));
        assert!(s.execute("file.importAep", json!({"path": "x.aep"})).is_err());
        return;
    }
    assert!(s.is_enabled("file.importAep"));
    // A missing file is an error, not a crash.
    assert!(s.execute("file.importAep", json!({"path": "/no/such/project.aep"})).is_err());
    let Some(path) = std::env::var_os("EC_TEST_AEP") else {
        println!("note: EC_TEST_AEP is not set; skipping the .aep round trip");
        return;
    };
    let r = s.execute("file.importAep", json!({"path": path.to_string_lossy()})).unwrap();
    let output = r["output"].as_str().unwrap_or_default();
    assert!(output.starts_with("Imported "), "{r}");
    let comps = s.project.items.values().filter(|i| matches!(i.kind, effectcraft_engine::project::ItemKind::Comp(_))).count();
    assert!(comps > 0, "no comps imported: {output}");
}

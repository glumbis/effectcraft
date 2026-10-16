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

//! What the host tells the user when a plug-in declines the host, fails to load or describe, or says
//! something: the statuses by name, the plug-in's own messages, and the properties it asked for
//! that this host lacks, in `LoadReport::{errors, notes, transcript}`.
//!
//! The test plug-in (`examples/ofx/ec-test-ofx`) misbehaves on request: `EC_TEST_OFX_MODE` names
//! how (it is read when each entry point runs). Every scenario loads its own copy of the binary
//! (a library is only loaded once per path), and the scenarios run one after the other because
//! the mode is process-wide. Set `EC_TEST_OFX_BUNDLE` to the built `ec-test-ofx.ofx.bundle`;
//! without it the tests print a note and return.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use effectcraft_ofx::{LoadReport, load_path};

/// One scenario at a time (the mode is an environment variable).
static SERIAL: Mutex<()> = Mutex::new(());

/// The platform binary inside the bundle named by `EC_TEST_OFX_BUNDLE`.
fn test_binary() -> Option<PathBuf> {
    let bundle = PathBuf::from(std::env::var_os("EC_TEST_OFX_BUNDLE")?);
    let contents = std::fs::read_dir(bundle.join("Contents")).ok()?;
    contents
        .filter_map(|e| e.ok())
        .flat_map(|d| std::fs::read_dir(d.path()).into_iter().flatten().filter_map(|e| e.ok()))
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "ofx"))
}

/// A scratch folder for this test process.
fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("effectcraft-ofx-diagnostics-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Copy the test binary to `<scratch>/<name>.ofx.bundle/Contents/<platform>/<name>.ofx` and return the bundle.
fn copy_bundle(bin: &Path, name: &str) -> PathBuf {
    let platform = bin.parent().and_then(Path::file_name).map(|n| n.to_owned()).unwrap_or_default();
    let bundle = scratch().join(format!("{name}.ofx.bundle"));
    let dir = bundle.join("Contents").join(platform);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(bin, dir.join(format!("{name}.ofx"))).unwrap();
    bundle
}

/// Load a fresh copy of the test plug-in with `EC_TEST_OFX_MODE` set to `mode` while it loads.
fn load_with_mode(name: &str, mode: &str) -> Option<LoadReport> {
    let bin = test_binary().or_else(|| {
        println!("note: EC_TEST_OFX_BUNDLE is not set; skipping the OFX diagnostics tests");
        None
    })?;
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let bundle = copy_bundle(&bin, name);
    // SAFETY: scenarios are serialised by `SERIAL` and nothing else in this process reads the
    // environment concurrently (the plug-in reads it from the thread that loads it).
    unsafe { std::env::set_var("EC_TEST_OFX_MODE", mode) };
    let report = load_path(&bundle);
    unsafe { std::env::remove_var("EC_TEST_OFX_MODE") };
    // `EC_TEST_OFX_VERBOSE=1 cargo test ... -- --nocapture` shows what the user would see.
    if std::env::var_os("EC_TEST_OFX_VERBOSE").is_some() {
        println!(
            "== mode {mode:?} ==
errors: {:#?}
notes: {:#?}
transcript:
{}",
            report.errors,
            report.notes,
            report.transcript.join(
                "
"
            )
        );
    }
    Some(report)
}

#[test]
fn a_plugin_that_declines_the_host_is_skipped_with_a_note() {
    let Some(r) = load_with_mode("decline", "decline-host") else { return };
    assert!(r.loaded.is_empty() && r.errors.is_empty(), "{r:?}");
    let note = r.notes.iter().find(|n| n.contains("declined this host")).unwrap_or_else(|| panic!("notes: {:?}", r.notes));
    assert!(note.contains("OfxSetHost returned kOfxStatFailed (1)") && note.contains("\"EffectCraft\""), "{note}");
    let t = r.transcript.join("\n");
    assert!(t.contains("OfxSetHost -> kOfxStatFailed (1)") && t.contains("host shown to the plug-in: \"EffectCraft\""), "{t}");
}

#[test]
fn a_binary_that_exports_no_plugins_is_noted() {
    let Some(r) = load_with_mode("none", "no-plugins") else { return };
    assert!(r.loaded.is_empty() && r.errors.is_empty(), "{r:?}");
    let note = r.notes.iter().find(|n| n.contains("exports no OpenFX plug-ins")).unwrap_or_else(|| panic!("notes: {:?}", r.notes));
    assert!(note.contains("OfxGetNumberOfPlugins returned 0"), "{note}");
    assert!(r.transcript.join("\n").contains("OfxGetNumberOfPlugins -> 0"));
}

#[test]
fn a_load_that_fails_says_why() {
    let Some(r) = load_with_mode("loadfails", "load-fails") else { return };
    assert!(r.loaded.is_empty(), "{r:?}");
    let e = r.errors.iter().find(|e| e.contains("org.effectcraft.test.Invert")).unwrap_or_else(|| panic!("errors: {:?}", r.errors));
    // The status by name, the likely cause, and what the plug-in said about it.
    assert!(e.contains("Load failed: kOfxStatFailed (1)") && e.contains("declined to load") && e.contains("host name \"EffectCraft\""), "{e}");
    assert!(e.contains("the plug-in said \"The licence file was not found\""), "{e}");
    let t = r.transcript.join("\n");
    assert!(t.contains("Load -> kOfxStatFailed (1)") && t.contains("plug-in said (error): \"The licence file was not found\""), "{t}");
}

#[test]
fn a_describe_that_fails_names_the_missing_host_feature() {
    let Some(r) = load_with_mode("describefails", "describe-fails") else { return };
    assert!(r.loaded.is_empty(), "{r:?}");
    let e = r.errors.iter().find(|e| e.contains("org.effectcraft.test.Invert")).unwrap_or_else(|| panic!("errors: {:?}", r.errors));
    assert!(e.contains("Describe failed: kOfxStatErrMissingHostFeature (4)"), "{e}");
    assert!(e.contains("This host lacks a feature this plug-in needs"), "{e}");
    // The property it asked for, and of which set (the host's).
    assert!(e.contains("property OfxTestOnlyHostFeature of the host"), "{e}");
    let t = r.transcript.join("\n");
    assert!(t.contains("Describe -> kOfxStatErrMissingHostFeature (4)"), "{t}");
    assert!(t.contains("asked for property OfxTestOnlyHostFeature of the host"), "{t}");
}

#[test]
fn a_clean_load_has_a_compact_transcript() {
    let Some(r) = load_with_mode("clean", "") else { return };
    assert!(r.errors.is_empty() && r.notes.is_empty(), "{r:?}");
    assert_eq!(r.loaded.len(), 6);
    let t = r.transcript.join(
        "
",
    );
    for needle in [
        "host shown to the plug-in: \"EffectCraft\"",
        "OfxSetHost -> kOfxStatOK (0)",
        "OfxGetNumberOfPlugins -> 6",
        // A plug-in that registered without anything to report is one line, not a page per effect.
        "org.effectcraft.test.Invert v1.0: registered as effect org.effectcraft.test.invert (filter context) (Load, Describe and DescribeInContext: OK)",
        "org.effectcraft.test.Mix v1.0: registered as effect org.effectcraft.test.mix (general context)",
    ] {
        assert!(
            t.contains(needle),
            "missing `{needle}` in:
{t}"
        );
    }
    assert!(!t.contains("Load -> kOfxStatOK"), "{t}");
}

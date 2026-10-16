//! A plug-in that crashes the process while rendering: the next start names it (the plug-in, the
//! action, on Windows the exception and the module), counts a strike, skips it after two crashes in
//! a row, and forgives it after a clean session that rendered it or when its binary changes.
//!
//! The crashes are real: the test plug-in's `Crash` effect writes through a null pointer
//! (`EC_TEST_OFX_CRASH=segv`) or aborts (`abort`) in `Render`. Every session is a child process of
//! this test binary (`common::Fixture`), running the `child_session` test. Set `EC_TEST_OFX_BUNDLE`
//! to the built `ec-test-ofx.ofx.bundle`; without it the scenarios return.

use std::path::PathBuf;

use crate::common::{Fixture, defaults};
use effectcraft_effects::plugin::{PluginFrame, PluginParams, plugin};
use effectcraft_ofx::load_path;
use serde_json::{Value, json};

const CRASH: &str = "org.effectcraft.test.crash";

/// One session of EffectCraft's OpenFX host. `EC_RC_ACTION`: `report` (start and say what was found),
/// `render` (start and render the Crash effect, which dies when the environment asks it to),
/// `render-clean` (render it, then quit normally), `retry` (start after "retry": forget the strikes).
#[test]
fn child_session() {
    let Some(cfg) = std::env::var_os("EC_FIXTURE_CFG") else { return };
    let plugins = PathBuf::from(std::env::var_os("EC_FIXTURE_PLUGINS").expect("EC_FIXTURE_PLUGINS"));
    let action = std::env::var("EC_RC_ACTION").unwrap_or_else(|_| "report".into());
    #[cfg(windows)]
    {
        // A crash must not open Windows' error dialog and wait for a click.
        use windows_sys::Win32::System::Diagnostics::Debug::{SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SetErrorMode};
        unsafe { SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX) };
    }
    effectcraft_ofx::set_state_dir(PathBuf::from(&cfg));
    if action == "retry" {
        effectcraft_ofx::blocklist::clear();
    }
    let report = load_path(&plugins);
    let registered = plugin(CRASH);
    if action.starts_with("render")
        && let Some(p) = &registered
    {
        let (flat, texts) = defaults(&**p, &[]);
        let params = PluginParams { manifest: p.manifest(), flat: &flat, texts: &texts };
        let mut pixels = vec![[0.2f32, 0.4, 0.6, 1.0]; 4];
        let mut frame = PluginFrame { width: 2, height: 2, pixels: &mut pixels, scale: 1.0, origin: [0.0; 2], layer_size: [2.0, 2.0] };
        p.render(&mut frame, &params, 1.0).expect("the render of an effect that does not crash works");
    }
    let files =
        |prefix: &str| std::fs::read_dir(&cfg).map(|d| d.flatten().filter(|e| e.file_name().to_string_lossy().starts_with(prefix)).count()).unwrap_or(0);
    let in_flight_files_before_quit = files("ofx_inflight.");
    if action == "render-clean" {
        effectcraft_ofx::shutdown();
    }
    let result = json!({
        "crashes": effectcraft_ofx::inflight::recovered_lines(),
        "errors": report.errors,
        "loaded": report.loaded.len(),
        "registered": registered.is_some(),
        "strikes": std::fs::read_to_string(PathBuf::from(&cfg).join("ofx_render_crashes.txt")).ok(),
        "in_flight_files_before_quit": in_flight_files_before_quit,
        "in_flight_files_after_quit": files("ofx_inflight."),
    });
    println!("RESULT {result}");
}

fn text(v: &Value, k: &str) -> String {
    v[k].as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>().join("\n")
}

/// The Crash effect dies in `Render` in a session of its own: the process ends badly.
fn crash(f: &Fixture, how: &str) {
    let out = f.run("render_crash::child_session", &[("EC_RC_ACTION", "render"), ("EC_TEST_OFX_CRASH", how)]);
    assert!(!out.status.success(), "the plug-in should have taken the process down ({how}): {}", String::from_utf8_lossy(&out.stdout));
    assert!(Fixture::result_of(&out).is_none(), "{}", String::from_utf8_lossy(&out.stdout));
}

fn report(f: &Fixture, action: &str) -> Value {
    f.start("render_crash::child_session", &[("EC_RC_ACTION", action)])
}

fn two_crashes_and_the_plugin_is_skipped(how: &str) {
    let Some(f) = Fixture::new(&format!("crash-{how}")) else { return };
    // A clean start finds nothing to report.
    let v = report(&f, "report");
    assert_eq!((text(&v, "crashes"), v["registered"].clone(), v["loaded"].clone()), (String::new(), json!(true), json!(16)), "{v}");

    // First crash: the next start names the plug-in and the action, one strike, still registered.
    crash(&f, how);
    let v = report(&f, "report");
    let lines = text(&v, "crashes");
    assert!(lines.starts_with("org.effectcraft.test.Crash: EffectCraft stopped while this plug-in was running OfxImageEffectActionRender"), "{lines}");
    assert!(lines.contains("strike 1 of 2"), "{lines}");
    #[cfg(windows)]
    if how == "segv" {
        // The unhandled exception filter saw the exception and where it happened.
        assert!(lines.contains("exception 0xC0000005") && lines.contains("cachetest.ofx"), "{lines}");
    }
    assert_eq!((v["registered"].clone(), v["loaded"].clone()), (json!(true), json!(16)), "{v}");
    assert_eq!(v["in_flight_files_before_quit"], 0, "the dead process's record is removed once read: {v}");
    assert!(v["strikes"].as_str().is_some_and(|s| s.starts_with("1\t") && s.contains("org.effectcraft.test.Crash")), "{v}");
    // The next start does not report it again.
    assert_eq!(text(&report(&f, "report"), "crashes"), "");

    // Second crash in a row: skipped from now on, the other effects still load.
    crash(&f, how);
    let v = report(&f, "report");
    let lines = text(&v, "crashes");
    assert!(lines.contains("crashed EffectCraft 2 times in a row and is skipped from now on") && lines.contains("ofx_render_crashes.txt"), "{lines}");
    assert_eq!((v["registered"].clone(), v["loaded"].clone()), (json!(false), json!(14)), "{v}");
    assert!(text(&v, "errors").contains("org.effectcraft.test.Crash: skipped: it crashed EffectCraft while rendering 2 times in a row"), "{v}");
    // Still skipped later, with nothing new to report.
    let v = report(&f, "report");
    assert_eq!((text(&v, "crashes"), v["registered"].clone()), (String::new(), json!(false)), "{v}");
    assert!(text(&v, "errors").contains("skipped: it crashed EffectCraft while rendering"), "{v}");

    // "retry" brings it back.
    let v = report(&f, "retry");
    assert_eq!((v["registered"].clone(), v["loaded"].clone()), (json!(true), json!(16)), "{v}");
    assert!(v["strikes"].is_null(), "{v}");
}

#[test]
fn a_plugin_that_segfaults_while_rendering_is_named_and_skipped_after_two_crashes() {
    two_crashes_and_the_plugin_is_skipped("segv");
}

#[test]
fn a_plugin_that_aborts_while_rendering_is_named_and_skipped_after_two_crashes() {
    two_crashes_and_the_plugin_is_skipped("abort");
}

/// Crashes in a row, not crashes in all: a session that renders the plug-in and quits normally
/// starts the count over.
#[test]
fn a_clean_session_that_rendered_the_plugin_forgives_it() {
    let Some(f) = Fixture::new("crash-forgiven") else { return };
    crash(&f, "abort");
    let v = f.start("render_crash::child_session", &[("EC_RC_ACTION", "render-clean")]);
    assert!(text(&v, "crashes").contains("strike 1 of 2"), "{v}");
    assert_eq!(v["registered"], true, "{v}");
    // While it ran, the session had a record file; quitting removed it and the strike.
    assert_eq!((v["in_flight_files_before_quit"].clone(), v["in_flight_files_after_quit"].clone()), (json!(1), json!(0)), "{v}");
    let v = report(&f, "report");
    assert!(v["strikes"].is_null() && text(&v, "crashes").is_empty(), "{v}");
    // The next crash is the first again, so the plug-in still runs.
    crash(&f, "abort");
    let v = report(&f, "report");
    assert!(text(&v, "crashes").contains("strike 1 of 2") && v["registered"] == true, "{v}");
}

/// A skipped plug-in comes back when its binary changes (an update, a reinstall).
#[test]
fn an_updated_binary_is_tried_again() {
    let Some(f) = Fixture::new("crash-updated") else { return };
    let _ = report(&f, "report");
    crash(&f, "abort");
    crash(&f, "abort");
    let v = report(&f, "report");
    assert_eq!(v["registered"], false, "{v}");
    f.touch();
    let v = report(&f, "report");
    assert_eq!((v["registered"].clone(), v["loaded"].clone()), (json!(true), json!(16)), "{v}");
}

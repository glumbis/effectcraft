//! Two copies of one plug-in binary (as alpha / beta / release folders leave them): the newest is
//! kept, the other is skipped before its `Load`, with a note.
//!
//! This is its own test binary because registering effects is permanent for a process: the other
//! scenarios load copies of the same plug-ins, which would be skipped as duplicates of these. Set
//! `EC_TEST_OFX_BUNDLE` to the built `ec-test-ofx.ofx.bundle`; without it the test returns.

mod common;

use std::path::Path;

use common::{scratch, test_binary};
use effectcraft_ofx::load_path;

/// A clean load has a compact transcript, and a second copy of the same plug-ins (as alpha / beta /
/// release folders leave them) is skipped before its `Load`, with a note. The versions are equal
/// here, so the copy found first stays; which version wins is covered by the loader's unit tests.
/// (This is the only test that registers the plug-ins: identifiers are registered once per process.)
#[test]
fn a_clean_load_has_a_compact_transcript_and_a_second_copy_is_skipped() {
    let Some(bin) = test_binary().or_else(|| {
        println!("note: EC_TEST_OFX_BUNDLE is not set; skipping the OFX diagnostics tests");
        None
    }) else {
        return;
    };
    let folder = scratch().join("duplicates");
    let _ = std::fs::remove_dir_all(&folder);
    let platform = bin.parent().and_then(Path::file_name).map(|n| n.to_owned()).unwrap_or_default();
    for name in ["a-first", "b-second"] {
        let dir = folder.join(format!("{name}.ofx.bundle")).join("Contents").join(&platform);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::copy(&bin, dir.join(format!("{name}.ofx"))).unwrap();
    }
    let r = load_path(&folder);
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    // Sixteen effects, from the first copy only.
    assert_eq!(r.loaded.len(), 16, "{:?}", r.loaded);
    assert!(r.loaded.iter().all(|e| e.bundle.contains("a-first")), "{:?}", r.loaded);
    // Notes: a context that fell back and an input that was guessed (first copy), the duplicates (second).
    assert_eq!(r.notes.len(), 3, "{:?}", r.notes);
    let note = r.notes.iter().find(|n| n.contains("duplicate plug-in(s) skipped")).unwrap_or_else(|| panic!("notes: {:?}", r.notes));
    assert!(note.contains("b-second") && note.contains("16 duplicate") && note.contains("a-first"), "{note}");
    // The first copy's transcript: the host, the entry points, and one line for each plug-in that
    // registered without anything to report.
    let first = r.transcript.iter().find(|t| t.contains("a-first.ofx")).expect("transcript of the first copy");
    for needle in [
        "host shown to the plug-in: \"EffectCraft\"",
        "OfxSetHost -> kOfxStatOK (0)",
        "OfxGetNumberOfPlugins -> 16",
        "org.effectcraft.test.Invert v1.0: registered as effect org.effectcraft.test.invert (filter context) (Load, Describe and DescribeInContext: OK)",
        "org.effectcraft.test.Mix v1.0: registered as effect org.effectcraft.test.mix (general context)",
        // The effect that needed a fallback keeps the detail: the failed attempt and what it said.
        "DescribeInContext(filter) -> kOfxStatErrMissingHostFeature (4)",
        "plug-in said (error): \"The filter context is not available\"",
    ] {
        assert!(
            first.contains(needle),
            "missing `{needle}` in:
{first}"
        );
    }
    assert!(
        !first.contains(
            "org.effectcraft.test.Invert v1.0
"
        ),
        "a clean plug-in is one line:
{first}"
    );
    // The second copy's plug-ins never ran Load: its transcript lists them as skipped.
    let second = r.transcript.iter().find(|t| t.contains("b-second.ofx")).expect("transcript of the second copy");
    assert!(second.contains("skipped, duplicate:") && !second.contains("Load ->"), "{second}");
}

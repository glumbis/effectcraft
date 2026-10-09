//! A crash blocklist for plug-in binaries, like the ones other OFX hosts keep.
//!
//! A native crash (access violation, C++ exception) inside a plug-in's library initialisers,
//! `OfxGetPlugin`, `Load`, `Describe` or `DescribeInContext` cannot be caught with
//! `catch_unwind`, so one bad plug-in in a standard OFX folder would crash EffectCraft on every
//! launch. Before the loader opens a binary it writes a marker file (`ofx_loading.txt`, holding
//! the binary's canonical path) in the state directory and removes it once the binary has been
//! loaded and described, successfully or with an ordinary error. If the process dies in between,
//! the marker survives; on the next run's first load it is moved to `ofx_blocklist.txt` (one
//! path per line) and that binary is skipped from then on. `effect.plugins.scanOfx {"retry": true}`
//! or deleting the line brings it back.
//!
//! The state directory is set once by the host ([`set_state_dir`]); without one (tests, tools with
//! no config directory) the blocklist is disabled. All file IO errors are ignored: the blocklist
//! is a safety net and must never make a load fail.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

/// The marker written while a binary is being loaded.
const MARKER: &str = "ofx_loading.txt";
/// The binaries that crashed the process while loading, one path per line.
const LIST: &str = "ofx_blocklist.txt";

fn state_dir_cell() -> &'static Mutex<Option<PathBuf>> {
    static C: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

/// Whether the previous run's marker has been dealt with in this process.
static RECOVERED: AtomicBool = AtomicBool::new(false);

/// Set the directory that holds `ofx_loading.txt` and `ofx_blocklist.txt` (the host's config
/// directory). Until this is called the blocklist is disabled.
pub fn set_state_dir(dir: PathBuf) {
    *state_dir_cell().lock().unwrap_or_else(PoisonError::into_inner) = Some(dir);
}

fn state_dir() -> Option<PathBuf> {
    state_dir_cell().lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// Empty the blocklist (and forget a leftover marker): every binary is tried again.
pub fn clear() {
    let Some(dir) = state_dir() else { return };
    clear_in(&dir);
    // A marker left by the previous run must not be blocklisted by the next load either.
    RECOVERED.store(true, Ordering::SeqCst);
}

/// Held while a binary is being loaded; removes the marker when dropped (not reached if the
/// process dies).
pub(crate) struct LoadGuard(Option<PathBuf>);

impl Drop for LoadGuard {
    fn drop(&mut self) {
        if let Some(dir) = &self.0 {
            finish(dir);
        }
    }
}

/// Called by the loader before it opens the binary at `bin` (canonical path; `label` is how
/// errors name it). `Err` = the binary crashed the process last time and is skipped.
pub(crate) fn begin(bin: &Path, label: &str) -> Result<LoadGuard, String> {
    let Some(dir) = state_dir() else { return Ok(LoadGuard(None)) };
    if !RECOVERED.swap(true, Ordering::SeqCst) {
        recover(&dir);
    }
    check_and_mark(&dir, bin, label)?;
    Ok(LoadGuard(Some(dir)))
}

/// How paths are compared: case-insensitively on Windows.
fn norm(path: &str) -> String {
    if cfg!(windows) { path.to_lowercase() } else { path.to_string() }
}

fn read_list(dir: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(dir.join(LIST)).unwrap_or_default();
    text.lines().map(|l| l.trim_end_matches('\r')).filter(|l| !l.is_empty()).map(str::to_string).collect()
}

fn is_blocked(dir: &Path, bin: &str) -> bool {
    let want = norm(bin);
    read_list(dir).iter().any(|l| norm(l) == want)
}

/// Skip a blocklisted binary; otherwise write the marker.
fn check_and_mark(dir: &Path, bin: &Path, label: &str) -> Result<(), String> {
    let key = bin.to_string_lossy();
    if is_blocked(dir, &key) {
        return Err(format!(
            "{label}: skipped: it crashed EffectCraft while loading last time (remove it from {} or run effect.plugins.scanOfx {{\"retry\": true}} to try again)",
            dir.join(LIST).display()
        ));
    }
    // A path with a line break can't be stored one per line; it just goes unprotected.
    if !key.contains(['\n', '\r']) {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(dir.join(MARKER), key.as_bytes());
    }
    Ok(())
}

/// The binary was loaded and described (or failed with an ordinary error): drop the marker.
fn finish(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(MARKER));
}

/// A marker left by a previous run means that run died while loading that binary: blocklist it.
fn recover(dir: &Path) {
    let marker = dir.join(MARKER);
    let Ok(text) = std::fs::read_to_string(&marker) else { return };
    let bin = text.lines().next().unwrap_or("").trim_end_matches('\r');
    if !bin.is_empty() && !is_blocked(dir, bin) {
        use std::io::Write;
        let _ = std::fs::create_dir_all(dir);
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join(LIST)) {
            let _ = writeln!(f, "{bin}");
        }
    }
    let _ = std::fs::remove_file(&marker);
}

fn clear_in(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(LIST));
    let _ = std::fs::remove_file(dir.join(MARKER));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, empty directory under the system temp folder; removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("effectcraft-ofx-blocklist-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn bin(name: &str) -> PathBuf {
        PathBuf::from(r"C:\OFX\Plugins").join(name)
    }

    #[test]
    fn a_clean_load_leaves_nothing_behind() {
        let t = TempDir::new("clean");
        let b = bin("Good.ofx");
        assert!(check_and_mark(&t.0, &b, "Good").is_ok());
        assert_eq!(std::fs::read_to_string(t.0.join(MARKER)).unwrap(), b.to_string_lossy());
        finish(&t.0);
        assert!(!t.0.join(MARKER).exists());
        // The next run has nothing to recover and nothing is blocked.
        recover(&t.0);
        assert!(!t.0.join(LIST).exists());
        assert!(check_and_mark(&t.0, &b, "Good").is_ok());
    }

    #[test]
    fn a_crash_blocklists_the_binary_on_the_next_run() {
        let t = TempDir::new("crash");
        let bad = bin("Bad.ofx");
        let good = bin("Good.ofx");
        // Run 1 dies inside the plug-in: no `finish`.
        assert!(check_and_mark(&t.0, &bad, "Bad").is_ok());
        // Run 2 notices the marker.
        recover(&t.0);
        assert!(!t.0.join(MARKER).exists());
        assert_eq!(read_list(&t.0), vec![bad.to_string_lossy().into_owned()]);
        let err = check_and_mark(&t.0, &bad, "Bad").unwrap_err();
        assert!(err.starts_with("Bad: skipped: it crashed EffectCraft while loading last time"), "{err}");
        assert!(err.contains("ofx_blocklist.txt") && err.contains("retry"), "{err}");
        // A refused binary writes no marker, and other binaries still load.
        assert!(!t.0.join(MARKER).exists());
        assert!(check_and_mark(&t.0, &good, "Good").is_ok());
    }

    #[test]
    fn recovering_twice_does_not_duplicate_entries() {
        let t = TempDir::new("twice");
        let bad = bin("Bad.ofx");
        assert!(check_and_mark(&t.0, &bad, "Bad").is_ok());
        recover(&t.0);
        // A second marker for the same binary (for example after a manual retry that crashed again).
        let _ = std::fs::write(t.0.join(MARKER), bad.to_string_lossy().as_bytes());
        recover(&t.0);
        assert_eq!(read_list(&t.0).len(), 1);
        assert!(!t.0.join(MARKER).exists());
    }

    #[test]
    fn several_crashes_accumulate() {
        let t = TempDir::new("several");
        for n in ["A.ofx", "B.ofx"] {
            assert!(check_and_mark(&t.0, &bin(n), n).is_ok());
            recover(&t.0);
        }
        assert_eq!(read_list(&t.0).len(), 2);
        assert!(is_blocked(&t.0, &bin("A.ofx").to_string_lossy()));
        assert!(is_blocked(&t.0, &bin("B.ofx").to_string_lossy()));
        assert!(!is_blocked(&t.0, &bin("C.ofx").to_string_lossy()));
    }

    #[test]
    fn clear_forgets_everything() {
        let t = TempDir::new("clear");
        let bad = bin("Bad.ofx");
        assert!(check_and_mark(&t.0, &bad, "Bad").is_ok());
        recover(&t.0);
        assert!(check_and_mark(&t.0, &bad, "Bad").is_err());
        // A leftover marker goes too.
        let _ = std::fs::write(t.0.join(MARKER), b"C:\\x.ofx");
        clear_in(&t.0);
        assert!(!t.0.join(LIST).exists() && !t.0.join(MARKER).exists());
        assert!(check_and_mark(&t.0, &bad, "Bad").is_ok());
    }

    #[test]
    fn io_problems_are_ignored() {
        // The "directory" is a file, so nothing can be written under it.
        let t = TempDir::new("io");
        let _ = std::fs::create_dir_all(&t.0);
        let file = t.0.join("not-a-dir");
        let _ = std::fs::write(&file, b"x");
        let b = bin("Any.ofx");
        assert!(check_and_mark(&file, &b, "Any").is_ok());
        finish(&file);
        recover(&file);
        clear_in(&file);
        // And a state directory that doesn't exist yet is created on demand.
        let fresh = t.0.join("a").join("b");
        assert!(check_and_mark(&fresh, &b, "Any").is_ok());
        assert!(fresh.join(MARKER).exists());
    }

    #[test]
    fn paths_with_line_breaks_are_not_recorded() {
        let t = TempDir::new("newline");
        assert!(check_and_mark(&t.0, Path::new("odd\nname.ofx"), "odd").is_ok());
        assert!(!t.0.join(MARKER).exists());
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_compare_case_insensitively() {
        let t = TempDir::new("case");
        assert!(check_and_mark(&t.0, Path::new(r"C:\OFX\Plugins\Bad.ofx"), "Bad").is_ok());
        recover(&t.0);
        assert!(check_and_mark(&t.0, Path::new(r"c:\ofx\plugins\BAD.OFX"), "Bad").is_err());
    }

    #[test]
    fn without_a_state_dir_the_guard_is_inert() {
        // `set_state_dir` is process-global and no test sets it, so `begin` must be a no-op here.
        if state_dir().is_none() {
            let guard = begin(&bin("Any.ofx"), "Any");
            assert!(guard.is_ok());
        }
    }
}

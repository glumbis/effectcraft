//! A crash blocklist for plug-in binaries, like the ones other OFX hosts keep.
//!
//! A native crash (access violation, C++ exception) inside a plug-in's library initialisers,
//! `OfxGetPlugin`, `Load`, `Describe` or `DescribeInContext` cannot be caught with
//! `catch_unwind`, so one bad plug-in in a standard OFX folder would crash EffectCraft on every
//! launch. Before the loader opens a binary it writes a marker file (`ofx_loading.<pid>.txt`,
//! holding the binary's canonical path) in the state directory, keeps it exclusively locked, and
//! removes it once the binary has been loaded and described, successfully or with an ordinary
//! error. If the process dies in between, the marker survives and the OS releases the lock; on
//! the next first load of any process a marker that can be locked counts as a *strike* against
//! that binary in `ofx_blocklist.txt`. A marker that is still locked belongs to another
//! EffectCraft process that is loading right now and is left alone.
//!
//! One strike isn't enough to skip a binary: the process may have been closed or killed while a
//! large suite was loading (the window isn't up yet, so the app looks stuck), or the plug-in hit
//! a one-off problem. Only a binary that crashed [`STRIKES`] loads in a row is skipped. A load
//! that finishes clears its strikes, and a binary whose file changed since its last strike
//! (updated, reinstalled) starts over. `effect.plugins.scanOfx {"retry": true}` or deleting the
//! line brings a binary back.
//!
//! `ofx_blocklist.txt` holds one binary per line: `<strikes>\t<file stamp>\t<path>`. A line with
//! only a path (written by earlier versions after a single crash) counts as one strike.
//!
//! The state directory is set once by the host ([`set_state_dir`]); without one (tests, tools with
//! no config directory) the blocklist is disabled. All file IO errors are ignored: the blocklist
//! is a safety net and must never make a load fail.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

/// Marker files are `ofx_loading.<owner>.txt`, one per process.
const MARKER_PREFIX: &str = "ofx_loading.";
const MARKER_SUFFIX: &str = ".txt";
/// The binaries that crashed the process while loading, one per line (see the module docs).
const LIST: &str = "ofx_blocklist.txt";
/// Crashes in a row after which a binary is skipped.
const STRIKES: u32 = 2;

fn state_dir_cell() -> &'static Mutex<Option<PathBuf>> {
    static C: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

/// Whether the previous runs' markers have been dealt with in this process.
static RECOVERED: AtomicBool = AtomicBool::new(false);

/// Set the directory that holds the markers and `ofx_blocklist.txt` (the host's config
/// directory). Until this is called the blocklist is disabled.
pub fn set_state_dir(dir: PathBuf) {
    *state_dir_cell().lock().unwrap_or_else(PoisonError::into_inner) = Some(dir);
}

/// The directory set by [`set_state_dir`] (also where `ofx_settings.json` lives).
pub(crate) fn state_dir() -> Option<PathBuf> {
    state_dir_cell().lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// Empty the blocklist (and forget leftover markers of dead processes): every binary is tried
/// again.
pub fn clear() {
    let Some(dir) = state_dir() else { return };
    clear_in(&dir);
    // A marker left by a previous run must not be blocklisted by the next load either.
    RECOVERED.store(true, Ordering::SeqCst);
}

/// The locked marker of the binary being loaded.
struct Marker {
    path: PathBuf,
    file: File,
    /// The binary being loaded.
    bin: String,
}

impl Marker {
    /// The load finished: unlock (closing the file releases the lock), remove the marker and
    /// forget the binary's earlier strikes.
    #[cfg(test)]
    fn finish(self) {
        self.close(true);
    }

    /// Unlock and remove the marker; `forgive` also forgets the binary's earlier strikes.
    fn close(self, forgive: bool) {
        let Marker { path, file, bin } = self;
        drop(file);
        let _ = std::fs::remove_file(&path);
        if forgive && let Some(dir) = path.parent() {
            forget(dir, &bin);
        }
    }
}

/// Held while a binary is being loaded; removes the marker when dropped (not reached if the
/// process dies).
pub(crate) struct LoadGuard {
    marker: Option<Marker>,
    /// Whether dropping the guard also clears the binary's strikes (a load that went all the way).
    forgive: bool,
}

impl LoadGuard {
    /// This phase was the last one and went without a crash: dropping the guard clears the
    /// binary's strikes.
    pub(crate) fn forgive(&mut self) {
        self.forgive = true;
    }
}

impl Drop for LoadGuard {
    fn drop(&mut self) {
        if let Some(m) = self.marker.take() {
            m.close(self.forgive);
        }
    }
}

/// Called by the loader before it opens the binary at `bin` (canonical path; `label` is how
/// errors name it). `Err` = the binary crashed the process last time and is skipped.
#[cfg(test)]
pub(crate) fn begin(bin: &Path, label: &str) -> Result<LoadGuard, String> {
    begin_phase(bin, label, true)
}

/// [`begin`] for one phase of loading a binary (listing its plug-ins, then describing them).
/// Only the phase that ends the load (`last`) clears the binary's earlier strikes when its guard
/// is dropped, so a crash in a later phase still counts against the binary.
pub(crate) fn begin_phase(bin: &Path, label: &str, last: bool) -> Result<LoadGuard, String> {
    let Some(dir) = state_dir() else { return Ok(LoadGuard { marker: None, forgive: last }) };
    if !RECOVERED.swap(true, Ordering::SeqCst) {
        recover(&dir);
    }
    check_and_mark(&dir, bin, label, std::process::id()).map(|marker| LoadGuard { marker, forgive: last })
}

/// Whether the binary at `bin` is skipped because it crashed the process while loading (the
/// loader asks before it registers a binary from the scan cache, which does not load it). `Err` is
/// the reason, worded for the report; the binary is not loaded or registered.
pub(crate) fn check(bin: &Path, label: &str) -> Result<(), String> {
    let Some(dir) = state_dir() else { return Ok(()) };
    if !RECOVERED.swap(true, Ordering::SeqCst) {
        recover(&dir);
    }
    if is_blocked(&dir, &bin.to_string_lossy()) { Err(blocked_message(&dir, label)) } else { Ok(()) }
}

fn blocked_message(dir: &Path, label: &str) -> String {
    format!(
        "{label}: skipped: it crashed EffectCraft while loading {STRIKES} times in a row (update or reinstall it, remove its line from {} or run effect.plugins.scanOfx {{\"retry\": true}} to try again)",
        dir.join(LIST).display()
    )
}

/// How paths are compared: case-insensitively on Windows.
fn norm(path: &str) -> String {
    if cfg!(windows) { path.to_lowercase() } else { path.to_string() }
}

/// One line of the blocklist.
#[derive(Clone, Debug, PartialEq)]
struct Entry {
    strikes: u32,
    /// [`stamp`] of the binary at its last strike ("" if unknown).
    stamp: String,
    path: String,
}

fn parse_line(line: &str) -> Option<Entry> {
    let line = line.trim_end_matches('\r');
    if line.is_empty() {
        return None;
    }
    let mut parts = line.splitn(3, '\t');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(n), Some(stamp), Some(path)) if !path.is_empty() => {
            Some(Entry { strikes: n.parse().unwrap_or(1).max(1), stamp: stamp.to_string(), path: path.to_string() })
        }
        // A single-path line from an earlier version: one crash.
        _ => Some(Entry { strikes: 1, stamp: String::new(), path: line.to_string() }),
    }
}

fn read_list(dir: &Path) -> Vec<Entry> {
    let text = std::fs::read_to_string(dir.join(LIST)).unwrap_or_default();
    text.lines().filter_map(parse_line).collect()
}

fn write_list(dir: &Path, list: &[Entry]) {
    if list.is_empty() {
        let _ = std::fs::remove_file(dir.join(LIST));
        return;
    }
    let text: String = list.iter().map(|e| format!("{}\t{}\t{}\n", e.strikes, e.stamp, e.path)).collect();
    let _ = std::fs::write(dir.join(LIST), text);
}

/// Identifies a binary's file version: its size and modification time ("" if unreadable).
fn stamp(bin: &str) -> String {
    let Ok(meta) = std::fs::metadata(bin) else { return String::new() };
    let secs = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    format!("{}-{secs}", meta.len())
}

/// The entry for `bin`, if it still describes the same file.
fn entry_of(list: &[Entry], bin: &str) -> Option<Entry> {
    let want = norm(bin);
    let e = list.iter().find(|e| norm(&e.path) == want)?;
    (e.stamp.is_empty() || e.stamp == stamp(bin)).then(|| e.clone())
}

fn is_blocked(dir: &Path, bin: &str) -> bool {
    entry_of(&read_list(dir), bin).is_some_and(|e| e.strikes >= STRIKES)
}

/// One more crash for `bin`.
fn strike(dir: &Path, bin: &str) {
    let mut list = read_list(dir);
    let strikes = entry_of(&list, bin).map_or(0, |e| e.strikes).saturating_add(1);
    let want = norm(bin);
    list.retain(|e| norm(&e.path) != want);
    list.push(Entry { strikes, stamp: stamp(bin), path: bin.to_string() });
    write_list(dir, &list);
}

/// `bin` loaded without crashing: drop its strikes.
fn forget(dir: &Path, bin: &str) {
    let mut list = read_list(dir);
    let want = norm(bin);
    let before = list.len();
    list.retain(|e| norm(&e.path) != want);
    if list.len() != before {
        write_list(dir, &list);
    }
}

fn marker_path(dir: &Path, owner: u32) -> PathBuf {
    dir.join(format!("{MARKER_PREFIX}{owner}{MARKER_SUFFIX}"))
}

fn is_marker(name: &str) -> bool {
    name.len() > MARKER_PREFIX.len() + MARKER_SUFFIX.len() && name.starts_with(MARKER_PREFIX) && name.ends_with(MARKER_SUFFIX)
}

fn marker_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(is_marker)).collect()
}

/// Open a marker and lock it; `None` while a live process still holds it (or it can't be opened).
fn take_dead(marker: &Path) -> Option<File> {
    let file = File::options().read(true).write(true).open(marker).ok()?;
    file.try_lock().ok()?;
    Some(file)
}

/// Skip a blocklisted binary; otherwise write and lock the marker of process `owner`.
fn check_and_mark(dir: &Path, bin: &Path, label: &str, owner: u32) -> Result<Option<Marker>, String> {
    let key = bin.to_string_lossy();
    if is_blocked(dir, &key) {
        return Err(blocked_message(dir, label));
    }
    // A path with a line break can't be stored one per line; it just goes unprotected.
    if key.contains(['\n', '\r']) {
        return Ok(None);
    }
    let _ = std::fs::create_dir_all(dir);
    let path = marker_path(dir, owner);
    let Ok(mut file) = File::create(&path) else { return Ok(None) };
    // Locked for as long as the load runs; a lock failure only weakens the multi-process check.
    let _ = file.lock();
    let _ = file.write_all(key.as_bytes()).and_then(|()| file.flush());
    Ok(Some(Marker { path, file, bin: key.into_owned() }))
}

/// Markers left by processes that died while loading a binary: blocklist those binaries.
/// Markers still locked by a live process are left alone.
fn recover(dir: &Path) {
    for marker in marker_files(dir) {
        let Some(mut file) = take_dead(&marker) else { continue };
        let mut text = String::new();
        let _ = file.read_to_string(&mut text);
        drop(file);
        let bin = text.lines().next().unwrap_or("").trim_end_matches('\r');
        if !bin.is_empty() {
            strike(dir, bin);
        }
        let _ = std::fs::remove_file(&marker);
    }
}

/// Forget the blocklist and the markers of dead processes.
fn clear_in(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(LIST));
    for marker in marker_files(dir) {
        if let Some(file) = take_dead(&marker) {
            drop(file);
            let _ = std::fs::remove_file(&marker);
        }
    }
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

    /// The process "dies" holding its marker: the handle closes (the OS drops the lock) but the
    /// file stays.
    fn crash(m: Option<Marker>) {
        drop(m.map(|m| m.file));
    }

    fn markers(dir: &Path) -> usize {
        marker_files(dir).len()
    }

    #[test]
    fn a_clean_load_leaves_nothing_behind() {
        let t = TempDir::new("clean");
        let b = bin("Good.ofx");
        let m = check_and_mark(&t.0, &b, "Good", 1).unwrap().unwrap();
        // (Windows won't let another handle read the marker while it is locked.)
        assert_eq!(markers(&t.0), 1);
        m.finish();
        assert_eq!(markers(&t.0), 0);
        // The next run has nothing to recover and nothing is blocked.
        recover(&t.0);
        assert!(!t.0.join(LIST).exists());
        assert!(check_and_mark(&t.0, &b, "Good", 2).is_ok());
    }

    /// A run that dies while loading `b`, then the next run's recovery.
    fn crash_and_recover(dir: &Path, b: &Path, owner: u32) {
        crash(check_and_mark(dir, b, "x", owner).unwrap());
        recover(dir);
    }

    #[test]
    fn two_crashes_in_a_row_blocklist_the_binary() {
        let t = TempDir::new("crash");
        let bad = bin("Bad.ofx");
        let good = bin("Good.ofx");
        // Run 1 dies inside the plug-in; run 2 notices the marker. One strike: still loaded.
        crash_and_recover(&t.0, &bad, 1);
        assert_eq!(markers(&t.0), 0);
        assert_eq!(read_list(&t.0).iter().map(|e| (e.strikes, e.path.clone())).collect::<Vec<_>>(), vec![(1, bad.to_string_lossy().into_owned())]);
        assert!(!is_blocked(&t.0, &bad.to_string_lossy()));
        // Run 2 dies there too: skipped from run 3 on.
        crash_and_recover(&t.0, &bad, 2);
        let err = check_and_mark(&t.0, &bad, "Bad", 3).err().unwrap_or_default();
        assert!(err.starts_with("Bad: skipped: it crashed EffectCraft while loading 2 times in a row"), "{err}");
        assert!(err.contains("ofx_blocklist.txt") && err.contains("retry"), "{err}");
        // A refused binary writes no marker, and other binaries still load.
        assert_eq!(markers(&t.0), 0);
        assert!(check_and_mark(&t.0, &good, "Good", 3).unwrap().is_some());
    }

    /// A process killed while loading (the user closing an app that looked stuck) is one strike,
    /// which the next good load forgets.
    #[test]
    fn a_clean_load_clears_earlier_strikes() {
        let t = TempDir::new("forgive");
        let b = bin("Suite.ofx");
        crash_and_recover(&t.0, &b, 1);
        check_and_mark(&t.0, &b, "Suite", 2).unwrap().unwrap().finish();
        assert!(!t.0.join(LIST).exists());
        crash_and_recover(&t.0, &b, 3);
        assert!(!is_blocked(&t.0, &b.to_string_lossy()));
    }

    /// Loading a binary has two phases (list its plug-ins, then describe them): finishing the
    /// first must not forgive earlier crashes, or a binary that dies while describing would never
    /// reach the strike limit.
    #[test]
    fn only_the_last_phase_of_a_load_clears_strikes() {
        let t = TempDir::new("phases");
        let b = bin("Phased.ofx");
        crash_and_recover(&t.0, &b, 1);
        check_and_mark(&t.0, &b, "Phased", 2).unwrap().unwrap().close(false);
        assert_eq!(read_list(&t.0).len(), 1, "the first phase alone keeps the strike");
        check_and_mark(&t.0, &b, "Phased", 2).unwrap().unwrap().close(true);
        assert!(!t.0.join(LIST).exists());
    }

    /// Lists written by earlier versions (a bare path after one crash) count as one strike.
    #[test]
    fn an_older_single_path_line_is_one_strike() {
        let t = TempDir::new("legacy");
        let _ = std::fs::create_dir_all(&t.0);
        let b = bin("Old.ofx");
        let _ = std::fs::write(t.0.join(LIST), format!("{}\r\n", b.to_string_lossy()));
        assert!(check_and_mark(&t.0, &b, "Old", 1).is_ok());
        crash_and_recover(&t.0, &b, 2);
        assert!(is_blocked(&t.0, &b.to_string_lossy()));
    }

    /// Updating or reinstalling a blocked plug-in changes its file: it is tried again.
    #[test]
    fn a_changed_binary_starts_over() {
        let t = TempDir::new("changed");
        let _ = std::fs::create_dir_all(&t.0);
        let b = t.0.join("Real.ofx");
        let _ = std::fs::write(&b, b"v1");
        crash_and_recover(&t.0, &b, 1);
        crash_and_recover(&t.0, &b, 2);
        assert!(is_blocked(&t.0, &b.to_string_lossy()));
        let _ = std::fs::write(&b, b"version 2");
        assert!(!is_blocked(&t.0, &b.to_string_lossy()));
        crash_and_recover(&t.0, &b, 3);
        assert_eq!(read_list(&t.0).first().map(|e| e.strikes), Some(1));
    }

    #[test]
    fn another_process_loading_right_now_is_left_alone() {
        let t = TempDir::new("live");
        let busy = bin("Busy.ofx");
        // Process 1 is still loading (its marker stays locked) when process 2 starts.
        let live = check_and_mark(&t.0, &busy, "Busy", 1).unwrap().unwrap();
        recover(&t.0);
        clear_in(&t.0);
        assert!(!is_blocked(&t.0, &busy.to_string_lossy()));
        assert!(read_list(&t.0).is_empty());
        assert_eq!(markers(&t.0), 1);
        live.finish();
        assert_eq!(markers(&t.0), 0);
    }

    #[test]
    fn recovering_twice_does_not_duplicate_entries() {
        let t = TempDir::new("twice");
        let bad = bin("Bad.ofx");
        crash_and_recover(&t.0, &bad, 1);
        // Another marker for the same binary (for example after a manual retry that crashed again).
        let _ = std::fs::write(marker_path(&t.0, 7), bad.to_string_lossy().as_bytes());
        recover(&t.0);
        assert_eq!(read_list(&t.0).len(), 1);
        assert_eq!(read_list(&t.0).first().map(|e| e.strikes), Some(2));
        assert_eq!(markers(&t.0), 0);
    }

    #[test]
    fn several_crashes_accumulate() {
        let t = TempDir::new("several");
        for (i, n) in ["A.ofx", "B.ofx"].into_iter().enumerate() {
            crash_and_recover(&t.0, &bin(n), 2 * i as u32);
            crash_and_recover(&t.0, &bin(n), 2 * i as u32 + 1);
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
        crash_and_recover(&t.0, &bad, 1);
        crash_and_recover(&t.0, &bad, 2);
        assert!(check_and_mark(&t.0, &bad, "Bad", 3).is_err());
        // A leftover marker of a dead process goes too.
        let _ = std::fs::write(marker_path(&t.0, 4), b"C:\\x.ofx");
        clear_in(&t.0);
        assert!(!t.0.join(LIST).exists());
        assert_eq!(markers(&t.0), 0);
        assert!(check_and_mark(&t.0, &bad, "Bad", 3).is_ok());
    }

    #[test]
    fn io_problems_are_ignored() {
        // The "directory" is a file, so nothing can be written under it.
        let t = TempDir::new("io");
        let _ = std::fs::create_dir_all(&t.0);
        let file = t.0.join("not-a-dir");
        let _ = std::fs::write(&file, b"x");
        let b = bin("Any.ofx");
        assert!(check_and_mark(&file, &b, "Any", 1).unwrap().is_none());
        recover(&file);
        clear_in(&file);
        // And a state directory that doesn't exist yet is created on demand.
        let fresh = t.0.join("a").join("b");
        let m = check_and_mark(&fresh, &b, "Any", 1).unwrap();
        assert_eq!(markers(&fresh), 1);
        drop(m);
    }

    #[test]
    fn paths_with_line_breaks_are_not_recorded() {
        let t = TempDir::new("newline");
        assert!(check_and_mark(&t.0, Path::new("odd\nname.ofx"), "odd", 1).unwrap().is_none());
        assert_eq!(markers(&t.0), 0);
    }

    #[test]
    fn only_marker_names_count() {
        assert!(is_marker("ofx_loading.1234.txt"));
        assert!(!is_marker("ofx_loading.txt") && !is_marker("ofx_blocklist.txt") && !is_marker("other.txt"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_compare_case_insensitively() {
        let t = TempDir::new("case");
        crash_and_recover(&t.0, Path::new(r"C:\OFX\Plugins\Bad.ofx"), 1);
        crash_and_recover(&t.0, Path::new(r"c:\ofx\plugins\bad.ofx"), 2);
        assert!(check_and_mark(&t.0, Path::new(r"c:\ofx\plugins\BAD.OFX"), "Bad", 3).is_err());
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

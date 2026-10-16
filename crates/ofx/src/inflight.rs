//! Crash protection for what plug-ins do while the app runs (rendering above all).
//!
//! [`crate::blocklist`] covers crashes while a library is loaded and described. A plug-in that
//! crashes in `Render` (or any other action) takes the process down, and the next start would know
//! nothing about it. This module keeps a small in-flight record so that it does:
//!
//! * One file per process, `ofx_inflight.<pid>.bin` in the config folder, is created once, the
//!   first time a plug-in action runs, kept exclusively locked while the process lives, and
//!   removed by [`close`] when the app quits normally. It holds a header and [`SLOTS`] fixed-size
//!   slots, one per plug-in action in progress (a thread renders one effect at a time, a nested
//!   render of an upstream effect takes one more slot, up to a small fixed number).
//! * [`begin`] overwrites a free slot in place with the plug-in's identifier, the action, the time
//!   and the thread, and the guard it returns overwrites the state word again when the action
//!   ends. No file is created per action and nothing is synced: the OS page cache outlives a crashed
//!   process, so one small positional write on each side of an action (microseconds) is all it
//!   costs. If the file cannot be written, nothing happens: recording never blocks rendering.
//! * On Windows an unhandled exception filter (chained to any other) adds the crashing thread's slot,
//!   the exception code and the faulting module to the header, so a crash with several plug-ins
//!   active is pinned on the right one.
//! * At the next start [`recover`] looks for record files of dead processes (the lock can be taken).
//!   A slot still marked active means the process died inside that action. The plug-in is named in
//!   `ofx_scan.txt` and the start-up toast, and counts as a strike in `ofx_render_crashes.txt`
//!   (`<strikes>\t<binary stamp>\t<plug-in id>\t<action>`). After [`STRIKES`] crashes in a row the
//!   plug-in is skipped (see [`skip_reason`]), like a binary on the load blocklist. A clean session
//!   that rendered it ([`forgive`]) or a changed binary (another stamp) starts over, and so does
//!   deleting its line or `effect.plugins.scanOfx {"retry": true}`.
//!
//! Without the crashing thread's slot (other platforms, a crash that bypasses the filter) a record
//! with several plug-ins active cannot say which one crashed: they are all listed and nobody is
//! struck, so an innocent plug-in is never skipped.

use std::cell::Cell;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

/// The record files are `ofx_inflight.<pid>.bin`.
const PREFIX: &str = "ofx_inflight.";
const SUFFIX: &str = ".bin";
/// The strikes, one plug-in per line.
pub const STRIKES_FILE: &str = "ofx_render_crashes.txt";
/// Crashes in a row after which a plug-in is skipped.
pub const STRIKES: u32 = 2;

const MAGIC: &[u8; 8] = b"OFXINFL1";
const VERSION: u32 = 1;
/// The header: fixed fields, then the faulting module's name.
const HEADER: usize = 256;
const MODULE_AT: usize = 64;
const MODULE_MAX: usize = 128;
/// The slots: one per plug-in action in progress.
pub const SLOTS: usize = 16;
const SLOT: usize = 192;
const ID_AT: usize = 40;
const ID_MAX: usize = 112;
const ACTION_AT: usize = 152;
const ACTION_MAX: usize = 40;
/// The largest record file read back (a real one is [`HEADER`] + [`SLOTS`] * [`SLOT`] bytes).
const MAX_FILE: u64 = 1 << 20;
/// "No slot" in the thread-local slot number.
const NO_SLOT: u32 = u32::MAX;

// Header layout (little endian): 0 magic, 8 version, 12 pid, 16 start time (Unix ms), 24 slot
// count, 28 slot size, 32 crashed slot + 1 (0 = not known), 36 exception code, 40 fault address.
// Slot layout: 0 state (0 idle, 1 active), 4 nesting depth, 8 thread number, 16 start time (Unix
// ms), 24 binary stamp, 32 identifier length, 36 action length, ID_AT identifier, ACTION_AT action.
const H_PID: usize = 12;
const H_START: usize = 16;
const H_COUNT: usize = 24;
const H_SIZE: usize = 28;
const H_CRASH_SLOT: usize = 32;
const H_CODE: usize = 36;
const H_ADDRESS: usize = 40;

/// FNV-1a over `bytes`, continuing from `h` (stable across runs and Rust versions, unlike the
/// std hasher).
fn fnv(mut h: u64, bytes: &[u8]) -> u64 {
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Identifies a binary's file version for the strikes: a hash of its size and modification time (0
/// when unreadable).
pub fn binary_stamp(path: &Path) -> u64 {
    match crate::cache::file_stamp(path) {
        Some((size, mtime)) => fnv(fnv(0xcbf2_9ce4_8422_2325, &size.to_le_bytes()), &mtime.to_le_bytes()) | 1,
        None => 0,
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok().and_then(|d| u64::try_from(d.as_millis()).ok()).unwrap_or(0)
}

/// `ms` since the Unix epoch as `2026-10-16 19:03:11 UTC`.
pub fn utc_string(ms: u64) -> String {
    let secs = ms / 1000;
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn put_u32(buf: &mut [u8], at: usize, v: u32) {
    if let Some(s) = buf.get_mut(at..at + 4) {
        s.copy_from_slice(&v.to_le_bytes());
    }
}

fn put_u64(buf: &mut [u8], at: usize, v: u64) {
    if let Some(s) = buf.get_mut(at..at + 8) {
        s.copy_from_slice(&v.to_le_bytes());
    }
}

fn get_u32(buf: &[u8], at: usize) -> u32 {
    buf.get(at..at + 4).and_then(|s| s.try_into().ok()).map_or(0, u32::from_le_bytes)
}

fn get_u64(buf: &[u8], at: usize) -> u64 {
    buf.get(at..at + 8).and_then(|s| s.try_into().ok()).map_or(0, u64::from_le_bytes)
}

/// Copy `text` into `buf` at `at` (at most `max` bytes, cut at a character boundary) and return the
/// length.
fn put_text(buf: &mut [u8], at: usize, max: usize, text: &str) -> u32 {
    let mut n = text.len().min(max);
    while !text.is_char_boundary(n) {
        n -= 1;
    }
    if let (Some(dst), Some(src)) = (buf.get_mut(at..at + n), text.as_bytes().get(..n)) {
        dst.copy_from_slice(src);
    }
    n as u32
}

fn get_text(buf: &[u8], at: usize, len: u32, max: usize) -> String {
    let n = (len as usize).min(max);
    buf.get(at..at + n).map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default()
}

thread_local! {
    /// The innermost slot of the plug-in action this thread is running (or on whose behalf it runs:
    /// the plug-in's own worker threads adopt their caller's slot). Const-initialised, so the
    /// Windows exception filter can read it on the crashing thread.
    static SLOT_NOW: Cell<u32> = const { Cell::new(NO_SLOT) };
    static DEPTH: Cell<u32> = const { Cell::new(0) };
    static THREAD_NO: Cell<u32> = const { Cell::new(0) };
}

static NEXT_THREAD: AtomicU32 = AtomicU32::new(1);

fn thread_no() -> u32 {
    THREAD_NO.with(|c| {
        if c.get() == 0 {
            c.set(NEXT_THREAD.fetch_add(1, Ordering::Relaxed));
        }
        c.get()
    })
}

/// The slot of the plug-in action the current thread is in, for the plug-in's worker threads.
pub fn current_slot() -> u32 {
    SLOT_NOW.with(Cell::get)
}

/// Puts the previous slot back when dropped.
pub struct SlotScope(u32);

impl Drop for SlotScope {
    fn drop(&mut self) {
        SLOT_NOW.with(|c| c.set(self.0));
    }
}

/// Make `slot` (from [`current_slot`] on the thread that started this one) the slot the current
/// thread works for, until the guard is dropped: a crash on a plug-in's worker thread is the crash
/// of the action that started it.
pub fn adopt_slot(slot: u32) -> SlotScope {
    SlotScope(SLOT_NOW.with(|c| c.replace(slot)))
}

/// One process's record file.
pub struct Inflight {
    file: File,
    path: PathBuf,
    /// Bit `i` set: slot `i` is free.
    free: AtomicU32,
    /// The record was closed (the app is quitting): nothing is written any more.
    closed: AtomicBool,
}

impl Inflight {
    /// Create (and lock) the record file of process `pid` in `dir`. `None` if that fails.
    pub fn open(dir: &Path, pid: u32) -> Option<Inflight> {
        std::fs::create_dir_all(dir).ok()?;
        let path = dir.join(format!("{PREFIX}{pid}{SUFFIX}"));
        let file = File::options().read(true).write(true).create(true).truncate(true).open(&path).ok()?;
        // Held until the process ends (or the file is closed): a lock that can be taken later
        // means the process is gone. A failed lock only weakens the multi-process check.
        let _ = file.lock();
        file.set_len((HEADER + SLOTS * SLOT) as u64).ok()?;
        let mut head = [0u8; HEADER];
        head[..8].copy_from_slice(MAGIC);
        put_u32(&mut head, 8, VERSION);
        put_u32(&mut head, H_PID, pid);
        put_u64(&mut head, H_START, now_ms());
        put_u32(&mut head, H_COUNT, SLOTS as u32);
        put_u32(&mut head, H_SIZE, SLOT as u32);
        let me = Inflight { file, path, free: AtomicU32::new(u32::MAX >> (32 - SLOTS)), closed: AtomicBool::new(false) };
        me.write_at(&head, 0);
        #[cfg(windows)]
        win::set_handle(&me.file);
        Some(me)
    }

    fn write_at(&self, buf: &[u8], offset: u64) {
        #[cfg(windows)]
        {
            use std::os::windows::fs::FileExt;
            let _ = self.file.seek_write(buf, offset);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileExt;
            let _ = self.file.write_at(buf, offset);
        }
    }

    /// The file's bytes, read through the open handle (another handle cannot read a locked file on
    /// Windows).
    #[cfg(test)]
    fn contents(&self) -> Vec<u8> {
        let mut buf = vec![0u8; HEADER + SLOTS * SLOT];
        #[cfg(windows)]
        {
            use std::os::windows::fs::FileExt;
            let _ = self.file.seek_read(&mut buf, 0);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileExt;
            let _ = self.file.read_at(&mut buf, 0);
        }
        buf
    }

    /// Record that `plugin` starts `action`; the slot, if one was free.
    fn start(&self, plugin: &str, action: &str, stamp: u64, depth: u32) -> Option<u32> {
        if self.closed.load(Ordering::Acquire) {
            return None;
        }
        let slot = loop {
            let free = self.free.load(Ordering::Acquire);
            if free == 0 {
                return None;
            }
            let i = free.trailing_zeros();
            if self.free.compare_exchange(free, free & !(1 << i), Ordering::AcqRel, Ordering::Acquire).is_ok() {
                break i;
            }
        };
        let mut buf = [0u8; SLOT];
        put_u32(&mut buf, 0, 1);
        put_u32(&mut buf, 4, depth);
        put_u32(&mut buf, 8, thread_no());
        put_u64(&mut buf, 16, now_ms());
        put_u64(&mut buf, 24, stamp);
        let id_len = put_text(&mut buf, ID_AT, ID_MAX, plugin);
        let action_len = put_text(&mut buf, ACTION_AT, ACTION_MAX, action);
        put_u32(&mut buf, 32, id_len);
        put_u32(&mut buf, 36, action_len);
        self.write_at(&buf, (HEADER + slot as usize * SLOT) as u64);
        Some(slot)
    }

    /// The action in `slot` is over: mark it idle and free it.
    fn finish(&self, slot: u32) {
        if !self.closed.load(Ordering::Acquire) {
            self.write_at(&0u32.to_le_bytes(), (HEADER + slot as usize * SLOT) as u64);
        }
        self.free.fetch_or(1 << slot, Ordering::AcqRel);
    }

    /// The process quits normally: nothing is recorded or blamed from here on (actions still running
    /// on other threads are marked idle), and the file is removed (it goes for good when the
    /// process ends and the handle closes).
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        #[cfg(windows)]
        win::clear_handle();
        self.write_at(&[0u8; SLOTS * SLOT], HEADER as u64);
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The record of this process, once there is a folder for it.
#[derive(Default)]
struct Global {
    dir: Option<PathBuf>,
    current: Option<Arc<Inflight>>,
    /// Opening failed (or recording was closed): do not try again.
    off: bool,
    /// What dead processes left in `dir` has been dealt with ([`recover_once`]).
    recovered: bool,
}

fn global() -> &'static Mutex<Global> {
    static G: OnceLock<Mutex<Global>> = OnceLock::new();
    G.get_or_init(|| Mutex::new(Global::default()))
}

/// The record of this process, created the first time it is needed.
fn current() -> Option<Arc<Inflight>> {
    let mut g = global().lock().unwrap_or_else(PoisonError::into_inner);
    if g.current.is_none() && !g.off {
        match g.dir.clone().and_then(|d| Inflight::open(&d, std::process::id())) {
            Some(f) => {
                g.current = Some(Arc::new(f));
                #[cfg(windows)]
                win::install();
            }
            None => g.off = true,
        }
    }
    g.current.clone()
}

/// Ends a recorded action when dropped.
pub struct ActionGuard {
    rec: Option<(Arc<Inflight>, u32, u32, u32)>,
}

impl Drop for ActionGuard {
    fn drop(&mut self) {
        if let Some((f, slot, prev_slot, prev_depth)) = self.rec.take() {
            f.finish(slot);
            SLOT_NOW.with(|c| c.set(prev_slot));
            DEPTH.with(|c| c.set(prev_depth));
        }
    }
}

/// Record that `plugin` (whose binary has `stamp`, see [`binary_stamp`]) runs `action` on this
/// thread, until the guard is dropped. Does nothing when there is no record file (no config
/// folder, or it could not be created) or no slot is free.
pub fn begin(plugin: &str, action: &str, stamp: u64) -> ActionGuard {
    let Some(f) = current() else { return ActionGuard { rec: None } };
    let (prev_slot, prev_depth) = (SLOT_NOW.with(Cell::get), DEPTH.with(Cell::get));
    match f.start(plugin, action, stamp, prev_depth + 1) {
        Some(slot) => {
            SLOT_NOW.with(|c| c.set(slot));
            DEPTH.with(|c| c.set(prev_depth + 1));
            ActionGuard { rec: Some((f, slot, prev_slot, prev_depth)) }
        }
        None => ActionGuard { rec: None },
    }
}

/// The folder for the record, the strikes and the report of the previous session. Called when the
/// host sets the state folder; nothing is read or written until a plug-in is loaded
/// ([`recover_once`]) or run ([`begin`]), so a tool that only builds a session leaves what a crashed
/// EffectCraft left for the next real start.
pub fn set_dir(dir: PathBuf) {
    let mut g = global().lock().unwrap_or_else(PoisonError::into_inner);
    if g.dir.as_deref() == Some(dir.as_path()) {
        return;
    }
    // A different folder (tests): the old record goes with the old folder.
    if let Some(old) = g.current.take() {
        old.close();
    }
    *g = Global { dir: Some(dir), current: None, off: false, recovered: false };
}

/// Deal with what dead processes left in the state folder, once per process: report the plug-ins
/// that were running when they died ([`recovered_lines`]), count the strikes, remove the files. The
/// loader calls it before it registers anything.
pub fn recover_once() {
    let dir = {
        let mut g = global().lock().unwrap_or_else(PoisonError::into_inner);
        if g.recovered {
            return;
        }
        g.recovered = true;
        g.dir.clone()
    };
    let Some(dir) = dir else { return };
    let mut list = read_strikes(&dir);
    let report = recover(&dir, &mut list);
    *recovered().lock().unwrap_or_else(PoisonError::into_inner) = report;
    *strikes().lock().unwrap_or_else(PoisonError::into_inner) = list;
}

/// The app quits normally: plug-ins that rendered cleanly this session (`clean`, their identifiers)
/// lose their strikes, and the record file is removed (a crash while quitting is not blamed on a
/// plug-in).
pub fn close(clean: &[String]) {
    let (dir, record) = {
        let mut g = global().lock().unwrap_or_else(PoisonError::into_inner);
        g.off = true;
        (g.dir.clone(), g.current.take())
    };
    if let Some(r) = record {
        r.close();
    }
    if let Some(dir) = dir {
        forgive(&dir, clean);
    }
}

// ---------------------------------------------------------------------------------------------
// The previous session
// ---------------------------------------------------------------------------------------------

/// One plug-in action that was in progress when a process died.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Active {
    pub plugin: String,
    pub action: String,
    pub started_ms: u64,
    pub thread: u32,
    pub depth: u32,
    pub stamp: u64,
    slot: u32,
}

/// What a record file says about how its process ended.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Analysis {
    pub pid: u32,
    /// The plug-in that crashed, when that is known: named by the exception filter, or the only one
    /// that was running.
    pub culprit: Option<Active>,
    /// Plug-in actions that were running as well (or, when the culprit is not known, all of them).
    pub others: Vec<Active>,
    /// The exception code and the faulting module and address (Windows), when the filter saw it.
    pub fault: Option<String>,
}

/// Read a record file's bytes: who was running when the process died. `Err`: not a record file.
/// `Ok` with no culprit and no others: the process ended between actions.
pub fn analyse(bytes: &[u8]) -> Result<Analysis, String> {
    if bytes.len() < HEADER {
        return Err("it is shorter than its header".into());
    }
    if bytes.get(..8) != Some(&MAGIC[..]) {
        return Err("it is not an in-flight record".into());
    }
    if get_u32(bytes, 8) != VERSION {
        return Err(format!("it has version {}, this EffectCraft reads {VERSION}", get_u32(bytes, 8)));
    }
    let (count, size) = (get_u32(bytes, H_COUNT) as usize, get_u32(bytes, H_SIZE) as usize);
    if size != SLOT || count == 0 || count > SLOTS || bytes.len() < HEADER + count * size {
        return Err("its slots do not fit its size".into());
    }
    let mut active: Vec<Active> = Vec::new();
    for i in 0..count {
        let Some(s) = bytes.get(HEADER + i * SLOT..HEADER + (i + 1) * SLOT) else { break };
        if get_u32(s, 0) != 1 {
            continue;
        }
        active.push(Active {
            plugin: get_text(s, ID_AT, get_u32(s, 32), ID_MAX),
            action: get_text(s, ACTION_AT, get_u32(s, 36), ACTION_MAX),
            started_ms: get_u64(s, 16),
            thread: get_u32(s, 8),
            depth: get_u32(s, 4),
            stamp: get_u64(s, 24),
            slot: i as u32,
        });
    }
    let mut out = Analysis { pid: get_u32(bytes, H_PID), ..Default::default() };
    let code = get_u32(bytes, H_CODE);
    let crashed = get_u32(bytes, H_CRASH_SLOT);
    if code != 0 || crashed != 0 {
        let module =
            bytes.get(MODULE_AT..MODULE_AT + MODULE_MAX).map(|b| String::from_utf8_lossy(&b[..b.iter().position(|c| *c == 0).unwrap_or(b.len())]).into_owned());
        let at = get_u64(bytes, H_ADDRESS);
        out.fault = Some(format!("exception 0x{code:08X} at 0x{at:X}{}", module.filter(|m| !m.is_empty()).map(|m| format!(" in {m}")).unwrap_or_default()));
    }
    // The crashing thread's slot, as the exception filter recorded it.
    if crashed > 0
        && let Some(pos) = active.iter().position(|a| a.slot + 1 == crashed)
    {
        out.culprit = Some(active.remove(pos));
        out.others = active;
        return Ok(out);
    }
    // Without it: the innermost action of each thread are the candidates.
    let mut innermost: Vec<Active> = Vec::new();
    for a in &active {
        if !active.iter().any(|b| b.thread == a.thread && b.depth > a.depth) {
            innermost.push(a.clone());
        }
    }
    if innermost.len() == 1 {
        let only = innermost.remove(0);
        out.others = active.into_iter().filter(|a| a.slot != only.slot).collect();
        out.culprit = Some(only);
    } else {
        out.others = active;
    }
    Ok(out)
}

/// One plug-in's crashes in a row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Strike {
    strikes: u32,
    /// [`binary_stamp`] of its binary when it last crashed.
    stamp: u64,
    id: String,
    action: String,
}

fn parse_strike(line: &str) -> Option<Strike> {
    let line = line.trim_end_matches('\r');
    let mut parts = line.splitn(4, '\t');
    let strikes = parts.next()?.parse::<u32>().ok()?.max(1);
    let stamp = u64::from_str_radix(parts.next()?, 16).ok()?;
    let id = parts.next().filter(|i| !i.is_empty())?.to_string();
    Some(Strike { strikes, stamp, id, action: parts.next().unwrap_or("").to_string() })
}

fn read_strikes(dir: &Path) -> Vec<Strike> {
    let text = std::fs::read_to_string(dir.join(STRIKES_FILE)).unwrap_or_default();
    text.lines().filter_map(parse_strike).collect()
}

fn write_strikes(dir: &Path, list: &[Strike]) {
    let path = dir.join(STRIKES_FILE);
    if list.is_empty() {
        let _ = std::fs::remove_file(path);
        return;
    }
    let text: String = list.iter().map(|s| format!("{}\t{:x}\t{}\t{}\n", s.strikes, s.stamp, s.id, s.action)).collect();
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(path, text);
}

fn strikes() -> &'static Mutex<Vec<Strike>> {
    static S: OnceLock<Mutex<Vec<Strike>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Vec::new()))
}

fn recovered() -> &'static Mutex<Vec<String>> {
    static R: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Vec::new()))
}

/// What the previous session's record said, one line per crashed plug-in (starting with its
/// identifier and a colon), for the scan report and the start-up toast.
pub fn recovered_lines() -> Vec<String> {
    recovered().lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// Why `plugin` (whose binary has `stamp`) is skipped: it crashed EffectCraft in [`STRIKES`]
/// sessions in a row since its binary last changed. `None`: it may run.
pub fn skip_reason(plugin: &str, stamp: u64) -> Option<String> {
    let list = strikes().lock().unwrap_or_else(PoisonError::into_inner);
    let file = crate::blocklist::state_dir().map(|d| d.join(STRIKES_FILE).display().to_string()).unwrap_or_else(|| STRIKES_FILE.to_string());
    skip_reason_in(&list, plugin, stamp, &file)
}

fn skip_reason_in(list: &[Strike], plugin: &str, stamp: u64, file: &str) -> Option<String> {
    let s = list.iter().find(|s| s.id == plugin && s.stamp == stamp && s.strikes >= STRIKES)?;
    Some(format!(
        "skipped: it crashed EffectCraft while rendering {} times in a row (last in {}); update or reinstall it, remove its line from {file} or run effect.plugins.scanOfx {{\"retry\": true}} to try again",
        s.strikes,
        if s.action.is_empty() { "an action" } else { &s.action }
    ))
}

/// Forget every strike (`effect.plugins.scanOfx {"retry": true}`).
pub fn clear_strikes() {
    strikes().lock().unwrap_or_else(PoisonError::into_inner).clear();
    if let Some(dir) = crate::blocklist::state_dir() {
        let _ = std::fs::remove_file(dir.join(STRIKES_FILE));
    }
}

/// The plug-ins `ids` rendered without a crash for a whole session: their strikes are gone.
pub fn forgive(dir: &Path, ids: &[String]) {
    let mut list = strikes().lock().unwrap_or_else(PoisonError::into_inner);
    forgive_in(&mut list, dir, ids);
}

fn forgive_in(list: &mut Vec<Strike>, dir: &Path, ids: &[String]) {
    let before = list.len();
    list.retain(|s| !ids.contains(&s.id));
    if list.len() != before {
        write_strikes(dir, list);
    }
}

/// Count a crash of `id` (binary `stamp`) in `action`: the plug-in's crashes in a row.
fn strike(list: &mut Vec<Strike>, dir: &Path, id: &str, stamp: u64, action: &str) -> u32 {
    // Crashes before the binary changed do not count.
    let before = list.iter().find(|s| s.id == id && s.stamp == stamp).map_or(0, |s| s.strikes);
    let n = before.saturating_add(1);
    list.retain(|s| s.id != id);
    list.push(Strike { strikes: n, stamp, id: id.to_string(), action: action.to_string() });
    write_strikes(dir, list);
    n
}

/// Open a record file and lock it; `None` while a live process holds it (or it cannot be opened).
fn take_dead(path: &Path) -> Option<File> {
    let file = File::options().read(true).write(true).open(path).ok()?;
    file.try_lock().ok()?;
    Some(file)
}

fn record_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    rd.filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.len() > PREFIX.len() + SUFFIX.len() && n.starts_with(PREFIX) && n.ends_with(SUFFIX))
        })
        .collect()
}

/// Deal with the record files dead processes left in `dir`: report the plug-ins that were running
/// when their process died, count them as strikes (only a plug-in known to have crashed) and remove
/// the files. Files of live processes are left alone; unreadable ones are reported and removed.
/// `strikes` is the list of the strikes file (updated and written back). Returns one report line per
/// crash.
pub fn recover(dir: &Path, strikes: &mut Vec<Strike>) -> Vec<String> {
    let mut lines = Vec::new();
    for path in record_files(dir) {
        let Some(mut file) = take_dead(&path) else { continue };
        let mut bytes = Vec::new();
        let read = (&mut file).take(MAX_FILE).read_to_end(&mut bytes);
        drop(file);
        let _ = std::fs::remove_file(&path);
        if let Err(e) = read {
            lines.push(format!("{}: the in-flight record of a previous session could not be read ({e}); it is ignored", path.display()));
            continue;
        }
        // A file that never got its header (the process died as it was created) says nothing.
        if bytes.len() < HEADER && bytes.iter().all(|b| *b == 0) {
            continue;
        }
        match analyse(&bytes) {
            Ok(a) => lines.extend(report_lines(dir, strikes, &a)),
            Err(why) => lines.push(format!("{}: the in-flight record of a previous session is not usable ({why}); it is ignored", path.display())),
        }
    }
    lines
}

/// The report lines for one dead process, and the strike for the plug-in that crashed.
fn report_lines(dir: &Path, strikes: &mut Vec<Strike>, a: &Analysis) -> Vec<String> {
    let when = |x: &Active| utc_string(x.started_ms);
    let fault = a.fault.as_deref().map(|f| format!("; {f}")).unwrap_or_default();
    let mut lines = Vec::new();
    match &a.culprit {
        Some(c) => {
            let n = strike(strikes, dir, &c.plugin, c.stamp, &c.action);
            let outcome = if n >= STRIKES {
                format!(
                    "it crashed EffectCraft {n} times in a row and is skipped from now on; update or reinstall it, remove its line from {} or run effect.plugins.scanOfx {{\"retry\": true}} to try again",
                    dir.join(STRIKES_FILE).display()
                )
            } else {
                format!(
                    "strike {n} of {STRIKES}: it is skipped after {STRIKES} crashes in a row, and a session that renders it without a crash starts the count over"
                )
            };
            let mut line = format!(
                "{}: EffectCraft stopped while this plug-in was running {} (started {}, thread {}{fault}); {outcome}",
                c.plugin,
                c.action,
                when(c),
                c.thread
            );
            if !a.others.is_empty() {
                let others: Vec<String> = a.others.iter().map(|o| format!("{} ({})", o.plugin, o.action)).collect();
                line.push_str(&format!(". Also in progress at the time: {}", others.join(", ")));
            }
            lines.push(line);
        }
        None if !a.others.is_empty() => {
            let mut ids: Vec<&str> = a.others.iter().map(|o| o.plugin.as_str()).collect();
            ids.dedup();
            let list: Vec<String> = a.others.iter().map(|o| format!("{} ({})", o.plugin, o.action)).collect();
            lines.push(format!(
                "{}: EffectCraft stopped while several OpenFX plug-in actions were running ({}{fault}); which of them crashed is not known, so none is blamed",
                ids.join(" or "),
                list.join(", ")
            ));
        }
        None => {}
    }
    lines
}

// ---------------------------------------------------------------------------------------------
// Windows: the crashing thread's slot, the exception and the module, written by an unhandled
// exception filter
// ---------------------------------------------------------------------------------------------

#[cfg(windows)]
mod win {
    use std::fs::File;
    use std::os::windows::io::AsRawHandle;
    use std::sync::Once;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use windows_sys::Win32::Storage::FileSystem::WriteFile;
    use windows_sys::Win32::System::Diagnostics::Debug::{EXCEPTION_POINTERS, SetUnhandledExceptionFilter};
    use windows_sys::Win32::System::IO::OVERLAPPED;
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW, GetModuleHandleExW,
    };

    use super::{H_ADDRESS, H_CODE, H_CRASH_SLOT, MODULE_AT, MODULE_MAX, NO_SLOT, SLOT_NOW};

    /// The record file's handle, for the filter (0: none).
    static HANDLE: AtomicUsize = AtomicUsize::new(0);
    /// The filter that was installed before ours.
    static PREVIOUS: AtomicUsize = AtomicUsize::new(0);
    static BUSY: AtomicBool = AtomicBool::new(false);
    static INSTALL: Once = Once::new();

    type Filter = unsafe extern "system" fn(*const EXCEPTION_POINTERS) -> i32;

    pub fn set_handle(file: &File) {
        HANDLE.store(file.as_raw_handle() as usize, Ordering::SeqCst);
    }

    pub fn clear_handle() {
        HANDLE.store(0, Ordering::SeqCst);
    }

    /// Install the filter (once), in front of whatever filter there is.
    pub fn install() {
        INSTALL.call_once(|| {
            // SAFETY: registers a process-wide callback that only reads thread-locals and statics and
            // writes to a file handle.
            let prev = unsafe { SetUnhandledExceptionFilter(Some(filter)) };
            PREVIOUS.store(prev.map_or(0, |f| f as usize), Ordering::SeqCst);
        });
    }

    fn write_at(handle: usize, bytes: &[u8], offset: u32) {
        // SAFETY: a plain synchronous write at an offset (an OVERLAPPED that only carries the
        // offset) to the record file's handle; no allocation, no locks.
        unsafe {
            let mut o: OVERLAPPED = std::mem::zeroed();
            o.Anonymous.Anonymous.Offset = offset;
            let mut written = 0u32;
            WriteFile(handle as *mut std::ffi::c_void, bytes.as_ptr(), bytes.len() as u32, &mut written, &mut o);
        }
    }

    /// Runs on the crashing thread, with the process already doomed: note which slot it was in and
    /// what happened, then let the next filter (or Windows) carry on. Must not allocate or panic.
    unsafe extern "system" fn filter(info: *const EXCEPTION_POINTERS) -> i32 {
        if !BUSY.swap(true, Ordering::SeqCst) {
            record(info);
        }
        let prev = PREVIOUS.load(Ordering::SeqCst);
        if prev != 0 {
            // SAFETY: the previous filter, as `SetUnhandledExceptionFilter` returned it.
            let f: Filter = unsafe { std::mem::transmute::<usize, Filter>(prev) };
            return unsafe { f(info) };
        }
        0 // EXCEPTION_CONTINUE_SEARCH
    }

    fn record(info: *const EXCEPTION_POINTERS) {
        let handle = HANDLE.load(Ordering::SeqCst);
        if handle == 0 || info.is_null() {
            return;
        }
        // SAFETY: the pointers Windows passes to an unhandled exception filter.
        let (code, address) = unsafe {
            let rec = (*info).ExceptionRecord;
            if rec.is_null() { (0u32, 0u64) } else { ((*rec).ExceptionCode as u32, (*rec).ExceptionAddress as usize as u64) }
        };
        let slot = SLOT_NOW.try_with(std::cell::Cell::get).unwrap_or(NO_SLOT);
        let mut fixed = [0u8; 16];
        fixed[..4].copy_from_slice(&(if slot == NO_SLOT { 0 } else { slot + 1 }).to_le_bytes());
        fixed[4..8].copy_from_slice(&code.to_le_bytes());
        fixed[8..].copy_from_slice(&address.to_le_bytes());
        debug_assert_eq!(H_CODE, H_CRASH_SLOT + 4);
        debug_assert_eq!(H_ADDRESS, H_CODE + 4);
        write_at(handle, &fixed, H_CRASH_SLOT as u32);
        // The module the exception happened in, by file name.
        let mut wide = [0u16; 260];
        let mut name = [0u8; MODULE_MAX];
        // SAFETY: Win32 calls on a stack buffer and the faulting address.
        unsafe {
            let mut module = std::ptr::null_mut();
            let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
            if GetModuleHandleExW(flags, address as usize as *const u16, &mut module) != 0 {
                let n = GetModuleFileNameW(module, wide.as_mut_ptr(), wide.len() as u32) as usize;
                // Keep the file name (after the last backslash), ASCII only.
                let path = wide.get(..n.min(wide.len())).unwrap_or(&[]);
                let start = path.iter().rposition(|c| *c == u16::from(b'\\')).map_or(0, |i| i + 1);
                for (d, s) in name.iter_mut().zip(path.get(start..).unwrap_or(&[])) {
                    *d = if *s < 128 { *s as u8 } else { b'?' };
                }
            }
        }
        write_at(handle, &name, MODULE_AT as u32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, empty folder under the system temp folder; removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("effectcraft-ofx-inflight-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn bytes_of(f: &Inflight) -> Vec<u8> {
        f.contents()
    }

    /// The process dies: the handle closes (the OS drops the lock) and the file stays.
    fn die(f: Inflight) -> PathBuf {
        let path = f.path.clone();
        drop(f);
        path
    }

    #[test]
    fn a_finished_action_leaves_the_slot_idle() {
        let t = TempDir::new("idle");
        let f = Inflight::open(&t.0, 4242).unwrap();
        let slot = f.start("org.example.A", "OfxImageEffectActionRender", 7, 1).unwrap();
        let during = analyse(&bytes_of(&f)).unwrap();
        assert_eq!(
            during.culprit.as_ref().map(|c| (c.plugin.as_str(), c.action.as_str(), c.stamp, c.depth)),
            Some(("org.example.A", "OfxImageEffectActionRender", 7, 1))
        );
        assert_eq!(during.pid, 4242);
        f.finish(slot);
        // Between actions a dead process says nothing.
        assert_eq!(analyse(&bytes_of(&f)).unwrap(), Analysis { pid: 4242, ..Default::default() });
        // The slot is reusable (many more actions than slots, one after the other).
        for _ in 0..(SLOTS * 3) {
            let s = f.start("p", "a", 0, 1).unwrap();
            f.finish(s);
        }
    }

    #[test]
    fn nested_actions_blame_the_innermost_and_list_the_outer_one() {
        let t = TempDir::new("nested");
        let f = Inflight::open(&t.0, 1).unwrap();
        let _outer = f.start("org.example.Outer", "Render", 1, 1).unwrap();
        let _inner = f.start("org.example.Inner", "GetRegionOfDefinition", 2, 2).unwrap();
        let a = analyse(&bytes_of(&f)).unwrap();
        assert_eq!(a.culprit.as_ref().map(|c| c.plugin.as_str()), Some("org.example.Inner"));
        assert_eq!(a.others.iter().map(|o| o.plugin.as_str()).collect::<Vec<_>>(), ["org.example.Outer"]);
    }

    #[test]
    fn several_threads_without_the_crashing_slot_blame_nobody() {
        let t = TempDir::new("ambiguous");
        let f = Arc::new(Inflight::open(&t.0, 1).unwrap());
        // Two plug-ins at once, on two threads.
        let slots: Vec<u32> = std::thread::scope(|s| {
            let a = s.spawn(|| f.start("org.example.A", "Render", 1, 1).unwrap());
            let b = s.spawn(|| f.start("org.example.B", "Render", 2, 1).unwrap());
            vec![a.join().unwrap(), b.join().unwrap()]
        });
        let a = analyse(&bytes_of(&f)).unwrap();
        assert!(a.culprit.is_none() && a.others.len() == 2, "{a:?}");
        // The filter says which thread crashed: that one is blamed, the other listed.
        let mut bytes = bytes_of(&f);
        put_u32(&mut bytes, H_CRASH_SLOT, slots[1] + 1);
        put_u32(&mut bytes, H_CODE, 0xC000_0005);
        put_u64(&mut bytes, H_ADDRESS, 0x1234);
        put_text(&mut bytes, MODULE_AT, MODULE_MAX, "Plugin.ofx");
        let a = analyse(&bytes).unwrap();
        assert_eq!(a.culprit.as_ref().map(|c| c.stamp), Some(2));
        assert_eq!(a.others.len(), 1);
        assert_eq!(a.fault.as_deref(), Some("exception 0xC0000005 at 0x1234 in Plugin.ofx"));
    }

    #[test]
    fn running_out_of_slots_is_harmless() {
        let t = TempDir::new("full");
        let f = Inflight::open(&t.0, 1).unwrap();
        let held: Vec<u32> = (0..SLOTS).map(|i| f.start("p", "a", i as u64, 1).unwrap()).collect();
        assert!(f.start("p", "a", 99, 1).is_none());
        f.finish(held[3]);
        assert_eq!(f.start("p", "a", 99, 1), Some(3));
    }

    #[test]
    fn long_names_are_cut_at_a_character_boundary() {
        let t = TempDir::new("long");
        let f = Inflight::open(&t.0, 1).unwrap();
        let id = "é".repeat(100);
        f.start(&id, &"Render".repeat(20), 0, 1).unwrap();
        let a = analyse(&bytes_of(&f)).unwrap();
        let c = a.culprit.unwrap();
        assert_eq!(c.plugin.chars().count(), ID_MAX / 2);
        assert!(c.action.len() <= ACTION_MAX && c.action.starts_with("RenderRender"));
    }

    #[test]
    fn a_crash_is_recovered_reported_and_struck_twice_before_the_plugin_is_skipped() {
        let t = TempDir::new("recover");
        let id = "org.example.Crashy";
        let stamp = 77;
        // Session 1 dies inside Render: one strike, still allowed to run.
        let f = Inflight::open(&t.0, 100).unwrap();
        f.start(id, "OfxImageEffectActionRender", stamp, 1).unwrap();
        // A live process's file (this one) is left alone.
        let mut list = Vec::new();
        assert!(recover(&t.0, &mut list).is_empty());
        assert_eq!(record_files(&t.0).len(), 1);
        die(f);
        let lines = recover(&t.0, &mut list);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].starts_with("org.example.Crashy: EffectCraft stopped while this plug-in was running OfxImageEffectActionRender"), "{}", lines[0]);
        assert!(lines[0].contains("strike 1 of 2"), "{}", lines[0]);
        assert!(record_files(&t.0).is_empty(), "the record file is removed");
        assert_eq!(read_strikes(&t.0), [Strike { strikes: 1, stamp, id: id.into(), action: "OfxImageEffectActionRender".into() }]);
        // Skipping needs STRIKES crashes: not yet.
        let file = t.0.join(STRIKES_FILE).display().to_string();
        assert_eq!(skip_reason_in(&list, id, stamp, &file), None);
        // Session 2 dies there too: the plug-in is skipped from the third session on.
        let f = Inflight::open(&t.0, 101).unwrap();
        f.start(id, "OfxImageEffectActionRender", stamp, 1).unwrap();
        die(f);
        let lines = recover(&t.0, &mut list);
        assert!(lines[0].contains("crashed EffectCraft 2 times in a row and is skipped from now on"), "{}", lines[0]);
        assert!(lines[0].contains("ofx_render_crashes.txt") && lines[0].contains("retry"), "{}", lines[0]);
        assert_eq!(read_strikes(&t.0)[0].strikes, 2);
        let why = skip_reason_in(&list, id, stamp, &file).unwrap();
        assert!(why.starts_with("skipped: it crashed EffectCraft while rendering 2 times in a row (last in OfxImageEffectActionRender)"), "{why}");
        // A changed binary (another stamp) and another plug-in are not skipped; a new crash after the
        // binary changed counts from one.
        assert_eq!(skip_reason_in(&list, id, stamp + 1, &file), None);
        assert_eq!(skip_reason_in(&list, "org.example.Other", stamp, &file), None);
        assert_eq!(strike(&mut list, &t.0, id, stamp + 1, "Render"), 1);
        assert_eq!(strike(&mut list, &t.0, id, stamp + 1, "Render"), 2);
        // A session that rendered it cleanly starts the count over (only possible while it runs).
        forgive_in(&mut list, &t.0, &[id.to_string()]);
        assert_eq!(skip_reason_in(&list, id, stamp + 1, &file), None);
        assert!(!t.0.join(STRIKES_FILE).exists());
    }

    #[test]
    fn an_ambiguous_crash_is_reported_but_nobody_is_struck() {
        let t = TempDir::new("nobody");
        let f = Arc::new(Inflight::open(&t.0, 5).unwrap());
        std::thread::scope(|s| {
            s.spawn(|| f.start("org.example.A", "Render", 1, 1).unwrap()).join().unwrap();
            s.spawn(|| f.start("org.example.B", "Render", 2, 1).unwrap()).join().unwrap();
        });
        die(Arc::try_unwrap(f).ok().unwrap());
        let before = read_strikes(&t.0);
        let lines = recover(&t.0, &mut Vec::new());
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("org.example.A or org.example.B:") && lines[0].contains("none is blamed"), "{}", lines[0]);
        assert_eq!(read_strikes(&t.0), before);
    }

    #[test]
    fn a_session_that_died_between_actions_says_nothing() {
        let t = TempDir::new("quiet");
        let f = Inflight::open(&t.0, 9).unwrap();
        let s = f.start("p", "a", 0, 1).unwrap();
        f.finish(s);
        die(f);
        assert!(recover(&t.0, &mut Vec::new()).is_empty());
        assert!(record_files(&t.0).is_empty());
    }

    #[test]
    fn unusable_record_files_are_reported_and_removed() {
        let t = TempDir::new("junk");
        std::fs::write(t.0.join("ofx_inflight.1.bin"), b"not a record file at all, just text that is long enough".repeat(8)).unwrap();
        let mut wrong_version = vec![0u8; HEADER + SLOTS * SLOT];
        wrong_version[..8].copy_from_slice(MAGIC);
        put_u32(&mut wrong_version, 8, 99);
        std::fs::write(t.0.join("ofx_inflight.2.bin"), wrong_version).unwrap();
        // A file that never got a header (the process died as it was created) and an empty one are silent.
        std::fs::write(t.0.join("ofx_inflight.3.bin"), vec![0u8; 10]).unwrap();
        std::fs::write(t.0.join("ofx_inflight.4.bin"), b"").unwrap();
        // Other files in the folder are not touched.
        std::fs::write(t.0.join("ofx_inflight.txt"), b"x").unwrap();
        let lines = recover(&t.0, &mut Vec::new());
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("is not usable (it is not an in-flight record)")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("it has version 99")), "{lines:?}");
        assert!(record_files(&t.0).is_empty());
        assert!(t.0.join("ofx_inflight.txt").exists());
        // Slots that claim a crazy size are refused, not indexed.
        let mut bad = vec![0u8; HEADER + 10];
        bad[..8].copy_from_slice(MAGIC);
        put_u32(&mut bad, 8, VERSION);
        put_u32(&mut bad, H_COUNT, 1000);
        put_u32(&mut bad, H_SIZE, SLOT as u32);
        assert!(analyse(&bad).unwrap_err().contains("do not fit"));
    }

    #[test]
    fn a_corrupt_strikes_file_is_ignored() {
        let t = TempDir::new("strikes");
        std::fs::write(t.0.join(STRIKES_FILE), "garbage\n\n2\tzz\tid\n1\t10\t\nx\t1\ty\n3\t1f\torg.example.Ok\tRender\r\n").unwrap();
        let list = read_strikes(&t.0);
        assert_eq!(list, [Strike { strikes: 3, stamp: 0x1f, id: "org.example.Ok".into(), action: "Render".into() }]);
    }

    #[test]
    fn closing_removes_the_file_and_the_time_formats() {
        let t = TempDir::new("close");
        let f = Inflight::open(&t.0, 3).unwrap();
        assert_eq!(record_files(&t.0).len(), 1);
        // An action still running when the app quits is not blamed on its plug-in.
        let running = f.start("org.example.Busy", "Render", 1, 1).unwrap();
        f.close();
        assert!(record_files(&t.0).is_empty());
        assert!(analyse(&f.contents()).unwrap().culprit.is_none());
        // Nothing is recorded after that, and finishing the action is harmless.
        assert!(f.start("p", "a", 0, 1).is_none());
        f.finish(running);
        assert_eq!(utc_string(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(utc_string(1_776_366_191_000), "2026-04-16 19:03:11 UTC");
        assert_eq!(utc_string(951_782_400_000), "2000-02-29 00:00:00 UTC");
    }

    #[test]
    fn recording_without_a_folder_does_nothing() {
        // No state folder in this process: begin() is inert and the guard drops quietly.
        let before = current_slot();
        let g = begin("p", "a", 0);
        assert_eq!(current_slot(), before);
        drop(g);
        // Adopting a slot restores the old one.
        {
            let _s = adopt_slot(5);
            assert_eq!(current_slot(), 5);
        }
        assert_eq!(current_slot(), before);
    }

    #[test]
    fn binary_stamps_follow_the_file() {
        let t = TempDir::new("stamp");
        let p = t.0.join("a.ofx");
        assert_eq!(binary_stamp(&p), 0);
        std::fs::write(&p, b"one").unwrap();
        let a = binary_stamp(&p);
        assert_ne!(a, 0);
        assert_eq!(binary_stamp(&p), a);
        std::fs::write(&p, b"another").unwrap();
        assert_ne!(binary_stamp(&p), a);
    }
}

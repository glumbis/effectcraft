//! Diagnostics: why a plug-in did not load or render, in words a user can send in a bug report.
//!
//! Native plug-ins fail in ways that say little (an OFX status number, nothing at all when a
//! plug-in declines the host). This module keeps what the host can learn about a plug-in while it
//! runs:
//!
//! * the **messages** the plug-in posted through the message suite (the text it shows other
//!   hosts' users, which usually names the problem: "licence not found", "needs DaVinci Resolve"),
//! * the **unmet requests**: properties and suites the plug-in asked for that this host does not
//!   provide (a plug-in that gets `kOfxStatErrUnknown` for a property it needs typically fails with
//!   `kOfxStatErrMissingHostFeature` without saying which one),
//! * the **host properties it read**, so the report shows what the plug-in was told about the host
//!   (its name above all: some plug-ins only run in hosts they know).
//!
//! Each loaded plug-in owns a [`PluginDiag`]. The suites find it through a thread-local
//! [`CallCtx`] that [`PluginEntry::call`](crate::instance::PluginEntry::call) sets for the
//! duration of every action (and `multiThread` copies onto its workers). Calls from threads the
//! host did not start (a plug-in's own threads) have no context and are only logged.
//!
//! Unmet requests are also logged at `warn`, once per (name, kind of property set) for the whole
//! process, so they reach the session log at the default log level.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashSet, VecDeque};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use crate::ffi::*;

/// The name of an OFX status, followed by its number: `kOfxStatErrMissingHostFeature (4)`.
pub fn status_name(s: OfxStatus) -> String {
    let name = match s {
        K_STAT_OK => "kOfxStatOK",
        K_STAT_FAILED => "kOfxStatFailed",
        K_STAT_ERR_FATAL => "kOfxStatErrFatal",
        K_STAT_ERR_UNKNOWN => "kOfxStatErrUnknown",
        K_STAT_ERR_MISSING_HOST_FEATURE => "kOfxStatErrMissingHostFeature",
        K_STAT_ERR_UNSUPPORTED => "kOfxStatErrUnsupported",
        K_STAT_ERR_EXISTS => "kOfxStatErrExists",
        K_STAT_ERR_FORMAT => "kOfxStatErrFormat",
        K_STAT_ERR_MEMORY => "kOfxStatErrMemory",
        K_STAT_ERR_BAD_HANDLE => "kOfxStatErrBadHandle",
        K_STAT_ERR_BAD_INDEX => "kOfxStatErrBadIndex",
        K_STAT_ERR_VALUE => "kOfxStatErrValue",
        K_STAT_REPLY_YES => "kOfxStatReplyYes",
        K_STAT_REPLY_NO => "kOfxStatReplyNo",
        K_STAT_REPLY_DEFAULT => "kOfxStatReplyDefault",
        K_STAT_UNLICENSED => "kOfxStatUnlicensed",
        K_STAT_ERR_IMAGE_FORMAT => "kOfxStatErrImageFormat",
        _ => return format!("unknown OFX status {s}"),
    };
    format!("{name} ({s})")
}

/// Whether `s` is one of the statuses that report a failure (as opposed to OK, the "reply"
/// statuses, or a value that is not an OFX status at all).
pub fn is_failure(s: OfxStatus) -> bool {
    matches!(s, K_STAT_FAILED..=K_STAT_ERR_VALUE | K_STAT_UNLICENSED | K_STAT_ERR_IMAGE_FORMAT)
}

/// Which kind of property set a [`PropSet`](crate::props::PropSet) is, for messages that say
/// "property X of the effect descriptor".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PropKind {
    /// The in / out arguments of one action (the default for sets the host builds for a call).
    #[default]
    Args = 0,
    Host,
    EffectDescriptor,
    EffectInstance,
    Clip,
    Param,
    Image,
}

impl PropKind {
    pub fn from_u8(v: u8) -> PropKind {
        match v {
            1 => PropKind::Host,
            2 => PropKind::EffectDescriptor,
            3 => PropKind::EffectInstance,
            4 => PropKind::Clip,
            5 => PropKind::Param,
            6 => PropKind::Image,
            _ => PropKind::Args,
        }
    }

    /// How a message names this kind of set.
    pub fn describe(self) -> &'static str {
        match self {
            PropKind::Args => "action arguments",
            PropKind::Host => "host",
            PropKind::EffectDescriptor => "effect descriptor",
            PropKind::EffectInstance => "effect instance",
            PropKind::Clip => "clip",
            PropKind::Param => "parameter",
            PropKind::Image => "image",
        }
    }
}

/// Something a plug-in asked for that this host does not provide.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Request {
    Property { kind: PropKind, name: String },
    Suite { name: String, version: i32 },
}

impl std::fmt::Display for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Request::Property { kind, name } => write!(f, "property {name} of the {}", kind.describe()),
            Request::Suite { name, version } => write!(f, "suite {name} v{version}"),
        }
    }
}

/// A message a plug-in posted through the message suite.
#[derive(Clone, Debug)]
pub struct Msg {
    seq: u64,
    pub action: String,
    /// `error`, `fatal`, `warning`, `message`, `log` or `question`.
    pub kind: &'static str,
    pub text: String,
}

/// A [`Request`] and the action it was made during.
#[derive(Clone, Debug)]
pub struct Unmet {
    seq: u64,
    pub request: Request,
    pub action: String,
}

const MAX_MESSAGES: usize = 32;
const MAX_UNMET: usize = 64;
const MAX_HOST_READS: usize = 128;
/// The longest message text kept.
const MAX_TEXT: usize = 600;

#[derive(Default)]
struct Inner {
    seq: u64,
    messages: VecDeque<Msg>,
    unmet: Vec<Unmet>,
    host_reads: BTreeSet<String>,
}

/// What the host learned about one plug-in while it ran (see the module docs).
#[derive(Default)]
pub struct PluginDiag {
    inner: Mutex<Inner>,
}

impl PluginDiag {
    pub fn new() -> Arc<PluginDiag> {
        Arc::new(PluginDiag::default())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A position to ask "what happened since" about.
    pub fn mark(&self) -> u64 {
        self.lock().seq
    }

    pub fn record_message(&self, action: &str, kind: &'static str, text: &str) {
        let mut g = self.lock();
        g.seq += 1;
        let seq = g.seq;
        let text: String = text.chars().take(MAX_TEXT).collect();
        if g.messages.len() >= MAX_MESSAGES {
            g.messages.pop_front();
        }
        g.messages.push_back(Msg { seq, action: action.to_string(), kind, text });
    }

    /// Record an unmet request (once per distinct request for this plug-in).
    pub fn record_unmet(&self, action: &str, request: Request) {
        let mut g = self.lock();
        if g.unmet.len() >= MAX_UNMET || g.unmet.iter().any(|u| u.request == request) {
            return;
        }
        g.seq += 1;
        let seq = g.seq;
        g.unmet.push(Unmet { seq, request, action: action.to_string() });
    }

    pub fn record_host_read(&self, name: &str) {
        let mut g = self.lock();
        if g.host_reads.len() < MAX_HOST_READS {
            g.host_reads.insert(name.to_string());
        }
    }

    /// The messages posted after `mark`.
    pub fn messages_since(&self, mark: u64) -> Vec<Msg> {
        self.lock().messages.iter().filter(|m| m.seq > mark).cloned().collect()
    }

    /// The most recent message (any kind).
    pub fn last_message(&self) -> Option<Msg> {
        self.lock().messages.back().cloned()
    }

    /// The requests first made after `mark`.
    pub fn unmet_since(&self, mark: u64) -> Vec<Unmet> {
        self.lock().unmet.iter().filter(|u| u.seq > mark).cloned().collect()
    }

    /// Every unmet request so far.
    pub fn unmet_all(&self) -> Vec<Unmet> {
        self.lock().unmet.clone()
    }

    /// The names of the host properties the plug-in read, sorted.
    pub fn host_reads(&self) -> Vec<String> {
        self.lock().host_reads.iter().cloned().collect()
    }
}

/// The plug-in and action whose code is running on this thread.
#[derive(Clone)]
pub struct CallCtx {
    pub diag: Arc<PluginDiag>,
    pub plugin: Arc<str>,
    pub action: Arc<str>,
}

thread_local! {
    static CTX: RefCell<Option<CallCtx>> = const { RefCell::new(None) };
}

/// Restores the previous context when dropped.
pub struct Scope(Option<CallCtx>);

impl Drop for Scope {
    fn drop(&mut self) {
        let prev = self.0.take();
        CTX.with(|c| {
            if let Ok(mut c) = c.try_borrow_mut() {
                *c = prev;
            }
        });
    }
}

impl CallCtx {
    /// Make this the context of the current thread until the returned guard is dropped.
    pub fn enter(&self) -> Scope {
        let prev = CTX.with(|c| c.try_borrow_mut().ok().and_then(|mut c| c.replace(self.clone())));
        Scope(prev)
    }
}

/// The context of the current thread, if a host action is running on it.
pub fn current() -> Option<CallCtx> {
    CTX.with(|c| c.try_borrow().ok().and_then(|c| c.clone()))
}

fn warned() -> &'static Mutex<HashSet<Request>> {
    static W: OnceLock<Mutex<HashSet<Request>>> = OnceLock::new();
    W.get_or_init(|| Mutex::new(HashSet::new()))
}

/// A plug-in asked for something this host does not provide: record it for the plug-in and log
/// it at `warn`, once per distinct request for the whole process.
pub fn unmet(request: Request) {
    let ctx = current();
    if let Some(c) = &ctx {
        c.diag.record_unmet(&c.action, request.clone());
    }
    let first = warned().lock().unwrap_or_else(PoisonError::into_inner).insert(request.clone());
    if first {
        match &ctx {
            Some(c) => log::warn!("openfx: {} asked for {request} during {}, which this host does not provide", c.plugin, c.action),
            None => log::warn!("openfx: a plug-in asked for {request}, which this host does not provide"),
        }
    }
}

/// A plug-in asked for a property that is not set.
pub fn unknown_property(kind: PropKind, name: &str) {
    unmet(Request::Property { kind, name: name.to_string() });
}

/// A plug-in asked for a suite this host does not provide.
pub fn unknown_suite(name: &str, version: i32) {
    unmet(Request::Suite { name: name.to_string(), version });
}

/// A plug-in read a property of the host.
pub fn host_read(name: &str) {
    if let Some(c) = current() {
        c.diag.record_host_read(name);
    }
}

/// A plug-in posted a message (through the message suite) on the current thread.
pub fn message(kind: &'static str, text: &str) {
    if let Some(c) = current() {
        c.diag.record_message(&c.action, kind, text);
    }
}

/// `text` shortened to `max` characters with an ellipsis, on one line.
pub fn one_line(text: &str, max: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max { flat } else { format!("{}...", flat.chars().take(max).collect::<String>()) }
}

/// The messages and unmet requests of an action as one sentence for an error line, or empty.
pub fn explain(msgs: &[Msg], unmet: &[Unmet]) -> String {
    let mut out = String::new();
    let said: Vec<String> = msgs.iter().filter(|m| m.kind != "log").map(|m| format!("\"{}\"", one_line(&m.text, 300))).collect();
    if !said.is_empty() {
        out.push_str(&format!("; the plug-in said {}", said.join(", ")));
    }
    if !unmet.is_empty() {
        let list: Vec<String> = unmet.iter().take(8).map(|u| u.request.to_string()).collect();
        let more = if unmet.len() > 8 { format!(" and {} more", unmet.len() - 8) } else { String::new() };
        out.push_str(&format!("; it asked for something this host does not provide: {}{more}", list.join(", ")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_have_names() {
        assert_eq!(status_name(K_STAT_OK), "kOfxStatOK (0)");
        assert_eq!(status_name(4), "kOfxStatErrMissingHostFeature (4)");
        assert_eq!(status_name(1000), "kOfxStatErrImageFormat (1000)");
        assert_eq!(status_name(77), "unknown OFX status 77");
        assert!(is_failure(K_STAT_FAILED) && is_failure(K_STAT_UNLICENSED) && is_failure(4));
        assert!(!is_failure(K_STAT_OK) && !is_failure(K_STAT_REPLY_DEFAULT) && !is_failure(K_STAT_REPLY_YES) && !is_failure(0x7fff_0000));
    }

    #[test]
    fn requests_and_messages_are_recorded_per_action() {
        let diag = PluginDiag::new();
        let ctx = CallCtx { diag: diag.clone(), plugin: "p".into(), action: "Describe".into() };
        let mark = diag.mark();
        {
            let _s = ctx.enter();
            unknown_property(PropKind::Host, "OfxPropTestOnlyA");
            // The same request again is recorded once.
            unknown_property(PropKind::Host, "OfxPropTestOnlyA");
            unknown_property(PropKind::Clip, "OfxPropTestOnlyA");
            unknown_suite("OfxTestSuite", 1);
            message("error", "licence not found");
            host_read("OfxPropName");
        }
        // Outside the scope nothing is recorded for the plug-in.
        unknown_property(PropKind::Host, "OfxPropTestOnlyB");
        assert!(current().is_none());
        let unmet = diag.unmet_since(mark);
        assert_eq!(unmet.len(), 3, "{unmet:?}");
        assert_eq!(unmet[0].request.to_string(), "property OfxPropTestOnlyA of the host");
        assert_eq!(unmet[1].request.to_string(), "property OfxPropTestOnlyA of the clip");
        assert_eq!(unmet[2].request.to_string(), "suite OfxTestSuite v1");
        assert_eq!(unmet[0].action, "Describe");
        let msgs = diag.messages_since(mark);
        assert_eq!((msgs.len(), msgs[0].kind, msgs[0].text.as_str()), (1, "error", "licence not found"));
        assert_eq!(diag.host_reads(), ["OfxPropName"]);
        let text = explain(&msgs, &unmet);
        assert!(text.contains("the plug-in said \"licence not found\"") && text.contains("suite OfxTestSuite v1"), "{text}");
        // Nothing new since now.
        assert!(diag.unmet_since(diag.mark()).is_empty() && diag.messages_since(diag.mark()).is_empty());
    }

    #[test]
    fn scopes_nest_and_restore() {
        let a = PluginDiag::new();
        let b = PluginDiag::new();
        let ca = CallCtx { diag: a, plugin: "a".into(), action: "x".into() };
        let cb = CallCtx { diag: b, plugin: "b".into(), action: "y".into() };
        let sa = ca.enter();
        {
            let _sb = cb.enter();
            assert_eq!(current().map(|c| c.plugin.to_string()).as_deref(), Some("b"));
        }
        assert_eq!(current().map(|c| c.plugin.to_string()).as_deref(), Some("a"));
        drop(sa);
        assert!(current().is_none());
    }

    #[test]
    fn long_text_is_shortened_to_one_line() {
        assert_eq!(one_line("a\n  b\tc", 10), "a b c");
        assert_eq!(one_line("abcdefghij", 4), "abcd...");
    }
}

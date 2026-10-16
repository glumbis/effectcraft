//! How the host manages plug-in instances and sequences of renders, seen through the test
//! plug-in's probe effects: each probe paints what it counted (which instance it is, how many
//! sequences were begun and ended, ...) into the first three pixels of its output.
//!
//! * `Probe` declares `kOfxImageEffectInstancePropSequentialRender` = 1,
//! * `ProbeSingle` also allows one instance only,
//! * `ProbeSlow` takes 300 ms to create an instance and keeps no state between frames,
//! * `Memory` answers `kOfxStatErrMemory` until the host sends `kOfxActionPurgeCaches`.
//!
//! The last scenario runs the host's shutdown in a child process and reads what the plug-in logged.
//! Set `EC_TEST_OFX_BUNDLE` to the built `ec-test-ofx.ofx.bundle`; without it the tests return.

use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use crate::common::{Fixture, defaults};
use effectcraft_effects::plugin::{EffectPlugin, PluginFrame, PluginHost, PluginImage, PluginParams, plugin};
use effectcraft_ofx::load_path;
use serde_json::json;

/// The effects share counters inside the plug-in library: one scenario at a time.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// The test library's effect `id` (the bundle is loaded once per process).
fn effect(id: &str) -> Option<Arc<dyn EffectPlugin>> {
    static LOADED: OnceLock<bool> = OnceLock::new();
    let ok = *LOADED.get_or_init(|| match std::env::var_os("EC_TEST_OFX_BUNDLE") {
        Some(p) => {
            let r = load_path(&PathBuf::from(p));
            assert!(r.errors.is_empty(), "{:?}", r.errors);
            true
        }
        None => {
            println!("note: EC_TEST_OFX_BUNDLE is not set; skipping the instance tests");
            false
        }
    });
    ok.then(|| plugin(id).unwrap_or_else(|| panic!("{id} is not registered")))
}

/// The host of one applied effect: its instance key, the frame rate, the source's span in seconds,
/// and optionally what is upstream (`nest`: the same effect `depth` more times, rendered when the
/// plug-in asks the host for an image).
struct Host {
    key: u64,
    fps: f64,
    span: Option<(f64, f64)>,
    nest: Option<(Arc<dyn EffectPlugin>, u32)>,
}

impl Host {
    fn new(key: u64) -> Host {
        Host { key, fps: 30.0, span: None, nest: None }
    }
}

impl PluginHost for Host {
    fn input_at(&self, _layer_time: f64) -> Option<PluginImage> {
        let (effect, depth) = self.nest.as_ref().filter(|(_, d)| *d > 0)?;
        let inner = Host { key: self.key + 1, fps: self.fps, span: self.span, nest: Some((effect.clone(), depth - 1)) };
        let seen = render(&**effect, 0.0, &inner);
        Some(PluginImage { width: 4, height: 1, pixels: seen.pixels, origin: [0.0; 2], scale: 1.0 })
    }
    fn layer_input(&self, _param_id: &str, _layer_time: f64) -> Option<PluginImage> {
        None
    }
    fn params_at(&self, _layer_time: f64) -> Option<(Vec<f64>, Vec<String>)> {
        None
    }
    fn frame_rate(&self) -> f64 {
        self.fps
    }
    fn time(&self) -> f64 {
        1.0
    }
    fn instance_key(&self) -> u64 {
        self.key
    }
    fn frame_range(&self) -> Option<(f64, f64)> {
        self.span
    }
}

/// What a render of a probe painted.
#[derive(Debug, Clone)]
struct Seen {
    pixels: Vec<[f32; 4]>,
}

impl Seen {
    fn at(&self, i: usize, c: usize) -> f64 {
        f64::from(self.pixels[i][c])
    }
    fn serial(&self) -> u32 {
        self.at(0, 0) as u32
    }
    fn begins(&self) -> u32 {
        self.at(0, 1) as u32
    }
    fn in_sequence(&self) -> u32 {
        self.at(0, 2) as u32
    }
    fn live(&self) -> u32 {
        self.at(0, 3) as u32
    }
    fn declared(&self) -> i32 {
        self.at(1, 0) as i32
    }
    fn ends(&self) -> u32 {
        self.at(1, 1) as u32
    }
    fn status(&self) -> i32 {
        self.at(1, 2) as i32
    }
    fn range(&self) -> (f64, f64) {
        (self.at(2, 0), self.at(1, 3))
    }
    fn begin_status(&self) -> i32 {
        self.at(2, 1) as i32
    }
    fn creates(&self) -> u32 {
        self.at(2, 2) as u32
    }
}

/// Render frame number `frame` of a 4x1 layer through `p` with `host`.
fn render(p: &dyn EffectPlugin, frame: f64, host: &Host) -> Seen {
    let (flat, texts) = defaults(p, &[]);
    let params = PluginParams { manifest: p.manifest(), flat: &flat, texts: &texts };
    let mut pixels = vec![[0.0f32; 4]; 4];
    let mut f = PluginFrame { width: 4, height: 1, pixels: &mut pixels, scale: 1.0, origin: [0.0; 2], layer_size: [4.0, 1.0] };
    p.render_v2(&mut f, &params, frame / host.fps, host).unwrap_or_else(|e| panic!("render of frame {frame}: {e}"));
    Seen { pixels }
}

/// A sequence stays open while the frames advance by one and ends on any other step; a plug-in's
/// declared need for frames in order is passed through and never overwritten.
#[test]
fn a_sequence_stays_open_while_frames_advance_and_ends_on_a_jump() {
    let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Some(p) = effect("org.effectcraft.test.probe") else { return };
    // The source lasts 10 s at 30 fps: frames 0 to 299.
    let host = Host { span: Some((0.0, 10.0)), ..Host::new(1000) };
    let a = render(&*p, 10.0, &host);
    assert_eq!((a.begins(), a.in_sequence(), a.ends()), (1, 1, 0), "{a:?}");
    // The plug-in's own declaration reaches its instance (not the host's 0), and it is told the
    // frames are rendered in order; the sequence runs from this frame to the end of the source.
    assert_eq!((a.declared(), a.status(), a.begin_status()), (1, 1, 1), "{a:?}");
    assert_eq!(a.range(), (10.0, 299.0), "{a:?}");
    // The next frames continue the sequence on the same instance: no Begin, no End.
    let b = render(&*p, 11.0, &host);
    let c = render(&*p, 12.0, &host);
    assert_eq!((b.serial(), b.begins(), b.in_sequence(), b.ends()), (a.serial(), 1, 2, 0), "{b:?}");
    assert_eq!((c.serial(), c.begins(), c.in_sequence(), c.ends()), (a.serial(), 1, 3, 0), "{c:?}");
    // A jump ends the sequence and begins another from the new frame; so does going backwards.
    let d = render(&*p, 40.0, &host);
    assert_eq!((d.serial(), d.begins(), d.in_sequence(), d.ends()), (a.serial(), 2, 1, 1), "{d:?}");
    assert_eq!(d.range(), (40.0, 299.0));
    let e = render(&*p, 39.0, &host);
    assert_eq!((e.begins(), e.in_sequence(), e.ends()), (3, 1, 2), "{e:?}");
    // The same frame again is not the next frame either.
    let f = render(&*p, 39.0, &host);
    assert_eq!((f.begins(), f.ends()), (4, 3), "{f:?}");
    // Without the source's span the range runs to the long fallback the clips report.
    let g = render(&*p, 5.0, &Host::new(1001));
    assert_eq!(g.range(), (5.0, 99999.0), "{g:?}");
}

/// Applied effects keep their own instances (and sequences), also when their frames interleave.
#[test]
fn each_applied_effect_has_its_own_instance_and_sequence() {
    let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Some(p) = effect("org.effectcraft.test.probe") else { return };
    let (a, b) = (Host::new(2000), Host::new(2001));
    let a1 = render(&*p, 1.0, &a);
    let b1 = render(&*p, 1.0, &b);
    let a2 = render(&*p, 2.0, &a);
    let b2 = render(&*p, 2.0, &b);
    assert_ne!(a1.serial(), b1.serial());
    assert_eq!((a2.serial(), b2.serial()), (a1.serial(), b1.serial()));
    // Each continued its own sequence although the other rendered in between.
    assert_eq!((a2.begins(), a2.in_sequence()), (1, 2), "{a2:?}");
    assert_eq!((b2.begins(), b2.in_sequence()), (1, 2), "{b2:?}");
}

/// A plug-in that allows one instance gets one, shared by every application of it, whichever thread
/// renders.
#[test]
fn a_single_instance_plugin_has_one_shared_instance() {
    let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Some(p) = effect("org.effectcraft.test.probesingle") else { return };
    let first = render(&*p, 1.0, &Host::new(3000));
    for key in [3001, 3002, 3000, 3003] {
        let seen = render(&*p, 1.0, &Host::new(key));
        assert_eq!(seen.serial(), first.serial(), "key {key}");
        assert_eq!(seen.live(), 1, "{seen:?}");
    }
    // Four threads at once take turns on it.
    let serials: Vec<u32> = std::thread::scope(|s| {
        let jobs: Vec<_> = (0..4u64)
            .map(|i| {
                let p = p.clone();
                s.spawn(move || (0..5).map(|f| render(&*p, f as f64, &Host::new(3100 + i)).serial()).collect::<Vec<_>>())
            })
            .collect();
        jobs.into_iter().flat_map(|j| j.join().unwrap()).collect()
    });
    assert!(serials.iter().all(|s| *s == first.serial()), "{serials:?}");
}

/// A plug-in that is slow to create instances (it loads a model) is limited to two of them: a render
/// that finds both in use or idle for other applications takes one of those instead of making a
/// third; a nested render that would otherwise wait for itself makes one more, which is dropped when
/// it comes back.
#[test]
fn a_plugin_that_is_slow_to_create_instances_is_limited_and_shares_them() {
    let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Some(p) = effect("org.effectcraft.test.probeslow") else { return };
    // The first instance takes 300 ms to create: the plug-in is heavy from now on.
    let a = render(&*p, 1.0, &Host::new(4000));
    assert_eq!((a.creates(), a.declared()), (1, 0), "{a:?}");
    let b = render(&*p, 1.0, &Host::new(4001));
    assert_eq!((b.creates(), b.live()), (2, 2), "{b:?}");
    assert_ne!(a.serial(), b.serial());
    // A third application finds the limit reached and takes the instance idle longest.
    let c = render(&*p, 1.0, &Host::new(4002));
    assert_eq!((c.creates(), c.live()), (2, 2), "no third instance: {c:?}");
    assert_eq!(c.serial(), a.serial(), "{c:?}");
    // An effect rendering while the host renders upstream effects of the same plug-in (here three levels): the
    // innermost has nothing to take and would wait for its own task, so it makes one more.
    let nested = Host { nest: Some((p.clone(), 2)), ..Host::new(4010) };
    let (tx, rx) = channel();
    let (effect, h) = (p.clone(), nested);
    std::thread::spawn(move || {
        let _ = tx.send(render(&*effect, 1.0, &h));
    });
    let outer = rx.recv_timeout(Duration::from_secs(15)).expect("a nested render of a limited plug-in waited for itself");
    assert!(outer.creates() >= 2, "{outer:?}");
    // Afterwards the plug-in is back to two instances: what was made beyond the limit was destroyed.
    let after = render(&*p, 2.0, &Host::new(4020));
    assert_eq!(after.live(), 2, "{after:?}");
    assert!(after.creates() >= 3, "{after:?}");
}

/// `kOfxStatErrMemory` from Render: the plug-in is sent `kOfxActionPurgeCaches` once and the render is
/// tried once more.
#[test]
fn an_out_of_memory_render_is_retried_once_after_the_caches_are_purged() {
    let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Some(p) = effect("org.effectcraft.test.memory") else { return };
    let host = Host::new(5000);
    // Two renders happened (the first one ran out of memory) and one purge.
    let first = render(&*p, 1.0, &host);
    assert_eq!((first.at(0, 0), first.at(0, 1)), (2.0, 1.0), "{first:?}");
    // The caches stay purged: no more purges, no more retries.
    let second = render(&*p, 2.0, &host);
    assert_eq!((second.at(0, 0), second.at(0, 1)), (3.0, 1.0), "{second:?}");
}

/// The child of the shutdown scenario: render a probe (a sequence is open on its instance), then
/// quit the way the app does.
#[test]
fn child_shutdown() {
    let Some(cfg) = std::env::var_os("EC_FIXTURE_CFG") else { return };
    let plugins = PathBuf::from(std::env::var_os("EC_FIXTURE_PLUGINS").expect("EC_FIXTURE_PLUGINS"));
    effectcraft_ofx::set_state_dir(PathBuf::from(cfg));
    let report = load_path(&plugins);
    let p = plugin("org.effectcraft.test.probe").expect("probe");
    let host = Host::new(6000);
    let _ = render(&*p, 5.0, &host);
    let _ = render(&*p, 6.0, &host);
    effectcraft_ofx::shutdown();
    println!("RESULT {}", json!({"loaded": report.loaded.len()}));
}

/// At shutdown an open sequence is ended, every pooled instance is destroyed, and every loaded
/// plug-in is unloaded, in that order.
#[test]
fn shutdown_ends_sequences_destroys_instances_and_unloads_plugins() {
    let Some(f) = Fixture::new("shutdown") else { return };
    let log = f.root.join("plugin.log");
    let v = f.start("instances::child_shutdown", &[("EC_TEST_OFX_LOG", log.to_str().unwrap())]);
    let loaded = v["loaded"].as_u64().unwrap() as usize;
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    let pos = |needle: &str| lines.iter().position(|l| l.starts_with(needle)).unwrap_or_else(|| panic!("no `{needle}` in:\n{text}"));
    // The sequence began at frame 5 and never ended until the app quit.
    assert!(lines.iter().filter(|l| l.starts_with("BeginSequenceRender EC Test Probe #")).count() == 1, "{text}");
    let (end, destroy, first_unload) = (pos("EndSequenceRender EC Test Probe #"), pos("DestroyInstance EC Test Probe #"), pos("Unload"));
    assert!(pos("BeginSequenceRender EC Test Probe #") < end && end < destroy && destroy < first_unload, "{text}");
    // Every plug-in was unloaded, once.
    assert_eq!(lines.iter().filter(|l| l.starts_with("Unload")).count(), loaded, "{text}");
    assert!(lines.contains(&"Unload EC Test Probe") && lines.contains(&"Unload EC Test Invert"), "{text}");
}

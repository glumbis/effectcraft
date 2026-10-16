//! [`OfxEffect`]: one OFX plug-in exposed as an EffectCraft [`EffectPlugin`].
//!
//! Plug-in instances are pooled per applied effect instance ([`PluginHost::instance_key`]), so a
//! plug-in that caches state between frames (temporal ones such as motion blur) never sees two
//! layers mixed up; concurrent renders of one key take different instances. Per render the effect:
//!
//! 1. decodes the flattened EffectCraft parameters into OFX values and applies them to the
//!    instance (sending `InstanceChanged` for the ones that changed),
//! 2. converts the frame to the OFX source image (the buffer minus the padding the host added
//!    for this effect, so the plug-in sees the layer's real region of definition),
//! 3. asks the plug-in for its region of definition and `IsIdentity`,
//! 4. makes sure a sequence is open on the instance (`BeginSequenceRender`, see below), runs
//!    `Render` into a transparent output image covering the whole padded buffer, and
//! 5. converts the output back into the frame.
//!
//! [`padding`](EffectPlugin::padding) runs the same `GetRegionOfDefinition` against a layer of
//! the real size (a nominal one when it is not known) and reports how far the plug-in grows it,
//! so glows and borders are not clipped.
//!
//! ## Sequences
//!
//! The engine renders frames one at a time and does not tell the effect about sequences, so the
//! effect works them out. A sequence stays open on a pooled instance while the frames it renders
//! advance by one (the pool hands a key's frame to the idle instance whose sequence continues at
//! that frame); a frame that is not the next one (a seek, a parallel render that took another
//! instance's turn, another render scale), the instance being dropped from the pool, a render
//! failing, the instance sitting idle for [`IDLE_SEQUENCE`] before its next use, and the
//! application quitting each end it (`EndSequenceRender`). A new sequence announces the frame
//! range from the current frame to the last frame of the source (the long fallback range the clips
//! report when the host does not know the source's duration). A plug-in that declared
//! `kOfxImageEffectInstancePropSequentialRender` (needs or prefers frames in order) is told
//! `kOfxImageEffectPropSequentialRenderStatus` = 1 in the sequence's actions; its declaration is
//! never overwritten. An engine hook that told the effect where a render sequence starts and ends
//! would make the ends exact.
//!
//! ## Instances
//!
//! * A plug-in that allows one instance (`kOfxImageEffectPluginPropSingleInstance`) has one
//!   shared instance for all its applications, used by one render at a time; an effect applied
//!   inside itself cannot have a second one and fails with a message.
//! * A plug-in whose `CreateInstance` took [`HEAVY_CREATE`] or longer twice in a row (it loads a
//!   model or starts a runtime for each instance; one slow first instance is just a library
//!   waking up) has at most [`HEAVY_CAP`] instances alive.
//!   Creating them is one at a time, and a render at the limit takes an idle instance of another
//!   applied effect (when the plug-in does not keep state from frame to frame, see
//!   `kOfxImageEffectInstancePropSequentialRender`), or waits for one to come back, or, when it
//!   would wait for itself (a nested render) or has waited [`WAIT_FOR_INSTANCE`], creates one more.
//! * `DestroyInstance` goes to every pooled instance and `Unload` to every loaded plug-in when the
//!   app quits ([`shutdown_all`]).

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use effectcraft_effects::plugin::{EffectPlugin, PluginFrame, PluginHost, PluginManifest, PluginParams, register_plugin};

use crate::cache::Registration;
use crate::clips::{ClipImage, Comps};
use crate::ffi::*;
use crate::instance::*;
use crate::loader::LazyBinary;
use crate::lock::ReentrantLock;
use crate::pool::PLUGIN_STACK;
use crate::props::PVal;

/// The most idle instances kept per instance key.
const MAX_IDLE: usize = 8;
/// The most instance keys that keep idle instances (the least recently used are dropped).
const MAX_KEYS: usize = 64;
/// The largest buffer edge, in pixels, that padding is measured on (larger layers use the
/// nominal size).
const MAX_MEASURE_EDGE: f64 = 65536.0;
/// The most padding a plug-in may request per side; larger regions are treated as unbounded.
const MAX_PAD: i32 = 4096;
/// A plug-in whose `CreateInstance` takes this long twice in a row (it loads a model, starts a
/// runtime) is limited to [`HEAVY_CAP`] instances.
const HEAVY_CREATE: Duration = Duration::from_millis(250);
/// The most live instances (in use and idle) of a heavy plug-in.
const HEAVY_CAP: usize = 2;
/// How long a render waits for an instance of a plug-in at its limit before it makes one more.
const WAIT_FOR_INSTANCE: Duration = Duration::from_secs(20);
/// A sequence left open on an idle instance this long is ended before the instance is used again.
const IDLE_SEQUENCE: Duration = Duration::from_secs(10);
/// The most time the app waits for plug-ins to shut down (`DestroyInstance`, `Unload`).
const SHUTDOWN_WAIT: Duration = Duration::from_secs(5);

/// The plug-in instances of one effect.
#[derive(Default)]
struct Pool {
    /// Counts check-ins, for least-recently-used eviction.
    tick: u64,
    /// Instance key -> (tick of the last check-in, idle instances).
    idle: HashMap<u64, (u64, Vec<Instance>)>,
    /// Instances alive: idle and checked out.
    live: usize,
    /// The tasks ([`crate::lock::current_task`]) that have an instance checked out, one entry each.
    out: Vec<u64>,
}

impl Pool {
    /// An idle instance of `key`: the one whose open sequence continues at `frame` if there is one,
    /// else the most recently used.
    fn take(&mut self, key: u64, frame: Option<f64>) -> Option<Instance> {
        let (_, list) = self.idle.get_mut(&key)?;
        let at = frame.and_then(|t| list.iter().position(|i| i.continues_at(t))).unwrap_or(list.len().checked_sub(1)?);
        let inst = list.remove(at);
        if list.is_empty() {
            self.idle.remove(&key);
        }
        Some(inst)
    }

    /// The least recently used idle instance of any key but `key`.
    fn steal(&mut self, key: u64) -> Option<Instance> {
        let other = self.idle.iter().filter(|(k, (_, list))| **k != key && !list.is_empty()).min_by_key(|(_, (tick, _))| *tick).map(|(k, _)| *k)?;
        self.take(other, None)
    }
}

/// The geometry padding is measured on: a layer of `layer_size` at the origin with an unpadded
/// buffer, or the nominal layer when the size is unknown (or absurd).
fn measure_geometry(scale: f64, layer_size: [f64; 2]) -> Geometry {
    let [lw, lh] = layer_size;
    let (bw, bh) = (lw * scale, lh * scale);
    if lw.is_finite() && lh.is_finite() && bw >= 1.0 && bh >= 1.0 && bw <= MAX_MEASURE_EDGE && bh <= MAX_MEASURE_EDGE {
        Geometry { scale, layer: layer_size, origin: [0.0; 2], w: bw.round() as i32, h: bh.round() as i32, pad: 0 }
    } else {
        Geometry::nominal(scale)
    }
}

/// Every effect registered by this crate in this process.
fn effects() -> &'static Mutex<Vec<std::sync::Weak<OfxEffect>>> {
    static E: OnceLock<Mutex<Vec<std::sync::Weak<OfxEffect>>>> = OnceLock::new();
    E.get_or_init(|| Mutex::new(Vec::new()))
}

fn all_effects() -> Vec<Arc<OfxEffect>> {
    effects().lock().unwrap_or_else(PoisonError::into_inner).iter().filter_map(std::sync::Weak::upgrade).collect()
}

/// The OFX identifiers of the plug-ins that have rendered in this process.
pub(crate) fn rendered_ids() -> Vec<String> {
    all_effects().iter().filter(|e| e.has_rendered()).map(|e| e.reg.ofx_id.clone()).collect()
}

/// The app is quitting: every pooled instance gets `DestroyInstance` (and `EndSequenceRender` if a
/// sequence is open on it), then every loaded plug-in gets `Unload`. Each call is guarded like any
/// other action, and the whole is given [`SHUTDOWN_WAIT`]: a plug-in that hangs while it shuts down
/// is left to the operating system.
pub(crate) fn shutdown_all() {
    let all = all_effects();
    if all.is_empty() {
        return;
    }
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let work = move || {
        for e in &all {
            e.shutdown_instances();
        }
        for e in &all {
            e.unload();
        }
        crate::clips::release_spare_buffers();
        let _ = tx.send(());
    };
    // Plug-in code wants a big stack here too.
    let started = std::thread::Builder::new().name("ofx-shutdown".into()).stack_size(PLUGIN_STACK).spawn(work);
    match started {
        Ok(_) => {
            if rx.recv_timeout(SHUTDOWN_WAIT).is_err() {
                log::warn!("openfx: the plug-ins did not shut down within {} s; leaving them to the operating system", SHUTDOWN_WAIT.as_secs());
            }
        }
        Err(e) => log::warn!("openfx: cannot start the shutdown thread ({e}); the plug-ins are not told to shut down"),
    }
}

/// A loaded plug-in: its entry point and the described effect that instances copy.
pub(crate) struct Live {
    pub entry: Arc<PluginEntry>,
    /// The described effect; instances copy its parameters and clips.
    pub desc: Box<EffectObj>,
}

/// An OFX plug-in as an EffectCraft effect.
pub struct OfxEffect {
    /// What the scan found out about the plug-in (or the scan cache remembered): the manifest, the
    /// clips and settings, the parameter bindings.
    pub(crate) reg: Registration,
    pub(crate) source: String,
    /// The loaded plug-in. An effect described in this process has it from the start; an effect
    /// registered from the scan cache loads its library and runs the load sequence the first
    /// time an instance is needed (`lazy`), and keeps the failure if that did not work.
    live: OnceLock<Result<Live, String>>,
    lazy: Option<Arc<LazyBinary>>,
    pool: Mutex<Pool>,
    /// Signalled when an instance goes back to the pool.
    returned: Condvar,
    /// Instances are made one at a time (a plug-in that loads a model per instance is not asked
    /// for four at once).
    create: Mutex<()>,
    /// `CreateInstance` took [`HEAVY_CREATE`] or longer twice in a row.
    heavy: AtomicBool,
    /// How many of the latest creations in a row were that slow.
    slow_creates: AtomicU32,
    /// Held by the one render that uses the single instance of a plug-in that allows only one.
    single: ReentrantLock,
    /// The app is quitting: no more instances.
    closed: AtomicBool,
    /// Instance key -> (hash of the measured inputs, padding).
    pad_cache: Mutex<HashMap<u64, (u64, i32)>>,
    /// The frame rate of the last render (padding queries have no host).
    fps_bits: AtomicU64,
}

impl OfxEffect {
    fn build(reg: Registration, source: String, live: OnceLock<Result<Live, String>>, lazy: Option<Arc<LazyBinary>>) -> OfxEffect {
        OfxEffect {
            reg,
            source,
            live,
            lazy,
            pool: Mutex::new(Pool::default()),
            returned: Condvar::new(),
            create: Mutex::new(()),
            heavy: AtomicBool::new(false),
            slow_creates: AtomicU32::new(0),
            single: ReentrantLock::new(),
            closed: AtomicBool::new(false),
            pad_cache: Mutex::new(HashMap::new()),
            fps_bits: AtomicU64::new(30.0f64.to_bits()),
        }
    }

    /// An effect whose plug-in is loaded and described already.
    pub(crate) fn eager(reg: Registration, live: Live, source: String) -> OfxEffect {
        let cell = OnceLock::new();
        let _ = cell.set(Ok(live));
        OfxEffect::build(reg, source, cell, None)
    }

    /// An effect registered from the scan cache: the plug-in is loaded when it is first used.
    pub(crate) fn lazy(reg: Registration, binary: Arc<LazyBinary>, source: String) -> OfxEffect {
        OfxEffect::build(reg, source, OnceLock::new(), Some(binary))
    }

    /// Register the effect with the effect registry (and remember it, for [`rendered_ids`] and
    /// [`shutdown_all`]).
    pub(crate) fn register(effect: OfxEffect) -> Result<(), String> {
        let effect = Arc::new(effect);
        register_plugin(effect.clone())?;
        let mut all = effects().lock().unwrap_or_else(PoisonError::into_inner);
        all.retain(|w| w.strong_count() > 0);
        all.push(Arc::downgrade(&effect));
        Ok(())
    }

    /// Whether a render of this effect's plug-in has completed in this process.
    fn has_rendered(&self) -> bool {
        matches!(self.live.get(), Some(Ok(live)) if live.entry.rendered.load(Ordering::Relaxed))
    }

    /// The loaded plug-in, loading it first if it came from the scan cache and has not been used
    /// yet. A plug-in that cannot be loaded stays unavailable for the rest of the process: the
    /// error says what happened, and the next start scans its binary again.
    fn live(&self) -> Result<&Live, String> {
        let r = self.live.get_or_init(|| match &self.lazy {
            Some(binary) => binary.load(&self.reg).map_err(|e| format!("the OpenFX plug-in {} could not be loaded: {e}", self.reg.ofx_id)),
            None => Err(format!("the OpenFX plug-in {} is not loaded", self.reg.ofx_id)),
        });
        r.as_ref().map_err(Clone::clone)
    }

    fn fps(&self) -> f64 {
        f64::from_bits(self.fps_bits.load(Ordering::Relaxed))
    }

    /// The most live instances (in use and idle) this plug-in gets: one for a plug-in that allows
    /// one, [`HEAVY_CAP`] for one that is slow to create, no limit (but the idle limits) otherwise.
    fn cap(&self) -> usize {
        if self.reg.cfg.single_instance {
            1
        } else if self.heavy.load(Ordering::Relaxed) {
            HEAVY_CAP
        } else {
            usize::MAX
        }
    }

    /// Whether an instance may move to another applied effect's frames: the plug-in did not declare
    /// that it keeps state from frame to frame.
    fn shares_across_keys(&self) -> bool {
        self.reg.cfg.sequential_render == 0
    }

    /// An idle instance last used for `key` (the one whose sequence continues at `frame` if there
    /// is one), or a new one. A new instance gets the parameter `values` before the plug-in's
    /// `CreateInstance` (and the clip preferences after it) run: plug-ins read their parameters there
    /// and cache what they find (a model, a mode), so creating them with the defaults and changing
    /// the values afterwards left them stale. See the module docs for the limits.
    fn checkout(&self, live: &Live, key: u64, values: &HashMap<String, Vec<PVal>>, frame: Option<f64>) -> Result<Instance, String> {
        let task = crate::lock::current_task().unwrap_or(0);
        let deadline = Instant::now() + WAIT_FOR_INSTANCE;
        let mut inst = loop {
            if self.closed.load(Ordering::Acquire) {
                return Err("OpenFX plug-ins are shutting down".into());
            }
            let mut pool = self.pool.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(i) = pool.take(key, frame) {
                pool.out.push(task);
                break i;
            }
            if pool.live < self.cap() {
                // Room for another instance. One is made at a time: when it was slow, the plug-in is
                // heavy and the next render sees the limit instead of making a second slow one too.
                drop(pool);
                let _one_at_a_time = self.create.lock().unwrap_or_else(PoisonError::into_inner);
                let mut pool = self.pool.lock().unwrap_or_else(PoisonError::into_inner);
                if let Some(i) = pool.take(key, frame) {
                    pool.out.push(task);
                    break i;
                }
                if pool.live >= self.cap() {
                    continue;
                }
                pool.live += 1;
                pool.out.push(task);
                drop(pool);
                let t0 = Instant::now();
                match self.create_instance(live, values) {
                    Ok(i) => {
                        if t0.elapsed() < HEAVY_CREATE {
                            self.slow_creates.store(0, Ordering::Relaxed);
                        } else if self.slow_creates.fetch_add(1, Ordering::Relaxed) >= 1 && !self.heavy.swap(true, Ordering::Relaxed) {
                            log::info!(
                                "openfx: {} is slow to create instances ({} ms): at most {HEAVY_CAP} live instances of it",
                                self.reg.ofx_id,
                                t0.elapsed().as_millis()
                            );
                        }
                        break i;
                    }
                    Err(e) => {
                        self.forget_instance(task);
                        return Err(e);
                    }
                }
            }
            // At the limit. An idle instance of another applied effect will do for a plug-in that
            // keeps no state between frames.
            if self.shares_across_keys()
                && let Some(i) = pool.steal(key)
            {
                pool.out.push(task);
                break i;
            }
            // Everything is in use. A render that holds an instance itself (the plug-in's render
            // asked the host for an image that is made by another application of the same plug-in)
            // would wait for itself, and one that waited long enough gives up: more instances than
            // the limit, then, unless the plug-in allows only one.
            let nested = task != 0 && pool.out.contains(&task);
            let left = deadline.saturating_duration_since(Instant::now());
            if nested || left.is_zero() {
                if self.reg.cfg.single_instance {
                    return Err(format!("the plug-in {} allows one instance and cannot be applied inside itself", self.reg.ofx_id));
                }
                log::warn!(
                    "openfx: {} has {} live instances (the limit is {HEAVY_CAP}) and all are in use: making one more{}",
                    self.reg.ofx_id,
                    pool.live,
                    if nested { " (a nested render)" } else { "" }
                );
                pool.live += 1;
                pool.out.push(task);
                drop(pool);
                match self.create_instance(live, values) {
                    Ok(i) => break i,
                    Err(e) => {
                        self.forget_instance(task);
                        return Err(e);
                    }
                }
            }
            let _ = self.returned.wait_timeout(pool, left);
        };
        inst.holder = task;
        // A sequence nobody continued for a while is over.
        if inst.seq.is_some() && inst.last_used.elapsed() > IDLE_SEQUENCE {
            inst.end_sequence_now();
        }
        Ok(inst)
    }

    /// Make an instance: parameter values, `CreateInstance`, clip preferences.
    fn create_instance(&self, live: &Live, values: &HashMap<String, Vec<PVal>>) -> Result<Instance, String> {
        let mut inst = Instance::build(live.entry.clone(), self.reg.cfg.clone(), &live.desc);
        inst.apply_values(values);
        inst.create()?;
        Ok(inst)
    }

    /// An instance that was counted could not be made: give its place back.
    fn forget_instance(&self, task: u64) {
        let mut pool = self.pool.lock().unwrap_or_else(PoisonError::into_inner);
        pool.live = pool.live.saturating_sub(1);
        if let Some(p) = pool.out.iter().position(|t| *t == task) {
            pool.out.swap_remove(p);
        }
        drop(pool);
        self.returned.notify_one();
    }

    /// Return an instance to `key`'s idle list. Instances over the live, per-key or key limit are
    /// dropped (EndSequenceRender, DestroyInstance) after the pool lock is released.
    fn checkin(&self, key: u64, mut inst: Instance) {
        inst.last_used = Instant::now();
        let mut evicted: Vec<Instance> = Vec::new();
        {
            let mut guard = self.pool.lock().unwrap_or_else(PoisonError::into_inner);
            let pool = &mut *guard;
            if let Some(p) = pool.out.iter().position(|t| *t == inst.holder) {
                pool.out.swap_remove(p);
            }
            inst.holder = 0;
            if self.closed.load(Ordering::Acquire) || pool.live > self.cap() {
                // Made beyond the limit (a nested render, a long wait): not kept.
                evicted.push(inst);
            } else {
                pool.tick = pool.tick.wrapping_add(1);
                let tick = pool.tick;
                let max_idle = if self.reg.cfg.single_instance { 1 } else { MAX_IDLE };
                let slot = pool.idle.entry(key).or_insert_with(|| (tick, Vec::new()));
                slot.0 = tick;
                if slot.1.len() < max_idle {
                    slot.1.push(inst);
                } else {
                    evicted.push(inst);
                }
                while pool.idle.len() > MAX_KEYS {
                    let oldest = pool.idle.iter().filter(|(k, _)| **k != key).min_by_key(|(_, (t, _))| *t).map(|(k, _)| *k);
                    let Some(oldest) = oldest else { break };
                    if let Some((_, list)) = pool.idle.remove(&oldest) {
                        evicted.extend(list);
                    }
                }
            }
            pool.live = pool.live.saturating_sub(evicted.len());
        }
        self.returned.notify_all();
        drop(evicted);
    }

    /// Serialise plug-ins that declare themselves render-thread-unsafe: the library's render lock.
    /// It is reentrant: a render that asks the host for an image can make the host render another
    /// effect of the same library ([`crate::lock`]).
    fn serialise<'a>(&self, live: &'a Live) -> Option<crate::lock::Guard<'a>> {
        (self.reg.cfg.safety == Safety::Unsafe).then(|| live.entry.lock.lock())
    }

    /// The single instance of a plug-in that allows one: held while it is used. `Err` when this
    /// task has it already (the effect is applied inside itself).
    fn single_gate(&self) -> Result<Option<crate::lock::Guard<'_>>, String> {
        if !self.reg.cfg.single_instance {
            return Ok(None);
        }
        let g = self.single.lock();
        if g.nested() {
            return Err(format!("the plug-in {} allows one instance and cannot be applied inside itself", self.reg.ofx_id));
        }
        Ok(Some(g))
    }

    fn input_comps(&self, prefs: &Prefs) -> Comps {
        self.reg.cfg.clips.iter().find(|c| c.role == ClipRole::Input).map(|c| prefs.comps.get(&c.name).copied().unwrap_or(c.comps)).unwrap_or(Comps::Rgba)
    }

    /// The padding the plug-in's region of definition needs around a layer of `layer_size`
    /// (layer pixels; 0 when unknown) for the applied effect instance `key`.
    fn compute_pad(&self, params: &PluginParams, time: f64, scale: f64, layer_size: [f64; 2], key: u64) -> i32 {
        let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
        let mut h = DefaultHasher::new();
        params.flat.iter().for_each(|v| v.to_bits().hash(&mut h));
        params.texts.hash(&mut h);
        time.to_bits().hash(&mut h);
        scale.to_bits().hash(&mut h);
        layer_size.iter().for_each(|v| v.to_bits().hash(&mut h));
        let hash = h.finish();
        let cached = self.pad_cache.lock().unwrap_or_else(PoisonError::into_inner).get(&key).copied();
        if let Some((k, p)) = cached
            && k == hash
        {
            return p;
        }
        let pad = self.measure_pad(params, time, scale, layer_size, key).unwrap_or(0);
        let mut cache = self.pad_cache.lock().unwrap_or_else(PoisonError::into_inner);
        if cache.len() >= MAX_KEYS && !cache.contains_key(&key) {
            cache.clear();
        }
        cache.insert(key, (hash, pad));
        pad
    }

    fn measure_pad(&self, params: &PluginParams, time: f64, scale: f64, layer_size: [f64; 2], key: u64) -> Option<i32> {
        let live = self.live().ok()?;
        let _task = crate::lock::enter_task();
        let _serial = self.serialise(live);
        let _single = self.single_gate().ok()?;
        let key = if self.reg.cfg.single_instance { 0 } else { key };
        let geom = measure_geometry(scale, layer_size);
        let values = self.reg.bindings.decode(params.flat, params.texts, &geom.coord());
        let fps = self.fps();
        let mut inst = self.checkout(live, key, &values, Some(time * fps)).ok()?;
        inst.apply_values(&values);
        let ctx = Arc::new(RenderCtx::new(
            self.reg.cfg.clone(),
            geom,
            fps,
            time,
            None,
            self.reg.bindings.clone(),
            self.reg.bindings.layer_ids(params.flat),
            inst.prefs.depth,
            inst.prefs.comps.clone(),
            None,
        ));
        inst.begin(&ctx);
        let default = ctx.input_rod();
        let rod = inst.region_of_definition(time * fps, scale, default);
        inst.end();
        self.checkin(key, inst);
        let rod = geom.from_canon(rod?);
        let b = geom.bounds();
        let grow = (b.x1 - rod.x1).max(rod.x2 - b.x2).max(b.y1 - rod.y1).max(rod.y2 - b.y2).max(0);
        Some(if grow > MAX_PAD { 0 } else { grow })
    }

    fn render_impl(&self, frame: &mut PluginFrame, params: &PluginParams, time: f64, host: Option<&dyn PluginHost>) -> Result<(), String> {
        let fps = host.map(|h| h.frame_rate()).filter(|f| f.is_finite() && *f > 0.0).unwrap_or(30.0).clamp(1.0, 1000.0);
        self.fps_bits.store(fps.to_bits(), Ordering::Relaxed);
        let (w, h) = (frame.width as i32, frame.height as i32);
        if w <= 0 || h <= 0 || frame.pixels.len() != (w as usize) * (h as usize) {
            return Err("the frame buffer does not match its size".into());
        }
        let scale = if frame.scale.is_finite() && frame.scale > 0.0 { frame.scale } else { 1.0 };
        let layer = if frame.layer_size[0] > 0.0 && frame.layer_size[1] > 0.0 { frame.layer_size } else { [w as f64 / scale, h as f64 / scale] };
        let applied_key = host.map_or(0, |h| h.instance_key());
        let pad = if host.is_some() { self.compute_pad(params, time, scale, frame.layer_size, applied_key).min(w / 2).min(h / 2) } else { 0 };
        let geom = Geometry { scale, layer, origin: frame.origin, w, h, pad };

        let live = self.live()?;
        let _task = crate::lock::enter_task();
        let _serial = self.serialise(live);
        let _single = self.single_gate()?;
        // One shared instance for a plug-in that allows one.
        let key = if self.reg.cfg.single_instance { 0 } else { applied_key };
        let values = self.reg.bindings.decode(params.flat, params.texts, &geom.coord());
        let mut inst = self.checkout(live, key, &values, Some(time * fps))?;
        let result = self.run(&mut inst, frame, params, &values, time, fps, geom, host);
        inst.end();
        let msg = inst.obj.message();
        self.checkin(key, inst);
        // (The plug-in's messages are already part of a failure's text; the effect's last error
        // message is only added when it is something else.)
        result.map_err(|e| if msg.is_empty() || e.contains(&msg) { e } else { format!("{e}; last message: \"{msg}\"") })
    }

    #[allow(clippy::too_many_arguments)]
    fn run(
        &self,
        inst: &mut Instance,
        frame: &mut PluginFrame,
        params: &PluginParams,
        values: &HashMap<String, Vec<PVal>>,
        time: f64,
        fps: f64,
        geom: Geometry,
        host: Option<&dyn PluginHost>,
    ) -> Result<(), String> {
        let (w, h, pad) = (geom.w as usize, geom.h as usize, geom.pad as usize);
        let t = time * fps;
        let changed = inst.apply_values(values);

        // The plug-in's own layer at the current time: the buffer minus our padding.
        let src_rect = geom.src_rect();
        let in_comps = self.input_comps(&inst.prefs);
        let depth = inst.prefs.depth;
        let has_input = self.reg.cfg.clips.iter().any(|c| c.role == ClipRole::Input);
        let src_now = if has_input {
            let img = ClipImage::from_rows(frame.pixels, w, (pad, pad, w.saturating_sub(pad), h.saturating_sub(pad)), src_rect, src_rect, depth, in_comps);
            Some(Arc::new(img.ok_or("there is not enough memory to convert the layer for the plug-in")?))
        } else {
            None
        };
        let ctx = Arc::new(RenderCtx::new(
            self.reg.cfg.clone(),
            geom,
            fps,
            time,
            host.map(HostPtr::new),
            self.reg.bindings.clone(),
            self.reg.bindings.layer_ids(params.flat),
            depth,
            inst.prefs.comps.clone(),
            src_now,
        ));
        inst.begin(&ctx);

        if inst.rendered {
            for name in &changed {
                inst.instance_changed(name, t);
            }
        }

        // Region of definition: where the output has pixels.
        let default_rod = ctx.input_rod();
        let rod = inst.region_of_definition(t, geom.scale, default_rod).unwrap_or(default_rod);
        *ctx.rod.lock().unwrap_or_else(PoisonError::into_inner) = rod;
        let bounds = geom.bounds();
        let rod_px = geom.from_canon(rod);
        let window = intersect(rod_px, bounds);
        if is_empty(window) {
            frame.pixels.fill([0.0; 4]);
            inst.rendered = true;
            return Ok(());
        }
        // Informational: every region is served from the full buffer.
        let _ = inst.regions_of_interest(t, geom.scale, geom.to_canon(window));

        // Identity: the plug-in may ask for a clip to be passed through.
        if let Some((clip, at)) = inst.is_identity(t, window, geom.scale)
            && let Some(cfg) = self.reg.cfg.clip(&clip)
        {
            let same = (at - t).abs() < 1e-6;
            match (&cfg.role, same) {
                (ClipRole::Input, true) => {
                    inst.rendered = true;
                    return Ok(());
                }
                (ClipRole::Input | ClipRole::Layer(_), _) => {
                    if let Some(img) = ctx.image(&clip, at)
                        && img.bounds == bounds
                    {
                        img.write_rows(frame.pixels, false);
                        inst.rendered = true;
                        return Ok(());
                    }
                }
                _ => {}
            }
        }

        let out_comps = self
            .reg
            .cfg
            .clips
            .iter()
            .find(|c| c.role == ClipRole::Output)
            .map(|c| inst.prefs.comps.get(&c.name).copied().unwrap_or(c.comps))
            .unwrap_or(Comps::Rgba);
        let out = Arc::new(ClipImage::blank(bounds, rod_px, depth, out_comps).ok_or("there is not enough memory for the plug-in's output image")?);
        *ctx.output.lock().unwrap_or_else(PoisonError::into_inner) = Some(out.clone());

        let entry = inst.entry.clone();
        let mut mark = entry.diag.mark();
        // The sequence the frame belongs to: still open when this is the frame after the last one.
        inst.ensure_sequence(t, geom.scale, ctx.frame_range().unwrap_or((0.0, UNKNOWN_LAST_FRAME)));
        let mut status = inst.render(t, window, geom.scale);
        if status == K_STAT_ERR_MEMORY {
            // Out of memory: the plug-in is asked to let go of what it caches, and tries once more.
            log::warn!("openfx: {} ran out of memory rendering; asking it to purge its caches and trying once more", self.reg.ofx_id);
            inst.purge_caches();
            crate::clips::release_spare_buffers();
            mark = entry.diag.mark();
            status = inst.render(t, window, geom.scale);
        }
        inst.rendered = true;
        match status {
            K_STAT_OK | K_STAT_REPLY_DEFAULT => inst.note_rendered(t),
            // A failed render leaves the sequence in an unknown state: the next frame starts a new one.
            K_STAT_UNLICENSED => {
                inst.end_sequence_now();
                return Err(format!("the plug-in reports it is unlicensed ({})", entry.failure("render", status, mark)));
            }
            s => {
                inst.end_sequence_now();
                return Err(format!("the plug-in's {}", entry.failure("render", s, mark)));
            }
        }
        out.write_rows(frame.pixels, inst.prefs.out_unpremult);
        Ok(())
    }

    /// The app is quitting: destroy the idle instances (a sequence open on one ends first). Instances
    /// a render is using are left to it.
    fn shutdown_instances(&self) {
        self.closed.store(true, Ordering::Release);
        let drained: Vec<Instance> = {
            let mut pool = self.pool.lock().unwrap_or_else(PoisonError::into_inner);
            let all: Vec<Instance> = pool.idle.drain().flat_map(|(_, (_, list))| list).collect();
            pool.live = pool.live.saturating_sub(all.len());
            all
        };
        self.returned.notify_all();
        drop(drained);
    }

    /// `kOfxActionUnload` for a plug-in that was loaded.
    fn unload(&self) {
        if let Some(Ok(live)) = self.live.get() {
            live.entry.call(ACT_UNLOAD, std::ptr::null(), None, None);
        }
    }
}

impl EffectPlugin for OfxEffect {
    fn manifest(&self) -> &PluginManifest {
        &self.reg.manifest
    }

    fn render(&self, frame: &mut PluginFrame, params: &PluginParams, time: f64) -> Result<(), String> {
        self.render_impl(frame, params, time, None)
    }

    fn source(&self) -> String {
        self.source.clone()
    }

    fn padding(&self, params: &PluginParams, time: f64, scale: f64, layer_size: [f64; 2], instance_key: u64) -> u32 {
        self.compute_pad(params, time, scale, layer_size, instance_key).max(0) as u32
    }

    fn render_v2(&self, frame: &mut PluginFrame, params: &PluginParams, time: f64, host: &dyn PluginHost) -> Result<(), String> {
        self.render_impl(frame, params, time, Some(host))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clips::Depth;
    use crate::diag::PluginDiag;

    unsafe extern "C" fn nop(_action: *const std::ffi::c_char, _handle: *const std::ffi::c_void, _in: Handle, _out: Handle) -> OfxStatus {
        K_STAT_REPLY_DEFAULT
    }

    /// An instance that was never created (so dropping it sends nothing), with a sequence whose last
    /// rendered frame is `last`.
    fn instance(last: Option<f64>) -> Instance {
        let desc = EffectObj::new_descriptor();
        let entry = Arc::new(PluginEntry::new(nop, "test".into(), Arc::new(ReentrantLock::new()), PluginDiag::new(), 0));
        let cfg = Arc::new(EffectCfg {
            context: CTX_FILTER.into(),
            clips: vec![],
            depth: Depth::Float,
            safety: Safety::InstanceSafe,
            slave_params: vec![],
            single_instance: false,
            sequential_render: 0,
        });
        let mut i = Instance::build(entry, cfg, &desc);
        i.seq = last.map(|last| Seq { range: (0.0, 100.0), scale: 1.0, sequential: true, last, renders: 1 });
        i
    }

    fn last_of(i: &Instance) -> Option<f64> {
        i.seq.map(|s| s.last)
    }

    /// A frame goes to the idle instance whose sequence continues at it; any other frame to the most
    /// recently returned one.
    #[test]
    fn the_pool_hands_a_frame_to_the_instance_whose_sequence_continues() {
        let mut pool = Pool::default();
        pool.idle.insert(7, (1, vec![instance(Some(3.0)), instance(Some(10.0)), instance(None)]));
        pool.live = 3;
        assert_eq!(pool.take(7, Some(11.0)).as_ref().and_then(last_of), Some(10.0));
        // Nothing continues at 50: the most recently returned one (the last in the list).
        assert_eq!(pool.take(7, Some(50.0)).as_ref().map(last_of), Some(None));
        assert_eq!(pool.take(7, None).as_ref().and_then(last_of), Some(3.0));
        // An empty key is gone, and there is nothing to take.
        assert!(pool.take(7, None).is_none() && !pool.idle.contains_key(&7));
        assert!(pool.take(8, Some(1.0)).is_none());
    }

    /// At its limit a plug-in takes the instance idle longest of another applied effect, never one of
    /// its own key's (those are taken first) and never when nothing is idle.
    #[test]
    fn the_pool_lends_the_instance_idle_longest_to_another_key() {
        let mut pool = Pool::default();
        pool.idle.insert(1, (5, vec![instance(Some(1.0))]));
        pool.idle.insert(2, (3, vec![instance(Some(2.0))]));
        pool.idle.insert(3, (9, vec![instance(Some(3.0))]));
        assert_eq!(pool.steal(3).as_ref().and_then(last_of), Some(2.0), "key 2 was returned least recently");
        assert_eq!(pool.steal(2).as_ref().and_then(last_of), Some(1.0));
        // Only the asker's own key is left: nothing to lend.
        assert!(pool.steal(3).is_none());
    }
}

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
//! 4. runs `BeginSequenceRender` / `Render` / `EndSequenceRender` into a transparent output
//!    image covering the whole padded buffer, and
//! 5. converts the output back into the frame.
//!
//! [`padding`](EffectPlugin::padding) runs the same `GetRegionOfDefinition` against a layer of
//! the real size (a nominal one when it is not known) and reports how far the plug-in grows it,
//! so glows and borders are not clipped.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use effectcraft_effects::plugin::{EffectPlugin, PluginFrame, PluginHost, PluginManifest, PluginParams, register_plugin};

use crate::cache::Registration;
use crate::clips::{ClipImage, Comps};
use crate::ffi::*;
use crate::instance::*;
use crate::loader::LazyBinary;
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

/// Idle plug-in instances, grouped by the applied effect instance that last used them.
#[derive(Default)]
struct Pool {
    /// Counts check-ins, for least-recently-used eviction.
    tick: u64,
    /// Instance key -> (tick of the last check-in, idle instances).
    idle: HashMap<u64, (u64, Vec<Instance>)>,
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

/// The OFX identifiers of the plug-ins that have rendered in this process.
pub(crate) fn rendered_ids() -> Vec<String> {
    let all = effects().lock().unwrap_or_else(PoisonError::into_inner);
    all.iter().filter_map(std::sync::Weak::upgrade).filter(|e| e.has_rendered()).map(|e| e.reg.ofx_id.clone()).collect()
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

    /// Register the effect with the effect registry (and remember it, for [`rendered_ids`]).
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

    /// An idle instance last used for `key`, or a new one. A new instance gets the parameter
    /// `values` before the plug-in's `CreateInstance` (and the clip preferences after it) run:
    /// plug-ins read their parameters there and cache what they find (a model, a mode), so
    /// creating them with the defaults and changing the values afterwards left them stale.
    fn checkout(&self, live: &Live, key: u64, values: &HashMap<String, Vec<PVal>>) -> Result<Instance, String> {
        let pooled = {
            let mut pool = self.pool.lock().unwrap_or_else(PoisonError::into_inner);
            let popped = pool.idle.get_mut(&key).and_then(|(_, list)| list.pop());
            if pool.idle.get(&key).is_some_and(|(_, list)| list.is_empty()) {
                pool.idle.remove(&key);
            }
            popped
        };
        if let Some(i) = pooled {
            return Ok(i);
        }
        let mut inst = Instance::build(live.entry.clone(), self.reg.cfg.clone(), &live.desc);
        inst.apply_values(values);
        inst.create()?;
        Ok(inst)
    }

    /// Return an instance to `key`'s idle list. Instances over the per-key or the key limit are
    /// dropped (DestroyInstance) after the pool lock is released.
    fn checkin(&self, key: u64, inst: Instance) {
        let mut evicted: Vec<Instance> = Vec::new();
        {
            let mut guard = self.pool.lock().unwrap_or_else(PoisonError::into_inner);
            let pool = &mut *guard;
            pool.tick = pool.tick.wrapping_add(1);
            let tick = pool.tick;
            let slot = pool.idle.entry(key).or_insert_with(|| (tick, Vec::new()));
            slot.0 = tick;
            if slot.1.len() < MAX_IDLE {
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
        drop(evicted);
    }

    /// Serialise plug-ins that declare themselves render-thread-unsafe.
    /// The library's render lock, for a plug-in that declares itself render-thread-unsafe. It is
    /// reentrant: a render that asks the host for an image can make the host render another
    /// effect of the same library ([`crate::lock`]).
    fn serialise<'a>(&self, live: &'a Live) -> Option<crate::lock::Guard<'a>> {
        (self.reg.cfg.safety == Safety::Unsafe).then(|| live.entry.lock.lock())
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
        let _serial = self.serialise(live);
        let geom = measure_geometry(scale, layer_size);
        let values = self.reg.bindings.decode(params.flat, params.texts, &geom.coord());
        let mut inst = self.checkout(live, key, &values).ok()?;
        inst.apply_values(&values);
        let fps = self.fps();
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
        let key = host.map_or(0, |h| h.instance_key());
        let pad = if host.is_some() { self.compute_pad(params, time, scale, frame.layer_size, key).min(w / 2).min(h / 2) } else { 0 };
        let geom = Geometry { scale, layer, origin: frame.origin, w, h, pad };

        let live = self.live()?;
        let _serial = self.serialise(live);
        let values = self.reg.bindings.decode(params.flat, params.texts, &geom.coord());
        let mut inst = self.checkout(live, key, &values)?;
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
        let src_now = has_input.then(|| {
            Arc::new(ClipImage::from_rows(frame.pixels, w, (pad, pad, w.saturating_sub(pad), h.saturating_sub(pad)), src_rect, src_rect, depth, in_comps))
        });
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
        let out = Arc::new(ClipImage::blank(bounds, rod_px, depth, out_comps));
        *ctx.output.lock().unwrap_or_else(PoisonError::into_inner) = Some(out.clone());

        let entry = inst.entry.clone();
        let mark = entry.diag.mark();
        inst.begin_sequence(t, geom.scale);
        let status = inst.render(t, window, geom.scale);
        inst.end_sequence(t, geom.scale);
        inst.rendered = true;
        match status {
            K_STAT_OK | K_STAT_REPLY_DEFAULT => {}
            K_STAT_UNLICENSED => return Err(format!("the plug-in reports it is unlicensed ({})", entry.failure("render", status, mark))),
            s => return Err(format!("the plug-in's {}", entry.failure("render", s, mark))),
        }
        out.write_rows(frame.pixels, inst.prefs.out_unpremult);
        Ok(())
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

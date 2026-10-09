//! [`OfxEffect`]: one OFX plug-in exposed as an EffectCraft [`EffectPlugin`].
//!
//! A pool of plug-in instances serves concurrent renders. Per render the effect:
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
//! [`padding`](EffectPlugin::padding) runs the same `GetRegionOfDefinition` against a nominal
//! layer (the real size is not known before the frame exists) and reports how far the
//! plug-in grows it, so glows and borders are not clipped.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use effectcraft_effects::plugin::{EffectPlugin, PluginFrame, PluginHost, PluginManifest, PluginParams};

use crate::clips::{ClipImage, Comps};
use crate::ffi::*;
use crate::instance::*;
use crate::params::Bindings;

/// The most idle instances kept per effect.
const MAX_IDLE: usize = 8;
/// The most padding a plug-in may request per side; larger regions are treated as unbounded.
const MAX_PAD: i32 = 4096;

/// An OFX plug-in as an EffectCraft effect.
pub struct OfxEffect {
    pub(crate) manifest: PluginManifest,
    pub(crate) entry: Arc<PluginEntry>,
    pub(crate) cfg: Arc<EffectCfg>,
    /// The described effect; instances copy its parameters and clips.
    pub(crate) desc: Box<EffectObj>,
    pub(crate) bindings: Arc<Bindings>,
    pub(crate) source: String,
    pool: Mutex<Vec<Instance>>,
    pad_cache: Mutex<Option<(u64, i32)>>,
    /// The frame rate of the last render (padding queries have no host).
    fps_bits: AtomicU64,
}

impl OfxEffect {
    pub fn new(manifest: PluginManifest, entry: Arc<PluginEntry>, cfg: Arc<EffectCfg>, desc: Box<EffectObj>, bindings: Bindings, source: String) -> OfxEffect {
        OfxEffect {
            manifest,
            entry,
            cfg,
            desc,
            bindings: Arc::new(bindings),
            source,
            pool: Mutex::new(Vec::new()),
            pad_cache: Mutex::new(None),
            fps_bits: AtomicU64::new(30.0f64.to_bits()),
        }
    }

    fn fps(&self) -> f64 {
        f64::from_bits(self.fps_bits.load(Ordering::Relaxed))
    }

    fn checkout(&self) -> Result<Instance, String> {
        if let Some(i) = self.pool.lock().unwrap_or_else(PoisonError::into_inner).pop() {
            return Ok(i);
        }
        let mut inst = Instance::build(self.entry.clone(), self.cfg.clone(), &self.desc);
        inst.create()?;
        Ok(inst)
    }

    fn checkin(&self, inst: Instance) {
        let mut pool = self.pool.lock().unwrap_or_else(PoisonError::into_inner);
        if pool.len() < MAX_IDLE {
            pool.push(inst);
        }
    }

    /// Serialise plug-ins that declare themselves render-thread-unsafe.
    fn serialise(&self) -> Option<std::sync::MutexGuard<'_, ()>> {
        (self.cfg.safety == Safety::Unsafe).then(|| self.entry.lock.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn input_comps(&self, prefs: &Prefs) -> Comps {
        self.cfg
            .clips
            .iter()
            .find(|c| c.role == ClipRole::Input)
            .map(|c| prefs.comps.get(&c.name).copied().unwrap_or(c.comps))
            .unwrap_or(Comps::Rgba)
    }

    /// The padding the plug-in's region of definition needs around a layer.
    fn compute_pad(&self, params: &PluginParams, time: f64, scale: f64) -> i32 {
        let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
        let mut h = DefaultHasher::new();
        params.flat.iter().for_each(|v| v.to_bits().hash(&mut h));
        params.texts.hash(&mut h);
        time.to_bits().hash(&mut h);
        scale.to_bits().hash(&mut h);
        let key = h.finish();
        if let Some((k, p)) = *self.pad_cache.lock().unwrap_or_else(PoisonError::into_inner)
            && k == key
        {
            return p;
        }
        let pad = self.measure_pad(params, time, scale).unwrap_or(0);
        *self.pad_cache.lock().unwrap_or_else(PoisonError::into_inner) = Some((key, pad));
        pad
    }

    fn measure_pad(&self, params: &PluginParams, time: f64, scale: f64) -> Option<i32> {
        let _serial = self.serialise();
        let inst = self.checkout().ok()?;
        let geom = Geometry::nominal(scale);
        let values = self.bindings.decode(params.flat, params.texts, &geom.coord());
        inst.apply_values(&values);
        let fps = self.fps();
        let ctx = Arc::new(RenderCtx::new(
            self.cfg.clone(),
            geom,
            fps,
            time,
            None,
            self.bindings.clone(),
            self.bindings.layer_ids(params.flat),
            inst.prefs.depth,
            inst.prefs.comps.clone(),
            None,
        ));
        inst.begin(&ctx);
        let default = ctx.input_rod();
        let rod = inst.region_of_definition(time * fps, scale, default);
        inst.end();
        self.checkin(inst);
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
        let pad = if host.is_some() { self.compute_pad(params, time, scale).min(w / 2).min(h / 2) } else { 0 };
        let geom = Geometry { scale, layer, origin: frame.origin, w, h, pad };

        let _serial = self.serialise();
        let mut inst = self.checkout()?;
        let result = self.run(&mut inst, frame, params, time, fps, geom, host);
        inst.end();
        let msg = inst.obj.message();
        self.checkin(inst);
        result.map_err(|e| if msg.is_empty() { e } else { format!("{e}: {msg}") })
    }

    #[allow(clippy::too_many_arguments)]
    fn run(&self, inst: &mut Instance, frame: &mut PluginFrame, params: &PluginParams, time: f64, fps: f64, geom: Geometry, host: Option<&dyn PluginHost>) -> Result<(), String> {
        let (w, h, pad) = (geom.w as usize, geom.h as usize, geom.pad as usize);
        let t = time * fps;
        let values = self.bindings.decode(params.flat, params.texts, &geom.coord());
        let changed = inst.apply_values(&values);

        // The plug-in's own layer at the current time: the buffer minus our padding.
        let src_rect = geom.src_rect();
        let in_comps = self.input_comps(&inst.prefs);
        let depth = inst.prefs.depth;
        let has_input = self.cfg.clips.iter().any(|c| c.role == ClipRole::Input);
        let src_now = has_input.then(|| {
            Arc::new(ClipImage::from_rows(frame.pixels, w, (pad, pad, w.saturating_sub(pad), h.saturating_sub(pad)), src_rect, src_rect, depth, in_comps))
        });
        let ctx = Arc::new(RenderCtx::new(
            self.cfg.clone(),
            geom,
            fps,
            time,
            host.map(HostPtr::new),
            self.bindings.clone(),
            self.bindings.layer_ids(params.flat),
            depth,
            inst.prefs.comps.clone(),
            src_now,
        ));
        inst.begin(&ctx);

        if inst.rendered {
            for name in &changed {
                inst.instance_changed(name, t);
            }
            if changed.iter().any(|n| self.cfg.slave_params.contains(n)) {
                inst.refresh_prefs();
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
            && let Some(cfg) = self.cfg.clip(&clip)
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

        let out_comps = self.cfg.clips.iter().find(|c| c.role == ClipRole::Output).map(|c| inst.prefs.comps.get(&c.name).copied().unwrap_or(c.comps)).unwrap_or(Comps::Rgba);
        let out = Arc::new(ClipImage::blank(bounds, rod_px, depth, out_comps));
        *ctx.output.lock().unwrap_or_else(PoisonError::into_inner) = Some(out.clone());

        inst.begin_sequence(t, geom.scale);
        let status = inst.render(t, window, geom.scale);
        inst.end_sequence(t, geom.scale);
        inst.rendered = true;
        match status {
            K_STAT_OK | K_STAT_REPLY_DEFAULT => {}
            K_STAT_UNLICENSED => return Err("the plug-in reports it is unlicensed".into()),
            s => return Err(format!("the plug-in's render failed (OFX status {s})")),
        }
        out.write_rows(frame.pixels, inst.prefs.out_unpremult);
        Ok(())
    }
}

impl EffectPlugin for OfxEffect {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn render(&self, frame: &mut PluginFrame, params: &PluginParams, time: f64) -> Result<(), String> {
        self.render_impl(frame, params, time, None)
    }

    fn source(&self) -> String {
        self.source.clone()
    }

    fn padding(&self, params: &PluginParams, time: f64, scale: f64) -> u32 {
        self.compute_pad(params, time, scale).max(0) as u32
    }

    fn render_v2(&self, frame: &mut PluginFrame, params: &PluginParams, time: f64, host: &dyn PluginHost) -> Result<(), String> {
        self.render_impl(frame, params, time, Some(host))
    }
}

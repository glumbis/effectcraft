//! A plug-in that declares itself render-thread-unsafe makes the host serialise every render of its
//! library. While it renders it asks the host for an image at another time, and the host renders what
//! is upstream to answer: another effect of the same library, on the same thread. That nested render
//! used to wait for the lock its own thread held, and the app hung (G-14).
//!
//! Set `EC_TEST_OFX_BUNDLE` to the built `ec-test-ofx.ofx.bundle`; without it the test returns.

use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crate::common::defaults;
use effectcraft_effects::plugin::{EffectPlugin, PluginFrame, PluginHost, PluginImage, PluginParams, plugin};
use effectcraft_ofx::load_path;

/// The test library's `Serial` effect: declared render-thread-unsafe, it averages its source with
/// the same layer one frame earlier, which it fetches through the host.
fn serial() -> Option<Arc<dyn EffectPlugin>> {
    static LOADED: OnceLock<Option<Arc<dyn EffectPlugin>>> = OnceLock::new();
    LOADED
        .get_or_init(|| {
            let bundle = PathBuf::from(std::env::var_os("EC_TEST_OFX_BUNDLE").or_else(|| {
                println!("note: EC_TEST_OFX_BUNDLE is not set; skipping the re-entrancy test");
                None
            })?);
            let r = load_path(&bundle);
            assert!(r.errors.is_empty(), "{:?}", r.errors);
            plugin("org.effectcraft.test.serial")
        })
        .clone()
}

/// Renders a 2x2 layer of `base` (grey) through the `Serial` effect on the calling thread, with `host`.
fn render(p: &dyn EffectPlugin, base: f32, host: &dyn PluginHost) -> Vec<[f32; 4]> {
    let (flat, texts) = defaults(p, &[]);
    let params = PluginParams { manifest: p.manifest(), flat: &flat, texts: &texts };
    let mut pixels = vec![[base, base, base, 1.0]; 4];
    let mut frame = PluginFrame { width: 2, height: 2, pixels: &mut pixels, scale: 1.0, origin: [0.0; 2], layer_size: [2.0, 2.0] };
    p.render_v2(&mut frame, &params, 1.0, host).expect("render");
    pixels
}

/// The host of one layer's effect: what is upstream of it is, `depth` levels down, the same
/// library's `Serial` effect on another layer (another instance key), rendered right here when the
/// plug-in asks for an image: that is what the engine does.
struct Upstream {
    effect: Arc<dyn EffectPlugin>,
    depth: u32,
    key: u64,
}

impl PluginHost for Upstream {
    fn input_at(&self, _layer_time: f64) -> Option<PluginImage> {
        let (w, h) = (2, 2);
        let pixels = if self.depth >= 2 {
            vec![[0.0; 4]; 4]
        } else {
            let inner = Upstream { effect: self.effect.clone(), depth: self.depth + 1, key: self.key + 1 };
            render(&*self.effect, 0.8, &inner)
        };
        Some(PluginImage { width: w, height: h, pixels, origin: [0.0; 2], scale: 1.0 })
    }
    fn layer_input(&self, _param_id: &str, _layer_time: f64) -> Option<PluginImage> {
        None
    }
    fn params_at(&self, _layer_time: f64) -> Option<(Vec<f64>, Vec<String>)> {
        None
    }
    fn frame_rate(&self) -> f64 {
        24.0
    }
    fn time(&self) -> f64 {
        1.0
    }
    fn instance_key(&self) -> u64 {
        self.key
    }
}

#[test]
fn an_unsafe_plugin_may_render_another_effect_of_its_library_while_it_renders() {
    let Some(p) = serial() else { return };
    // On a thread of its own with a timeout: the failure this guards against is a hang.
    let (tx, rx) = channel();
    let effect = p.clone();
    std::thread::spawn(move || {
        let host = Upstream { effect: effect.clone(), depth: 0, key: 1 };
        let _ = tx.send(render(&*effect, 0.8, &host));
    });
    let out = rx.recv_timeout(Duration::from_secs(30)).expect("the nested render of a render-thread-unsafe plug-in deadlocked");
    // Three nested levels: the innermost fetches transparent black, so it makes (0.8 + 0) / 2 = 0.4, the next
    // (0.8 + 0.4) / 2 = 0.6 and the outer (0.8 + 0.6) / 2 = 0.7; alpha is averaged the same way: 0.5, 0.75, 0.875.
    for px in out {
        assert!((px[0] - 0.7).abs() < 1e-4 && (px[3] - 0.875).abs() < 1e-4, "{px:?}");
    }
}

/// Renders from several threads at once are still one at a time for such a plug-in, and none of
/// them hangs when each also nests.
#[test]
fn concurrent_renders_of_an_unsafe_plugin_take_turns_and_all_finish() {
    let Some(p) = serial() else { return };
    let (tx, rx) = channel();
    for i in 0..4u64 {
        let (effect, tx) = (p.clone(), tx.clone());
        std::thread::spawn(move || {
            let host = Upstream { effect: effect.clone(), depth: 0, key: 100 * (i + 1) };
            let _ = tx.send(render(&*effect, 0.8, &host)[0][0]);
        });
    }
    for _ in 0..4 {
        let v = rx.recv_timeout(Duration::from_secs(30)).expect("a concurrent render deadlocked");
        assert!((v - 0.7).abs() < 1e-4, "{v}");
    }
}

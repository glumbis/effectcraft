//! A tiny OpenFX plug-in bundle used to test EffectCraft's OFX host. It exports sixteen effects:
//!
//! * `org.effectcraft.test.Invert`: filter, inverts the colours (double param `amount`);
//! * `org.effectcraft.test.Echo`: filter, averages Source at `t` and `t - 1` (temporal access);
//! * `org.effectcraft.test.Border`: filter, grows the region of definition by int param `size`
//!   pixels and paints the new area opaque red;
//! * `org.effectcraft.test.Mix`: general context, clips Source / Matte / Output, the Matte clip's
//!   alpha multiplies the Source;
//! * `org.effectcraft.test.AllParams`: declares every parameter type, passes the image through;
//! * `org.effectcraft.test.SupportLib`: describes itself the way the OFX C++ Support library does
//!   (every list is appended to at the index `propGetDimension` returned, and a failing
//!   `propGetDimension` fails Describe), passes the image through;
//! * `org.effectcraft.test.CreateTime`: reads its `level` parameter in CreateInstance (and keeps it
//!   as instance data), multiplies the source by that created value, and answers
//!   GetClipPreferences from the *current* `level` (a slave parameter): "unpremultiplied" below 0.5;
//! * `org.effectcraft.test.ContextFallback`: lists the Filter and General contexts, but its Filter
//!   DescribeInContext fails (after defining a parameter), so the host has to fall back to General;
//! * `org.effectcraft.test.NoSource`: general context, clips Output / Mask / Overlay / Image: the
//!   main input is not called Source;
//! * `org.effectcraft.test.Serial`: like Echo (averages Source at `t` and `t - 1`) but declares
//!   itself render-thread-unsafe, so the host serialises every render of the library;
//! * `org.effectcraft.test.Probe` and `org.effectcraft.test.ProbeSingle` (which also sets
//!   `kOfxImageEffectPluginPropSingleInstance`): declare `kOfxImageEffectInstancePropSequentialRender`
//!   = 1, count what the host does to them and paint the counts into the first three pixels of the
//!   bottom row, so a test can see instance sharing and sequence handling (see `probe_pixels`);
//! * `org.effectcraft.test.ProbeSlow`: a probe that declares it keeps no state between frames
//!   (SequentialRender 0), takes 300 ms to create an instance (a plug-in that loads a model) and
//!   fetches its source at `t - 1` while it renders (so the host renders upstream effects);
//! * `org.effectcraft.test.Memory`: answers its first Render with `kOfxStatErrMemory`; once the host
//!   has sent `kOfxActionPurgeCaches` it renders, and it paints how many renders and purges it saw;
//! * `org.effectcraft.test.Crash`: passes the image through, but crashes the process in Render when
//!   the environment variable `EC_TEST_OFX_CRASH` is `segv` (a null write) or `abort`.
//! * `org.effectcraft.test.Parametric`: defines a two-curve parametric parameter with default control
//!   points while it describes itself, and at render reads them back through the parametric suite and
//!   paints what it saw into the first four pixels of the bottom row (see `parametric_render`).
//!
//! With the environment variable `EC_TEST_OFX_LOG` set to a file, the probes append a line for
//! each `BeginSequenceRender`, `EndSequenceRender`, `DestroyInstance` and every plug-in a line for
//! `Unload` (what the host does at shutdown).
//!
//! With `EC_TEST_OFX_MODE=renamed` every label gets a suffix, so the description differs from the
//! one made without it.
//!
//! With `EC_TEST_OFX_MODE=extras` it exports only two more, which cannot become effects:
//! `GpuOnly` (needs OpenGL, no CPU rendering) and `NotAnImageEffect` (another plug-in API).
//!
//! Float RGBA only. Rows are addressed through `OfxImagePropRowBytes` (which may be negative).

mod ffi;

use std::ffi::{CStr, c_char, c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::{null, null_mut};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

use ffi::*;

const INVERT: usize = 0;
const ECHO: usize = 1;
const BORDER: usize = 2;
const MIX: usize = 3;
const ALL_PARAMS: usize = 4;
const SUPPORT_LIB: usize = 5;
const CREATE_TIME: usize = 6;
const CONTEXT_FALLBACK: usize = 7;
const NO_SOURCE: usize = 8;
const GPU_ONLY: usize = 9;
const NOT_AN_IMAGE_EFFECT: usize = 10;
const SERIAL: usize = 11;
const PROBE: usize = 12;
const PROBE_SINGLE: usize = 13;
const CRASH: usize = 14;
const PROBE_SLOW: usize = 15;
const MEMORY: usize = 16;
const PARAMETRIC: usize = 17;

const LABELS: [&CStr; 18] = [
    c"EC Test Invert",
    c"EC Test Echo",
    c"EC Test Border",
    c"EC Test Mix",
    c"EC Test All Params",
    c"EC Test Support Lib",
    c"EC Test Create Time",
    c"EC Test Context Fallback",
    c"EC Test No Source",
    c"EC Test GPU Only",
    c"EC Test Not An Image Effect",
    c"EC Test Serial",
    c"EC Test Probe",
    c"EC Test Probe Single",
    c"EC Test Crash",
    c"EC Test Probe Slow",
    c"EC Test Memory",
    c"EC Test Parametric",
];

static HOST: AtomicPtr<OfxHost> = AtomicPtr::new(null_mut());
/// Set when the host has called `OfxSetHost` (the header orders it before `OfxGetNumberOfPlugins`).
static SET_HOST_CALLED: AtomicBool = AtomicBool::new(false);
static SUITES: OnceLock<Suites> = OnceLock::new();

fn suites() -> &'static Suites {
    SUITES.get().expect("OfxActionLoad has not run")
}

unsafe extern "C" fn set_host(host: *mut OfxHost) {
    HOST.store(host, Ordering::SeqCst);
}

/// How the host's tests make this plug-in misbehave: the environment variable `EC_TEST_OFX_MODE`
/// (read when each entry point runs) is one of `decline-host`, `no-plugins`, `load-fails`,
/// `describe-fails` (see the host's `tests/diagnostics.rs`). Unset: a well-behaved plug-in.
fn mode() -> String {
    std::env::var("EC_TEST_OFX_MODE").unwrap_or_default()
}

/// `OfxSetHost`, the optional OFX 1.5 entry point that runs before `OfxGetNumberOfPlugins`; a
/// plug-in that is not for this host answers `kOfxStatFailed` and the host skips it.
#[unsafe(no_mangle)]
pub extern "C" fn OfxSetHost(host: *mut OfxHost) -> c_int {
    HOST.store(host, Ordering::SeqCst);
    SET_HOST_CALLED.store(true, Ordering::SeqCst);
    if mode() == "decline-host" { FAILED } else { OK }
}

// ---- exports ---------------------------------------------------------------------------------

struct Plug(OfxPlugin);
// SAFETY: the table is immutable and only holds pointers to 'static data.
unsafe impl Sync for Plug {}

macro_rules! plug {
    ($id:expr, $k:expr) => {
        plug!($id, $k, c"OfxImageEffectPluginAPI")
    };
    ($id:expr, $k:expr, $api:expr) => {
        Plug(OfxPlugin {
            plugin_api: $api.as_ptr(),
            api_version: 1,
            plugin_identifier: $id.as_ptr(),
            version_major: 1,
            version_minor: 0,
            set_host,
            main_entry: entry::<{ $k }>,
        })
    };
}

static PLUGINS: [Plug; 16] = [
    plug!(c"org.effectcraft.test.Invert", INVERT),
    plug!(c"org.effectcraft.test.Echo", ECHO),
    plug!(c"org.effectcraft.test.Border", BORDER),
    plug!(c"org.effectcraft.test.Mix", MIX),
    plug!(c"org.effectcraft.test.AllParams", ALL_PARAMS),
    plug!(c"org.effectcraft.test.SupportLib", SUPPORT_LIB),
    plug!(c"org.effectcraft.test.CreateTime", CREATE_TIME),
    plug!(c"org.effectcraft.test.ContextFallback", CONTEXT_FALLBACK),
    plug!(c"org.effectcraft.test.NoSource", NO_SOURCE),
    plug!(c"org.effectcraft.test.Serial", SERIAL),
    plug!(c"org.effectcraft.test.Probe", PROBE),
    plug!(c"org.effectcraft.test.ProbeSingle", PROBE_SINGLE),
    plug!(c"org.effectcraft.test.Crash", CRASH),
    plug!(c"org.effectcraft.test.ProbeSlow", PROBE_SLOW),
    plug!(c"org.effectcraft.test.Memory", MEMORY),
    plug!(c"org.effectcraft.test.Parametric", PARAMETRIC),
];

/// What `EC_TEST_OFX_MODE=extras` exports instead of [`PLUGINS`].
static EXTRAS: [Plug; 2] =
    [plug!(c"org.effectcraft.test.GpuOnly", GPU_ONLY), plug!(c"org.effectcraft.test.NotAnImageEffect", NOT_AN_IMAGE_EFFECT, c"OfxEffectTestOtherPluginAPI")];

fn table() -> &'static [Plug] {
    if mode() == "extras" { &EXTRAS } else { &PLUGINS }
}

#[unsafe(no_mangle)]
pub extern "C" fn OfxGetNumberOfPlugins() -> c_int {
    // A plug-in library may decide what it exports once it has seen the host.
    if mode() == "no-plugins" || !SET_HOST_CALLED.load(Ordering::SeqCst) { 0 } else { table().len() as c_int }
}

#[unsafe(no_mangle)]
pub extern "C" fn OfxGetPlugin(nth: c_int) -> *mut OfxPlugin {
    usize::try_from(nth).ok().and_then(|n| table().get(n)).map_or(null_mut(), |p| &p.0 as *const OfxPlugin as *mut OfxPlugin)
}

// ---- actions ---------------------------------------------------------------------------------

unsafe extern "C" fn entry<const K: usize>(action: *const c_char, handle: *const c_void, in_args: Props, out_args: Props) -> Status {
    let action = unsafe { CStr::from_ptr(action) }.to_bytes();
    catch_unwind(AssertUnwindSafe(|| unsafe { dispatch(K, action, handle as Effect, in_args, out_args) })).unwrap_or(FAILED)
}

unsafe fn dispatch(k: usize, action: &[u8], effect: Effect, in_args: Props, out_args: Props) -> Status {
    unsafe {
        match action {
            b"OfxActionLoad" => match Suites::fetch(HOST.load(Ordering::SeqCst)) {
                Some(s) => {
                    let _ = SUITES.set(s);
                    if mode() == "load-fails" {
                        // As a licence check does: say why, then decline.
                        suites().post_error(null_mut(), c"The licence file was not found");
                        return FAILED;
                    }
                    OK
                }
                None => ERR_UNSUPPORTED,
            },
            b"OfxActionDescribe" => describe(k, effect),
            b"OfxImageEffectActionDescribeInContext" => describe_in_context(k, effect, in_args),
            b"OfxActionCreateInstance" if k == CREATE_TIME => create_time_create(effect),
            b"OfxActionDestroyInstance" if k == CREATE_TIME => create_time_destroy(effect),
            b"OfxActionCreateInstance" if is_probe(k) => probe_create(k, effect),
            b"OfxActionDestroyInstance" if is_probe(k) => probe_destroy(k, effect),
            b"OfxImageEffectActionBeginSequenceRender" if is_probe(k) => probe_begin(k, effect, in_args),
            b"OfxImageEffectActionEndSequenceRender" if is_probe(k) => probe_end(k, effect),
            b"OfxActionCreateInstance" if k == MEMORY => memory_create(effect),
            b"OfxActionDestroyInstance" if k == MEMORY => memory_destroy(effect),
            b"OfxActionPurgeCaches" if k == MEMORY => memory_purge(effect),
            b"OfxActionUnload" => {
                log_line(&format!("Unload {}", LABELS.get(k).map(|l| l.to_string_lossy()).unwrap_or_default()));
                OK
            }
            b"OfxImageEffectActionGetClipPreferences" if k == CREATE_TIME => create_time_prefs(effect, out_args),
            b"OfxImageEffectActionGetRegionOfDefinition" if k == BORDER => border_rod(effect, in_args, out_args),
            b"OfxImageEffectActionRender" => render(k, effect, in_args),
            _ => REPLY_DEFAULT,
        }
    }
}

unsafe fn describe(k: usize, effect: Effect) -> Status {
    let s = suites();
    unsafe {
        if mode() == "describe-fails" {
            // Needs something of the host that it does not have, and says so.
            let host = HOST.load(Ordering::SeqCst).as_ref().map_or(null_mut(), |h| h.host);
            if s.get_string(host, c"OfxTestOnlyHostFeature").is_none() {
                s.post_error(effect, c"This host lacks a feature this plug-in needs");
                return ERR_MISSING_HOST_FEATURE;
            }
        }
        let mut p = null_mut();
        (s.fx.get_property_set)(effect, &mut p);
        if mode() == "renamed" {
            let renamed = std::ffi::CString::new(format!("{} (renamed)", LABELS[k].to_string_lossy())).unwrap_or_default();
            s.set_str(p, c"OfxPropLabel", 0, &renamed);
        } else {
            s.set_str(p, c"OfxPropLabel", 0, LABELS[k]);
        }
        s.set_str(p, c"OfxImageEffectPluginPropGrouping", 0, c"EC Test");
        if k == SUPPORT_LIB {
            // Like the Support library's `fetchHostDescription`, look at the host first (its name
            // above all): the host's load transcript lists the properties a plug-in read.
            let host = HOST.load(Ordering::SeqCst).as_ref().map_or(null_mut(), |h| h.host);
            let _ = s.get_string(host, c"OfxPropName");
            // The Support library's way: append at the current dimension. These lists start empty
            // on a conforming host, so the dimension query must succeed (with 0).
            let appended = append_str(p, c"OfxImageEffectPropSupportedContexts", c"OfxImageEffectContextFilter")
                .and_then(|()| append_str(p, c"OfxImageEffectPropSupportedContexts", c"OfxImageEffectContextGeneral"))
                .and_then(|()| append_str(p, c"OfxImageEffectPropSupportedPixelDepths", c"OfxBitDepthFloat"))
                .and_then(|()| append_str(p, c"OfxImageEffectPropSupportedPixelDepths", c"OfxBitDepthShort"))
                .and_then(|()| append_str(p, c"OfxImageEffectPropClipPreferencesSlaveParam", c"mode"));
            if let Err(status) = appended {
                return status;
            }
        } else {
            let general = k == MIX || k == NO_SOURCE;
            s.set_str(p, c"OfxImageEffectPropSupportedContexts", 0, if general { c"OfxImageEffectContextGeneral" } else { c"OfxImageEffectContextFilter" });
            if k == CONTEXT_FALLBACK {
                s.set_str(p, c"OfxImageEffectPropSupportedContexts", 1, c"OfxImageEffectContextGeneral");
            }
            s.set_str(p, c"OfxImageEffectPropSupportedPixelDepths", 0, c"OfxBitDepthFloat");
        }
        if k == GPU_ONLY {
            // Renders on the GPU only: says it needs OpenGL and has no CPU path.
            s.set_str(p, c"OfxImageEffectPropOpenGLRenderSupported", 0, c"needed");
            s.set_str(p, c"OfxImageEffectPropCPURenderSupported", 0, c"false");
        }
        let safety = if k == SERIAL { c"OfxImageEffectRenderUnsafe" } else { c"OfxImageEffectRenderFullySafe" };
        s.set_str(p, c"OfxImageEffectPluginRenderThreadSafety", 0, safety);
        s.set_int(p, c"OfxImageEffectPropSupportsTiles", 0, 0);
        s.set_int(p, c"OfxImageEffectPropTemporalClipAccess", 0, (k == ECHO || k == SERIAL || k == PROBE_SLOW) as c_int);
        if is_probe(k) {
            // The plug-in keeps state from frame to frame: frames in order, please (the slow probe says
            // it does not).
            s.set_int(p, c"OfxImageEffectInstancePropSequentialRender", 0, if k == PROBE_SLOW { 0 } else { 1 });
        }
        if k == PROBE_SINGLE {
            s.set_int(p, c"OfxImageEffectPluginPropSingleInstance", 0, 1);
        }
        if k == CREATE_TIME {
            // The clip preferences depend on `level`: the host asks again when it changes.
            s.set_str(p, c"OfxImageEffectPropClipPreferencesSlaveParam", 0, c"level");
        }
    }
    OK
}

/// What the OFX C++ Support library does for every list property it extends
/// (`propSetString(name, value, propGetDimension(name))`): a failing dimension query or set is
/// thrown as an exception, which the library turns into `kOfxStatErrMissingHostFeature`.
unsafe fn append_str(props: Props, key: &CStr, value: &CStr) -> Result<(), Status> {
    let s = suites();
    let mut n: c_int = -1;
    unsafe {
        if (s.prop.get_dimension)(props, key.as_ptr(), &mut n) != OK || n < 0 {
            return Err(ERR_MISSING_HOST_FEATURE);
        }
        if (s.prop.set_string)(props, key.as_ptr(), n, value.as_ptr()) != OK {
            return Err(ERR_MISSING_HOST_FEATURE);
        }
    }
    Ok(())
}

unsafe fn define_clip(effect: Effect, name: &CStr, optional: bool, temporal: bool, support_lib: bool) {
    let s = suites();
    unsafe {
        let mut p = null_mut();
        (s.fx.clip_define)(effect, name.as_ptr(), &mut p);
        if support_lib {
            // RGBA first, then RGB: the second append lands at index 1.
            let _ = append_str(p, c"OfxImageEffectPropSupportedComponents", c"OfxImageComponentRGBA")
                .and_then(|()| append_str(p, c"OfxImageEffectPropSupportedComponents", c"OfxImageComponentRGB"));
        } else {
            s.set_str(p, c"OfxImageEffectPropSupportedComponents", 0, c"OfxImageComponentRGBA");
        }
        s.set_int(p, c"OfxImageClipPropOptional", 0, optional as c_int);
        s.set_int(p, c"OfxImageEffectPropTemporalClipAccess", 0, temporal as c_int);
    }
}

/// Defines a parameter and returns its property set.
unsafe fn define_param(set: ParamSet, kind: &CStr, name: &CStr, label: &CStr) -> Props {
    let s = suites();
    unsafe {
        let mut p = null_mut();
        (s.param.define)(set, kind.as_ptr(), name.as_ptr(), &mut p);
        s.set_str(p, c"OfxPropLabel", 0, label);
        p
    }
}

unsafe fn describe_in_context(k: usize, effect: Effect, in_args: Props) -> Status {
    let s = suites();
    unsafe {
        let context = s.get_string(in_args, c"OfxImageEffectPropContext").unwrap_or_default();
        if k == NO_SOURCE {
            // The main input is `Image`, not `Source`; a required mask and an optional overlay
            // come before it.
            define_clip(effect, c"Output", false, false, false);
            define_clip(effect, c"Mask", false, false, false);
            let (mut c, mut p) = (null_mut(), null_mut());
            (s.fx.clip_get_handle)(effect, c"Mask".as_ptr(), &mut c, &mut p);
            s.set_int(p, c"OfxImageClipPropIsMask", 0, 1);
            define_clip(effect, c"Overlay", true, false, false);
            define_clip(effect, c"Image", false, false, false);
        } else {
            define_clip(effect, c"Output", false, false, k == SUPPORT_LIB);
            define_clip(effect, c"Source", false, k == ECHO || k == SERIAL || k == PROBE_SLOW, k == SUPPORT_LIB);
            if k == MIX {
                define_clip(effect, c"Matte", true, false, false);
            }
        }
        let mut set = null_mut();
        (s.fx.get_param_set)(effect, &mut set);
        match k {
            INVERT => {
                let p = define_param(set, c"OfxParamTypeDouble", c"amount", c"Amount");
                s.set_dbl(p, c"OfxParamPropDefault", 0, 1.0);
                s.set_dbl(p, c"OfxParamPropMin", 0, 0.0);
                s.set_dbl(p, c"OfxParamPropMax", 0, 1.0);
                s.set_dbl(p, c"OfxParamPropDisplayMin", 0, 0.0);
                s.set_dbl(p, c"OfxParamPropDisplayMax", 0, 1.0);
            }
            BORDER => {
                let p = define_param(set, c"OfxParamTypeInteger", c"size", c"Size");
                s.set_int(p, c"OfxParamPropDefault", 0, 4);
                s.set_int(p, c"OfxParamPropMin", 0, 0);
                s.set_int(p, c"OfxParamPropMax", 0, 256);
            }
            ALL_PARAMS => describe_all_params(set),
            PARAMETRIC => {
                let status = describe_parametric(set);
                if status != OK {
                    return status;
                }
            }
            CONTEXT_FALLBACK => {
                if context == "OfxImageEffectContextFilter" {
                    // Fails after defining a parameter: the host must not keep it for the General
                    // context it falls back to.
                    define_param(set, c"OfxParamTypeDouble", c"stale", c"Stale");
                    s.post_error(effect, c"The filter context is not available");
                    return ERR_MISSING_HOST_FEATURE;
                }
                let p = define_param(set, c"OfxParamTypeDouble", c"gain", c"Gain");
                s.set_dbl(p, c"OfxParamPropDefault", 0, 1.0);
            }
            CREATE_TIME => {
                let p = define_param(set, c"OfxParamTypeDouble", c"level", c"Level");
                s.set_dbl(p, c"OfxParamPropDefault", 0, 1.0);
                s.set_dbl(p, c"OfxParamPropMin", 0, 0.0);
                s.set_dbl(p, c"OfxParamPropMax", 0, 1.0);
            }
            SUPPORT_LIB => {
                if let Err(status) = describe_support_lib_params(set) {
                    return status;
                }
            }
            _ => {}
        }
    }
    OK
}

/// A choice whose options are appended, and two pages whose order is set through
/// `kOfxPluginPropParamPageOrder` (appended at the dimension, on the param set's property set).
unsafe fn describe_support_lib_params(set: ParamSet) -> Result<(), Status> {
    let s = suites();
    unsafe {
        let mode = define_param(set, c"OfxParamTypeChoice", c"mode", c"Mode");
        for o in [c"Fast", c"Balanced", c"Best"] {
            append_str(mode, c"OfxParamPropChoiceOption", o)?;
        }
        s.set_int(mode, c"OfxParamPropDefault", 0, 1);
        define_param(set, c"OfxParamTypeDouble", c"pa", c"Param A");
        define_param(set, c"OfxParamTypeDouble", c"pb", c"Param B");
        let page_a = define_param(set, c"OfxParamTypePage", c"pageA", c"Page A");
        append_str(page_a, c"OfxParamPropPageChild", c"pa")?;
        append_str(page_a, c"OfxParamPropPageChild", c"mode")?;
        let page_b = define_param(set, c"OfxParamTypePage", c"pageB", c"Page B");
        append_str(page_b, c"OfxParamPropPageChild", c"pb")?;
        let mut set_props = null_mut();
        if (s.param.set_get_property_set)(set, &mut set_props) != OK {
            return Err(ERR_MISSING_HOST_FEATURE);
        }
        // Page B is listed first.
        append_str(set_props, c"OfxPluginPropParamPageOrder", c"pageB")?;
        append_str(set_props, c"OfxPluginPropParamPageOrder", c"pageA")?;
    }
    Ok(())
}

/// A two-curve parametric parameter, described the way the Support library does it: the dimension
/// and colours are properties, the control points go through the suite on the descriptor's parameter
/// handle (fetched with `paramGetHandle`). Curve 0 gets four points added out of order (the last one
/// is at the key of an earlier one and replaces it); curve 1 gets none, so it stays the identity.
/// Any failing status fails the description, as the Support library's exceptions would.
unsafe fn describe_parametric(set: ParamSet) -> Status {
    let s = suites();
    let Some(ps) = s.parametric else { return ERR_MISSING_HOST_FEATURE };
    unsafe {
        let mut props = null_mut();
        let st = (s.param.define)(set, c"OfxParamTypeParametric".as_ptr(), c"curve".as_ptr(), &mut props);
        if st != OK {
            return st;
        }
        s.set_str(props, c"OfxPropLabel", 0, c"Curve");
        s.set_int(props, c"OfxParamPropParametricDimension", 0, 2);
        // Red for the first curve, green for the second (three numbers per curve).
        for (i, v) in [1.0, 0.0, 0.0, 0.0, 1.0, 0.0].into_iter().enumerate() {
            s.set_dbl(props, c"OfxParamPropParametricUIColour", i as c_int, v);
        }
        // The range stays at its default, [0, 1]; no background interact.
        s.set_ptr(props, c"OfxParamPropParametricInteractBackground", 0, null_mut());
        let mut handle = null_mut();
        let st = (s.param.get_handle)(set, c"curve".as_ptr(), &mut handle, null_mut());
        if st != OK {
            return st;
        }
        for (key, value) in [(1.0, 0.4), (0.0, 0.2), (0.5, 0.9), (0.5, 0.8)] {
            let st = (ps.add_control_point)(handle, 0, 0.0, key, value, false);
            if st != OK {
                return st;
            }
        }
    }
    OK
}

/// One parameter of every OFX type, plus a group (with a child) and a page listing them all.
unsafe fn describe_all_params(set: ParamSet) {
    let s = suites();
    unsafe {
        let p = define_param(set, c"OfxParamTypeDouble", c"double", c"Double");
        s.set_dbl(p, c"OfxParamPropDefault", 0, 0.5);
        s.set_dbl(p, c"OfxParamPropMin", 0, -10.0);
        s.set_dbl(p, c"OfxParamPropMax", 0, 10.0);
        let p = define_param(set, c"OfxParamTypeInteger", c"int", c"Int");
        s.set_int(p, c"OfxParamPropDefault", 0, 3);
        s.set_int(p, c"OfxParamPropMin", 0, 0);
        s.set_int(p, c"OfxParamPropMax", 0, 10);
        let p = define_param(set, c"OfxParamTypeBoolean", c"bool", c"Bool");
        s.set_int(p, c"OfxParamPropDefault", 0, 1);
        let p = define_param(set, c"OfxParamTypeChoice", c"choice", c"Choice");
        for (i, o) in [c"First", c"Second", c"Third"].iter().enumerate() {
            s.set_str(p, c"OfxParamPropChoiceOption", i as c_int, o);
        }
        s.set_int(p, c"OfxParamPropDefault", 0, 1);
        let p = define_param(set, c"OfxParamTypeRGBA", c"rgba", c"Colour");
        for (i, v) in [0.25, 0.5, 0.75, 1.0].into_iter().enumerate() {
            s.set_dbl(p, c"OfxParamPropDefault", i as c_int, v);
        }
        let p = define_param(set, c"OfxParamTypeDouble2D", c"double2d", c"Double 2D");
        // A position, so the host shows it as a viewer point (the 3D one stays plain: sliders).
        s.set_str(p, c"OfxParamPropDoubleType", 0, c"OfxParamDoubleTypeXYAbsolute");
        for i in 0..2 {
            s.set_dbl(p, c"OfxParamPropDefault", i, 10.0 * (i + 1) as f64);
        }
        let p = define_param(set, c"OfxParamTypeDouble3D", c"double3d", c"Double 3D");
        for i in 0..3 {
            s.set_dbl(p, c"OfxParamPropDefault", i, (i + 1) as f64);
        }
        let p = define_param(set, c"OfxParamTypeString", c"string", c"String");
        s.set_str(p, c"OfxParamPropDefault", 0, c"hello");
        s.set_str(p, c"OfxParamPropStringMode", 0, c"OfxParamStringIsSingleLine");
        let p = define_param(set, c"OfxParamTypeCustom", c"custom", c"Custom");
        s.set_str(p, c"OfxParamPropDefault", 0, c"opaque-data");
        define_param(set, c"OfxParamTypePushButton", c"button", c"Button");
        define_param(set, c"OfxParamTypeGroup", c"group", c"Group");
        let p = define_param(set, c"OfxParamTypeDouble", c"grouped", c"Grouped");
        s.set_dbl(p, c"OfxParamPropDefault", 0, 2.0);
        s.set_str(p, c"OfxParamPropParent", 0, c"group");
        let page = define_param(set, c"OfxParamTypePage", c"page", c"Main");
        let names = [c"double", c"int", c"bool", c"choice", c"rgba", c"double2d", c"double3d", c"string", c"custom", c"button", c"group"];
        for (i, n) in names.iter().enumerate() {
            s.set_str(page, c"OfxParamPropPageChild", i as c_int, n);
        }
    }
}

// ---- images ----------------------------------------------------------------------------------

/// A float RGBA image fetched from a clip; released on drop.
struct Img {
    props: Props,
    data: *mut u8,
    row_bytes: isize,
    /// Pixel bounds `[x1, y1, x2, y2)`, y up.
    bounds: [i32; 4],
}

impl Img {
    unsafe fn fetch(clip: Clip, time: f64) -> Option<Img> {
        let s = suites();
        unsafe {
            let mut props = null_mut();
            if (s.fx.clip_get_image)(clip, time, null(), &mut props) != OK || props.is_null() {
                return None;
            }
            let data = s.get_ptr(props, c"OfxImagePropData").unwrap_or(null_mut()) as *mut u8;
            let row_bytes = s.get_int(props, c"OfxImagePropRowBytes", 0).unwrap_or(0) as isize;
            let mut bounds = [0; 4];
            for (i, b) in bounds.iter_mut().enumerate() {
                *b = s.get_int(props, c"OfxImagePropBounds", i as c_int).unwrap_or(0);
            }
            // Dropping releases the image, also when there is no pixel data.
            let img = Img { props, data, row_bytes, bounds };
            if data.is_null() { None } else { Some(img) }
        }
    }

    fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.bounds[0] && x < self.bounds[2] && y >= self.bounds[1] && y < self.bounds[3]
    }

    /// Pixel address: `data + (y - y1) * rowBytes + (x - x1) * 16`, valid for negative row bytes.
    fn ptr(&self, x: i32, y: i32) -> *mut [f32; 4] {
        let off = (y - self.bounds[1]) as isize * self.row_bytes + (x - self.bounds[0]) as isize * 16;
        unsafe { self.data.offset(off) as *mut [f32; 4] }
    }

    /// Transparent black outside the bounds.
    fn get(&self, x: i32, y: i32) -> [f32; 4] {
        if self.contains(x, y) { unsafe { self.ptr(x, y).read_unaligned() } } else { [0.0; 4] }
    }

    fn set(&self, x: i32, y: i32, v: [f32; 4]) {
        if self.contains(x, y) {
            unsafe { self.ptr(x, y).write_unaligned(v) }
        }
    }
}

impl Drop for Img {
    fn drop(&mut self) {
        unsafe { (suites().fx.clip_release_image)(self.props) };
    }
}

fn at(img: &Option<Img>, x: i32, y: i32) -> [f32; 4] {
    img.as_ref().map_or([0.0; 4], |i| i.get(x, y))
}

unsafe fn clip(effect: Effect, name: &CStr) -> Clip {
    let mut c = null_mut();
    unsafe { (suites().fx.clip_get_handle)(effect, name.as_ptr(), &mut c, null_mut()) };
    c
}

unsafe fn param(effect: Effect, name: &CStr) -> Param {
    let s = suites();
    let (mut set, mut p) = (null_mut(), null_mut());
    unsafe {
        (s.fx.get_param_set)(effect, &mut set);
        (s.param.get_handle)(set, name.as_ptr(), &mut p, null_mut());
    }
    p
}

/// The double parameter `name` as `paramGetValue` (no time) reports it right now: a real C
/// variadic call with the out-pointer as the only variadic argument.
unsafe fn double_param_now(effect: Effect, name: &CStr) -> f64 {
    let (p, mut v) = (unsafe { param(effect, name) }, 0.0f64);
    unsafe { (suites().param.get_value)(p, &mut v as *mut f64) };
    v
}

/// The effect instance's own property set.
unsafe fn effect_props(effect: Effect) -> Props {
    let mut p = null_mut();
    unsafe { (suites().fx.get_property_set)(effect, &mut p) };
    p
}

/// CreateInstance: remember what `level` is *now*, as plug-ins keep the model or mode they were
/// created with. The host has to have applied the parameter values before this action.
unsafe fn create_time_create(effect: Effect) -> Status {
    unsafe {
        let level = double_param_now(effect, c"level");
        suites().set_ptr(effect_props(effect), c"OfxPropInstanceData", 0, Box::into_raw(Box::new(level)) as *mut c_void);
    }
    OK
}

unsafe fn create_time_destroy(effect: Effect) -> Status {
    unsafe {
        let props = effect_props(effect);
        if let Some(data) = suites().get_ptr(props, c"OfxPropInstanceData").filter(|d| !d.is_null()) {
            drop(Box::from_raw(data as *mut f64));
            suites().set_ptr(props, c"OfxPropInstanceData", 0, null_mut());
        }
    }
    OK
}

/// The `level` the instance was created with (1 when there is no instance data).
unsafe fn created_level(effect: Effect) -> f64 {
    unsafe {
        match suites().get_ptr(effect_props(effect), c"OfxPropInstanceData") {
            Some(data) if !data.is_null() => *(data as *const f64),
            _ => 1.0,
        }
    }
}

/// GetClipPreferences: from the current `level`, a slave parameter: below 0.5 the output is
/// straight (unpremultiplied) alpha, which the host premultiplies after the render.
unsafe fn create_time_prefs(effect: Effect, out_args: Props) -> Status {
    unsafe {
        let level = double_param_now(effect, c"level");
        let value = if level < 0.5 { c"OfxImageAlphaUnPremultiplied" } else { c"OfxImageAlphaPremultiplied" };
        suites().set_str(out_args, c"OfxImageEffectPropPreMultiplication", 0, value);
    }
    OK
}

unsafe fn double_param(effect: Effect, name: &CStr, time: f64) -> f64 {
    let (p, mut v) = (unsafe { param(effect, name) }, 0.0f64);
    unsafe { (suites().param.get_value_at_time)(p, time, &mut v as *mut f64) };
    v
}

unsafe fn int_param(effect: Effect, name: &CStr, time: f64) -> i32 {
    let (p, mut v) = (unsafe { param(effect, name) }, 0 as c_int);
    unsafe { (suites().param.get_value_at_time)(p, time, &mut v as *mut c_int) };
    v
}

// ---- probes ----------------------------------------------------------------------------------

fn is_probe(k: usize) -> bool {
    k == PROBE || k == PROBE_SINGLE || k == PROBE_SLOW
}

/// Which counters a probe kind uses.
fn probe_slot(k: usize) -> usize {
    match k {
        PROBE => 0,
        PROBE_SINGLE => 1,
        _ => 2,
    }
}

/// What a probe instance counts about what the host did to it.
struct ProbeData {
    serial: u32,
    begins: u32,
    ends: u32,
    /// Renders since the last BeginSequenceRender.
    in_sequence: u32,
    /// The frame range and the sequential-render status of the last BeginSequenceRender.
    range: [f64; 2],
    begin_status: i32,
}

/// Instances ever created (the next serial number) and instances alive now, per probe kind.
static PROBE_CREATES: [std::sync::atomic::AtomicU32; 3] = [const { std::sync::atomic::AtomicU32::new(0) }; 3];
static PROBE_LIVE: [std::sync::atomic::AtomicU32; 3] = [const { std::sync::atomic::AtomicU32::new(0) }; 3];

/// Append a line to the file named by `EC_TEST_OFX_LOG`, if there is one.
fn log_line(line: &str) {
    use std::io::Write;
    if let Some(path) = std::env::var_os("EC_TEST_OFX_LOG")
        && let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path)
    {
        let _ = writeln!(f, "{line}");
    }
}

unsafe fn probe_data<'a>(effect: Effect) -> Option<&'a mut ProbeData> {
    unsafe { suites().get_ptr(effect_props(effect), c"OfxPropInstanceData").filter(|d| !d.is_null()).map(|d| &mut *(d as *mut ProbeData)) }
}

unsafe fn probe_create(k: usize, effect: Effect) -> Status {
    if k == PROBE_SLOW {
        // A plug-in that loads a model per instance.
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    let slot = probe_slot(k);
    let serial = PROBE_CREATES[slot].fetch_add(1, Ordering::SeqCst) + 1;
    PROBE_LIVE[slot].fetch_add(1, Ordering::SeqCst);
    let data = Box::new(ProbeData { serial, begins: 0, ends: 0, in_sequence: 0, range: [0.0; 2], begin_status: -1 });
    unsafe { suites().set_ptr(effect_props(effect), c"OfxPropInstanceData", 0, Box::into_raw(data) as *mut c_void) };
    OK
}

unsafe fn probe_destroy(k: usize, effect: Effect) -> Status {
    unsafe {
        let props = effect_props(effect);
        if let Some(data) = suites().get_ptr(props, c"OfxPropInstanceData").filter(|d| !d.is_null()) {
            let d = Box::from_raw(data as *mut ProbeData);
            log_line(&format!("DestroyInstance {} #{}", LABELS[k].to_string_lossy(), d.serial));
            suites().set_ptr(props, c"OfxPropInstanceData", 0, null_mut());
            PROBE_LIVE[probe_slot(k)].fetch_sub(1, Ordering::SeqCst);
        }
    }
    OK
}

unsafe fn probe_begin(k: usize, effect: Effect, in_args: Props) -> Status {
    let s = suites();
    unsafe {
        if let Some(d) = probe_data(effect) {
            d.begins += 1;
            d.in_sequence = 0;
            d.range = [
                s.get_dbl(in_args, c"OfxImageEffectPropFrameRange", 0).unwrap_or(-1.0),
                s.get_dbl(in_args, c"OfxImageEffectPropFrameRange", 1).unwrap_or(-1.0),
            ];
            d.begin_status = s.get_int(in_args, c"OfxImageEffectPropSequentialRenderStatus", 0).unwrap_or(-1);
            log_line(&format!("BeginSequenceRender {} #{} {}..{}", LABELS[k].to_string_lossy(), d.serial, d.range[0], d.range[1]));
        }
    }
    OK
}

unsafe fn probe_end(k: usize, effect: Effect) -> Status {
    unsafe {
        if let Some(d) = probe_data(effect) {
            d.ends += 1;
            log_line(&format!("EndSequenceRender {} #{}", LABELS[k].to_string_lossy(), d.serial));
        }
    }
    OK
}

/// A probe paints twelve numbers into the first three pixels of the output's bottom row:
///
/// 1. `[serial, begins, renders since the last begin, instances of this kind alive]`,
/// 2. `[the declared SequentialRender seen on the instance, ends, the render's SequentialRenderStatus, last frame of the last begin's range]`,
/// 3. `[first frame of the last begin's range, the last begin's SequentialRenderStatus, instances of this kind ever created, 0]`.
unsafe fn probe_render(k: usize, effect: Effect, in_args: Props, out: &Img) -> Status {
    let s = suites();
    unsafe {
        if k == PROBE_SLOW {
            // Asks the host for the source one frame back, which makes it render what is upstream.
            let time = s.get_dbl(in_args, c"OfxPropTime", 0).unwrap_or(0.0);
            let _ = Img::fetch(clip(effect, c"Source"), time - 1.0);
        }
        let Some(d) = probe_data(effect) else { return FAILED };
        d.in_sequence += 1;
        let slot = probe_slot(k);
        let declared = s.get_int(effect_props(effect), c"OfxImageEffectInstancePropSequentialRender", 0).unwrap_or(-1);
        let status = s.get_int(in_args, c"OfxImageEffectPropSequentialRenderStatus", 0).unwrap_or(-1);
        let rows = [
            [d.serial as f32, d.begins as f32, d.in_sequence as f32, PROBE_LIVE[slot].load(Ordering::SeqCst) as f32],
            [declared as f32, d.ends as f32, status as f32, d.range[1] as f32],
            [d.range[0] as f32, d.begin_status as f32, PROBE_CREATES[slot].load(Ordering::SeqCst) as f32, 0.0],
        ];
        for (i, v) in rows.into_iter().enumerate() {
            out.set(out.bounds[0] + i as i32, out.bounds[1], v);
        }
    }
    OK
}

/// The Parametric effect: reads its curves back through the suite and paints, into the first four
/// pixels of the output's bottom row,
///
/// 1. `[curve 0 at 0.25, curve 0 at 0.75, curve 1 at 0.3, curve 0 at 2.0]`,
/// 2. `[control points of curve 0, of curve 1, key and value of curve 0's second point]`,
/// 3. `[status of a value on curve 2, of a count on curve -1, of the fourth point of curve 0, the host's
///    OfxParamHostPropSupportsParametricAnimation]`,
/// 4. `[range min, range max, dimension, curve 1 at 0.3 while a point at (1, 0.5) is added to it]`
///    (the point is deleted again, so the next render sees the curve as it was described).
unsafe fn parametric_render(effect: Effect, out: &Img) -> Status {
    let s = suites();
    let Some(ps) = s.parametric else { return ERR_MISSING_HOST_FEATURE };
    unsafe {
        let (mut set, mut handle, mut props) = (null_mut(), null_mut(), null_mut());
        (s.fx.get_param_set)(effect, &mut set);
        if (s.param.get_handle)(set, c"curve".as_ptr(), &mut handle, &mut props) != OK {
            return FAILED;
        }
        let value = |curve: c_int, x: f64| {
            let mut v = f64::NAN;
            let st = (ps.get_value)(handle, curve, 0.0, x, &mut v);
            (st, v as f32)
        };
        let count = |curve: c_int| {
            let mut n = -1;
            let st = (ps.get_n_control_points)(handle, curve, 0.0, &mut n);
            (st, n as f32)
        };
        let nth = |curve: c_int, n: c_int| {
            let (mut k, mut v) = (f64::NAN, f64::NAN);
            let st = (ps.get_nth_control_point)(handle, curve, 0.0, n, &mut k, &mut v);
            (st, k as f32, v as f32)
        };
        let host = HOST.load(Ordering::SeqCst).as_ref().map_or(null_mut(), |h| h.host);
        let animation = s.get_int(host, c"OfxParamHostPropSupportsParametricAnimation", 0).unwrap_or(-1);
        let (_, second_key, second_value) = nth(0, 1);
        let range = [s.get_dbl(props, c"OfxParamPropParametricRange", 0).unwrap_or(-1.0), s.get_dbl(props, c"OfxParamPropParametricRange", 1).unwrap_or(-1.0)];
        let dimension = s.get_int(props, c"OfxParamPropParametricDimension", 0).unwrap_or(-1);
        let first = [value(0, 0.25).1, value(0, 0.75).1, value(1, 0.3).1, value(0, 2.0).1];
        let second = [count(0).1, count(1).1, second_key, second_value];
        let third = [value(2, 0.5).0 as f32, count(-1).0 as f32, nth(0, 3).0 as f32, animation as f32];
        // An edit while rendering, undone straight away.
        let _ = (ps.add_control_point)(handle, 1, 0.0, 1.0, 0.5, false);
        let edited = value(1, 0.3).1;
        let _ = (ps.delete_all_control_points)(handle, 1);
        let fourth = [range[0] as f32, range[1] as f32, dimension as f32, edited];
        for (i, v) in [first, second, third, fourth].into_iter().enumerate() {
            out.set(out.bounds[0] + i as i32, out.bounds[1], v);
        }
    }
    OK
}

/// What the Memory effect's instance counts.
struct MemoryData {
    renders: u32,
    purges: u32,
}

unsafe fn memory_data<'a>(effect: Effect) -> Option<&'a mut MemoryData> {
    unsafe { suites().get_ptr(effect_props(effect), c"OfxPropInstanceData").filter(|d| !d.is_null()).map(|d| &mut *(d as *mut MemoryData)) }
}

unsafe fn memory_create(effect: Effect) -> Status {
    let data = Box::new(MemoryData { renders: 0, purges: 0 });
    unsafe { suites().set_ptr(effect_props(effect), c"OfxPropInstanceData", 0, Box::into_raw(data) as *mut c_void) };
    OK
}

unsafe fn memory_destroy(effect: Effect) -> Status {
    unsafe {
        let props = effect_props(effect);
        if let Some(data) = suites().get_ptr(props, c"OfxPropInstanceData").filter(|d| !d.is_null()) {
            drop(Box::from_raw(data as *mut MemoryData));
            suites().set_ptr(props, c"OfxPropInstanceData", 0, null_mut());
        }
    }
    OK
}

unsafe fn memory_purge(effect: Effect) -> Status {
    unsafe {
        if let Some(d) = memory_data(effect) {
            d.purges += 1;
        }
    }
    OK
}

/// The Memory effect: out of memory until the host has asked it to purge its caches. Paints
/// `[renders, purges, 0, 0]` into the first pixel of the output's bottom row.
unsafe fn memory_render(effect: Effect, out: &Img) -> Status {
    unsafe {
        let Some(d) = memory_data(effect) else { return FAILED };
        d.renders += 1;
        if d.purges == 0 {
            return ERR_MEMORY;
        }
        out.set(out.bounds[0], out.bounds[1], [d.renders as f32, d.purges as f32, 0.0, 0.0]);
    }
    OK
}

/// The Crash effect: with `EC_TEST_OFX_CRASH` set, the process dies here the way a buggy plug-in
/// takes it down (the host cannot catch either).
fn crash_if_asked() {
    match std::env::var("EC_TEST_OFX_CRASH").as_deref() {
        Ok("segv") => {
            // A write through a null pointer: an access violation / SIGSEGV.
            let p = std::hint::black_box(0usize) as *mut u8;
            unsafe { std::ptr::write_volatile(p, 1) };
        }
        Ok("abort") => std::process::abort(),
        _ => {}
    }
}

// ---- region of definition and render ---------------------------------------------------------

unsafe fn border_rod(effect: Effect, in_args: Props, out_args: Props) -> Status {
    let s = suites();
    unsafe {
        let time = s.get_dbl(in_args, c"OfxPropTime", 0).unwrap_or(0.0);
        let mut r = RectD::default();
        if (s.fx.clip_get_region_of_definition)(clip(effect, c"Source"), time, &mut r) != OK {
            return REPLY_DEFAULT;
        }
        let n = int_param(effect, c"size", time) as f64;
        for (i, v) in [r.x1 - n, r.y1 - n, r.x2 + n, r.y2 + n].into_iter().enumerate() {
            s.set_dbl(out_args, c"OfxImageEffectPropRegionOfDefinition", i as c_int, v);
        }
    }
    OK
}

unsafe fn render(k: usize, effect: Effect, in_args: Props) -> Status {
    let s = suites();
    unsafe {
        let time = s.get_dbl(in_args, c"OfxPropTime", 0).unwrap_or(0.0);
        let mut win = [0; 4];
        for (i, w) in win.iter_mut().enumerate() {
            *w = s.get_int(in_args, c"OfxImageEffectPropRenderWindow", i as c_int).unwrap_or(0);
        }
        let scale = [0, 1].map(|i| s.get_dbl(in_args, c"OfxImageEffectPropRenderScale", i).unwrap_or(1.0));
        let Some(out) = Img::fetch(clip(effect, c"Output"), time) else { return FAILED };
        if is_probe(k) {
            return probe_render(k, effect, in_args, &out);
        }
        if k == MEMORY {
            return memory_render(effect, &out);
        }
        if k == PARAMETRIC {
            return parametric_render(effect, &out);
        }
        if k == CRASH {
            crash_if_asked();
        }
        let src_clip = clip(effect, if k == NO_SOURCE { c"Image" } else { c"Source" });
        let src = Img::fetch(src_clip, time);

        // The per-pixel function of each effect, in output pixel coordinates (y up).
        let pixel: Box<dyn Fn(i32, i32) -> [f32; 4]> = match k {
            INVERT => {
                let amount = double_param(effect, c"amount", time) as f32;
                Box::new(move |x, y| {
                    let c = at(&src, x, y);
                    // Premultiplied inversion (alpha - colour), mixed in by `amount`.
                    let inv = [c[3] - c[0], c[3] - c[1], c[3] - c[2], c[3]];
                    std::array::from_fn(|i| c[i] + amount * (inv[i] - c[i]))
                })
            }
            ECHO | SERIAL => {
                // The same clip one frame earlier (needs temporal clip access).
                let prev = Img::fetch(src_clip, time - 1.0);
                Box::new(move |x, y| {
                    let a = at(&src, x, y);
                    let b = if prev.is_some() { at(&prev, x, y) } else { a };
                    std::array::from_fn(|i| 0.5 * (a[i] + b[i]))
                })
            }
            BORDER => {
                // The source's region of definition in pixels; everything outside it is red.
                let mut r = RectD::default();
                (s.fx.clip_get_region_of_definition)(src_clip, time, &mut r);
                let rect =
                    [(r.x1 * scale[0]).floor() as i32, (r.y1 * scale[1]).floor() as i32, (r.x2 * scale[0]).ceil() as i32, (r.y2 * scale[1]).ceil() as i32];
                Box::new(move |x, y| if x >= rect[0] && x < rect[2] && y >= rect[1] && y < rect[3] { at(&src, x, y) } else { [1.0, 0.0, 0.0, 1.0] })
            }
            MIX => {
                let matte_clip = clip(effect, c"Matte");
                let matte = if matte_clip.is_null() { None } else { Img::fetch(matte_clip, time) };
                Box::new(move |x, y| {
                    let a = if matte.is_some() { at(&matte, x, y)[3] } else { 1.0 };
                    at(&src, x, y).map(|v| v * a)
                })
            }
            CREATE_TIME => {
                // What the plug-in read when it was created, not what the parameter says now.
                let created = created_level(effect) as f32;
                Box::new(move |x, y| at(&src, x, y).map(|v| v * created))
            }
            _ => Box::new(move |x, y| at(&src, x, y)),
        };

        for y in win[1].max(out.bounds[1])..win[3].min(out.bounds[3]) {
            for x in win[0].max(out.bounds[0])..win[2].min(out.bounds[2]) {
                out.set(x, y, pixel(x, y));
            }
        }
    }
    OK
}

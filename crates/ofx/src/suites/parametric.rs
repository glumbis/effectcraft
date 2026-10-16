//! `OfxParametricParameterSuiteV1` (`ofxParametricParam.h`): evaluate and edit the curves of
//! parametric parameters.
//!
//! The suite is optional for plug-ins but a plug-in that defines a parametric parameter needs it
//! (and the parameter type itself: without it the Support library fails `DescribeInContext`). The
//! host keeps the control points of each curve on the parameter ([`crate::curve`] explains the
//! interpolation, which the header leaves to the host: piecewise linear here). The `time` and
//! `addAnimationKey` arguments are accepted and ignored: the host does not animate parametric
//! parameters (`OfxParamHostPropSupportsParametricAnimation` is 0), so a curve has one set of control
//! points for all times.
//!
//! A plug-in adds its default control points to the *descriptor's* parameter while it describes
//! itself (it fetches the handle with `paramGetHandle` on the descriptor's parameter set); every
//! instance starts with a copy. The handle checks and statuses follow the header: a handle that is
//! not a parametric parameter is `kOfxStatErrBadHandle`, a curve or control point that does not
//! exist is `kOfxStatErrBadIndex`.

use std::ffi::c_int;
use std::sync::PoisonError;

use crate::curve::{Curve, MAX_CURVES, merge_distance};
use crate::ffi::*;
use crate::params::{PKind, ParamObj};

/// The parametric parameter behind a handle.
fn parametric<'a>(h: Handle) -> Result<&'a ParamObj, OfxStatus> {
    // SAFETY: a handle from the plug-in, validated by its magic number; the reference is only used
    // during this callback.
    match unsafe { ParamObj::from_handle(h) } {
        Some(p) if p.kind == PKind::Parametric => Ok(p),
        _ => Err(K_STAT_ERR_BAD_HANDLE),
    }
}

/// How many curves the parameter has (`kOfxParamPropParametricDimension`, at least 0).
fn dimension(p: &ParamObj) -> usize {
    usize::try_from(p.props.int(PP_PARAMETRIC_DIMENSION).unwrap_or(1)).unwrap_or(0).min(MAX_CURVES)
}

/// A curve index the parameter has, or `kOfxStatErrBadIndex`.
fn curve_index(p: &ParamObj, curve: c_int) -> Result<usize, OfxStatus> {
    usize::try_from(curve).ok().filter(|i| *i < dimension(p)).ok_or(K_STAT_ERR_BAD_INDEX)
}

fn lock(p: &ParamObj) -> std::sync::MutexGuard<'_, Vec<Curve>> {
    p.curves.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Run `f` on the parameter and curve `curve_index` of a callback's arguments: the curve is made
/// when the plug-in has not given it a control point yet.
fn with_curve(h: Handle, curve: c_int, f: impl FnOnce(&ParamObj, &mut Curve) -> OfxStatus) -> OfxStatus {
    let p = match parametric(h) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let i = match curve_index(p, curve) {
        Ok(i) => i,
        Err(e) => return e,
    };
    let mut curves = lock(p);
    if curves.len() <= i {
        curves.resize_with(i + 1, Curve::default);
    }
    match curves.get_mut(i) {
        Some(c) => f(p, c),
        None => K_STAT_ERR_BAD_INDEX,
    }
}

extern "C" fn get_value(h: Handle, curve: c_int, _time: f64, position: f64, out: *mut f64) -> OfxStatus {
    guard(|| {
        let p = match parametric(h) {
            Ok(p) => p,
            Err(e) => return e,
        };
        let i = match curve_index(p, curve) {
            Ok(i) => i,
            Err(e) => return e,
        };
        if out.is_null() || position.is_nan() {
            return K_STAT_ERR_VALUE;
        }
        // A curve the plug-in never added a point to is the identity.
        let v = lock(p).get(i).map_or(position, |c| c.eval(position));
        // SAFETY: `out` is the plug-in's `double*`.
        unsafe { *out = v };
        K_STAT_OK
    })
}

extern "C" fn get_n_control_points(h: Handle, curve: c_int, _time: f64, out: *mut c_int) -> OfxStatus {
    guard(|| {
        let p = match parametric(h) {
            Ok(p) => p,
            Err(e) => return e,
        };
        let i = match curve_index(p, curve) {
            Ok(i) => i,
            Err(e) => return e,
        };
        if out.is_null() {
            return K_STAT_ERR_VALUE;
        }
        let n = lock(p).get(i).map_or(0, Curve::len);
        // SAFETY: `out` is the plug-in's `int*`.
        unsafe { *out = c_int::try_from(n).unwrap_or(c_int::MAX) };
        K_STAT_OK
    })
}

extern "C" fn get_nth_control_point(h: Handle, curve: c_int, _time: f64, nth: c_int, key: *mut f64, value: *mut f64) -> OfxStatus {
    guard(|| {
        let p = match parametric(h) {
            Ok(p) => p,
            Err(e) => return e,
        };
        let i = match curve_index(p, curve) {
            Ok(i) => i,
            Err(e) => return e,
        };
        if key.is_null() || value.is_null() {
            return K_STAT_ERR_VALUE;
        }
        let Some((k, v)) = usize::try_from(nth).ok().and_then(|n| lock(p).get(i).and_then(|c| c.point(n))) else { return K_STAT_ERR_BAD_INDEX };
        // SAFETY: the plug-in's `double*` out-pointers.
        unsafe {
            *key = k;
            *value = v;
        }
        K_STAT_OK
    })
}

extern "C" fn set_nth_control_point(h: Handle, curve: c_int, _time: f64, nth: c_int, key: f64, value: f64, _add_animation_key: u8) -> OfxStatus {
    guard(|| {
        let Ok(n) = usize::try_from(nth) else { return K_STAT_ERR_BAD_INDEX };
        with_curve(h, curve, |_, c| c.set_nth(n, key, value))
    })
}

extern "C" fn add_control_point(h: Handle, curve: c_int, _time: f64, key: f64, value: f64, _add_animation_key: u8) -> OfxStatus {
    guard(|| {
        with_curve(h, curve, |p, c| {
            let merge = merge_distance(&p.props.doubles(PP_PARAMETRIC_RANGE));
            c.add(key, value, merge)
        })
    })
}

extern "C" fn delete_control_point(h: Handle, curve: c_int, nth: c_int) -> OfxStatus {
    guard(|| {
        let Ok(n) = usize::try_from(nth) else { return K_STAT_ERR_BAD_INDEX };
        with_curve(h, curve, |_, c| c.delete_nth(n))
    })
}

extern "C" fn delete_all_control_points(h: Handle, curve: c_int) -> OfxStatus {
    guard(|| {
        with_curve(h, curve, |_, c| {
            c.clear();
            K_STAT_OK
        })
    })
}

/// The parametric parameter suite table.
pub static SUITE: OfxParametricParameterSuiteV1 = OfxParametricParameterSuiteV1 {
    get_value,
    get_n_control_points,
    get_nth_control_point,
    set_nth_control_point,
    add_control_point,
    delete_control_point,
    delete_all_control_points,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::EffectObj;
    use crate::props::PVal;

    /// A parametric parameter `name` with `dim` curves in a descriptor (which must outlive the handle).
    fn param(eff: &EffectObj, name: &str, dim: i32) -> Handle {
        let p = eff.params.define(eff as *const EffectObj, PKind::Parametric, name, false).expect("define");
        // SAFETY: a stable pointer into the set, which lives as long as `eff`.
        let p = unsafe { &*p };
        assert_eq!(p.props.set_at(PP_PARAMETRIC_DIMENSION, 0, PVal::Int(dim)), K_STAT_OK);
        p.handle()
    }

    fn value(h: Handle, curve: c_int, x: f64) -> Result<f64, OfxStatus> {
        let mut v = f64::NAN;
        match (SUITE.get_value)(h, curve, 0.0, x, &mut v) {
            K_STAT_OK => Ok(v),
            e => Err(e),
        }
    }

    fn count(h: Handle, curve: c_int) -> Result<c_int, OfxStatus> {
        let mut n = -1;
        match (SUITE.get_n_control_points)(h, curve, 0.0, &mut n) {
            K_STAT_OK => Ok(n),
            e => Err(e),
        }
    }

    fn nth(h: Handle, curve: c_int, n: c_int) -> Result<(f64, f64), OfxStatus> {
        let (mut k, mut v) = (f64::NAN, f64::NAN);
        match (SUITE.get_nth_control_point)(h, curve, 0.0, n, &mut k, &mut v) {
            K_STAT_OK => Ok((k, v)),
            e => Err(e),
        }
    }

    /// The suite's description of a curve before the plug-in adds anything: it is the identity and has
    /// no control points, for every dimension.
    #[test]
    fn a_new_parametric_parameter_is_the_identity() {
        let eff = EffectObj::new_descriptor();
        let h = param(&eff, "curve", 3);
        for curve in 0..3 {
            assert_eq!(count(h, curve), Ok(0));
            assert_eq!(value(h, curve, 0.3), Ok(0.3));
            assert_eq!(nth(h, curve, 0), Err(K_STAT_ERR_BAD_INDEX));
        }
        // The properties a plug-in reads back.
        let p = unsafe { ParamObj::from_handle(h) }.expect("handle");
        assert_eq!(p.props.doubles(PP_PARAMETRIC_RANGE), [0.0, 1.0]);
        assert_eq!(p.props.str(PP_TYPE).as_deref(), Some(PT_PARAMETRIC));
        assert!(!p.kind.has_value());
    }

    #[test]
    fn control_points_are_added_read_back_and_evaluated() {
        let eff = EffectObj::new_descriptor();
        let h = param(&eff, "curve", 2);
        // Added out of order; the second add at 0.5 replaces the first.
        for (k, v) in [(1.0, 0.4), (0.0, 0.2), (0.5, 0.9), (0.5, 0.8)] {
            assert_eq!((SUITE.add_control_point)(h, 0, 0.0, k, v, 0), K_STAT_OK);
        }
        assert_eq!(count(h, 0), Ok(3));
        assert_eq!(count(h, 1), Ok(0));
        assert_eq!((nth(h, 0, 0), nth(h, 0, 1), nth(h, 0, 2)), (Ok((0.0, 0.2)), Ok((0.5, 0.8)), Ok((1.0, 0.4))));
        assert!((value(h, 0, 0.25).unwrap() - 0.5).abs() < 1e-12);
        assert!((value(h, 0, 0.75).unwrap() - 0.6).abs() < 1e-12);
        assert_eq!(value(h, 0, 2.0), Ok(0.4));
        // The other curve is untouched: still the identity.
        assert_eq!(value(h, 1, 0.75), Ok(0.75));
        // Editing: move a point past its neighbour, delete one, delete all.
        assert_eq!((SUITE.set_nth_control_point)(h, 0, 0.0, 0, 0.9, 0.0, 1), K_STAT_OK);
        assert_eq!((nth(h, 0, 0), nth(h, 0, 2)), (Ok((0.5, 0.8)), Ok((0.9, 0.0))));
        assert_eq!((SUITE.delete_control_point)(h, 0, 1), K_STAT_OK);
        assert_eq!(count(h, 0), Ok(2));
        assert_eq!((SUITE.delete_all_control_points)(h, 0), K_STAT_OK);
        assert_eq!((count(h, 0), value(h, 0, 0.6)), (Ok(0), Ok(0.6)));
    }

    #[test]
    fn instances_start_with_the_described_control_points_and_keep_their_own() {
        let desc = EffectObj::new_descriptor();
        let h = param(&desc, "curve", 1);
        assert_eq!((SUITE.add_control_point)(h, 0, 0.0, 0.0, 1.0, 0), K_STAT_OK);
        assert_eq!((SUITE.add_control_point)(h, 0, 0.0, 1.0, 0.0, 0), K_STAT_OK);
        let owner = &*desc as *const EffectObj;
        let (a, b) = (ParamSetCopy::of(&desc, owner), ParamSetCopy::of(&desc, owner));
        // Both copies see the defaults: a falling line.
        assert!((value(a.handle(), 0, 0.25).unwrap() - 0.75).abs() < 1e-12);
        assert!((value(b.handle(), 0, 0.25).unwrap() - 0.75).abs() < 1e-12);
        // An edit of one instance changes neither the other nor the descriptor.
        assert_eq!((SUITE.delete_all_control_points)(a.handle(), 0), K_STAT_OK);
        assert_eq!(value(a.handle(), 0, 0.25), Ok(0.25));
        assert!((value(b.handle(), 0, 0.25).unwrap() - 0.75).abs() < 1e-12);
        assert!((value(h, 0, 0.25).unwrap() - 0.75).abs() < 1e-12);
    }

    /// An instance parameter set made from a descriptor, as `Instance::build` does.
    struct ParamSetCopy(crate::params::ParamSet);
    impl ParamSetCopy {
        fn of(desc: &EffectObj, owner: *const EffectObj) -> ParamSetCopy {
            let set = crate::params::ParamSet::default();
            desc.params.instantiate_into(&set, owner);
            ParamSetCopy(set)
        }
        fn handle(&self) -> Handle {
            let p = self.0.find("curve").expect("curve");
            // SAFETY: a stable pointer into the set.
            unsafe { (*p).handle() }
        }
    }

    #[test]
    fn bad_handles_indices_and_pointers_are_statuses() {
        let eff = EffectObj::new_descriptor();
        let h = param(&eff, "curve", 1);
        let not_parametric = {
            let p = eff.params.define(&*eff as *const EffectObj, PKind::Double, "d", false).expect("define");
            // SAFETY: a stable pointer into the set.
            unsafe { (*p).handle() }
        };
        // Null and a parameter of another type (plug-ins pass handles they got from this host).
        for bad in [std::ptr::null_mut(), not_parametric] {
            assert_eq!(value(bad, 0, 0.5), Err(K_STAT_ERR_BAD_HANDLE));
            assert_eq!(count(bad, 0), Err(K_STAT_ERR_BAD_HANDLE));
            assert_eq!(nth(bad, 0, 0), Err(K_STAT_ERR_BAD_HANDLE));
            assert_eq!((SUITE.add_control_point)(bad, 0, 0.0, 0.0, 0.0, 0), K_STAT_ERR_BAD_HANDLE);
            assert_eq!((SUITE.set_nth_control_point)(bad, 0, 0.0, 0, 0.0, 0.0, 0), K_STAT_ERR_BAD_HANDLE);
            assert_eq!((SUITE.delete_control_point)(bad, 0, 0), K_STAT_ERR_BAD_HANDLE);
            assert_eq!((SUITE.delete_all_control_points)(bad, 0), K_STAT_ERR_BAD_HANDLE);
        }
        // A curve the parameter does not have.
        for curve in [-1, 1, 7, c_int::MAX] {
            assert_eq!(value(h, curve, 0.5), Err(K_STAT_ERR_BAD_INDEX), "{curve}");
            assert_eq!(count(h, curve), Err(K_STAT_ERR_BAD_INDEX));
            assert_eq!(nth(h, curve, 0), Err(K_STAT_ERR_BAD_INDEX));
            assert_eq!((SUITE.add_control_point)(h, curve, 0.0, 0.0, 0.0, 0), K_STAT_ERR_BAD_INDEX);
            assert_eq!((SUITE.delete_all_control_points)(h, curve), K_STAT_ERR_BAD_INDEX);
        }
        // A control point that does not exist.
        assert_eq!((SUITE.add_control_point)(h, 0, 0.0, 0.5, 0.5, 0), K_STAT_OK);
        for n in [-1, 1, 99] {
            assert_eq!(nth(h, 0, n), Err(K_STAT_ERR_BAD_INDEX), "{n}");
            assert_eq!((SUITE.set_nth_control_point)(h, 0, 0.0, n, 0.0, 0.0, 0), K_STAT_ERR_BAD_INDEX);
            assert_eq!((SUITE.delete_control_point)(h, 0, n), K_STAT_ERR_BAD_INDEX);
        }
        // Non-finite numbers and null out-pointers.
        assert_eq!((SUITE.add_control_point)(h, 0, 0.0, f64::NAN, 0.0, 0), K_STAT_ERR_VALUE);
        assert_eq!((SUITE.set_nth_control_point)(h, 0, 0.0, 0, 0.0, f64::INFINITY, 0), K_STAT_ERR_VALUE);
        assert_eq!(value(h, 0, f64::NAN), Err(K_STAT_ERR_VALUE));
        assert_eq!((SUITE.get_value)(h, 0, 0.0, 0.5, std::ptr::null_mut()), K_STAT_ERR_VALUE);
        assert_eq!((SUITE.get_n_control_points)(h, 0, 0.0, std::ptr::null_mut()), K_STAT_ERR_VALUE);
        assert_eq!((SUITE.get_nth_control_point)(h, 0, 0.0, 0, std::ptr::null_mut(), std::ptr::null_mut()), K_STAT_ERR_VALUE);
        // The one good point is still there.
        assert_eq!((count(h, 0), nth(h, 0, 0)), (Ok(1), Ok((0.5, 0.5))));
    }

    /// A plug-in that sets a huge dimension gets at most `MAX_CURVES` curves, and a dimension that is
    /// not a count at all has none.
    #[test]
    fn the_dimension_is_bounded() {
        let eff = EffectObj::new_descriptor();
        let h = param(&eff, "curve", i32::MAX);
        assert_eq!(count(h, MAX_CURVES as c_int - 1), Ok(0));
        assert_eq!(count(h, MAX_CURVES as c_int), Err(K_STAT_ERR_BAD_INDEX));
        let h = param(&eff, "none", -4);
        assert_eq!(count(h, 0), Err(K_STAT_ERR_BAD_INDEX));
    }

    /// The suite is served by name and version, and only as version 1.
    #[test]
    fn the_suite_is_fetched_as_version_1() {
        let host = unsafe { &*crate::suites::host_ptr() };
        let get = |v: c_int| (host.fetch_suite)(host.host, c"OfxParametricParameterSuite".as_ptr(), v);
        assert_eq!(get(1), &SUITE as *const OfxParametricParameterSuiteV1 as *const std::ffi::c_void);
        assert!(get(2).is_null());
    }
}

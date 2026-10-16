//! The parameter suite's value functions are C variadics (`paramGetValue(handle, ...)`): where the
//! arguments of a call land depends on the platform's ABI (Windows x64 slots, System V registers,
//! Apple Silicon's stack), and the host reads them through fixed-argument declarations (see the
//! `ffi` module docs).
//!
//! This test calls the six functions the way a C plug-in does: through function pointers typed as
//! real C variadics (stable Rust can *call* variadics, it only cannot define them), with doubles,
//! ints and pointers in the positions the OFX headers document. It runs on every platform, so each
//! ABI's declaration is exercised by the CI runner of that platform (a failure here on Apple
//! Silicon means the stack-slot declarations are wrong).

use std::ffi::{CStr, c_char, c_int};

use effectcraft_ofx::ffi::*;
use effectcraft_ofx::instance::EffectObj;
use effectcraft_ofx::params::PKind;
use effectcraft_ofx::suites::parameter::SUITE;

/// `paramGetValue(handle, ...)`, `paramSetValue(handle, ...)`.
type Var = unsafe extern "C" fn(Handle, ...) -> OfxStatus;
/// `paramGetValueAtTime(handle, time, ...)` and the other functions with one fixed `OfxTime`.
type VarT = unsafe extern "C" fn(Handle, f64, ...) -> OfxStatus;
/// `paramGetIntegral(handle, time1, time2, ...)`.
type VarT2 = unsafe extern "C" fn(Handle, f64, f64, ...) -> OfxStatus;

fn get_value() -> Var {
    // SAFETY: only the type changes: the suite entry is the host's implementation of the variadic
    // function, and a C caller reaches it through exactly this kind of pointer.
    unsafe { std::mem::transmute::<VarParamFn, Var>(SUITE.param_get_value) }
}
fn set_value() -> Var {
    // SAFETY: as above.
    unsafe { std::mem::transmute::<VarParamFn, Var>(SUITE.param_set_value) }
}
fn get_value_at_time() -> VarT {
    // SAFETY: as above.
    unsafe { std::mem::transmute::<VarParamTimeFn, VarT>(SUITE.param_get_value_at_time) }
}
fn set_value_at_time() -> VarT {
    // SAFETY: as above.
    unsafe { std::mem::transmute::<VarParamTimeFn, VarT>(SUITE.param_set_value_at_time) }
}
fn get_derivative() -> VarT {
    // SAFETY: as above.
    unsafe { std::mem::transmute::<VarParamTimeFn, VarT>(SUITE.param_get_derivative) }
}
fn get_integral() -> VarT2 {
    // SAFETY: as above.
    unsafe { std::mem::transmute::<VarParamTime2Fn, VarT2>(SUITE.param_get_integral) }
}

/// A parameter of `kind` in a descriptor (the descriptor must outlive the handle).
fn param(eff: &EffectObj, kind: PKind, name: &str) -> Handle {
    let p = eff.params.define(eff as *const EffectObj, kind, name, false).expect("define");
    // SAFETY: a stable pointer into the set, which lives as long as `eff`.
    unsafe { (*p).handle() }
}

const UNSET: f64 = -12345.0;

#[test]
fn values_round_trip_through_the_variadic_functions() {
    let eff = EffectObj::new_descriptor();
    let (set, get) = (set_value(), get_value());

    // A double.
    let h = param(&eff, PKind::Double, "d");
    let mut d = UNSET;
    unsafe {
        assert_eq!(set(h, 1.5f64), K_STAT_OK);
        assert_eq!(get(h, &mut d as *mut f64), K_STAT_OK);
    }
    assert_eq!(d, 1.5);

    // An int and a boolean (C `int` arguments).
    for (kind, name, v) in [(PKind::Int, "i", 42 as c_int), (PKind::Bool, "b", 1), (PKind::Choice, "c", 2)] {
        let h = param(&eff, kind, name);
        let mut out: c_int = -1;
        unsafe {
            assert_eq!(set(h, v), K_STAT_OK, "{name}");
            assert_eq!(get(h, &mut out as *mut c_int), K_STAT_OK, "{name}");
        }
        assert_eq!(out, v, "{name}");
    }

    // A colour: four doubles in, four pointers out (the longest argument lists there are).
    let h = param(&eff, PKind::Rgba, "rgba");
    let mut c = [UNSET; 4];
    unsafe {
        assert_eq!(set(h, 0.1f64, 0.2f64, 0.3f64, 0.4f64), K_STAT_OK);
        let [r, g, b, a] = &mut c;
        assert_eq!(get(h, r as *mut f64, g as *mut f64, b as *mut f64, a as *mut f64), K_STAT_OK);
    }
    assert_eq!(c, [0.1, 0.2, 0.3, 0.4]);

    // Three doubles and two ints.
    let h = param(&eff, PKind::Double3D, "d3");
    let mut v = [UNSET; 3];
    unsafe {
        assert_eq!(set(h, 1.0f64, 2.0f64, 3.0f64), K_STAT_OK);
        let [x, y, z] = &mut v;
        assert_eq!(get(h, x as *mut f64, y as *mut f64, z as *mut f64), K_STAT_OK);
    }
    assert_eq!(v, [1.0, 2.0, 3.0]);
    let h = param(&eff, PKind::Int2D, "i2");
    let (mut x, mut y): (c_int, c_int) = (-1, -1);
    unsafe {
        assert_eq!(set(h, 7 as c_int, -8 as c_int), K_STAT_OK);
        assert_eq!(get(h, &mut x as *mut c_int, &mut y as *mut c_int), K_STAT_OK);
    }
    assert_eq!((x, y), (7, -8));

    // A string (a pointer in, a pointer out).
    let h = param(&eff, PKind::Str, "s");
    let mut s: *const c_char = std::ptr::null();
    unsafe {
        assert_eq!(set(h, c"hello".as_ptr()), K_STAT_OK);
        assert_eq!(get(h, &mut s as *mut *const c_char), K_STAT_OK);
        assert_eq!(CStr::from_ptr(s).to_str().unwrap(), "hello");
    }
}

#[test]
fn the_time_functions_read_the_time_and_then_the_values() {
    let eff = EffectObj::new_descriptor();
    let (set_at, get_at) = (set_value_at_time(), get_value_at_time());

    let h = param(&eff, PKind::Double, "d");
    let mut d = UNSET;
    unsafe {
        assert_eq!(set_at(h, 3.0f64, 2.5f64), K_STAT_OK);
        assert_eq!(get_at(h, 3.0f64, &mut d as *mut f64), K_STAT_OK);
    }
    assert_eq!(d, 2.5);

    let h = param(&eff, PKind::Int, "i");
    let mut i: c_int = -1;
    unsafe {
        assert_eq!(set_at(h, 3.0f64, 9 as c_int), K_STAT_OK);
        assert_eq!(get_at(h, 4.0f64, &mut i as *mut c_int), K_STAT_OK);
    }
    assert_eq!(i, 9);

    // The colour: the time, then four values (set) or four pointers (get).
    let h = param(&eff, PKind::Rgba, "rgba");
    let mut c = [UNSET; 4];
    unsafe {
        assert_eq!(set_at(h, 3.0f64, 0.5f64, 0.25f64, 0.125f64, 1.0f64), K_STAT_OK);
        let [r, g, b, a] = &mut c;
        assert_eq!(get_at(h, 3.0f64, r as *mut f64, g as *mut f64, b as *mut f64, a as *mut f64), K_STAT_OK);
    }
    assert_eq!(c, [0.5, 0.25, 0.125, 1.0]);
}

#[test]
fn derivative_and_integral_take_their_times_before_the_out_pointers() {
    let eff = EffectObj::new_descriptor();
    let (set, deriv, integral) = (set_value(), get_derivative(), get_integral());

    // Outside a render the value is constant: no change, and the integral is value * duration.
    let h = param(&eff, PKind::Double, "d");
    let (mut dd, mut ii) = (UNSET, UNSET);
    unsafe {
        assert_eq!(set(h, 2.0f64), K_STAT_OK);
        assert_eq!(deriv(h, 10.0f64, &mut dd as *mut f64), K_STAT_OK);
        assert_eq!(integral(h, 1.0f64, 4.0f64, &mut ii as *mut f64), K_STAT_OK);
    }
    assert_eq!((dd, ii), (0.0, 6.0));

    // Four out-pointers after two times (the most any of them takes).
    let h = param(&eff, PKind::Rgba, "rgba");
    let (mut dc, mut ic) = ([UNSET; 4], [UNSET; 4]);
    unsafe {
        assert_eq!(set(h, 1.0f64, 0.5f64, 0.25f64, 2.0f64), K_STAT_OK);
        let [a, b, c, d] = &mut dc;
        assert_eq!(deriv(h, 10.0f64, a as *mut f64, b as *mut f64, c as *mut f64, d as *mut f64), K_STAT_OK);
        let [a, b, c, d] = &mut ic;
        assert_eq!(integral(h, 0.0f64, 2.0f64, a as *mut f64, b as *mut f64, c as *mut f64, d as *mut f64), K_STAT_OK);
    }
    assert_eq!(dc, [0.0; 4]);
    assert_eq!(ic, [2.0, 1.0, 0.5, 4.0]);
}

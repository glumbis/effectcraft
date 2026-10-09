//! `OfxParameterSuiteV1`: define parameters (descriptors), fetch their handles (instances) and
//! read / write values.
//!
//! The value functions are C variadic; see the module docs of [`crate::ffi`] for how the argument
//! slots are read. Reads during a render (`paramGetValueAtTime`) go to the host's animated values
//! through the render context; reads outside a render, and `paramGetValue`, return the
//! instance's stored value.
//!
//! Keyframe access reports "no keys": the host owns animation, the plug-in only ever sees the
//! evaluated value at a time.

use std::ffi::{c_char, c_int, c_void};

use crate::ffi::*;
use crate::instance::EffectObj;
use crate::params::{PKind, ParamObj};
use crate::props::{PVal, as_double, as_int};

extern "C" fn param_define(set: Handle, ty: *const c_char, name: *const c_char, props: *mut Handle) -> OfxStatus {
    guard(|| {
        // SAFETY: handles and strings come from the plug-in.
        let Some(eff) = (unsafe { EffectObj::from_handle(set) }) else { return K_STAT_ERR_BAD_HANDLE };
        let (Some(ty), Some(name)) = (unsafe { str_arg(ty) }, unsafe { str_arg(name) }) else { return K_STAT_ERR_UNKNOWN };
        let Some(kind) = PKind::from_type(ty) else { return K_STAT_ERR_UNKNOWN };
        match eff.params.define(eff as *const EffectObj, kind, name, eff.is_instance) {
            Ok(p) => {
                if !props.is_null() {
                    // SAFETY: `p` is a stable pointer into the set; `props` is an out-pointer.
                    unsafe { *props = (*p).props.as_handle() };
                }
                K_STAT_OK
            }
            Err(e) => e,
        }
    })
}

extern "C" fn param_get_handle(set: Handle, name: *const c_char, param: *mut Handle, props: *mut Handle) -> OfxStatus {
    guard(|| {
        // SAFETY: as above.
        let Some(eff) = (unsafe { EffectObj::from_handle(set) }) else { return K_STAT_ERR_BAD_HANDLE };
        let Some(name) = (unsafe { str_arg(name) }) else { return K_STAT_ERR_UNKNOWN };
        let Some(p) = eff.params.find(name) else { return K_STAT_ERR_UNKNOWN };
        // SAFETY: stable pointer into the set.
        let p = unsafe { &*p };
        if param.is_null() {
            return K_STAT_ERR_BAD_HANDLE;
        }
        unsafe {
            *param = p.handle();
            if !props.is_null() {
                *props = p.props.as_handle();
            }
        }
        K_STAT_OK
    })
}

extern "C" fn param_set_get_property_set(set: Handle, props: *mut Handle) -> OfxStatus {
    guard(|| {
        // SAFETY: as above.
        let Some(eff) = (unsafe { EffectObj::from_handle(set) }) else { return K_STAT_ERR_BAD_HANDLE };
        if props.is_null() {
            return K_STAT_ERR_BAD_HANDLE;
        }
        unsafe { *props = eff.props.as_handle() };
        K_STAT_OK
    })
}

extern "C" fn param_get_property_set(param: Handle, props: *mut Handle) -> OfxStatus {
    guard(|| {
        // SAFETY: as above.
        let Some(p) = (unsafe { ParamObj::from_handle(param) }) else { return K_STAT_ERR_BAD_HANDLE };
        if props.is_null() {
            return K_STAT_ERR_BAD_HANDLE;
        }
        unsafe { *props = p.props.as_handle() };
        K_STAT_OK
    })
}

/// Write `vals` to the pointers in `va`, as `paramGetValue` does for the parameter's type.
fn write_value(kind: PKind, vals: &[PVal], va: &mut VarArgs) -> OfxStatus {
    let dbl = |i: usize| vals.get(i).and_then(as_double).unwrap_or(0.0);
    let int = |i: usize| vals.get(i).and_then(as_int).unwrap_or(0);
    let n = match kind {
        PKind::Double | PKind::Int | PKind::Bool | PKind::Choice | PKind::Str | PKind::Custom | PKind::StrChoice => 1,
        PKind::Double2D | PKind::Int2D => 2,
        PKind::Double3D | PKind::Int3D | PKind::Rgb => 3,
        PKind::Rgba => 4,
        PKind::Bytes | PKind::Group | PKind::Page | PKind::Push => return K_STAT_ERR_UNSUPPORTED,
    };
    for i in 0..n {
        let p: *mut c_void = va.next_ptr();
        if p.is_null() {
            return K_STAT_ERR_BAD_HANDLE;
        }
        // SAFETY: the plug-in passes one valid out-pointer of the documented type per component.
        unsafe {
            match kind {
                PKind::Int | PKind::Bool | PKind::Choice | PKind::Int2D | PKind::Int3D => *(p as *mut c_int) = int(i),
                PKind::Str | PKind::Custom | PKind::StrChoice => {
                    *(p as *mut *const c_char) = match vals.first() {
                        Some(PVal::Str(s)) => s.as_ptr(),
                        _ => c"".as_ptr(),
                    };
                }
                _ => *(p as *mut f64) = dbl(i),
            }
        }
    }
    K_STAT_OK
}

fn get_value(h: Handle, mut va: VarArgs) -> OfxStatus {
    // SAFETY: handle from the plug-in.
    let Some(p) = (unsafe { ParamObj::from_handle(h) }) else { return K_STAT_ERR_BAD_HANDLE };
    // The lock is held while pointers into a string value are handed out, so a concurrent
    // `set` cannot free it mid-copy of the pointer; the plug-in copies the string itself.
    let guard = p.value.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    write_value(p.kind, &guard, &mut va)
}

fn get_value_at_time(h: Handle, mut va: VarArgs) -> OfxStatus {
    // SAFETY: handle from the plug-in.
    let Some(p) = (unsafe { ParamObj::from_handle(h) }) else { return K_STAT_ERR_BAD_HANDLE };
    let time = va.next_f64();
    // SAFETY: the owner outlives its parameters.
    let ctx = unsafe { p.owner.as_ref() }.and_then(EffectObj::current_render);
    if let Some(ctx) = ctx
        && ctx.has_host()
        && let Some(map) = ctx.params_at(time)
        && let Some(vals) = map.get(&p.name)
    {
        return write_value(p.kind, vals, &mut va);
    }
    let guard = p.value.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    write_value(p.kind, &guard, &mut va)
}

fn unsupported(_h: Handle, _va: VarArgs) -> OfxStatus {
    K_STAT_ERR_UNSUPPORTED
}

/// Read the new value from the argument slots and store it.
fn read_new_value(kind: PKind, va: &mut VarArgs) -> Option<Vec<PVal>> {
    Some(match kind {
        PKind::Double => vec![PVal::Double(va.next_f64())],
        PKind::Int | PKind::Bool | PKind::Choice => vec![PVal::Int(va.next_i32())],
        PKind::Double2D => (0..2).map(|_| PVal::Double(va.next_f64())).collect(),
        PKind::Double3D | PKind::Rgb => (0..3).map(|_| PVal::Double(va.next_f64())).collect(),
        PKind::Rgba => (0..4).map(|_| PVal::Double(va.next_f64())).collect(),
        PKind::Int2D => (0..2).map(|_| PVal::Int(va.next_i32())).collect(),
        PKind::Int3D => (0..3).map(|_| PVal::Int(va.next_i32())).collect(),
        PKind::Str | PKind::Custom | PKind::StrChoice => {
            let p: *const c_char = va.next_ptr();
            // SAFETY: a C string from the plug-in.
            vec![PVal::text(unsafe { str_arg(p) }.unwrap_or(""))]
        }
        PKind::Bytes | PKind::Group | PKind::Page | PKind::Push => return None,
    })
}

fn set_value(h: Handle, mut va: VarArgs) -> OfxStatus {
    // SAFETY: handle from the plug-in.
    let Some(p) = (unsafe { ParamObj::from_handle(h) }) else { return K_STAT_ERR_BAD_HANDLE };
    match read_new_value(p.kind, &mut va) {
        Some(v) => {
            p.set_values(v);
            K_STAT_OK
        }
        None => K_STAT_ERR_UNSUPPORTED,
    }
}

fn set_value_at_time(h: Handle, mut va: VarArgs) -> OfxStatus {
    // SAFETY: handle from the plug-in.
    let Some(p) = (unsafe { ParamObj::from_handle(h) }) else { return K_STAT_ERR_BAD_HANDLE };
    let _time = va.next_f64();
    match read_new_value(p.kind, &mut va) {
        Some(v) => {
            p.set_values(v);
            K_STAT_OK
        }
        None => K_STAT_ERR_UNSUPPORTED,
    }
}

/// Declare a variadic suite function as the fixed-slot function the C caller's arguments land
/// in (see the `ffi` module docs).
macro_rules! var_fn {
    ($name:ident, $body:path) => {
        #[cfg(windows)]
        extern "C" fn $name(h: Handle, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64, a6: u64) -> OfxStatus {
            let va = VarArgs::new([a1, a2, a3, a4, a5, a6]);
            guard(move || $body(h, va))
        }
        #[cfg(not(windows))]
        extern "C" fn $name(h: Handle, i1: u64, i2: u64, i3: u64, i4: u64, i5: u64, f1: f64, f2: f64, f3: f64, f4: f64, f5: f64) -> OfxStatus {
            let va = VarArgs::new([i1, i2, i3, i4, i5], [f1, f2, f3, f4, f5]);
            guard(move || $body(h, va))
        }
    };
}

var_fn!(param_get_value, get_value);
var_fn!(param_get_value_at_time, get_value_at_time);
var_fn!(param_get_derivative, unsupported);
var_fn!(param_get_integral, unsupported);
var_fn!(param_set_value, set_value);
var_fn!(param_set_value_at_time, set_value_at_time);

extern "C" fn param_get_num_keys(h: Handle, n: *mut u32) -> OfxStatus {
    guard(|| {
        // SAFETY: handle from the plug-in.
        if unsafe { ParamObj::from_handle(h) }.is_none() {
            return K_STAT_ERR_BAD_HANDLE;
        }
        if !n.is_null() {
            unsafe { *n = 0 };
        }
        K_STAT_OK
    })
}
extern "C" fn param_get_key_time(_h: Handle, _n: u32, _t: *mut f64) -> OfxStatus {
    K_STAT_ERR_BAD_INDEX
}
extern "C" fn param_get_key_index(_h: Handle, _t: f64, _dir: c_int, _out: *mut c_int) -> OfxStatus {
    K_STAT_FAILED
}
extern "C" fn param_delete_key(_h: Handle, _t: f64) -> OfxStatus {
    K_STAT_OK
}
extern "C" fn param_delete_all_keys(_h: Handle) -> OfxStatus {
    K_STAT_OK
}
extern "C" fn param_copy(to: Handle, from: Handle, _offset: f64, _range: *const OfxRangeD) -> OfxStatus {
    guard(|| {
        // SAFETY: handles from the plug-in.
        let (Some(a), Some(b)) = (unsafe { ParamObj::from_handle(to) }, unsafe { ParamObj::from_handle(from) }) else { return K_STAT_ERR_BAD_HANDLE };
        if a.kind != b.kind {
            return K_STAT_ERR_BAD_HANDLE;
        }
        a.set_values(b.values());
        K_STAT_OK
    })
}
extern "C" fn param_edit_begin(_set: Handle, _name: *const c_char) -> OfxStatus {
    K_STAT_OK
}
extern "C" fn param_edit_end(_set: Handle) -> OfxStatus {
    K_STAT_OK
}

/// The parameter suite table.
pub static SUITE: OfxParameterSuiteV1 = OfxParameterSuiteV1 {
    param_define,
    param_get_handle,
    param_set_get_property_set,
    param_get_property_set,
    param_get_value,
    param_get_value_at_time,
    param_get_derivative,
    param_get_integral,
    param_set_value,
    param_set_value_at_time,
    param_get_num_keys,
    param_get_key_time,
    param_get_key_index,
    param_delete_key,
    param_delete_all_keys,
    param_copy,
    param_edit_begin,
    param_edit_end,
};

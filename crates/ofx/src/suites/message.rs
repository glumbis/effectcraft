//! `OfxMessageSuiteV1` / `V2`: plug-in messages go to the `log` crate (and, for errors, to the
//! effect so a failed render can report the plug-in's own explanation).
//!
//! The suite's functions are printf-style variadics, which stable Rust cannot receive; only the
//! fixed arguments are read, so a format string with `%` conversions is logged unexpanded.

use std::ffi::c_char;

use crate::ffi::*;
use crate::instance::EffectObj;

fn post(handle: Handle, kind: *const c_char, format: *const c_char, persistent: bool) -> OfxStatus {
    guard(|| {
        // SAFETY: strings come from the plug-in.
        let kind = unsafe { str_arg(kind) }.unwrap_or(MSG_MESSAGE);
        let text = unsafe { str_arg(format) }.unwrap_or("");
        match kind {
            MSG_FATAL | MSG_ERROR => log::error!("openfx plug-in: {text}"),
            MSG_WARNING => log::warn!("openfx plug-in: {text}"),
            MSG_LOG => log::debug!("openfx plug-in: {text}"),
            _ => log::info!("openfx plug-in: {text}"),
        }
        if (persistent || kind == MSG_ERROR || kind == MSG_FATAL)
            // SAFETY: effect handle or null.
            && let Some(e) = unsafe { EffectObj::from_handle(handle) }
        {
            e.set_message(text);
        }
        if kind == MSG_QUESTION { K_STAT_REPLY_YES } else { K_STAT_OK }
    })
}

extern "C" fn message(handle: Handle, kind: *const c_char, _id: *const c_char, format: *const c_char) -> OfxStatus {
    post(handle, kind, format, false)
}

extern "C" fn set_persistent_message(handle: Handle, kind: *const c_char, _id: *const c_char, format: *const c_char) -> OfxStatus {
    post(handle, kind, format, true)
}

extern "C" fn clear_persistent_message(handle: Handle) -> OfxStatus {
    guard(|| {
        // SAFETY: effect handle or null.
        if let Some(e) = unsafe { EffectObj::from_handle(handle) } {
            e.set_message("");
        }
        K_STAT_OK
    })
}

/// Message suite V1.
pub static SUITE_V1: OfxMessageSuiteV1 = OfxMessageSuiteV1 { message };

/// Message suite V2.
pub static SUITE_V2: OfxMessageSuiteV2 = OfxMessageSuiteV2 { message, set_persistent_message, clear_persistent_message };

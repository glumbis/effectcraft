//! Clips and images: the clip objects plug-ins define and query, the OFX image handles returned
//! by `clipGetImage`, and the conversion between EffectCraft's premultiplied RGBA `f32` rows
//! (row 0 = top) and OFX pixel buffers (any depth, RGBA / RGB / alpha, rows bottom-up).
//!
//! OFX images are y-up. Instead of handing out negative row bytes (some plug-ins mishandle
//! them), the conversion flips the rows: OFX row 0 is EffectCraft's bottom row and `rowBytes`
//! is positive.
//!
//! Image buffers start on a 64-byte boundary and every row starts on one too: `rowBytes` is the
//! row's size rounded up to a multiple of 64 (an RGB float row of 5 pixels is 60 bytes of pixels
//! in a 64-byte row), as plug-ins that load rows with aligned SIMD instructions expect. Every
//! conversion goes through `rowBytes`, never through width times pixel size. Freed buffers are
//! kept for the next image of the same size (the frames of one render are all the same size), so
//! a render does not ask the operating system for tens of megabytes and have it zeroed again for
//! every effect.

use std::alloc::{Layout, alloc_zeroed, dealloc};
use std::collections::HashSet;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use serde::{Deserialize, Serialize};

use crate::diag::PropKind;
use crate::ffi::*;
use crate::instance::EffectObj;
use crate::props::PropSet;

/// The marker at the start of every [`ClipObj`].
pub const CLIP_MAGIC: u32 = 0x4F46_5843; // "OFXC"

/// A clip of an effect descriptor or instance.
#[repr(C)]
pub struct ClipObj {
    magic: u32,
    pub name: String,
    pub props: PropSet,
    pub owner: *const EffectObj,
}

unsafe impl Send for ClipObj {}
unsafe impl Sync for ClipObj {}

impl ClipObj {
    pub fn new(name: &str, owner: *const EffectObj) -> ClipObj {
        let props = PropSet::with_kind(PropKind::Clip);
        props.put_str(P_TYPE, TYPE_CLIP);
        props.put_str(P_NAME, name);
        props.put_str(P_LABEL, name);
        props.put_str(P_SHORT_LABEL, name);
        props.put_str(P_LONG_LABEL, name);
        props.put_int(CLIP_OPTIONAL, 0);
        props.put_int(CLIP_IS_MASK, 0);
        props.put_int(IE_TEMPORAL_CLIP_ACCESS, 0);
        props.put_int(IE_SUPPORTS_TILES, 1);
        props.put(IE_SUPPORTED_COMPONENTS, vec![]);
        props.put_str(CLIP_FIELD_EXTRACTION, "OfxImageFieldDoubled");
        ClipObj { magic: CLIP_MAGIC, name: name.to_string(), props, owner }
    }

    /// An instance's copy of a descriptor clip.
    pub fn instantiate(&self, owner: *const EffectObj) -> ClipObj {
        ClipObj { magic: CLIP_MAGIC, name: self.name.clone(), props: self.props.clone(), owner }
    }

    /// Validate a raw handle.
    ///
    /// # Safety
    /// `h` must be null or point to readable memory; the reference is only used during a callback.
    pub unsafe fn from_handle<'a>(h: *mut c_void) -> Option<&'a ClipObj> {
        if h.is_null() || unsafe { std::ptr::read(h as *const u32) } != CLIP_MAGIC {
            return None;
        }
        Some(unsafe { &*(h as *const ClipObj) })
    }

    pub fn handle(&self) -> *mut c_void {
        self as *const ClipObj as *mut c_void
    }
}

/// OFX pixel depths this host can produce.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Depth {
    Byte,
    Short,
    Float,
}

impl Depth {
    pub fn name(self) -> &'static str {
        match self {
            Depth::Byte => DEPTH_BYTE,
            Depth::Short => DEPTH_SHORT,
            Depth::Float => DEPTH_FLOAT,
        }
    }
    pub fn from_name(s: &str) -> Option<Depth> {
        match s {
            DEPTH_BYTE => Some(Depth::Byte),
            DEPTH_SHORT => Some(Depth::Short),
            DEPTH_FLOAT => Some(Depth::Float),
            _ => None,
        }
    }
    pub fn bytes(self) -> usize {
        match self {
            Depth::Byte => 1,
            Depth::Short => 2,
            Depth::Float => 4,
        }
    }
}

/// OFX pixel components this host can produce.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Comps {
    Rgba,
    Rgb,
    Alpha,
}

impl Comps {
    pub fn name(self) -> &'static str {
        match self {
            Comps::Rgba => COMP_RGBA,
            Comps::Rgb => COMP_RGB,
            Comps::Alpha => COMP_ALPHA,
        }
    }
    pub fn from_name(s: &str) -> Option<Comps> {
        match s {
            COMP_RGBA => Some(Comps::Rgba),
            COMP_RGB => Some(Comps::Rgb),
            COMP_ALPHA => Some(Comps::Alpha),
            _ => None,
        }
    }
    pub fn channels(self) -> usize {
        match self {
            Comps::Rgba => 4,
            Comps::Rgb => 3,
            Comps::Alpha => 1,
        }
    }
}

/// Buffers and rows are aligned to this many bytes.
pub const IMAGE_ALIGN: usize = 64;

/// The most freed buffers kept for reuse, and the most bytes they hold together.
const MAX_SPARE: usize = 12;
const MAX_SPARE_BYTES: usize = 384 << 20;

/// A freed buffer: its address and size (a multiple of [`IMAGE_ALIGN`]).
struct Spare(NonNull<u8>, usize);

// The pointer is owned by the pool until it is handed out again.
unsafe impl Send for Spare {}

fn spare_buffers() -> &'static Mutex<Vec<Spare>> {
    static S: OnceLock<Mutex<Vec<Spare>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Vec::new()))
}

/// Give back the memory of the buffers kept for reuse (a plug-in ran out of memory, the app quits).
pub fn release_spare_buffers() {
    let all: Vec<Spare> = std::mem::take(&mut *spare_buffers().lock().unwrap_or_else(PoisonError::into_inner));
    for Spare(ptr, cap) in all {
        if let Ok(layout) = Layout::from_size_align(cap, IMAGE_ALIGN) {
            // SAFETY: allocated with this layout by `PixBuf::zeroed` and not used since.
            unsafe { dealloc(ptr.as_ptr(), layout) };
        }
    }
}

/// A 64-byte-aligned pixel buffer the plug-in reads (and, for the output image, writes) through
/// a raw pointer while the host keeps it alive.
pub struct PixBuf {
    ptr: NonNull<u8>,
    /// The bytes of the image.
    bytes: usize,
    /// The bytes allocated: `bytes` rounded up to a multiple of [`IMAGE_ALIGN`] (at least one unit).
    cap: usize,
}

// The buffer is only written by the plug-in's render (one effect instance at a time owns the
// output image) or by the host before / after the plug-in runs.
unsafe impl Sync for PixBuf {}
unsafe impl Send for PixBuf {}

impl PixBuf {
    /// A zeroed buffer of `bytes` bytes, from the ones kept for reuse if one of that size is
    /// there. `None` when the memory is not there (or the size is absurd): a huge layer is an
    /// error, not an abort.
    pub fn zeroed(bytes: usize) -> Option<PixBuf> {
        let cap = bytes.max(1).checked_next_multiple_of(IMAGE_ALIGN)?;
        let spare = {
            let mut s = spare_buffers().lock().unwrap_or_else(PoisonError::into_inner);
            s.iter().position(|b| b.1 == cap).map(|i| s.swap_remove(i))
        };
        if let Some(Spare(ptr, _)) = spare {
            // SAFETY: `cap` bytes owned by this buffer now.
            unsafe { std::ptr::write_bytes(ptr.as_ptr(), 0, cap) };
            return Some(PixBuf { ptr, bytes, cap });
        }
        let layout = Layout::from_size_align(cap, IMAGE_ALIGN).ok()?;
        // SAFETY: a non-zero size with a valid layout.
        let ptr = NonNull::new(unsafe { alloc_zeroed(layout) })?;
        Some(PixBuf { ptr, bytes, cap })
    }
    pub fn ptr(&self) -> *mut u8 {
        self.ptr.as_ptr()
    }
    /// # Safety
    /// No plug-in may be running that holds this buffer.
    // The bytes are shared with the plug-in; exclusivity is the caller's contract above.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn bytes_mut(&self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr(), self.bytes) }
    }
    /// # Safety
    /// No plug-in may be writing this buffer.
    pub unsafe fn bytes_ref(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr(), self.bytes) }
    }
}

impl Drop for PixBuf {
    fn drop(&mut self) {
        {
            let mut spare = spare_buffers().lock().unwrap_or_else(PoisonError::into_inner);
            let held: usize = spare.iter().map(|b| b.1).sum();
            if spare.len() < MAX_SPARE && held + self.cap <= MAX_SPARE_BYTES {
                spare.push(Spare(self.ptr, self.cap));
                return;
            }
        }
        if let Ok(layout) = Layout::from_size_align(self.cap, IMAGE_ALIGN) {
            // SAFETY: allocated with this layout by `PixBuf::zeroed`; this buffer is going away.
            unsafe { dealloc(self.ptr.as_ptr(), layout) };
        }
    }
}

/// One image in the plug-in's pixel format, plus its geometry in OFX pixel coordinates.
pub struct ClipImage {
    pub buf: PixBuf,
    /// The addressable pixels (y up, x2/y2 exclusive).
    pub bounds: OfxRectI,
    /// The region of definition.
    pub rod: OfxRectI,
    /// The distance between rows in bytes: the row's pixels rounded up to [`IMAGE_ALIGN`].
    pub row_bytes: i32,
    pub depth: Depth,
    pub comps: Comps,
    pub premult: &'static str,
}

impl ClipImage {
    pub fn width(&self) -> usize {
        (self.bounds.x2 - self.bounds.x1).max(0) as usize
    }
    pub fn height(&self) -> usize {
        (self.bounds.y2 - self.bounds.y1).max(0) as usize
    }
    pub fn pixel_bytes(&self) -> usize {
        self.depth.bytes() * self.comps.channels()
    }

    /// A transparent image covering `bounds`; `None` if it does not fit in memory.
    pub fn blank(bounds: OfxRectI, rod: OfxRectI, depth: Depth, comps: Comps) -> Option<ClipImage> {
        let w = (bounds.x2 as i64 - bounds.x1 as i64).max(0) as usize;
        let h = (bounds.y2 as i64 - bounds.y1 as i64).max(0) as usize;
        let px = depth.bytes() * comps.channels();
        // Rows start on aligned addresses; `rowBytes` is an int in the plug-in's world.
        let row = w.checked_mul(px)?.checked_next_multiple_of(IMAGE_ALIGN)?;
        let row_bytes = i32::try_from(row).ok()?;
        let buf = PixBuf::zeroed(row.checked_mul(h)?)?;
        Some(ClipImage { buf, bounds, rod, row_bytes, depth, comps, premult: IMAGE_PREMULTIPLIED })
    }

    /// Convert the sub-rectangle `[x0, x1) x [y0, y1)` of an EffectCraft buffer (`sw` pixels per
    /// row, row 0 = top) into an OFX image with the given geometry. `None` if it does not fit in
    /// memory.
    pub fn from_rows(
        src: &[[f32; 4]],
        sw: usize,
        crop: (usize, usize, usize, usize),
        bounds: OfxRectI,
        rod: OfxRectI,
        depth: Depth,
        comps: Comps,
    ) -> Option<ClipImage> {
        let (x0, y0, x1, y1) = crop;
        let w = x1.saturating_sub(x0);
        let h = y1.saturating_sub(y0);
        let img = ClipImage::blank(bounds, rod, depth, comps)?;
        if img.width() != w || img.height() != h {
            return Some(img);
        }
        let px = img.pixel_bytes();
        let row_bytes = img.row_bytes as usize;
        // SAFETY: freshly created, not shared yet.
        let out = unsafe { img.buf.bytes_mut() };
        for j in 0..h {
            // OFX row j (from the bottom) is EffectCraft row y1 - 1 - j.
            let sy = y1 - 1 - j;
            let Some(row) = src.get(sy * sw + x0..sy * sw + x0 + w) else { continue };
            let Some(dst) = out.get_mut(j * row_bytes..j * row_bytes + w * px) else { continue };
            for (p, d) in row.iter().zip(dst.chunks_exact_mut(px)) {
                encode_pixel(*p, depth, comps, d);
            }
        }
        Some(img)
    }

    /// Write the image back into an EffectCraft buffer of exactly the image's size. Non-finite
    /// values become 0; `premultiply` converts straight-alpha output to premultiplied.
    pub fn write_rows(&self, dst: &mut [[f32; 4]], premultiply: bool) {
        let (w, h) = (self.width(), self.height());
        if dst.len() != w * h || w == 0 {
            return;
        }
        let px = self.pixel_bytes();
        let row_bytes = self.row_bytes as usize;
        // SAFETY: the plug-in's render has returned.
        let src = unsafe { self.buf.bytes_ref() };
        for j in 0..h {
            let dy = h - 1 - j;
            let Some(row) = src.get(j * row_bytes..j * row_bytes + w * px) else { continue };
            let Some(out) = dst.get_mut(dy * w..dy * w + w) else { continue };
            for (s, d) in row.chunks_exact(px).zip(out.iter_mut()) {
                let mut p = decode_pixel(s, self.depth, self.comps);
                if premultiply {
                    p[0] *= p[3];
                    p[1] *= p[3];
                    p[2] *= p[3];
                }
                for c in &mut p {
                    if !c.is_finite() {
                        *c = 0.0;
                    }
                }
                *d = p;
            }
        }
    }
}

fn encode_pixel(p: [f32; 4], depth: Depth, comps: Comps, out: &mut [u8]) {
    let vals: [f32; 4] = match comps {
        Comps::Rgba => p,
        Comps::Rgb => [p[0], p[1], p[2], 0.0],
        Comps::Alpha => [p[3], 0.0, 0.0, 0.0],
    };
    let n = comps.channels();
    for (i, v) in vals.iter().take(n).enumerate() {
        match depth {
            Depth::Float => {
                if let Some(o) = out.get_mut(i * 4..i * 4 + 4) {
                    o.copy_from_slice(&v.to_ne_bytes());
                }
            }
            Depth::Short => {
                let q = (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16;
                if let Some(o) = out.get_mut(i * 2..i * 2 + 2) {
                    o.copy_from_slice(&q.to_ne_bytes());
                }
            }
            Depth::Byte => {
                if let Some(o) = out.get_mut(i) {
                    *o = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                }
            }
        }
    }
}

fn decode_pixel(s: &[u8], depth: Depth, comps: Comps) -> [f32; 4] {
    let n = comps.channels();
    let mut v = [0.0f32; 4];
    for (i, slot) in v.iter_mut().take(n).enumerate() {
        *slot = match depth {
            Depth::Float => s.get(i * 4..i * 4 + 4).map(|b| f32::from_ne_bytes([b[0], b[1], b[2], b[3]])).unwrap_or(0.0),
            Depth::Short => s.get(i * 2..i * 2 + 2).map(|b| u16::from_ne_bytes([b[0], b[1]]) as f32 / 65535.0).unwrap_or(0.0),
            Depth::Byte => s.get(i).map(|b| *b as f32 / 255.0).unwrap_or(0.0),
        };
    }
    match comps {
        Comps::Rgba => v,
        Comps::Rgb => [v[0], v[1], v[2], 1.0],
        Comps::Alpha => [0.0, 0.0, 0.0, v[0]],
    }
}

// ---------------------------------------------------------------------------------------------
// Image handles
// ---------------------------------------------------------------------------------------------

/// The object behind the property-set handle `clipGetImage` returns (props first, so the
/// handle is also a valid `PropSet` pointer).
#[repr(C)]
pub struct ImageObj {
    props: PropSet,
    _img: Arc<ClipImage>,
}

fn live_images() -> &'static Mutex<HashSet<usize>> {
    static L: OnceLock<Mutex<HashSet<usize>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Hand an image to the plug-in; the handle stays valid until [`release_image`].
pub fn new_image_handle(img: &Arc<ClipImage>, render_scale: f64, unique: u64) -> *mut c_void {
    let props = PropSet::with_kind(PropKind::Image);
    props.put_str(P_TYPE, TYPE_IMAGE);
    props.put_ptr(IMG_DATA, img.buf.ptr() as usize);
    props.put_ints(IMG_BOUNDS, &[img.bounds.x1, img.bounds.y1, img.bounds.x2, img.bounds.y2]);
    props.put_ints(IMG_ROD, &[img.rod.x1, img.rod.y1, img.rod.x2, img.rod.y2]);
    props.put_int(IMG_ROW_BYTES, img.row_bytes);
    props.put_str(IMG_FIELD, FIELD_NONE);
    props.put_str(IMG_UNIQUE_ID, &format!("effectcraft-{unique}"));
    props.put_double(IMG_PIXEL_ASPECT_RATIO, 1.0);
    props.put_str(IE_PIXEL_DEPTH, img.depth.name());
    props.put_str(IE_COMPONENTS, img.comps.name());
    props.put_str(IE_PREMULTIPLICATION, img.premult);
    props.put_doubles(IE_RENDER_SCALE, &[render_scale, render_scale]);
    let obj = Box::new(ImageObj { props, _img: img.clone() });
    let ptr = Box::into_raw(obj) as usize;
    live_images().lock().unwrap_or_else(PoisonError::into_inner).insert(ptr);
    ptr as *mut c_void
}

/// Release a handle from [`new_image_handle`].
pub fn release_image(h: *mut c_void) -> OfxStatus {
    let ptr = h as usize;
    if ptr == 0 || !live_images().lock().unwrap_or_else(PoisonError::into_inner).remove(&ptr) {
        return K_STAT_ERR_BAD_HANDLE;
    }
    // SAFETY: the pointer was produced by Box::into_raw in `new_image_handle` and removed from
    // the live set exactly once.
    drop(unsafe { Box::from_raw(ptr as *mut ImageObj) });
    K_STAT_OK
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(w: i32, h: i32) -> OfxRectI {
        OfxRectI { x1: 0, y1: 0, x2: w, y2: h }
    }

    /// Buffers and rows start on 64-byte boundaries whatever the size, depth and components, and
    /// `rowBytes` covers the pixels of a row.
    #[test]
    fn buffers_and_rows_are_64_byte_aligned() {
        for (w, h) in [(1, 1), (3, 2), (5, 3), (16, 4), (17, 1), (1920, 2)] {
            for depth in [Depth::Byte, Depth::Short, Depth::Float] {
                for comps in [Comps::Rgba, Comps::Rgb, Comps::Alpha] {
                    let img = ClipImage::blank(rect(w, h), rect(w, h), depth, comps).unwrap();
                    assert_eq!(img.buf.ptr() as usize % IMAGE_ALIGN, 0, "{w}x{h} {depth:?} {comps:?}");
                    let (stride, pixels) = (img.row_bytes as usize, w as usize * img.pixel_bytes());
                    assert!(
                        stride >= pixels && stride % IMAGE_ALIGN == 0 && stride - pixels < IMAGE_ALIGN,
                        "{w}x{h} {depth:?} {comps:?}: {stride} for {pixels}"
                    );
                    // SAFETY: nothing else holds the image.
                    assert_eq!(unsafe { img.buf.bytes_ref() }.len(), stride * h as usize);
                }
            }
        }
    }

    /// Rows that are padded for alignment convert back to what went in, for every pixel format.
    #[test]
    fn padded_rows_round_trip() {
        // 5 pixels wide: an RGB float row is 60 bytes in a 64-byte row, an alpha byte row 5 in 64.
        let (w, h) = (5usize, 3usize);
        let src: Vec<[f32; 4]> = (0..w * h).map(|i| [i as f32 / 16.0, 0.25 + i as f32 / 32.0, 0.5, 1.0 - i as f32 / 32.0]).collect();
        for depth in [Depth::Float, Depth::Short, Depth::Byte] {
            for comps in [Comps::Rgba, Comps::Rgb, Comps::Alpha] {
                let img = ClipImage::from_rows(&src, w, (0, 0, w, h), rect(w as i32, h as i32), rect(w as i32, h as i32), depth, comps).unwrap();
                let mut back = vec![[0.0f32; 4]; w * h];
                img.write_rows(&mut back, false);
                let tol = match depth {
                    Depth::Float => 1e-6,
                    Depth::Short => 1e-4,
                    Depth::Byte => 3e-3,
                };
                for (a, b) in src.iter().zip(&back) {
                    let want = match comps {
                        Comps::Rgba => *a,
                        Comps::Rgb => [a[0], a[1], a[2], 1.0],
                        Comps::Alpha => [0.0, 0.0, 0.0, a[3]],
                    };
                    assert!(want.iter().zip(b).all(|(x, y)| (x - y).abs() <= tol), "{depth:?} {comps:?}: {want:?} vs {b:?}");
                }
            }
        }
    }

    /// OFX row 0 is the bottom row, and the padding bytes between rows are never read as pixels.
    #[test]
    fn rows_are_flipped_and_padding_is_skipped() {
        let (w, h) = (3usize, 2usize);
        let src = vec![[1.0f32, 0.0, 0.0, 1.0]; 3].into_iter().chain(vec![[0.0f32, 1.0, 0.0, 1.0]; 3]).collect::<Vec<_>>();
        let img = ClipImage::from_rows(&src, w, (0, 0, w, h), rect(3, 2), rect(3, 2), Depth::Float, Comps::Rgba).unwrap();
        let stride = img.row_bytes as usize;
        // SAFETY: nothing else holds the image.
        let bytes = unsafe { img.buf.bytes_ref() };
        let px =
            |row: usize, x: usize| -> [f32; 4] { std::array::from_fn(|c| f32::from_ne_bytes(bytes[row * stride + x * 16 + c * 4..][..4].try_into().unwrap())) };
        // The top EffectCraft row (red) is OFX row 1.
        assert_eq!((px(0, 0), px(1, 2)), ([0.0, 1.0, 0.0, 1.0], [1.0, 0.0, 0.0, 1.0]));
        // Padding bytes (48..64 of each row) stay zero, and filling them changes nothing coming back.
        assert!(bytes[w * 16..stride].iter().all(|b| *b == 0));
        let mut back = vec![[0.0f32; 4]; w * h];
        img.write_rows(&mut back, false);
        assert_eq!(back, src);
    }

    /// A buffer that comes back from the pool is zero again, and its size matters.
    #[test]
    fn reused_buffers_are_zeroed() {
        // A size no other test uses, so the buffer that is freed is the one that is handed out.
        let bytes = 1_000_000 + 64 * 7;
        let a = PixBuf::zeroed(bytes).unwrap();
        // SAFETY: nothing else holds the buffer.
        unsafe { a.bytes_mut() }.fill(0xAB);
        drop(a);
        let b = PixBuf::zeroed(bytes).unwrap();
        // SAFETY: nothing else holds the buffer.
        assert!(unsafe { b.bytes_ref() }.iter().all(|v| *v == 0), "a reused buffer must be cleared");
        assert_eq!(b.ptr() as usize % IMAGE_ALIGN, 0);
        // A different size is a different buffer, and the pool can be emptied.
        let c = PixBuf::zeroed(bytes + 4096).unwrap();
        assert_ne!(c.ptr(), b.ptr());
        drop((b, c));
        release_spare_buffers();
    }

    /// An absurd layer is a failed allocation, not an abort or an overflow.
    #[test]
    fn absurd_images_fail_cleanly() {
        let huge = OfxRectI { x1: i32::MIN / 2, y1: 0, x2: i32::MAX / 2, y2: 4 };
        assert!(ClipImage::blank(huge, huge, Depth::Float, Comps::Rgba).is_none());
        assert!(ClipImage::from_rows(&[], 0, (0, 0, 0, 0), huge, huge, Depth::Float, Comps::Rgba).is_none());
        assert!(PixBuf::zeroed(usize::MAX).is_none());
        // Empty images are fine.
        let empty = ClipImage::blank(rect(0, 0), rect(0, 0), Depth::Float, Comps::Rgba).unwrap();
        assert_eq!((empty.width(), empty.height(), empty.row_bytes), (0, 0, 0));
    }
}

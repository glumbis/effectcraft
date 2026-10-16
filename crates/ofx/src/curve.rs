//! The curves of parametric parameters (`OfxParamTypeParametric`).
//!
//! A parametric parameter is a function a plug-in asks the host to evaluate at a position (a look-up
//! table, a tone curve): one curve per dimension, each a list of control points `(key, value)` kept
//! in key order. `ofxParametricParam.h` leaves the shape between control points to the host ("a
//! host would probably represent this to a user as a cubic curve"); this host interpolates
//! **piecewise linearly**, holds the first value before the first control point and the last value
//! after the last one, and treats a curve with no control points as the identity (the header: "the
//! default default value of a parametric curve is to be an identity lookup").
//!
//! The control points come from the plug-in: it adds them to the *descriptor's* parameter while it
//! describes itself (`parametricParamAddControlPoint`), and every instance starts as a copy of that
//! ([`crate::params::ParamSet::instantiate_into`]). EffectCraft has no curve editor, so nothing else
//! changes them but the plug-in itself, and the curves are not part of a project: they are made again
//! from the plug-in's own description whenever the plug-in is described (also when a project is
//! opened), so saving and loading loses nothing the plug-in needs.

use crate::ffi::{K_STAT_ERR_BAD_INDEX, K_STAT_ERR_MEMORY, K_STAT_ERR_VALUE, K_STAT_OK, OfxStatus};

/// The most control points on one curve.
pub const MAX_POINTS: usize = 4096;
/// The most curves (`kOfxParamPropParametricDimension`) one parametric parameter may have.
pub const MAX_CURVES: usize = 256;

/// One curve: control points `[key, value]`, sorted by key, all finite.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Curve {
    points: Vec<[f64; 2]>,
}

impl Curve {
    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// The `n`-th control point as `(key, value)`.
    pub fn point(&self, n: usize) -> Option<(f64, f64)> {
        self.points.get(n).map(|p| (p[0], p[1]))
    }

    pub fn clear(&mut self) {
        self.points.clear();
    }

    /// Add a control point. A point whose key is within `merge` of an existing one replaces it
    /// ("if a key exists sufficiently close to 'key', then it will be set to the indicated control
    /// point"). A non-finite key or value is `kOfxStatErrValue`, a full curve `kOfxStatErrMemory`.
    pub fn add(&mut self, key: f64, value: f64, merge: f64) -> OfxStatus {
        if !(key.is_finite() && value.is_finite()) {
            return K_STAT_ERR_VALUE;
        }
        // The first point whose key is not smaller, and the one before it, are the neighbours.
        let after = self.points.partition_point(|p| p[0] < key);
        let distance = |i: usize| self.points.get(i).map(|p| (p[0] - key).abs());
        let near = [after.checked_sub(1), Some(after)]
            .into_iter()
            .flatten()
            .filter_map(|i| distance(i).filter(|d| *d <= merge).map(|d| (i, d)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i);
        if let Some(i) = near {
            if let Some(p) = self.points.get_mut(i) {
                *p = [key, value];
            }
            self.sort();
            return K_STAT_OK;
        }
        if self.points.len() >= MAX_POINTS {
            return K_STAT_ERR_MEMORY;
        }
        self.points.insert(after, [key, value]);
        K_STAT_OK
    }

    /// Change the `n`-th control point; the order follows the new key.
    pub fn set_nth(&mut self, n: usize, key: f64, value: f64) -> OfxStatus {
        if !(key.is_finite() && value.is_finite()) {
            return K_STAT_ERR_VALUE;
        }
        let Some(p) = self.points.get_mut(n) else { return K_STAT_ERR_BAD_INDEX };
        *p = [key, value];
        self.sort();
        K_STAT_OK
    }

    /// Delete the `n`-th control point.
    pub fn delete_nth(&mut self, n: usize) -> OfxStatus {
        if n >= self.points.len() {
            return K_STAT_ERR_BAD_INDEX;
        }
        self.points.remove(n);
        K_STAT_OK
    }

    fn sort(&mut self) {
        self.points.sort_by(|a, b| a[0].total_cmp(&b[0]));
    }

    /// The curve's value at `x`: piecewise linear between control points, held outside them, `x`
    /// itself when there are none (the identity).
    pub fn eval(&self, x: f64) -> f64 {
        let (Some(first), Some(last)) = (self.points.first(), self.points.last()) else { return x };
        // The number of control points whose key is at or before `x` (0 for a NaN).
        let at_or_before = self.points.partition_point(|p| p[0] <= x);
        if at_or_before == 0 {
            return first[1];
        }
        let (Some(a), Some(b)) = (self.points.get(at_or_before - 1), self.points.get(at_or_before)) else { return last[1] };
        let span = b[0] - a[0];
        if span > 0.0 { a[1] + (b[1] - a[1]) * ((x - a[0]) / span) } else { b[1] }
    }
}

/// How close two keys have to be for the later one to replace the earlier: a millionth of the
/// parametric range (1e-6 when the range is empty or not finite).
pub fn merge_distance(range: &[f64]) -> f64 {
    match range {
        [lo, hi, ..] if lo.is_finite() && hi.is_finite() && (hi - lo).abs() > 0.0 => (hi - lo).abs() * 1.0e-6,
        _ => 1.0e-6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn curve(points: &[(f64, f64)]) -> Curve {
        let mut c = Curve::default();
        for (k, v) in points {
            assert_eq!(c.add(*k, *v, 1e-6), K_STAT_OK);
        }
        c
    }

    fn keys(c: &Curve) -> Vec<f64> {
        (0..c.len()).filter_map(|i| c.point(i)).map(|p| p.0).collect()
    }

    #[test]
    fn an_empty_curve_is_the_identity() {
        let c = Curve::default();
        for x in [-3.0, 0.0, 0.25, 1.0, 7.5] {
            assert_eq!(c.eval(x), x);
        }
        assert!(c.is_empty());
    }

    #[test]
    fn values_are_piecewise_linear_and_held_outside() {
        let c = curve(&[(0.0, 0.2), (0.5, 0.8), (1.0, 0.4)]);
        assert!((c.eval(0.25) - 0.5).abs() < 1e-12);
        assert!((c.eval(0.75) - 0.6).abs() < 1e-12);
        // At a control point exactly.
        assert_eq!((c.eval(0.0), c.eval(0.5), c.eval(1.0)), (0.2, 0.8, 0.4));
        // Held before the first and after the last.
        assert_eq!((c.eval(-5.0), c.eval(9.0)), (0.2, 0.4));
        assert_eq!((c.eval(f64::NEG_INFINITY), c.eval(f64::INFINITY)), (0.2, 0.4));
        // A NaN position does not panic.
        let _ = c.eval(f64::NAN);
        // One control point is a constant.
        assert_eq!(curve(&[(0.3, 0.9)]).eval(0.0), 0.9);
    }

    #[test]
    fn points_are_kept_sorted_and_close_keys_merge() {
        let mut c = curve(&[(1.0, 0.4), (0.0, 0.2), (0.5, 0.9)]);
        assert_eq!(keys(&c), [0.0, 0.5, 1.0]);
        // A key within the merge distance replaces the point there.
        assert_eq!(c.add(0.5 + 1e-9, 0.8, 1e-6), K_STAT_OK);
        assert_eq!((c.len(), c.point(1)), (3, Some((0.5 + 1e-9, 0.8))));
        // Moving a point past a neighbour re-orders the curve.
        assert_eq!(c.set_nth(0, 0.75, 0.1), K_STAT_OK);
        assert_eq!(keys(&c), [0.5 + 1e-9, 0.75, 1.0]);
        assert_eq!(c.point(1), Some((0.75, 0.1)));
        assert_eq!(c.delete_nth(1), K_STAT_OK);
        assert_eq!(c.len(), 2);
        c.clear();
        assert!(c.is_empty());
    }

    #[test]
    fn duplicate_keys_do_not_divide_by_zero() {
        let mut c = curve(&[(0.0, 0.0), (1.0, 1.0)]);
        // Two points at one key can only come from set_nth.
        assert_eq!(c.set_nth(0, 1.0, 0.25), K_STAT_OK);
        assert_eq!(c.len(), 2);
        for x in [0.0, 0.999, 1.0, 2.0] {
            assert!(c.eval(x).is_finite(), "{x}");
        }
    }

    #[test]
    fn bad_input_is_an_error_not_a_panic() {
        let mut c = curve(&[(0.0, 0.0)]);
        assert_eq!(c.add(f64::NAN, 0.0, 1e-6), K_STAT_ERR_VALUE);
        assert_eq!(c.add(0.5, f64::INFINITY, 1e-6), K_STAT_ERR_VALUE);
        assert_eq!(c.set_nth(0, f64::NAN, 1.0), K_STAT_ERR_VALUE);
        assert_eq!(c.set_nth(5, 0.1, 0.1), K_STAT_ERR_BAD_INDEX);
        assert_eq!(c.delete_nth(1), K_STAT_ERR_BAD_INDEX);
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn a_curve_cannot_grow_without_bound() {
        let mut c = Curve::default();
        for i in 0..MAX_POINTS {
            assert_eq!(c.add(i as f64, 0.0, 0.1), K_STAT_OK);
        }
        assert_eq!(c.add(MAX_POINTS as f64 + 10.0, 0.0, 0.1), K_STAT_ERR_MEMORY);
        // Replacing a point still works when the curve is full.
        assert_eq!(c.add(3.0, 1.0, 0.1), K_STAT_OK);
        assert_eq!(c.len(), MAX_POINTS);
    }

    #[test]
    fn the_merge_distance_follows_the_range() {
        assert_eq!(merge_distance(&[0.0, 1.0]), 1.0e-6);
        assert!((merge_distance(&[0.0, 255.0]) - 255.0e-6).abs() < 1e-12);
        assert_eq!(merge_distance(&[1.0, 1.0]), 1.0e-6);
        assert_eq!(merge_distance(&[]), 1.0e-6);
        assert_eq!(merge_distance(&[f64::NAN, 1.0]), 1.0e-6);
    }
}

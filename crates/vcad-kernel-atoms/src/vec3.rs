//! Tiny 3-vector helpers over `[f64; 3]`. Kept dependency-free and inlinable so
//! the hot force loops don't pay for a general linear-algebra type.

/// `a - b`.
#[inline]
pub fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// `a + b`.
#[inline]
pub fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// `a * s`.
#[inline]
pub fn scale(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

/// Dot product.
#[inline]
pub fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Squared length.
#[inline]
pub fn norm2(a: [f64; 3]) -> f64 {
    dot(a, a)
}

/// Length.
#[inline]
pub fn norm(a: [f64; 3]) -> f64 {
    norm2(a).sqrt()
}

/// Cross product.
#[inline]
pub fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Add `b` into `a` in place.
#[inline]
pub fn add_assign(a: &mut [f64; 3], b: [f64; 3]) {
    a[0] += b[0];
    a[1] += b[1];
    a[2] += b[2];
}

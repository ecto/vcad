//! GDSII 8-byte excess-64 floating point conversion.
//!
//! GDSII does not use IEEE 754. Its 8-byte "real" is a hold-over from IBM
//! System/360 hexadecimal floats:
//!
//! ```text
//! bit 63       : sign (1 = negative)
//! bits 62..56  : exponent, excess-64, base 16
//! bits 55..0   : mantissa as an unsigned fraction (value = mant / 2^56)
//! value        = ±(mant / 2^56) · 16^(exp − 64)
//! ```
//!
//! A normalized value keeps the mantissa in `[1/16, 1)`. Because both the
//! mantissa scaling (`2^56`) and the base (`16 = 2^4`) are powers of two,
//! every finite `f64` in range converts exactly and round-trips bit-for-bit.

use crate::error::{GdsError, Result};

/// Decode a GDSII 8-byte excess-64 real into an `f64`.
///
/// A zero mantissa decodes to `0.0` regardless of sign/exponent bits.
pub fn real8_to_f64(bytes: [u8; 8]) -> f64 {
    let sign = bytes[0] & 0x80 != 0;
    let exp = (bytes[0] & 0x7f) as i32 - 64;
    let mut mant: u64 = 0;
    for b in &bytes[1..] {
        mant = (mant << 8) | u64::from(*b);
    }
    if mant == 0 {
        return 0.0;
    }
    let value = (mant as f64) / 2f64.powi(56) * 16f64.powi(exp);
    if sign {
        -value
    } else {
        value
    }
}

/// Encode an `f64` as a GDSII 8-byte excess-64 real.
///
/// Returns [`GdsError::Unencodable`] for non-finite values or magnitudes
/// outside the representable range (roughly `1e-78 ..= 7e75`).
pub fn f64_to_real8(value: f64) -> Result<[u8; 8]> {
    if !value.is_finite() {
        return Err(GdsError::Unencodable(format!(
            "{value} is not a finite number"
        )));
    }
    if value == 0.0 {
        return Ok([0u8; 8]);
    }

    let sign = value < 0.0;
    let mut v = value.abs();
    let mut exp: i32 = 0;
    // Normalize the mantissa into [1/16, 1). Multiplying/dividing by 16 is
    // exact for f64 (power of two), so no precision is lost here.
    while v >= 1.0 {
        v /= 16.0;
        exp += 1;
    }
    while v < 0.0625 {
        v *= 16.0;
        exp -= 1;
    }

    // v * 2^56 is again an exact power-of-two scaling.
    let mut mant = (v * 2f64.powi(56)).round() as u64;
    if mant >= 1 << 56 {
        // Rounding carried the mantissa up to 1.0 — renormalize.
        mant >>= 4;
        exp += 1;
    }

    let e = exp + 64;
    if !(0..=127).contains(&e) {
        return Err(GdsError::Unencodable(format!(
            "{value} is outside the excess-64 exponent range"
        )));
    }

    let mut out = [0u8; 8];
    out[0] = (u8::from(sign) << 7) | (e as u8);
    for (i, byte) in out[1..].iter_mut().enumerate() {
        *byte = ((mant >> (8 * (6 - i))) & 0xff) as u8;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_encoding_of_one() {
        // 1.0 = (1/16) · 16^1 → exponent 65 (0x41), mantissa 2^52.
        let bytes = f64_to_real8(1.0).unwrap();
        assert_eq!(bytes, [0x41, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        assert_eq!(real8_to_f64(bytes), 1.0);
    }

    #[test]
    fn known_encoding_of_two() {
        let bytes = f64_to_real8(2.0).unwrap();
        assert_eq!(bytes, [0x41, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn known_exponent_of_milli() {
        // 0.001 / 16^-2 = 0.256 ∈ [1/16, 1) → exponent byte 62 (0x3E).
        let bytes = f64_to_real8(0.001).unwrap();
        assert_eq!(bytes[0], 0x3e);
    }

    #[test]
    fn zero_roundtrip() {
        assert_eq!(f64_to_real8(0.0).unwrap(), [0u8; 8]);
        assert_eq!(real8_to_f64([0u8; 8]), 0.0);
        // Sign/exponent garbage with zero mantissa still decodes to zero.
        assert_eq!(real8_to_f64([0xc1, 0, 0, 0, 0, 0, 0, 0]), 0.0);
    }

    #[test]
    fn exact_roundtrip() {
        // Both directions are power-of-two scalings, so every in-range f64
        // must round-trip exactly — including the classic UNITS values.
        let cases = [
            1e-9,
            0.001,
            1.0,
            -1.0,
            1e-6,
            25.4,
            -0.062_5,
            std::f64::consts::PI,
            -6.022_140_76e-4,
            1234.567,
            -9.999e9,
        ];
        for &v in &cases {
            let bytes = f64_to_real8(v).unwrap();
            assert_eq!(real8_to_f64(bytes), v, "round-trip failed for {v}");
        }
    }

    #[test]
    fn negative_sets_sign_bit() {
        let bytes = f64_to_real8(-1.0).unwrap();
        assert_eq!(bytes[0], 0xc1);
        assert_eq!(real8_to_f64(bytes), -1.0);
    }

    #[test]
    fn rejects_non_finite() {
        assert!(f64_to_real8(f64::NAN).is_err());
        assert!(f64_to_real8(f64::INFINITY).is_err());
        assert!(f64_to_real8(1e300).is_err());
    }
}

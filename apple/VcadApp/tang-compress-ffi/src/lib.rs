//! C ABI over tang-compress's CPU reference decoder for TCG1/TCG3 streams,
//! for the native app (Sources/CTangCompress/tang_compress.h). The integer
//! decoder is the byte-exact reference: native GPU and browser decodes are
//! gated against its FNV-1a hash.

use std::ffi::{c_char, CString};
use tang_compress::{gpuwav, gpuwav3};

fn decode_bits(bytes: &[u8]) -> Result<(Vec<u32>, [usize; 3]), String> {
    let e = |e| format!("{e:?}");
    match bytes.get(..4) {
        Some(m) if m == gpuwav::MAGIC => {
            let ps = gpuwav::parse(bytes).map_err(e)?;
            let bits = gpuwav::decode_box_parsed(&ps, bytes, [0; 3], ps.d3, 1).map_err(e)?;
            Ok((bits, ps.d3))
        }
        Some(m) if m == gpuwav3::MAGIC3 || m == tang_compress::shipdec::MAGIC => {
            let ps = gpuwav3::parse3(bytes).map_err(e)?;
            let bits = gpuwav3::decode3_box_parsed(&ps, bytes, [0; 3], ps.d3, 1).map_err(e)?.bits;
            Ok((bits, ps.d3))
        }
        _ => Err("not a TCG1/TCG3 stream".into()),
    }
}

fn fnv1a(v: &[u32]) -> u64 {
    v.iter().fold(0xcbf29ce484222325u64, |h, &x| (h ^ x as u64).wrapping_mul(0x100000001b3))
}

/// A decoded field. `data` holds `len` f32 values, row-major `dims` = (z, y, x).
#[repr(C)]
pub struct TcField {
    pub data: *mut f32,
    pub len: usize,
    pub dims: [usize; 3],
    /// FNV-1a 64 over the f32 bit patterns (the cross-platform gate hash).
    pub fnv1a: u64,
}

/// Decode a TCG1/TCG3 stream on the CPU. Returns 0 on success; on failure
/// returns nonzero and, if `err` is non-null, stores a message to be freed
/// with `tc_string_free`.
///
/// # Safety
/// `bytes` must point to `len` readable bytes; `out` must be writable.
#[no_mangle]
pub unsafe extern "C" fn tc_decode_cpu(bytes: *const u8, len: usize, out: *mut TcField, err: *mut *mut c_char) -> i32 {
    let input = std::slice::from_raw_parts(bytes, len);
    match std::panic::catch_unwind(|| decode_bits(input)) {
        Ok(Ok((bits, dims))) => {
            let hash = fnv1a(&bits);
            let mut f: Vec<f32> = bits.into_iter().map(f32::from_bits).collect();
            f.shrink_to_fit();
            let mut f = std::mem::ManuallyDrop::new(f);
            *out = TcField { data: f.as_mut_ptr(), len: f.len(), dims, fnv1a: hash };
            0
        }
        r => {
            if !err.is_null() {
                let msg = match r {
                    Ok(Err(m)) => m,
                    _ => "decoder panicked".into(),
                };
                *err = CString::new(msg).unwrap_or_default().into_raw();
            }
            1
        }
    }
}

/// Free a field returned by `tc_decode_cpu`.
///
/// # Safety
/// `field` must come from `tc_decode_cpu` and not be freed twice.
#[no_mangle]
pub unsafe extern "C" fn tc_field_free(field: *mut TcField) {
    if field.is_null() || (*field).data.is_null() {
        return;
    }
    drop(Vec::from_raw_parts((*field).data, (*field).len, (*field).len));
    (*field).data = std::ptr::null_mut();
}

/// Free an error string.
///
/// # Safety
/// `s` must come from this library.
#[no_mangle]
pub unsafe extern "C" fn tc_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}

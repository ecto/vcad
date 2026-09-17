//! The CAM surface every front end is built on: JSON in, JSON out.
//!
//! Wave 1 put a whole 2.5D CAM stack in `vcad-kernel-cam` — contour roughing
//! and finishing, drilling and helical boring, arc fitting, a verification
//! oracle, a cutter-fit report, contours from a mesh, materials and feeds,
//! gear geometry. Wave 2 put a request schema in front of it so the native app
//! could reach it through the C ABI. This crate is that schema, lifted out of
//! the C ABI so there is exactly **one** implementation behind the app's FFI,
//! the browser's WASM and an agent's MCP tools.
//!
//! That matters for one reason: an agent and the app must never be able to
//! disagree about whether a job is safe to run. A second implementation of
//! "is this blocked?" is a second answer.
//!
//! # Conventions every entry point here shares
//!
//! - **Units are mm, feeds mm/min, Z is up, the stock top is Z0 and the stock
//!   frame has its XY lower-left at the origin.** There is no unit field:
//!   there is one unit.
//! - **One JSON string in, one JSON string out.** [`job`] and its siblings
//!   take a UTF-8 JSON document and answer with one.
//! - **Failure is a document, not an exception.** Everything answers
//!   `{"error": "<a sentence a machinist can act on>"}` rather than returning
//!   nothing, so every caller has one decode path. The `*_value` variants hand
//!   the error back as `Err(String)` instead, for callers (the C ABI) that
//!   also record it on a side channel.
//! - **Nothing here panics on bad input.** A panic would still be a bug; the
//!   ABI edges keep their `catch_unwind` for that, since this crate does not
//!   own the process it runs in.
//!
//! # The entry points
//!
//! | Function | Answers |
//! |---|---|
//! | [`job`] | a whole job: operations, tools, post, verification, G-code |
//! | [`verify_gcode`] | is *this* program the one that makes *this* part? |
//! | [`fit`] | does this cutter fit this contour? |
//! | [`outline_from_mesh`] | a contour out of a triangle mesh at a Z plane |
//! | [`compare_outline`] | are these two outlines the same part? |
//! | [`materials`] | the material table |
//! | [`recommend`] | feeds, speeds and the router dial to set |
//! | [`check_feeds`] | a second opinion on numbers the operator has |
//! | [`gear`] | gear geometry, contours, over-pins, compensation |
//!
//! A caller that holds a solid rather than a mesh — the C ABI's scene, the
//! WASM kernel's B-rep — tessellates it itself and calls [`section_mesh`],
//! which is the same code path [`outline_from_mesh`] runs.

#![warn(missing_docs)]

use serde_json::Value;

mod feeds;
mod gearing;
mod job;
mod outline;
mod placement;
mod types;
mod verify;

#[cfg(test)]
mod tests;

pub use outline::{section_mesh, section_segments};
pub use placement::Placement;

/// Build the error document every entry point answers with.
fn error_value(message: impl Into<String>) -> Value {
    serde_json::json!({ "error": message.into() })
}

/// Render a result as the JSON text a caller receives.
///
/// Serialisation can only fail on a non-finite number, which would mean a
/// report carried a NaN; that is worth saying rather than hiding, so it
/// becomes an error document of its own.
fn render(result: Result<Value, String>) -> String {
    let value = match result {
        Ok(v) => v,
        Err(message) => error_value(message),
    };
    serde_json::to_string(&value)
        .unwrap_or_else(|e| format!("{{\"error\":\"the result could not be serialised: {e}\"}}"))
}

macro_rules! entry {
    ($(#[$meta:meta])* $name:ident, $value_name:ident, $inner:path) => {
        $(#[$meta])*
        ///
        /// Answers a JSON document; on failure that document is
        /// `{"error": "…"}`.
        pub fn $name(request: &str) -> String {
            render($inner(request))
        }

        $(#[$meta])*
        ///
        /// The structured form of the call above: the error is `Err` rather
        /// than an `{"error": …}` document.
        pub fn $value_name(request: &str) -> Result<Value, String> {
            $inner(request)
        }
    };
}

entry!(
    /// A whole job: several operations, several tools, one spindle start per
    /// tool, verified before it is handed over.
    ///
    /// Fails closed: when verification is on and an error-severity check
    /// fails, the response carries `"blocked": true` and **no `gcode` key at
    /// all**, so a caller cannot export or run it by accident.
    job,
    job_value,
    job::run
);

entry!(
    /// Verify arbitrary G-code text against the part it is meant to make.
    verify_gcode,
    verify_gcode_value,
    verify::verify_gcode_request
);

entry!(
    /// Cutter-fit report for one contour, one tool and one side.
    fit,
    fit_value,
    verify::fit_request
);

entry!(
    /// Section a triangle mesh handed over inline (`positions`, `indices`).
    outline_from_mesh,
    outline_from_mesh_value,
    outline::from_mesh_request
);

entry!(
    /// Compare a DXF (or explicit loops) against an outline: the check that
    /// catches a stale outline before it machines something else.
    compare_outline,
    compare_outline_value,
    outline::compare_request
);

entry!(
    /// Feeds and speeds for one operation in one material on one machine.
    recommend,
    recommend_value,
    feeds::recommend_request
);

entry!(
    /// Check feeds and speeds the caller already has against the material.
    check_feeds,
    check_feeds_value,
    feeds::check_request
);

entry!(
    /// Gear geometry: report, contours, tool-centre paths, over-pins
    /// measurement and the compensation a measured reading implies.
    gear,
    gear_value,
    gearing::run
);

/// The material table. Takes no request.
pub fn materials() -> String {
    render(Ok(feeds::materials_list()))
}

/// The material table, structured.
pub fn materials_value() -> Value {
    feeds::materials_list()
}

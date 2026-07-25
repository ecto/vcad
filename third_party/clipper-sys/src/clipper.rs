#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(dead_code)]
#![allow(clippy::unreadable_literal)]
#![allow(deref_nullptr)]

#[cfg(all(not(feature = "update-bindings"), feature = "generate-bindings"))]
include!(concat!(env!("OUT_DIR"), "/bindings.rs"));

#[cfg(any(feature = "update-bindings", not(feature = "generate-bindings")))]
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/generated/bindings.rs"
));

impl Path {
    pub fn vertices(&self) -> &[[i64; 2]] {
        unsafe { std::slice::from_raw_parts(self.vertices, self.vertices_count as usize) }
    }
}

impl PartialEq for Path {
    fn eq(&self, other: &Self) -> bool {
        self.closed == other.closed && self.vertices() == other.vertices()
    }
}

impl Eq for Path {}

impl Polygon {
    pub fn paths(&self) -> &[Path] {
        unsafe { std::slice::from_raw_parts(self.paths, self.paths_count as usize) }
    }
}

impl PartialEq for Polygon {
    fn eq(&self, other: &Self) -> bool {
        self.type_ == other.type_ && self.paths() == other.paths()
    }
}

impl Eq for Polygon {}

impl Polygons {
    pub fn polygons(&self) -> &[Polygon] {
        unsafe { std::slice::from_raw_parts(self.polygons, self.polygons_count as usize) }
    }
}

impl PartialEq for Polygons {
    fn eq(&self, other: &Self) -> bool {
        self.polygons() == other.polygons()
    }
}

impl Eq for Polygons {}

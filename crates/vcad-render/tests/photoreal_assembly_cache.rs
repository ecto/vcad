//! A cached photoreal render of an assembly must be the uncached render.
//!
//! Assembly instances are cached as their part definition tessellated
//! *after* placement — tessellating in the part's own frame and moving the
//! triangles is a different mesh (the creased-normal welder buckets
//! positions on a fixed grid), which showed up on the rose humanoid as a
//! shading seam down one shin. This pins uncached == cold == warm, and that
//! the warm render is served from the cache without evaluating a BRep.
#![cfg(feature = "raytrace")]

use std::rc::Rc;

use vcad_eval::cache::{MemoryMeshCache, RootMeshCache};
use vcad_render::photoreal::{render_photoreal_png_str, PhotorealOptions, MESH_SEGMENTS};
use vcad_render::{with_root_cache, RasterOptions};

/// A bored boss (curved faces, so tessellation is frame-sensitive) on a
/// revolute joint at an awkward angle, plus a second instance of it placed
/// by a plain transform.
const ASSEMBLY: &str = r#"{
  "version": "0.1",
  "nodes": {
    "1": { "id": 1, "op": { "type": "Cylinder", "radius": 9, "height": 14, "segments": 0 } },
    "2": { "id": 2, "op": { "type": "Cylinder", "radius": 4, "height": 30, "segments": 0 } },
    "3": { "id": 3, "op": { "type": "Translate", "child": 2, "offset": { "x": 2, "y": 0, "z": -5 } } },
    "4": { "id": 4, "name": "boss", "op": { "type": "Difference", "left": 1, "right": 3 } },
    "5": { "id": 5, "name": "base", "op": { "type": "Cube", "size": { "x": 40, "y": 30, "z": 6 } } }
  },
  "materials": {},
  "part_materials": {},
  "roots": [],
  "partDefs": {
    "base": { "id": "base", "name": "base", "root": 5 },
    "boss": { "id": "boss", "name": "boss", "root": 4 }
  },
  "instances": [
    { "id": "base", "partDefId": "base", "material": "aluminum" },
    { "id": "swung", "partDefId": "boss", "material": "copper" },
    { "id": "moved", "partDefId": "boss", "material": "copper",
      "transform": {
        "translation": { "x": 28, "y": 18, "z": 6 },
        "rotation": { "x": 0, "y": 0, "z": 23 },
        "scale": { "x": 1, "y": 1, "z": 1 } } }
  ],
  "joints": [
    { "id": "j", "name": "swing", "parentInstanceId": "base",
      "childInstanceId": "swung",
      "parentAnchor": { "x": 12, "y": 15, "z": 6 },
      "childAnchor":  { "x": 0, "y": 0, "z": 0 },
      "kind": { "type": "Revolute", "axis": { "x": 0, "y": 0, "z": 1 },
                "limits": [-180, 180] },
      "state": 37.0 }
  ],
  "groundInstanceId": "base"
}"#;

fn render(cache: Option<Rc<MemoryMeshCache>>) -> Vec<u8> {
    let opts = RasterOptions {
        size_px: 96,
        ..Default::default()
    };
    let pr = PhotorealOptions {
        spp: 4,
        ..Default::default()
    };
    let go = || render_photoreal_png_str(ASSEMBLY, &opts, &pr).expect("render");
    match cache {
        Some(c) => with_root_cache(c as Rc<dyn RootMeshCache>, MESH_SEGMENTS, go),
        None => go(),
    }
}

#[test]
fn cached_assembly_renders_match_the_uncached_render() {
    let uncached = render(None);
    let cache = Rc::new(MemoryMeshCache::new());

    let cold = render(Some(cache.clone()));
    let after_cold = cache.stats();
    assert!(
        after_cold.stored > 0,
        "cold render cached nothing: {after_cold:?}"
    );
    assert!(
        cold == uncached,
        "cold cached render differs from the uncached one"
    );

    let warm = render(Some(cache.clone()));
    let after_warm = cache.stats();
    assert_eq!(
        after_warm.misses, after_cold.misses,
        "warm render missed the cache: {after_warm:?}"
    );
    assert_eq!(after_warm.stored, after_cold.stored);
    assert!(
        warm == uncached,
        "warm cached render differs from the uncached one"
    );
}

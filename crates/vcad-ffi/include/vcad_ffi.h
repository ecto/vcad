/* vcad-ffi — C ABI for the vcad kernel. Canonical header; kept in sync with
 * crates/vcad-ffi/src/lib.rs by hand for now (cbindgen automation is a TODO).
 * Buffer layout matches vcad_kernel_tessellate::TriangleMesh: flat f32 triples
 * for positions/normals, flat u32 indices. Lengths are element counts. */
#ifndef VCAD_FFI_H
#define VCAD_FFI_H

#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct VcadSolid VcadSolid;
typedef struct VcadMesh VcadMesh;
typedef struct VcadScene VcadScene;

typedef struct VcadMeshView {
  const float *vertices;
  size_t vertices_len; /* number of f32s (3 per vertex) */
  const float *normals;
  size_t normals_len;  /* 0 if absent/mismatched — synthesize on the caller side */
  const uint32_t *indices;
  size_t indices_len; /* number of u32s (3 per triangle) */
} VcadMeshView;

typedef struct VcadAabb {
  double min[3];
  double max[3];
} VcadAabb;

typedef struct VcadHit {
  uint8_t hit;
  double point[3];
  double normal[3];
  double t;
} VcadHit;

uint32_t vcad_ffi_abi_version(void);

VcadSolid *vcad_solid_cube(double sx, double sy, double sz);
VcadSolid *vcad_solid_cylinder(double radius, double height, uint32_t segments);
VcadSolid *vcad_solid_sphere(double radius, uint32_t segments);

VcadSolid *vcad_solid_fillet(const VcadSolid *solid, double radius);
VcadSolid *vcad_solid_chamfer(const VcadSolid *solid, double distance);
VcadAabb vcad_solid_bbox(const VcadSolid *solid);

VcadMesh *vcad_solid_to_mesh(const VcadSolid *solid, uint32_t segments);
VcadMeshView vcad_mesh_view(const VcadMesh *mesh);

void vcad_mesh_free(VcadMesh *mesh);
void vcad_solid_free(VcadSolid *solid);

VcadHit vcad_solid_raycast(const VcadSolid *solid, const double *origin, const double *dir);

/* Document evaluation: parse + evaluate a .vcad JSON document into a scene.
 * vcad_scene_from_loon compiles a loon source program (the AI-intent path) to
 * the same IR and evaluates it identically. */
VcadScene *vcad_scene_from_json(const uint8_t *json, size_t json_len);
VcadScene *vcad_scene_from_loon(const uint8_t *loon, size_t loon_len);
size_t vcad_scene_part_count(const VcadScene *scene);
VcadMeshView vcad_scene_part_mesh(const VcadScene *scene, size_t index);
void vcad_scene_free(VcadScene *scene);

/* Resident parametric document — set one parameter, re-solve every bound domain.
 * vcad_doc_gripper_slice1 builds the cross-domain worked example (one connector_x
 * drives an enclosure cutout + a board connector). */
typedef struct VcadDoc VcadDoc;
VcadDoc *vcad_doc_load(const uint8_t *json, size_t json_len);
VcadDoc *vcad_doc_gripper_slice1(void);
VcadScene *vcad_doc_set_param(VcadDoc *doc, const char *name, double value);
/* Per-frame path: re-solve only cheap roots (skips the sheet-metal fold), so the
 * mechanical cutout follows the drag live while the fold waits for settle
 * (vcad_doc_set_param). Same ownership. */
VcadScene *vcad_doc_set_param_cheap(VcadDoc *doc, const char *name, double value);
void vcad_doc_free(VcadDoc *doc);

/* Slice 2 — copper re-route. vcad_route_traces builds the tiny 2-net gripper
 * board at connector_x (mm, board-local) and routes it with the OHM auto-router,
 * returning copper segments to draw as the connector moves. layer: 0=FCu 1=BCu
 * 2+=inner; net_id: 0=SIG 1=GND. Coords are board-local mm (same frame as the
 * board plate). Caller owns the result (vcad_route_result_free). */
typedef struct VcadRouteResult VcadRouteResult;
typedef struct VcadTraceLine {
  double start[2];
  double end[2];
  double width;
  uint32_t layer;
  uint32_t net_id;
} VcadTraceLine;
VcadRouteResult *vcad_route_traces(double connector_x, double width);
size_t vcad_route_result_trace_count(const VcadRouteResult *r);
VcadTraceLine vcad_route_result_trace(const VcadRouteResult *r, size_t idx);
size_t vcad_route_result_unrouted_count(const VcadRouteResult *r);
void vcad_route_result_free(VcadRouteResult *r);

#ifdef __cplusplus
}
#endif

#endif /* VCAD_FFI_H */

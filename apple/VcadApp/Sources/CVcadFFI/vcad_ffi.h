/* Mirror of crates/vcad-ffi/include/vcad_ffi.h for the SwiftPM systemLibrary
 * target. Keep in sync with the canonical crate header. */
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
  size_t vertices_len;
  const float *normals;
  size_t normals_len;
  const uint32_t *indices;
  size_t indices_len;
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
/* Per-frame: re-solve only cheap roots (skip the sheet-metal fold); fold waits
 * for settle via vcad_doc_set_param. */
VcadScene *vcad_doc_set_param_cheap(VcadDoc *doc, const char *name, double value);
void vcad_doc_free(VcadDoc *doc);

/* Slice 2 — copper re-route. Build the 2-net gripper board at connector_x and
 * route it; returns copper segments. layer: 0=FCu 1=BCu 2+=inner; net_id: 0=SIG
 * 1=GND. Coords board-local mm. Caller owns the result (vcad_route_result_free). */
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

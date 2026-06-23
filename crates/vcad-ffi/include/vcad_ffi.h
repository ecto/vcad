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

/* Document evaluation: parse + evaluate a .vcad JSON document into a scene. */
VcadScene *vcad_scene_from_json(const uint8_t *json, size_t json_len);
size_t vcad_scene_part_count(const VcadScene *scene);
VcadMeshView vcad_scene_part_mesh(const VcadScene *scene, size_t index);
void vcad_scene_free(VcadScene *scene);

#ifdef __cplusplus
}
#endif

#endif /* VCAD_FFI_H */

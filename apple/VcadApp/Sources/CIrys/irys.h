/* irys over a C ABI (M1 item 7): render into an IOSurface with no copy.
 * Every function returns 0 / non-NULL on success; irys_last_error() has the
 * message otherwise. Handles are not thread-safe. */
#ifndef IRYS_H
#define IRYS_H
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct IrysContext IrysContext;
typedef struct IrysScene IrysScene;

const char *irys_last_error(void);

/* The process's GPU; NULL without one or without hardware ray queries. */
IrysContext *irys_context_create(void);
void irys_context_destroy(IrysContext *c);

/* A Blender export directory and / or a 3DGS .ply (either may be NULL; with
 * neither, a demo scene of four spheres), rendered at width x height. */
IrysScene *irys_scene_load(const IrysContext *c, const char *export_dir, const char *splat_ply,
                           uint32_t width, uint32_t height);
void irys_scene_destroy(IrysScene *s);

/* Mesh ingest: a scene from the caller's own geometry. Each mesh carries its
 * object-space positions (3 floats/vertex), optional normals (NULL = flat),
 * triangle indices and a material index. Instances place meshes:
 * instance_mesh[k] and instance_xf[12k..] (row-major 3x4 object -> world).
 * World Z is up; units are the caller's. Lit by a studio sky + sun;
 * IRYS_FLOOR adds a matte floor under the lowest point. */
typedef struct IrysMeshDesc {
  const float *positions;
  const float *normals;
  size_t vertex_count;
  const uint32_t *indices;
  size_t index_count;
  uint32_t material;
} IrysMeshDesc;
typedef struct IrysMaterialDesc {
  float base_color[3]; /* linear Rec.709 */
  float metallic;
  float roughness;
  float transmission;
} IrysMaterialDesc;
#define IRYS_FLOOR 1u
/* Studio lighting (NULL = defaults: sky 1, sun 1 at az 50.7 / el 44.4,
 * size 3.4 deg, floor albedo 0.31). */
typedef struct IrysLighting {
  float sky;               /* sky brightness multiplier */
  float sun;               /* sun irradiance multiplier, 0 = no sun */
  float sun_azimuth_deg;   /* CCW from +X */
  float sun_elevation_deg; /* above the horizon */
  float sun_size_deg;      /* angular radius: shadow softness */
  float floor_albedo;
} IrysLighting;
IrysScene *irys_scene_from_meshes(const IrysContext *c,
                                  const IrysMeshDesc *meshes, size_t n_meshes,
                                  const IrysMaterialDesc *materials, size_t n_materials,
                                  const uint32_t *instance_mesh, const float *instance_xf,
                                  size_t n_instances, uint32_t width, uint32_t height,
                                  uint32_t flags, const IrysLighting *lighting);
/* transparent: camera rays that miss show nothing and the presented alpha is
 * coverage (premultiplied), for compositing; exposure multiplies the image;
 * max_bounces caps path length (0 = keep). Restarts accumulation. */
int irys_scene_set_render_options(IrysScene *s, uint32_t transparent, float exposure,
                                  uint32_t max_bounces);
/* Move the instances (n = the count given at creation, 12 floats each).
 * TLAS only; restarts progressive accumulation. */
int irys_scene_set_transforms(IrysScene *s, const float *xf, size_t n);
/* Camera by eye / target / up and vertical fov; restarts accumulation. */
int irys_scene_set_look_at(IrysScene *s, const double eye[3], const double target[3],
                           const double up[3], double vfov_deg);
/* Progressive: add spp samples to the running image and present its mean
 * into the IOSurface. irys_scene_sample_count() = samples so far. */
int irys_scene_accumulate_iosurface(IrysScene *s, const void *iosurface, uint32_t spp);
uint32_t irys_scene_sample_count(const IrysScene *s);
int irys_scene_reset(IrysScene *s);

/* OpenCV pinhole intrinsics (pixels) and the camera -> world pose: rot is
 * row-major 3x3 whose columns are the optical axes (x right, y down, z
 * forward) in the world; pos is the camera centre. */
int irys_scene_set_camera(IrysScene *s, double fx, double fy, double cx, double cy,
                          const double rot[9], const double pos[3]);

/* Render spp samples into an IOSurfaceRef ('RGBA', 4 bytes per element, the
 * scene's size). Written on the GPU, nothing read back; done on return. */
int irys_scene_render_iosurface(IrysScene *s, const void *iosurface, uint32_t spp);
/* The same by IOSurfaceGetID() (looked up in this process). */
int irys_scene_render_iosurface_id(IrysScene *s, uint32_t iosurface_id, uint32_t spp);

/* The readback path, for comparison: sRGB8 RGBA, rows packed. */
int irys_scene_render_rgba8(IrysScene *s, uint8_t *out, uint32_t spp);

#ifdef __cplusplus
}
#endif
#endif

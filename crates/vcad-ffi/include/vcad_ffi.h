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
/* Like vcad_scene_from_json, but resolves RELATIVE MeshImport paths against
 * base_dir (the document's own directory). A document that ships its meshes
 * alongside itself references them relatively; without a base directory they
 * resolve against the process working directory, so the same file renders from
 * one launch and comes back empty from another. NULL/empty base_dir behaves
 * exactly like vcad_scene_from_json. */
VcadScene *vcad_scene_from_json_in(const uint8_t *json, size_t json_len,
                                   const uint8_t *base_dir, size_t base_dir_len);

VcadScene *vcad_scene_from_loon(const uint8_t *loon, size_t loon_len);
size_t vcad_scene_part_count(const VcadScene *scene);
VcadMeshView vcad_scene_part_mesh(const VcadScene *scene, size_t index);
void vcad_scene_free(VcadScene *scene);

/* Assembly instances + kinematic joint playback. Assembly documents place
 * geometry via partDefs + instances + joints (joints fully place children).
 * Instance meshes are part-def-LOCAL; transforms come separately so playback
 * can drive them per frame. When instance_count > 0 render instances INSTEAD
 * of the root parts (assembly docs may carry both). Id/material pointers are
 * UTF-8, NOT NUL-terminated (length via out_len), valid for the scene's
 * lifetime. Transforms are 16 doubles, COLUMN-major (out[col*4+row]).
 * vcad_scene_solve_fk takes a JSON object {"jointId": value, ...} (degrees /
 * mm; joint name accepted as fallback), runs the kernel forward-kinematics
 * solver, and writes instance_count 4x4 world matrices in instance-index
 * order into out (capacity out_cap doubles). Returns instances written, 0 on
 * error. Only scenes from vcad_scene_from_json/_loon support FK. */
size_t vcad_scene_instance_count(const VcadScene *scene);
VcadMeshView vcad_scene_instance_mesh(const VcadScene *scene, size_t index);
const uint8_t *vcad_scene_instance_id(const VcadScene *scene, size_t index, size_t *out_len);
const uint8_t *vcad_scene_instance_material(const VcadScene *scene, size_t index, size_t *out_len);
uint8_t vcad_scene_instance_transform(const VcadScene *scene, size_t index, double *out);
size_t vcad_scene_solve_fk(const VcadScene *scene, const uint8_t *joints_json,
                           size_t json_len, double *out, size_t out_cap);

/* Feature edges (boundary + creases sharper than angle_deg) for the
 * wireframe/edge overlay. 6 floats per segment: ax ay az bx by bz. */
typedef struct VcadEdges VcadEdges;
typedef struct VcadEdgesView {
  const float *floats;
  size_t floats_len;
} VcadEdgesView;
VcadEdges *vcad_scene_part_edges(const VcadScene *scene, size_t index, float angle_deg);
VcadEdges *vcad_mesh_edges(const VcadMesh *mesh, float angle_deg);
VcadEdgesView vcad_edges_view(const VcadEdges *edges);
void vcad_edges_free(VcadEdges *edges);

/* Per-triangle FACE ids for a scene part: one u32 per triangle of
 * vcad_scene_part_mesh, naming which face of the solid produced it (ordinal
 * within the shell). UINT32_MAX marks a triangle with no face (bridging fill).
 * Writes the count to out_len; returns NULL when the part carries no tags
 * (e.g. a frozen or imported mesh with no B-rep), so face picking is an
 * optional capability. Borrows the scene; valid until the scene is freed.
 * NOT durable across edits — any change to the B-rep renumbers the ordinals. */
const uint32_t *vcad_scene_part_face_ids(const VcadScene *scene, size_t index,
                                         size_t *out_len);

/* Direct BRep ray tracing (pixel-perfect mode): rays hit analytic surfaces,
 * no tessellation. Kernel coords (Z-up, mm); colors = 3 f32 per part
 * (linear RGB); RGBA8 row-major output. CPU renderer today, signature is
 * renderer-agnostic (Metal/wgpu can swap in behind it). */
typedef struct VcadImage VcadImage;
typedef struct VcadImageView {
  const uint8_t *pixels;
  size_t pixels_len;
  uint32_t width;
  uint32_t height;
} VcadImageView;
VcadImage *vcad_scene_raytrace(const VcadScene *scene, const double *cam,
                               const double *target, double fov_deg,
                               uint32_t width, uint32_t height,
                               const float *colors, size_t colors_len);
VcadImage *vcad_scene_raytrace_gpu(const VcadScene *scene, const double *cam,
                                   const double *target, double fov_deg,
                                   uint32_t width, uint32_t height,
                                   const float *colors, size_t colors_len);
VcadImageView vcad_image_view(const VcadImage *image);
void vcad_image_free(VcadImage *image);

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
/* Per-frame REAL min-wall (mm): resolve the doc at its current params and measure
 * the enclosure's thinnest enclosed-face clearance to the connector cutout from
 * geometry — cheap enough to run live (no fold, no routing). f64::INFINITY when
 * unmeasurable. The −Y front is the connector port and is excluded. */
double vcad_doc_min_wall(const VcadDoc *doc);
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

/* The unified cross-domain solve. vcad_doc_solve sets connector_x and returns
 * EVERY domain at once — meshes (enclosure + board + sheet-metal bracket), copper
 * traces, and the receipt scalars — all from the one resolved parameter. The
 * SETTLE path (full + expensive); per-frame still uses vcad_doc_set_param_cheap.
 * Caller owns the result (vcad_solve_free). */
typedef struct VcadSolve VcadSolve;
VcadSolve *vcad_doc_solve(VcadDoc *doc, const char *name, double value);
size_t vcad_solve_part_count(const VcadSolve *s);
VcadMeshView vcad_solve_part_mesh(const VcadSolve *s, size_t index);
size_t vcad_solve_trace_count(const VcadSolve *s);
VcadTraceLine vcad_solve_trace(const VcadSolve *s, size_t idx);
size_t vcad_solve_unrouted(const VcadSolve *s);  /* receipt: nets unrouted (0 = ok) */
double vcad_solve_min_wall(const VcadSolve *s);   /* receipt: connector-wall mm */
/* Receipt verdicts (ABI 4). bracket_ok: 1=Held, 0=Violated (DFM Error).
 * bracket_severity: 0 clean / 1 Warning / 2 Error. quote_cost_cents: integer
 * cents TOTAL (enclosure + board + bracket, qty 1). lead_days: HEURISTIC.
 * all_held: AND of gating domains (min-wall, copper, bracket; quote never
 * gates) — gate the Make-it button. */
uint8_t vcad_solve_bracket_ok(const VcadSolve *s);
uint8_t vcad_solve_bracket_severity(const VcadSolve *s);
uint64_t vcad_solve_quote_cost_cents(const VcadSolve *s);
uint32_t vcad_solve_lead_days(const VcadSolve *s);
uint8_t vcad_solve_all_held(const VcadSolve *s);
/* Per-domain quote breakdown (ABI 5): enclosure CNC + bracket fold are
 * kernel-real cost models (removed-volume / unfold); board is a labeled estimate
 * (no Rust PCB cost model). quote_has_estimate = 1 when the total includes the
 * labeled board line, so the UI tags it "est." Sum == quote_cost_cents. */
uint64_t vcad_solve_quote_enclosure_cents(const VcadSolve *s);
uint64_t vcad_solve_quote_board_cents(const VcadSolve *s);
uint64_t vcad_solve_quote_bracket_cents(const VcadSolve *s);
uint8_t vcad_solve_quote_has_estimate(const VcadSolve *s);
void vcad_solve_free(VcadSolve *s);

/* =====================================================================
 * Simulation (ABI 6) — physics envs, the render seam, policy inference,
 * and in-process training. See crates/vcad-ffi/src/gym.rs and train.rs.
 *
 * UNITS. Body transforms come back in MILLIMETERS, column-major, exactly
 * like vcad_scene_solve_fk — a renderer that draws kinematic playback draws
 * a physics rollout unchanged. Observations are NOT rescaled: they are the
 * policy's input space (degrees / mm / meters as documented per field) and
 * must stay bit-identical to what training saw.
 *
 * ERRORS. Every entry point below returns NULL/0 on failure AND records a
 * human-readable reason retrievable with vcad_last_error() on the SAME
 * thread. Read it immediately after a failed call.
 * ===================================================================== */

/* Borrow this thread's last error as UTF-8 (NOT null-terminated; use the
 * length). Returns NULL when no error is recorded. Valid until the next FFI
 * call on this thread. */
const uint8_t *vcad_last_error(size_t *out_len);

typedef struct VcadGym VcadGym;
typedef struct VcadPolicy VcadPolicy;
typedef struct VcadTrainer VcadTrainer;

/* A borrowed view of the most recent step/reset. Every pointer borrows from
 * the owning gym and is invalidated by the next step/reset on it — copy out
 * before stepping again. Lengths are element counts.
 *
 * base_height_m / base_tilt_deg are NOISE-FREE ground truth; base_pose is the
 * (possibly noisy) sensor view the policy trains against. They deliberately
 * disagree when observation noise is configured. Reward and termination read
 * the former; the policy reads the latter. */
typedef struct VcadGymStepView {
  const double *joint_positions;      /* degrees (rotational) / mm (linear) */
  size_t joint_positions_len;
  const double *joint_velocities;     /* per second, same per-DOF units */
  size_t joint_velocities_len;
  const double *end_effector_poses;   /* 7 per EE: x,y,z (m), qw,qx,qy,qz */
  size_t end_effector_poses_len;
  const double *end_effector_contacts;/* 5 per EE: touch(0/1), N, cop xyz */
  size_t end_effector_contacts_len;
  const double *base_pose;            /* 7 doubles, or NULL */
  const double *base_velocity;        /* 6 doubles (v xyz m/s, w xyz rad/s) */
  double reward;                      /* always 0 — compute task reward yourself */
  uint8_t done;
  uint8_t terminated;
  uint8_t truncated;
  uint8_t has_base;
  uint32_t step;
  uint32_t action_latency_substeps;
  double base_height_m;               /* ground truth */
  double base_tilt_deg;               /* ground truth */
  const uint8_t *termination_reason;  /* UTF-8, or NULL */
  size_t termination_reason_len;
} VcadGymStepView;

/* Create an env from a .vcad JSON document and a GymSpec JSON blob. Pass
 * spec_json = NULL (len 0) for defaults. Returns NULL on failure. */
VcadGym *vcad_gym_create(const uint8_t *doc_json, size_t doc_json_len,
                         const uint8_t *spec_json, size_t spec_json_len);
void vcad_gym_free(VcadGym *gym);

/* Reset, drawing domain randomization from `seed`. Returns 1 on success. */
uint8_t vcad_gym_reset(VcadGym *gym, uint64_t seed);

/* Step once. action_kind: 0 = torque (Nm/N), 1 = position target (deg/mm),
 * 2 = velocity target (deg/s, mm/s). actions_len MUST equal
 * vcad_gym_action_dim — a mismatch is refused, never padded. */
uint8_t vcad_gym_step(VcadGym *gym, const double *actions, size_t actions_len,
                      uint32_t action_kind);

/* Refresh the view from true state without advancing time. */
uint8_t vcad_gym_observe(VcadGym *gym);
VcadGymStepView vcad_gym_step_view(const VcadGym *gym);

/* Introspection. */
size_t vcad_gym_action_dim(const VcadGym *gym);       /* action vector length */
size_t vcad_gym_obs_dim(const VcadGym *gym);          /* POLICY feature count */
size_t vcad_gym_observation_dim(const VcadGym *gym);  /* raw observation slots */
double vcad_gym_control_dt(const VcadGym *gym);       /* dt*substeps, seconds */
uint32_t vcad_gym_max_steps(const VcadGym *gym);
size_t vcad_gym_actuated_joint_count(const VcadGym *gym);
const uint8_t *vcad_gym_actuated_joint_id(const VcadGym *gym, size_t index,
                                          size_t *out_len);
size_t vcad_gym_body_count(const VcadGym *gym);
const uint8_t *vcad_gym_body_id(const VcadGym *gym, size_t index, size_t *out_len);

/* Render seam. Writes 16 doubles per body, COLUMN-MAJOR, MILLIMETERS —
 * identical layout to vcad_scene_solve_fk. out_cap is in doubles. */
size_t vcad_gym_body_transforms(const VcadGym *gym, double *out, size_t out_cap);

/* Bind the env's bodies to a scene's instance ordering (call once, after
 * creating both from the same document). Returns the number of scene
 * instances that matched a simulated body. */
size_t vcad_gym_bind_scene(VcadGym *gym, const VcadScene *scene);
size_t vcad_gym_scene_binding_len(const VcadGym *gym);
/* Transforms in SCENE INSTANCE ORDER — the index space the vcad_scene_instance_*
 * calls use. Unmatched instances are left UNTOUCHED, so pre-fill `out` with
 * authored transforms to keep static scenery. Requires vcad_gym_bind_scene. */
size_t vcad_gym_scene_transforms(const VcadGym *gym, double *out, size_t out_cap);

/* Shove the floating base: angular (rad/s) then BODY-FRAME linear (m/s),
 * added to its current velocity. Returns 0 if there is no floating base. */
uint8_t vcad_gym_nudge_base(VcadGym *gym, double dwx, double dwy, double dwz,
                            double dvx, double dvy, double dvz);

/* Write the current policy feature vector (for plotting). Returns the count. */
size_t vcad_gym_features(VcadGym *gym, double *out, size_t out_cap);

/* --- Policy inference -------------------------------------------------
 * Inference lives in Rust so the forward pass matches training EXACTLY
 * (whitening, output clamp, default-pose offset). Do not reimplement it in
 * Swift: a drift of one clamp gives a robot that almost stands. */
VcadPolicy *vcad_policy_load(const uint8_t *json, size_t json_len);
/* Load a .vcadpolicy bundle. document_json may be NULL to skip the drift
 * check; when supplied and the hash differs the policy STILL loads (staleness
 * is a judgement, not a load error) and vcad_last_error describes the drift. */
VcadPolicy *vcad_policy_load_bundle(const uint8_t *bundle_json, size_t bundle_json_len,
                                    const uint8_t *document_json, size_t document_json_len);
void vcad_policy_free(VcadPolicy *policy);
size_t vcad_policy_obs_dim(const VcadPolicy *policy);
size_t vcad_policy_act_dim(const VcadPolicy *policy);
uint8_t vcad_policy_is_mlp(const VcadPolicy *policy);
/* Dimensional compatibility check with a descriptive error. Call at load. */
uint8_t vcad_policy_check(const VcadGym *gym, const VcadPolicy *policy);
/* A zero (hold-rest-pose) policy matched to this env — the baseline. */
VcadPolicy *vcad_policy_zeros(const VcadGym *gym, double action_scale_deg);
/* Step by evaluating the policy: features -> act -> position targets. */
uint8_t vcad_gym_policy_step(VcadGym *gym, const VcadPolicy *policy);
/* The action the last policy step issued (joint targets, degrees), or NULL. */
const double *vcad_gym_last_action(const VcadGym *gym, size_t *out_len);

/* --- Reward and provenance -------------------------------------------- */
/* Evaluate a RewardSpec JSON against the gym's most recent step. Returns 0
 * before the first step of an episode, NaN on failure. */
double vcad_gym_reward(const VcadGym *gym, const uint8_t *reward_json,
                       size_t reward_json_len);
/* Content hash of a document ("fnv1a64:xxxxxxxxxxxxxxxx", 23 bytes). */
size_t vcad_document_hash(const uint8_t *doc_json, size_t doc_json_len,
                          uint8_t *out, size_t out_cap);

/* --- Training ---------------------------------------------------------- */
typedef struct VcadTrainProgress {
  uint32_t iteration;
  uint32_t total_iterations;
  double mean_reward;
  double eval_reward;   /* trainer's own eval — NOT trustworthy, see docs */
  uint32_t eval_steps;
  double sigma;         /* top-k spread; collapsing toward 0 precedes divergence */
  double update_norm;
  double step_size;
  double best_held_out; /* the only number a run may be judged by */
  uint32_t best_held_out_full;
  uint32_t best_iteration;
  uint8_t running;
  uint8_t finished;
  uint8_t failed;
  uint8_t cancelled;
} VcadTrainProgress;

/* Start a run on a worker thread; returns immediately. train_spec_json and
 * reward_json may be NULL for defaults. gym_spec_json uses the SAME GymSpec
 * shape as vcad_gym_create, so the simulated env and the trained env cannot
 * disagree. */
VcadTrainer *vcad_train_start(const uint8_t *doc_json, size_t doc_json_len,
                              const uint8_t *gym_spec_json, size_t gym_spec_json_len,
                              const uint8_t *train_spec_json, size_t train_spec_json_len,
                              const uint8_t *reward_json, size_t reward_json_len);
uint8_t vcad_train_poll(const VcadTrainer *trainer, VcadTrainProgress *out);
void vcad_train_stop(VcadTrainer *trainer);
/* Two-call protocol. Sizing call: out=NULL (or out_cap=0) RETURNS the number of
 * bytes required, or 0 if no policy has been scored yet. Copy call: pass a
 * buffer of at least that size; returns bytes written, or 0 if too small. */
size_t vcad_train_best_policy_json(const VcadTrainer *trainer, uint8_t *out, size_t out_cap);
const uint8_t *vcad_train_error(const VcadTrainer *trainer, size_t *out_len);
/* Cancels, JOINS the worker, then frees. Blocks — deliberately. */
void vcad_train_free(VcadTrainer *trainer);

#ifdef __cplusplus
}
#endif

#endif /* VCAD_FFI_H */

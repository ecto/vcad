/* tslint:disable */
/* eslint-disable */

/**
 * Physics simulation environment for robotics and RL.
 *
 * This provides a gym-style interface for simulating robot assemblies
 * with physics, joints, and collision detection.
 */
export class PhysicsSim {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Get the action dimension.
     */
    actionDim(): number;
    /**
     * Create a new physics simulation from a vcad document JSON.
     *
     * # Arguments
     * * `doc_json` - JSON string representing a vcad IR Document
     * * `end_effector_ids` - Array of instance IDs to track as end effectors
     * * `dt` - Simulation timestep in seconds (default: 1/240)
     * * `substeps` - Number of physics substeps per step (default: 4)
     */
    constructor(doc_json: string, end_effector_ids: string[], dt?: number | null, substeps?: number | null);
    /**
     * Get the number of joints in the environment.
     */
    numJoints(): number;
    /**
     * Get the observation dimension.
     */
    observationDim(): number;
    /**
     * Get current observation without stepping.
     *
     * Returns observation as JSON.
     */
    observe(): any;
    /**
     * Reset the environment to initial state.
     *
     * Returns the initial observation as JSON.
     */
    reset(): any;
    /**
     * Set the maximum episode length.
     */
    setMaxSteps(max_steps: number): void;
    /**
     * Set the random seed.
     */
    setSeed(seed: bigint): void;
    /**
     * Step the simulation with position targets.
     *
     * # Arguments
     * * `targets` - Array of position targets for each joint (degrees or mm)
     *
     * # Returns
     * Object with { observation, reward, done }
     */
    stepPosition(targets: Float64Array): any;
    /**
     * Step the simulation with a torque action.
     *
     * # Arguments
     * * `torques` - Array of torques/forces for each joint (Nm or N)
     *
     * # Returns
     * Object with { observation, reward, done }
     */
    stepTorque(torques: Float64Array): any;
    /**
     * Step the simulation with velocity targets.
     *
     * # Arguments
     * * `targets` - Array of velocity targets for each joint (deg/s or mm/s)
     *
     * # Returns
     * Object with { observation, reward, done }
     */
    stepVelocity(targets: Float64Array): any;
}

/**
 * GPU-accelerated ray tracer for direct BRep rendering.
 *
 * This ray tracer renders BRep surfaces directly without tessellation,
 * achieving pixel-perfect silhouettes at any zoom level.
 *
 * All mutable state lives behind a `RefCell` so every wasm-bindgen entry point
 * can be `&self`. The async `render` previously held `&mut self` across `.await`
 * and tripped wasm-bindgen's "recursive use of an object detected" guard
 * whenever a setter (theme/debug/edges/upload) fired while a render was in
 * flight. Now setters take a brief mutable borrow on `inner`, the scene is
 * stored as `Rc<GpuScene>` so a render can hold a stable handle across the
 * await even if the scene gets swapped, and the accumulation buffers are
 * taken out for the duration of the render and re-installed after — gated by
 * an epoch counter so resets that happen mid-render correctly invalidate the
 * returned buffers.
 */
export class RayTracer {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Check if a solid can be ray traced.
     *
     * Returns true if the solid has a BRep representation.
     */
    static canRaytrace(solid: Solid): boolean;
    /**
     * Clear all uploaded geometry. Call before re-uploading a fresh
     * scene; subsequent `upload_solid` calls will accumulate into a
     * new merged scene.
     */
    clearScene(): void;
    /**
     * Create a new ray tracer.
     *
     * Requires WebGPU to be available and initialized.
     * Call `initGpu()` before calling this method.
     */
    static create(): RayTracer;
    /**
     * Get the current debug render mode.
     */
    getDebugMode(): number;
    /**
     * Get whether edge detection is enabled.
     */
    getEdgeDetectionEnabled(): boolean;
    /**
     * Get the current frame index for progressive rendering.
     */
    getFrameIndex(): number;
    /**
     * Get the current refinement sample count.
     */
    getRefineSamples(): number;
    /**
     * Check if the ray tracer has a scene loaded.
     */
    hasScene(): boolean;
    /**
     * Pick a face at the given pixel coordinates.
     *
     * # Arguments
     * * `camera`, `target`, `up` - Camera parameters
     * * `width`, `height`, `fov` - View parameters
     * * `pixel_x`, `pixel_y` - Pixel coordinates to pick
     *
     * # Returns
     * Face index if a face was hit, or -1 if background was hit.
     */
    pick(camera: Float64Array, target: Float64Array, up: Float64Array, width: number, height: number, fov: number, pixel_x: number, pixel_y: number): number;
    /**
     * Render the scene to an RGBA image with progressive anti-aliasing.
     *
     * Each call accumulates another sample. Call `resetAccumulation()` when the
     * camera moves to restart the accumulation.
     *
     * # Arguments
     * * `camera` - Camera position [x, y, z]
     * * `target` - Look-at target [x, y, z]
     * * `up` - Up vector [x, y, z]
     * * `width` - Image width in pixels
     * * `height` - Image height in pixels
     * * `fov` - Field of view in radians
     *
     * # Returns
     * RGBA pixel data as a byte array (width * height * 4 bytes).
     *
     * # Note
     * This function is async to support WASM's single-threaded environment.
     * In JavaScript, it returns a Promise<Uint8Array>.
     */
    render(camera: Float64Array, target: Float64Array, up: Float64Array, width: number, height: number, fov: number): Promise<Uint8Array>;
    /**
     * Reset the progressive accumulation (call when camera moves).
     */
    resetAccumulation(): void;
    /**
     * Set SSAO (screen-space ambient occlusion) parameters.
     *
     * # Arguments
     * * `radius` - World-space hemisphere sample radius (default 0.3)
     * * `intensity` - Occlusion strength: 0 = disabled, 1 = default (>1 stylized)
     * * `bias` - Depth bias to prevent self-occlusion (default 0.001)
     * * `sample_count` - Hemisphere samples per frame: 8, 16, or 32 (default 16)
     */
    setAO(radius: number, intensity: number, bias: number, sample_count: number): void;
    /**
     * Set the debug render mode.
     *
     * # Arguments
     * * `mode` - Debug mode: 0=normal, 1=normals as RGB, 2=face_id colors, 3=N·L grayscale, 4=orientation
     *
     * Call resetAccumulation() after changing mode to see immediate effect.
     */
    setDebugMode(mode: number): void;
    /**
     * Set edge detection settings.
     *
     * # Arguments
     * * `enabled` - Whether to show edge detection overlay
     * * `depth_threshold` - Depth discontinuity threshold (default: 0.1)
     * * `normal_threshold` - Normal angle threshold in degrees (default: 30.0)
     */
    setEdgeDetection(enabled: boolean, depth_threshold: number, normal_threshold: number): void;
    /**
     * Set per-type edge style (colors, widths, softness, and individual toggles).
     *
     * Colors are RGBA in linear space (0–1). Width 1.0 = one pixel; softness controls
     * the sub-pixel anti-aliasing transition width.
     */
    setEdgeStyle(enable_silhouette: boolean, enable_crease: boolean, enable_boundary: boolean, silhouette_r: number, silhouette_g: number, silhouette_b: number, silhouette_a: number, crease_r: number, crease_g: number, crease_b: number, crease_a: number, boundary_r: number, boundary_g: number, boundary_b: number, boundary_a: number, silhouette_width: number, crease_width: number, boundary_width: number, edge_softness: number): void;
    /**
     * Set the material for all faces in the scene.
     *
     * # Arguments
     * * `r`, `g`, `b` - RGB color components (0-1 range, linear)
     * * `metallic` - Metallic factor (0 = dielectric, 1 = metal)
     * * `roughness` - Roughness factor (0 = smooth/mirror, 1 = rough/diffuse)
     */
    setMaterial(r: number, g: number, b: number, metallic: number, roughness: number): void;
    /**
     * Set the adaptive refinement sample count.
     *
     * Edge pixels on silhouettes receive additional stratified rays for sub-pixel
     * anti-aliasing. Set to 0 to disable (default), or 4/9/16 for typical quality.
     * Mode 5 in setDebugMode shows a heatmap of rays per pixel for tuning.
     */
    setRefineSamples(count: number): void;
    /**
     * Set the visible-background theme. 0 = dark (default), 1 = light.
     * IBL panels and direct lighting stay constant across themes — this
     * only swaps the atmospheric backdrop and ground tint.
     */
    setTheme(theme: number): void;
    /**
     * Upload a solid's BRep representation for ray tracing.
     *
     * First call after clearScene seeds the GPU scene. Subsequent calls
     * merge into the existing scene — surfaces/faces/BVH from each new
     * solid are unified under a fresh root, so multi-part scenes render
     * in a single ray-trace pass.
     */
    uploadSolid(solid: Solid): void;
}

/**
 * Slice result for WASM.
 */
export class SliceResult {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Get layer data for preview.
     */
    getLayerPreview(layer_index: number): any;
    /**
     * Get stats as JSON.
     */
    statsJson(): string;
    /**
     * Get filament weight in grams.
     */
    readonly filamentGrams: number;
    /**
     * Get filament usage in mm.
     */
    readonly filamentMm: number;
    /**
     * Get number of layers.
     */
    readonly layerCount: number;
    /**
     * Get estimated print time in seconds.
     */
    readonly printTimeSeconds: number;
}

/**
 * Slicer settings for WASM.
 */
export class SlicerSettings {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Create from JSON.
     */
    static fromJson(json: string): SlicerSettings;
    /**
     * Create default settings.
     */
    constructor();
    /**
     * First layer height (mm).
     */
    first_layer_height: number;
    /**
     * Infill density (0-1).
     */
    infill_density: number;
    /**
     * Infill pattern (0=Grid, 1=Lines, 2=Triangles, 3=Honeycomb, 4=Gyroid).
     */
    infill_pattern: number;
    /**
     * Layer height (mm).
     */
    layer_height: number;
    /**
     * Line width (mm).
     */
    line_width: number;
    /**
     * Nozzle diameter (mm).
     */
    nozzle_diameter: number;
    /**
     * Support angle threshold.
     */
    support_angle: number;
    /**
     * Enable support.
     */
    support_enabled: boolean;
    /**
     * Wall count.
     */
    wall_count: number;
}

/**
 * A 3D solid geometry object.
 *
 * Create solids from primitives, combine with boolean operations,
 * transform, and extract triangle meshes for rendering.
 */
export class Solid {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Return mesh boundary edges as a flat float array
     * `[x0, y0, z0, x1, y1, z1, ...]` with each pair of 3-component
     * positions defining one edge segment. Used by the viewport's
     * "show boundary edges" overlay to surface tessellation holes.
     *
     * Closed, manifold meshes return an empty array; each entry means
     * there's a hole in the mesh.
     */
    boundaryEdges(segments?: number | null): Float32Array;
    /**
     * Get the bounding box as [minX, minY, minZ, maxX, maxY, maxZ].
     */
    boundingBox(): Float64Array;
    /**
     * Check if the solid can be exported to STEP format.
     *
     * Returns `true` if the solid has B-rep data available for STEP export.
     * Returns `false` for mesh-only or empty solids.
     */
    canExportStep(): boolean;
    /**
     * Get the center of mass as [x, y, z].
     */
    centerOfMass(): Float64Array;
    /**
     * Chamfer all edges of the solid by the given distance.
     */
    chamfer(distance: number): Solid;
    /**
     * Create a circular pattern of the solid around an axis.
     *
     * # Arguments
     *
     * * `axis_origin_x/y/z` - A point on the rotation axis
     * * `axis_dir_x/y/z` - Direction of the rotation axis
     * * `count` - Number of copies (including original)
     * * `angle_deg` - Total angle span in degrees
     */
    circularPattern(axis_origin_x: number, axis_origin_y: number, axis_origin_z: number, axis_dir_x: number, axis_dir_y: number, axis_dir_z: number, count: number, angle_deg: number): Solid;
    /**
     * Create a cone/frustum along Z axis.
     */
    static cone(radius_bottom: number, radius_top: number, height: number, segments?: number | null): Solid;
    /**
     * Create a box with corner at origin and dimensions (sx, sy, sz).
     */
    static cube(sx: number, sy: number, sz: number): Solid;
    /**
     * Create a cylinder along Z axis with given radius and height.
     */
    static cylinder(radius: number, height: number, segments?: number | null): Solid;
    /**
     * Boolean difference (self − other).
     */
    difference(other: Solid): Solid;
    /**
     * Create an empty solid.
     */
    static empty(): Solid;
    /**
     * Create a solid by extruding a 2D sketch profile.
     *
     * Takes a sketch profile and extrusion direction as JS objects.
     */
    static extrude(profile_json: string, direction: Float64Array): Solid;
    /**
     * Create a solid by extruding a 2D sketch profile with twist and/or scale.
     *
     * Takes a sketch profile, extrusion direction, twist angle (radians),
     * and scale factor at the end (1.0 = no taper).
     */
    static extrudeWithOptions(profile_json: string, direction: Float64Array, twist_angle: number, scale_end: number): Solid;
    /**
     * Fillet all edges of the solid with the given radius.
     */
    fillet(radius: number): Solid;
    /**
     * Get the triangle mesh representation.
     *
     * Returns a JS object with `positions` (Float32Array) and `indices` (Uint32Array).
     *
     * Runs the tessellator output through
     * [`vcad_kernel_tessellate::render_bake`] so the emitted mesh carries
     * angle-based creased vertex normals. Every downstream renderer —
     * three.js today, wgpu / STL / GLB / ray tracer later — consumes this
     * same attribute layout without recomputing anything.
     */
    getMesh(segments?: number | null): any;
    /**
     * Generate a horizontal section view at a given Z height.
     *
     * Convenience method that creates a horizontal section plane.
     */
    horizontalSection(z: number, hatch_spacing?: number | null, hatch_angle?: number | null, segments?: number | null): any;
    /**
     * Boolean intersection (self ∩ other).
     */
    intersection(other: Solid): Solid;
    /**
     * Check if the solid is empty (has no geometry).
     */
    isEmpty(): boolean;
    /**
     * Create a linear pattern of the solid along a direction.
     *
     * # Arguments
     *
     * * `dir_x`, `dir_y`, `dir_z` - Direction vector
     * * `count` - Number of copies (including original)
     * * `spacing` - Distance between copies
     */
    linearPattern(dir_x: number, dir_y: number, dir_z: number, count: number, spacing: number): Solid;
    /**
     * Create a solid by lofting between multiple profiles.
     *
     * Takes an array of sketch profiles (minimum 2).
     */
    static loft(profiles_json: string, closed?: boolean | null): Solid;
    /**
     * Get the number of triangles in the tessellated mesh.
     */
    numTriangles(): number;
    /**
     * Project the solid to a 2D view for technical drawing.
     *
     * # Arguments
     * * `view_direction` - View direction: "front", "back", "top", "bottom", "left", "right", or "isometric"
     * * `segments` - Number of segments for tessellation (optional, default 32)
     *
     * # Returns
     * A JS object containing the projected view with edges and bounds.
     */
    projectView(view_direction: string, segments?: number | null): any;
    /**
     * Create a solid by revolving a 2D sketch profile around an axis.
     *
     * Takes a sketch profile, axis origin, axis direction, and angle in degrees.
     */
    static revolve(profile_json: string, axis_origin: Float64Array, axis_dir: Float64Array, angle_deg: number): Solid;
    /**
     * Rotate the solid by angles in degrees around X, Y, Z axes.
     */
    rotate(x_deg: number, y_deg: number, z_deg: number): Solid;
    /**
     * Scale the solid by (x, y, z).
     */
    scale(x: number, y: number, z: number): Solid;
    /**
     * Generate a section view by cutting the solid with a plane.
     *
     * # Arguments
     * * `plane_json` - JSON string with plane definition: `{"origin": [x,y,z], "normal": [x,y,z], "up": [x,y,z]}`
     * * `hatch_json` - Optional JSON string with hatch pattern: `{"spacing": f64, "angle": f64}`
     * * `segments` - Number of segments for tessellation (optional, default 32)
     *
     * # Returns
     * A JS object containing the section view with curves, hatch lines, and bounds.
     */
    sectionView(plane_json: string, hatch_json?: string | null, segments?: number | null): any;
    /**
     * Shell (hollow) the solid by offsetting all faces inward.
     */
    shell(thickness: number): Solid;
    /**
     * Create a sphere centered at origin with given radius.
     */
    static sphere(radius: number, segments?: number | null): Solid;
    /**
     * Compute the surface area of the solid.
     */
    surfaceArea(): number;
    /**
     * Create a solid by sweeping a profile along a helix path.
     *
     * Takes a sketch profile and helix parameters.
     */
    static sweepHelix(profile_json: string, radius: number, pitch: number, height: number, turns: number, twist_angle?: number | null, scale_start?: number | null, scale_end?: number | null, path_segments?: number | null, arc_segments?: number | null, orientation?: number | null): Solid;
    /**
     * Create a solid by sweeping a profile along a line path.
     *
     * Takes a sketch profile and path endpoints.
     */
    static sweepLine(profile_json: string, start: Float64Array, end: Float64Array, twist_angle?: number | null, scale_start?: number | null, scale_end?: number | null, orientation?: number | null): Solid;
    /**
     * Create a solid by extruding text as 2D profiles.
     *
     * Converts text to sketch profiles and extrudes them. Each character glyph
     * becomes a separate profile, and holes (like in 'O') are subtracted.
     *
     * # Arguments
     *
     * * `text` - The text string to convert
     * * `origin` - Origin point [x, y, z]
     * * `x_dir` - X direction vector [x, y, z]
     * * `y_dir` - Y direction vector [x, y, z]
     * * `direction` - Extrusion direction [x, y, z] (magnitude = extrusion depth)
     * * `height` - Text height in mm
     * * `font` - Font name (currently only "sans-serif" supported)
     * * `alignment` - Text alignment: "left", "center", or "right"
     * * `letter_spacing` - Letter spacing multiplier (1.0 = normal)
     * * `line_spacing` - Line spacing multiplier (1.0 = normal)
     */
    static textExtrude(text: string, origin: Float64Array, x_dir: Float64Array, y_dir: Float64Array, direction: Float64Array, height: number, font?: string | null, alignment?: string | null, letter_spacing?: number | null, line_spacing?: number | null): Solid;
    /**
     * Export the solid to STEP format.
     *
     * # Returns
     * A byte buffer containing the STEP file data.
     *
     * # Errors
     * Returns an error if the solid has no B-rep data (e.g., mesh-only after certain operations).
     */
    toStepBuffer(): Uint8Array;
    /**
     * Translate the solid by (x, y, z).
     */
    translate(x: number, y: number, z: number): Solid;
    /**
     * Boolean union (self ∪ other).
     */
    union(other: Solid): Solid;
    /**
     * Compute the volume of the solid.
     */
    volume(): number;
}

/**
 * Annotation layer for dimension annotations.
 *
 * This class provides methods for creating and rendering dimension annotations
 * on 2D projected views.
 */
export class WasmAnnotationLayer {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Add an aligned dimension between two points.
     *
     * The dimension line is parallel to the line connecting the two points.
     *
     * # Arguments
     * * `x1`, `y1` - First point coordinates
     * * `x2`, `y2` - Second point coordinates
     * * `offset` - Distance from points to dimension line
     */
    addAlignedDimension(x1: number, y1: number, x2: number, y2: number, offset: number): void;
    /**
     * Add an angular dimension between three points.
     *
     * The angle is measured at the vertex (middle point).
     *
     * # Arguments
     * * `x1`, `y1` - First point on one leg
     * * `vx`, `vy` - Vertex point (angle measured here)
     * * `x2`, `y2` - Second point on other leg
     * * `arc_radius` - Radius of the arc showing the angle
     */
    addAngleDimension(x1: number, y1: number, vx: number, vy: number, x2: number, y2: number, arc_radius: number): void;
    /**
     * Add a diameter dimension for a circle.
     *
     * # Arguments
     * * `cx`, `cy` - Center of the circle
     * * `radius` - Radius of the circle
     * * `leader_angle` - Angle in radians for the leader line direction
     */
    addDiameterDimension(cx: number, cy: number, radius: number, leader_angle: number): void;
    /**
     * Add a horizontal dimension between two points.
     *
     * # Arguments
     * * `x1`, `y1` - First point coordinates
     * * `x2`, `y2` - Second point coordinates
     * * `offset` - Distance from points to dimension line (positive = above)
     */
    addHorizontalDimension(x1: number, y1: number, x2: number, y2: number, offset: number): void;
    /**
     * Add a radius dimension for a circle.
     *
     * # Arguments
     * * `cx`, `cy` - Center of the circle
     * * `radius` - Radius of the circle
     * * `leader_angle` - Angle in radians for the leader line direction
     */
    addRadiusDimension(cx: number, cy: number, radius: number, leader_angle: number): void;
    /**
     * Add a vertical dimension between two points.
     *
     * # Arguments
     * * `x1`, `y1` - First point coordinates
     * * `x2`, `y2` - Second point coordinates
     * * `offset` - Distance from points to dimension line (positive = right)
     */
    addVerticalDimension(x1: number, y1: number, x2: number, y2: number, offset: number): void;
    /**
     * Get the number of annotations in the layer.
     */
    annotationCount(): number;
    /**
     * Clear all annotations from the layer.
     */
    clear(): void;
    /**
     * Check if the layer has any annotations.
     */
    isEmpty(): boolean;
    /**
     * Create a new empty annotation layer.
     */
    constructor();
    /**
     * Render all dimensions and return as JSON.
     *
     * Returns an array of rendered dimensions, each containing:
     * - `lines`: Array of line segments [[x1, y1], [x2, y2]]
     * - `arcs`: Array of arc definitions
     * - `arrows`: Array of arrow definitions
     * - `texts`: Array of text labels
     *
     * # Arguments
     * * `view_json` - Optional JSON string of a ProjectedView for geometry resolution
     */
    renderAll(view_json?: string | null): any;
}

/**
 * CAM settings for WASM.
 */
export class WasmCamSettings {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Create from JSON.
     */
    static fromJson(json: string): WasmCamSettings;
    /**
     * Create default CAM settings.
     */
    constructor();
    /**
     * Feed rate (mm/min).
     */
    feed_rate: number;
    /**
     * Plunge rate (mm/min).
     */
    plunge_rate: number;
    /**
     * Retract Z height (mm).
     */
    retract_z: number;
    /**
     * Safe Z height (mm).
     */
    safe_z: number;
    /**
     * Spindle RPM.
     */
    spindle_rpm: number;
    /**
     * Stepdown distance (mm).
     */
    stepdown: number;
    /**
     * Stepover distance (mm).
     */
    stepover: number;
}

/**
 * CRDT-backed document engine for WASM.
 *
 * Wraps a `DocumentApi` (which wraps a `CrdtDocument`) and exposes both
 * typed mutations via `add_feature(json)` and legacy low-level CRDT methods.
 */
export class WasmDocumentEngine {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Add a feature from a JSON-serialized `FeatureInput` discriminated union.
     *
     * Example: `{"type":"Cube","size_x":10,"size_y":20,"size_z":30}`
     *
     * Returns `{ document, parts, consumedPartIds, createdFeatureId }`.
     */
    add_feature(input_json: string): any;
    /**
     * Whether redo is available.
     */
    can_redo(): boolean;
    /**
     * Whether undo is available.
     */
    can_undo(): boolean;
    /**
     * Compute a FractionalIndex position between two neighbor feature IDs.
     */
    compute_position_between(before_id_json: string, after_id_json: string): string;
    /**
     * Create a feature with the given kind and params (JSON string).
     *
     * Returns `{ document, parts, createdFeatureId }` as a JsValue.
     */
    create_feature(kind: string, params_json: string): any;
    /**
     * Delete a feature by ID (JSON string).
     */
    delete_feature(feature_id_json: string): any;
    /**
     * Delete a feature by stable ID.
     */
    delete_feature_by_id(stable_id: string): any;
    /**
     * Load a legacy v1 JSON document and migrate to CRDT.
     */
    static from_v1_json(json: string): WasmDocumentEngine;
    /**
     * Get the materialized document as JSON.
     */
    get_document_json(): string;
    /**
     * Get operations since a remote clock state (JSON).
     */
    get_ops_since(remote_clock_json: string): string;
    /**
     * Get ordered features (for the feature tree) as JSON.
     */
    get_ordered_features_json(): string;
    /**
     * Get the parts list as JSON.
     */
    get_parts_json(): string;
    /**
     * Get the sync clock as JSON.
     */
    get_sync_clock(): string;
    /**
     * Import IR JSON into the current document (e.g. AI-generated geometry).
     */
    import_ir(ir_json: string): any;
    /**
     * Load a document from bytes.
     *
     * Auto-detects format: if CRDT (v2), loads directly; if legacy JSON (v1),
     * migrates to CRDT first.
     */
    static load(bytes: Uint8Array): WasmDocumentEngine;
    /**
     * Merge remote operations (JSON array of Op).
     */
    merge_remote(ops_json: string): any;
    /**
     * Move a feature to a new position.
     */
    move_feature(feature_id_json: string, position_json: string): any;
    /**
     * Create a new empty document engine.
     */
    constructor();
    /**
     * Redo the last undone action.
     */
    redo(): any;
    /**
     * Rename a feature.
     */
    rename_feature(stable_id: string, name: string): any;
    /**
     * Save the document to bytes.
     */
    save(): Uint8Array;
    /**
     * Set joint state.
     */
    set_joint_state(stable_id: string, state: number): any;
    /**
     * Set material on a feature.
     */
    set_material(stable_id: string, material: string): any;
    /**
     * Set a parameter on a feature.
     */
    set_param(feature_id_json: string, key: string, value_json: string): any;
    /**
     * Set rotation on a feature.
     */
    set_rotation(stable_id: string, x: number, y: number, z: number): any;
    /**
     * Set scale on a feature.
     */
    set_scale(stable_id: string, x: number, y: number, z: number): any;
    /**
     * Set translation on a feature.
     */
    set_translation(stable_id: string, x: number, y: number, z: number): any;
    /**
     * Set visibility on a feature.
     */
    set_visible(stable_id: string, visible: boolean): any;
    /**
     * Undo the last action.
     */
    undo(): any;
    /**
     * Update a feature with new params from a JSON-serialized `FeatureInput`.
     */
    update_feature(stable_id: string, input_json: string): any;
}

export class WasmKeybindings {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Return the effective chord (user override or default) for a command
     * id, or `None` if disabled / unbound.
     */
    chordFor(id: string): string | undefined;
    /**
     * Returns a JSON array describing every registered command. The TS UI
     * (command palette, keyboard prefs) reads this once at startup.
     *
     * Each entry is a [`CommandView`] — a flattened, owned projection of
     * `Command` that serde can serialize (the source struct uses `&'static
     * str` and a non-serializable `ModeScope` enum).
     */
    commandsJson(): string;
    /**
     * Report binding conflicts in the given mode: pairs of commands that
     * share the same chord. Returns a JSON array for the prefs UI to
     * highlight.
     */
    conflictsJson(mode_name: string): string;
    /**
     * Load overrides previously returned by [`save_overrides`]. Malformed
     * entries are skipped — the caller never sees a parse failure for
     * stale config.
     */
    loadOverrides(json: string): boolean;
    /**
     * Construct a fresh registry with all default bindings.
     */
    constructor();
    /**
     * Clear all user overrides, restoring default bindings.
     */
    resetAll(): void;
    /**
     * Resolve a chord to a command id.
     *
     * - `chord_json` is the JSON-serialized [`Chord`] produced by the TS
     *   adapter (`chord.ts` normalizes `KeyboardEvent` → `Chord`).
     * - `mode_name` is one of `"Normal" | "Sketch" | "Assembly" | ...`
     *   (see [`AppMode`]).
     * - `ctx_bits` is the packed u32 from [`WhenContext::bits`].
     *
     * Returns the command id on match, or `None` — the TS side checks for
     * `null` and falls through if nothing binds.
     */
    resolve(chord_json: string, mode_name: string, ctx_bits: number): string | undefined;
    /**
     * Serialize user overrides for persistence (e.g. localStorage).
     */
    saveOverrides(): string;
    /**
     * Rebind a command. Pass a JSON-encoded chord to set, or `None` to
     * clear (disabling the binding).
     */
    setBinding(id: string, chord_json?: string | null): void;
}

/**
 * A sketch editing session bound to JavaScript. See module docs.
 */
export class WasmSketchSession {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Add a full circle.
     */
    addCircle(cx: number, cy: number, radius: number): void;
    /**
     * Add a constraint from a JSON object matching the TypeScript
     * `SketchConstraint` shape.
     */
    addConstraint(json: string): void;
    /**
     * Add a line directly (for scripted / MCP use).
     */
    addLine(x1: number, y1: number, x2: number, y2: number): void;
    /**
     * Add an axis-aligned rectangle between two corners.
     */
    addRectangle(x1: number, y1: number, x2: number, y2: number): void;
    /**
     * Clear pending input.
     */
    cancelPending(): void;
    /**
     * Clear every entity and constraint.
     */
    clear(): void;
    /**
     * Clear the selection.
     */
    clearSelection(): void;
    /**
     * Construct a new session on the given plane.
     *
     * `plane_json` is either a JSON string (`"XY"` / `"XZ"` / `"YZ"`) or a
     * JSON object `{ origin, xDir, yDir }` for a custom plane.
     */
    constructor(plane_json: string);
    /**
     * Handle a primary-button click at the current cursor position. Returns
     * a short outcome string: `"no-cursor"`, `"selection"`, `"pending"`, or
     * `"committed"`.
     */
    onClick(): string;
    /**
     * Clear the cursor.
     */
    onCursorLeave(): void;
    /**
     * Update the cursor from a world-space ray (e.g. camera pick ray).
     */
    onCursorRay(ox: number, oy: number, oz: number, dx: number, dy: number, dz: number): void;
    /**
     * Update the cursor directly from 2D sketch coordinates.
     */
    onCursorSketch(x: number, y: number): void;
    /**
     * Handle a double-click (closes a polyline for the line tool).
     */
    onDoubleClick(): void;
    /**
     * Redo the last undone mutation. Returns `true` if anything was redone.
     */
    redo(): boolean;
    /**
     * Remove the constraint at `index`.
     */
    removeConstraint(index: number): void;
    /**
     * Configure snapping behavior.
     */
    setSnap(grid_enabled: boolean, grid_size: number, point_enabled: boolean, point_tolerance: number): void;
    /**
     * Change the active drawing tool. Unknown names are ignored.
     */
    setTool(tool: string): void;
    /**
     * Return a JSON snapshot of the full session state. React can mirror
     * this into its own store on every mutation.
     */
    snapshot(): string;
    /**
     * Run the constraint solver. Returns `true` if it converged.
     */
    solve(): boolean;
    /**
     * Test-select or deselect a segment.
     */
    toggleSelection(segment_index: number): void;
    /**
     * Undo the last mutation. Returns `true` if anything was undone.
     */
    undo(): boolean;
}

/**
 * Analyze a solid for 3D printing characteristics.
 *
 * Returns JSON with wall thicknesses, overhang angles, hole sizes, etc.
 * Only works on solids with BRep data (primitives, not boolean results).
 */
export function analyzeForPrinting(solid: Solid): any;

/**
 * Build a built-in part's sub-document given its path and params JSON.
 *
 * `path` is either a bare id (`"fastener.bolt.socket-head"`) or prefixed
 * with `std:`. `params_json` is a JSON object whose keys are parameter
 * names. Returns a JSON-serialized [`vcad_ir::Document`] that the engine
 * can splice into the parent document.
 */
export function buildPart(path: string, params_json: string): string;

/**
 * Build the system prompt sent with every `/api/chat` request.
 *
 * `parts_json` must deserialize into `Vec<vcad_chat::PartInfo>` (the TS
 * web caller already walks its own document store to build this shape,
 * so we accept it pre-built rather than reserializing the full Document
 * through the wasm boundary on every request). `selection_json` must
 * deserialize into `Vec<vcad_chat::SelectionInfo>`. Either defaults to
 * an empty array on parse failure.
 *
 * Returns the rendered prompt string — byte-identical to what the TUI
 * produces via `vcad_chat::build_system_prompt` for the same inputs.
 */
export function build_chat_system_prompt(parts_json: string, selection_json: string): string;

/**
 * Generate a height field from mesh using drop-cutter algorithm.
 *
 * # Arguments
 * * `vertices_json` - Vertex array as JSON [[x,y,z], ...]
 * * `indices_json` - Triangle indices as JSON [i0, i1, i2, ...]
 * * `tool_json` - Tool definition as JSON
 * * `bounds_json` - Bounds [min_x, min_y, max_x, max_y] as JSON
 * * `resolution` - Sample spacing in mm
 *
 * # Returns
 * Height field as JSON with { nx, ny, bounds, heights }
 */
export function camDropCutter(vertices_json: string, indices_json: string, tool_json: string, bounds_json: string, resolution: number): string;

/**
 * Export toolpath to GRBL G-code.
 *
 * # Arguments
 * * `toolpath_json` - Toolpath as JSON string
 * * `job_name` - Name for the G-code file header
 * * `tool_json` - Tool definition as JSON
 * * `settings` - CAM settings
 *
 * # Returns
 * G-code as string.
 */
export function camExportGcode(toolpath_json: string, job_name: string, tool_json: string, settings: WasmCamSettings): string;

/**
 * Export toolpath to LinuxCNC G-code.
 *
 * # Arguments
 * * `toolpath_json` - Toolpath as JSON string
 * * `job_name` - Name for the G-code file header
 * * `tool_json` - Tool definition as JSON
 * * `settings` - CAM settings
 * * `program_number` - O-word program number
 *
 * # Returns
 * G-code as string.
 */
export function camExportLinuxCnc(toolpath_json: string, job_name: string, tool_json: string, settings: WasmCamSettings, program_number: number): string;

/**
 * Generate a circular pocket toolpath.
 *
 * # Arguments
 * * `cx`, `cy` - Center point
 * * `radius` - Pocket radius
 * * `depth` - Cut depth
 * * `tool_json` - Tool definition as JSON
 * * `settings` - CAM settings
 *
 * # Returns
 * Toolpath as JSON string.
 */
export function camGenerateCircularPocket(cx: number, cy: number, radius: number, depth: number, tool_json: string, settings: WasmCamSettings): string;

/**
 * Generate a rectangular contour toolpath.
 *
 * # Arguments
 * * `x`, `y` - Top-left corner
 * * `width`, `height` - Rectangle dimensions
 * * `depth` - Cut depth
 * * `offset` - Offset from contour (positive = outside)
 * * `tab_count` - Number of tabs (0 for none)
 * * `tab_width` - Tab width in mm
 * * `tab_height` - Tab height in mm
 * * `tool_json` - Tool definition as JSON
 * * `settings` - CAM settings
 *
 * # Returns
 * Toolpath as JSON string.
 */
export function camGenerateContour(x: number, y: number, width: number, height: number, depth: number, offset: number, tab_count: number, tab_width: number, tab_height: number, tool_json: string, settings: WasmCamSettings): string;

/**
 * Generate a face toolpath.
 *
 * # Arguments
 * * `min_x`, `min_y`, `max_x`, `max_y` - Bounds of the area to face
 * * `depth` - Cut depth (positive value)
 * * `tool_json` - Tool definition as JSON
 * * `settings` - CAM settings
 *
 * # Returns
 * Toolpath as JSON string.
 */
export function camGenerateFace(min_x: number, min_y: number, max_x: number, max_y: number, depth: number, tool_json: string, settings: WasmCamSettings): string;

/**
 * Generate a rectangular pocket toolpath.
 *
 * # Arguments
 * * `x`, `y` - Top-left corner
 * * `width`, `height` - Pocket dimensions
 * * `depth` - Cut depth
 * * `tool_json` - Tool definition as JSON
 * * `settings` - CAM settings
 *
 * # Returns
 * Toolpath as JSON string.
 */
export function camGeneratePocket(x: number, y: number, width: number, height: number, depth: number, tool_json: string, settings: WasmCamSettings): string;

/**
 * Generate 3D roughing toolpath from a height field.
 *
 * # Arguments
 * * `height_field_json` - Height field from cam_drop_cutter
 * * `tool_json` - Tool definition as JSON
 * * `settings` - CAM settings
 * * `target_z` - Target bottom Z depth
 * * `top_z` - Top Z (stock surface)
 * * `stock_margin` - Extra material to leave (mm)
 * * `direction` - Raster direction in degrees (0=X, 90=Y)
 *
 * # Returns
 * Toolpath as JSON string.
 */
export function camGenerateRoughing3d(height_field_json: string, tool_json: string, settings: WasmCamSettings, target_z: number, top_z: number, stock_margin: number, direction: number): string;

/**
 * Get default tool library.
 *
 * # Returns
 * Tool library as JSON array.
 */
export function camGetDefaultTools(): string;

/**
 * Get toolpath statistics.
 *
 * # Arguments
 * * `toolpath_json` - Toolpath as JSON string
 *
 * # Returns
 * JSON object with statistics: { cutting_length, estimated_time, bounding_box }
 */
export function camToolpathStats(toolpath_json: string): any;

/**
 * Check a solid for DFM (Design for Manufacturing) printability issues.
 *
 * Returns warnings with face indices for viewport highlighting.
 */
export function checkPrintability(solid: Solid, printer_profile: string): any;

/**
 * Compute creased normals using GPU acceleration.
 *
 * # Arguments
 * * `positions` - Flat array of vertex positions (x, y, z, ...)
 * * `indices` - Triangle indices
 * * `crease_angle` - Angle in radians; faces meeting at sharper angles get hard edges
 *
 * # Returns
 * Flat array of normals (nx, ny, nz, ...), same length as positions.
 */
export function computeCreasedNormalsGpu(positions: Float32Array, indices: Uint32Array, crease_angle: number): Promise<Float32Array>;

/**
 * Compute volume of a closed triangle mesh using the divergence theorem.
 *
 * Positions are `[x, y, z, ...]` (flat f32), indices are `[i0, i1, i2, ...]`.
 * Returns volume in mm³ (same units as positions).
 */
export function computeMeshVolume(positions: Float32Array, indices: Uint32Array): number;

/**
 * Create a detail view from a projected view.
 *
 * A detail view is a magnified region of a parent view, useful for showing
 * fine features that would be too small in the main view.
 *
 * # Arguments
 * * `parent_json` - JSON string of the parent ProjectedView
 * * `center_x` - X coordinate of the region center
 * * `center_y` - Y coordinate of the region center
 * * `scale` - Magnification factor (e.g., 2.0 = 2x)
 * * `width` - Width of the region to capture
 * * `height` - Height of the region to capture
 * * `label` - Label for the detail view (e.g., "A")
 *
 * # Returns
 * A JS object containing the detail view with edges and bounds.
 */
export function createDetailView(parent_json: string, center_x: number, center_y: number, scale: number, width: number, height: number, label: string): any;

/**
 * Decimate a mesh to reduce triangle count.
 *
 * # Arguments
 * * `positions` - Flat array of vertex positions
 * * `indices` - Triangle indices
 * * `target_ratio` - Target ratio of triangles to keep (0.5 = 50%)
 *
 * # Returns
 * A JS object with decimated positions, indices, and normals.
 */
export function decimateMeshGpu(positions: Float32Array, indices: Uint32Array, target_ratio: number): Promise<any>;

/**
 * Derive parts from a Document (as JSON).
 *
 * Returns a JSON-serialized Vec<PartInfo>.
 */
export function deriveParts(doc_json: string): any;

/**
 * Digitize sketch segments into embroidery stitches.
 *
 * Takes a JSON array of `SketchSegment2D` (from a Sketch2D node) plus
 * stitch options, and returns an `EmbPattern` JSON string.
 */
export function digitizeSketch(segments_json: string, options_json: string): string;

/**
 * Digitize text into embroidery stitches.
 *
 * Converts a text string into glyph outlines, then applies the specified
 * stitch algorithm (running, satin, or fill) to produce an `EmbPattern`.
 * Returns the same JSON shape as `readEmbroideryPes`.
 */
export function digitizeText(text: string, height: number, options_json: string): string;

/**
 * Convert a Document (as JSON) back to loon source code.
 */
export function documentToLoon(doc_json: string): string;

/**
 * Convert a Document (as JSON) to loon, also returning unsupported variant names.
 *
 * Returns a JS object `{ source: string, unsupported: string[] }`.
 * When `unsupported` is non-empty, the output contains comment placeholders for
 * those nodes and callers should warn the user that data will be lost.
 */
export function documentToLoonChecked(doc_json: string): any;

/**
 * Return all builtin symbol definitions.
 *
 * # Returns
 * Array of `SymbolDef` as JsValue.
 */
export function ecadBuiltinSymbols(): any;

/**
 * Run Design Rule Check on a PCB layout.
 *
 * # Arguments
 * * `pcb_json` - JSON-serialized `Pcb` struct
 *
 * # Returns
 * Array of DRC violations as JsValue.
 */
export function ecadCheckDrc(pcb_json: string): any;

/**
 * Run Electrical Rule Check on a schematic sheet.
 *
 * # Arguments
 * * `sch_json` - JSON-serialized `SchematicSheet` struct
 *
 * # Returns
 * Array of ERC violations as JsValue.
 */
export function ecadCheckErc(sch_json: string): any;

/**
 * Generate 3D component body meshes for all footprints on a PCB.
 *
 * # Arguments
 * * `pcb_json` - JSON-serialized `Pcb` struct
 *
 * # Returns
 * Array of component meshes as JsValue.
 */
export function ecadComponentMeshes(pcb_json: string): any;

/**
 * Compute ratsnest lines for unrouted net connections.
 *
 * # Arguments
 * * `pcb_json` - JSON-serialized `Pcb` struct
 * * `netlist_json` - JSON-serialized netlist
 *
 * # Returns
 * Array of ratsnest lines as JsValue.
 */
export function ecadComputeRatsnest(pcb_json: string, netlist_json: string): any;

/**
 * Fill copper pour zones on the PCB.
 *
 * # Arguments
 * * `pcb_json` - JSON-serialized `Pcb` struct
 *
 * # Returns
 * Array of filled zone polygons.
 */
export function ecadFillZones(pcb_json: string): any;

/**
 * Generate a netlist from a schematic sheet.
 *
 * # Arguments
 * * `sch_json` - JSON-serialized `SchematicSheet` struct
 *
 * # Returns
 * Netlist as JsValue.
 */
export function ecadGenerateNetlist(sch_json: string): any;

/**
 * Look up a single builtin symbol by ID.
 *
 * # Arguments
 * * `id` - Symbol identifier (e.g. "resistor", "capacitor", "npn")
 *
 * # Returns
 * `SymbolDef` as JsValue, or null if not found.
 */
export function ecadGetSymbol(id: string): any;

/**
 * Compute Z offset for a PCB layer.
 *
 * # Arguments
 * * `layer` - Layer name (e.g. "FCu", "BCu")
 * * `thickness` - Board thickness in mm
 * * `explosion` - Explosion factor (0 = normal, >0 = exploded)
 */
export function ecadLayerZ(layer: string, thickness: number, explosion: number): number;

/**
 * Get the net for a wire based on endpoint proximity to component pins.
 *
 * # Arguments
 * * `wire_json` - JSON-serialized `SchematicWire`
 * * `netlist_json` - JSON-serialized `Netlist`
 * * `components_json` - JSON-serialized `SchematicComponent[]`
 *
 * # Returns
 * Net name as string, or null.
 */
export function ecadNetForWire(wire_json: string, netlist_json: string, components_json: string): any;

/**
 * Route a net between two points on the PCB using the grid router.
 *
 * # Arguments
 * * `pcb_json` - JSON-serialized `Pcb` struct
 * * `net` - Net name to route
 * * `start_x`, `start_y` - Start coordinates (mm)
 * * `end_x`, `end_y` - End coordinates (mm)
 * * `width` - Trace width (mm)
 *
 * # Returns
 * Route result with segments and vias.
 */
export function ecadRouteNet(pcb_json: string, net: string, start_x: number, start_y: number, end_x: number, end_y: number, width: number): any;

/**
 * Snap a position to the nearest component pin or grid point.
 *
 * # Arguments
 * * `x`, `y` - Cursor position
 * * `components_json` - JSON-serialized `SchematicComponent[]`
 * * `grid` - Grid spacing
 * * `threshold` - Max distance to snap to a pin
 *
 * # Returns
 * `{ position: { x, y }, is_pin: bool }` as JsValue.
 */
export function ecadSnapToGridOrPin(x: number, y: number, components_json: string, grid: number, threshold: number): any;

/**
 * Estimate print cost from volume (instant, pre-slice).
 */
export function estimatePrintCost(volume_mm3: number, infill_density: number, wall_count: number, line_width: number, material_name: string): any;

/**
 * Evaluate a loon source string and return a JSON-serialized vcad Document.
 *
 * The vcad library (types, constructors) is automatically prepended.
 * Module resolution (`[use ...]`) is not available in WASM — all code
 * must be self-contained or use the bundled vcad library.
 */
export function evalVcadSource(source: string): any;

/**
 * Evaluate a full vcad document JSON into a serialized EvaluatedScene.
 *
 * This is the canonical Rust-side evaluator that handles all CsgOp variants
 * including Sketch2D, Extrude, Revolve, Sweep, Loft, Text2D, ImportedMesh,
 * assembly with forward kinematics, and clash detection.
 *
 * # Arguments
 *
 * * `doc_json` - A JSON string representing a vcad Document
 * * `skip_clash_detection` - If true, skip O(n²) clash detection
 *
 * # Returns
 *
 * A JsValue containing the serialized EvaluatedScene.
 */
export function evaluateDocument(doc_json: string, skip_clash_detection: boolean): any;

/**
 * Evaluate VCode and return a Solid for rendering.
 *
 * This is a convenience function that parses VCode and evaluates
 * the geometry in a single step.
 *
 * # Arguments
 * * `vcode` - The VCode text to evaluate
 *
 * # Returns
 * A Solid object that can be rendered or queried.
 */
export function evaluateVCode(vcode: string): Solid;

/**
 * Export a projected view to DXF format.
 *
 * Returns the DXF content as bytes.
 *
 * # Arguments
 * * `view_json` - JSON string of a ProjectedView
 *
 * # Returns
 * A byte array containing the DXF file content.
 */
export function exportProjectedViewToDxf(view_json: string): Uint8Array;

/**
 * Generate a 3MF file from mesh data.
 *
 * Returns the 3MF file as a byte array suitable for download or upload to a printer.
 */
export function generate3mf(name: string, vertices: Float32Array, indices: Uint32Array, settings_json: string): Uint8Array;

/**
 * Generate a Bambu sliced `.gcode.3mf` containing the mesh and the
 * pre-generated G-code, ready to send to a Bambu printer over LAN.
 */
export function generate3mfWithGcode(name: string, vertices: Float32Array, indices: Uint32Array, gcode: Uint8Array, settings_json: string): Uint8Array;

/**
 * Generate G-code from slice result.
 */
export function generateGcode(result: SliceResult, printer_profile: string, print_temp: number, bed_temp: number): string;

/**
 * Returns the `WebAssembly.Module` instance backing this kernel-wasm
 * import. Workers can pass this to `wasm.default({ module_or_path })`
 * to skip the multi-second recompile of a fresh fetch — see
 * `packages/engine/src/eval-worker.ts` for the consumer.
 */
export function getCompiledModule(): any;

/**
 * Return the full parts manifest JSON for the built-in stdlib.
 *
 * The app consumes this on boot to populate the palette's Parts tab and
 * the Cmd+K search index.
 */
export function getPartsManifest(): string;

/**
 * Get available printer profiles.
 */
export function getSlicerPrinterProfiles(): any;

/**
 * Get the five Anthropic CRUD tool definitions
 * (`create` / `read` / `update` / `delete` / `set_material`) as a JSON
 * array, with the `create` tool's `type` enum pre-populated from the
 * kernel's tool schema list. Consumers on the web (TypeScript
 * `CommandRegistry.toAnthropicTools`) and in the TUI (`vcad_chat::
 * anthropic_tools`) render byte-identical payloads — single source of
 * truth lives in `vcad-chat::tools`.
 */
export function get_anthropic_tools_json(): string;

/**
 * Get the kernel version string.
 * Use this in browser console to verify the correct WASM build is loaded:
 * `kernelWasm.get_kernel_version()` returns `<crate-version>-<sha>[-dirty]`.
 */
export function get_kernel_version(): string;

/**
 * Get tool schema definitions for all CsgOp variants.
 * Returns JSON array of ToolSchemaEntry objects.
 */
export function get_tool_schemas(): string;

/**
 * Import solids from STEP file bytes.
 *
 * Returns a JS array of mesh data for each imported body.
 * Each mesh contains `positions` (Float32Array) and `indices` (Uint32Array).
 *
 * # Arguments
 * * `data` - Raw STEP file contents as bytes
 *
 * # Returns
 * A JS array of mesh objects for rendering the imported geometry.
 */
export function importStepBuffer(data: Uint8Array): any;

/**
 * Import a URDF (Unified Robot Description Format) file and return a
 * serialised vcad [`Document`].
 *
 * Browsers cannot resolve `package://` URIs or relative mesh paths
 * against the user's filesystem, so any `<mesh>` reference in the URDF
 * falls back to a 1cm placeholder cube — the kinematic + inertial tree
 * is still imported correctly. Loading STL/DAE meshes in the browser
 * would require either uploading them alongside or vendoring them.
 *
 * # Arguments
 *
 * * `data` - Raw URDF XML bytes (UTF-8).
 *
 * # Returns
 *
 * JSON-encoded `Document` string. The web app parses it via
 * `Document.fromJson` (TS) or `vcad_ir::Document::from_json` (Rust).
 */
export function importUrdfBuffer(data: Uint8Array): string;

/**
 * Initialize the WASM module (sets up panic hook for better error messages).
 */
export function init(): void;

/**
 * Initialize the GPU context for accelerated geometry processing.
 *
 * Returns `true` if WebGPU is available and initialized, `false` otherwise.
 * This should be called once at application startup.
 */
export function initGpu(): Promise<boolean>;

/**
 * Check if CAM is available.
 */
export function isCamAvailable(): boolean;

/**
 * Check if ECAD features are available in this build.
 */
export function isEcadAvailable(): boolean;

/**
 * Check if embroidery support is available.
 */
export function isEmbroideryAvailable(): boolean;

/**
 * Check if GPU processing is available.
 */
export function isGpuAvailable(): boolean;

/**
 * Check if physics simulation is available.
 */
export function isPhysicsAvailable(): boolean;

/**
 * Check if slicer is available.
 */
export function isSlicerAvailable(): boolean;

/**
 * Chamfer all edges of a solid by the given distance.
 *
 * This is a standalone wrapper for lazy loading via wasmosis.
 */
export function op_chamfer(solid: Solid, distance: number): Solid;

/**
 * Create a circular pattern of a solid around an axis.
 *
 * This is a standalone wrapper for lazy loading via wasmosis.
 */
export function op_circular_pattern(solid: Solid, axis_origin_x: number, axis_origin_y: number, axis_origin_z: number, axis_dir_x: number, axis_dir_y: number, axis_dir_z: number, count: number, angle_deg: number): Solid;

/**
 * Fillet all edges of a solid with the given radius.
 *
 * This is a standalone wrapper for lazy loading via wasmosis.
 */
export function op_fillet(solid: Solid, radius: number): Solid;

/**
 * Create a linear pattern of a solid along a direction.
 *
 * This is a standalone wrapper for lazy loading via wasmosis.
 */
export function op_linear_pattern(solid: Solid, dir_x: number, dir_y: number, dir_z: number, count: number, spacing: number): Solid;

/**
 * Create a solid by lofting between multiple profiles.
 *
 * This is a standalone wrapper for lazy loading via wasmosis.
 */
export function op_loft(profiles_json: string, closed?: boolean | null): Solid;

/**
 * Create a solid by revolving a 2D sketch profile around an axis.
 *
 * This is a standalone wrapper for lazy loading via wasmosis.
 */
export function op_revolve(profile_json: string, axis_origin: Float64Array, axis_dir: Float64Array, angle_deg: number): Solid;

/**
 * Shell (hollow) a solid by offsetting all faces inward.
 *
 * This is a standalone wrapper for lazy loading via wasmosis.
 */
export function op_shell(solid: Solid, thickness: number): Solid;

/**
 * Create a solid by sweeping a profile along a helix path.
 *
 * This is a standalone wrapper for lazy loading via wasmosis.
 */
export function op_sweep_helix(profile_json: string, radius: number, pitch: number, height: number, turns: number, twist_angle?: number | null, scale_start?: number | null, scale_end?: number | null, path_segments?: number | null, arc_segments?: number | null, orientation?: number | null): Solid;

/**
 * Create a solid by sweeping a profile along a line path.
 *
 * This is a standalone wrapper for lazy loading via wasmosis.
 */
export function op_sweep_line(profile_json: string, start: Float64Array, end: Float64Array, twist_angle?: number | null, scale_start?: number | null, scale_end?: number | null, orientation?: number | null): Solid;

/**
 * Parse a KiCad `.kicad_pcb` file content into a JSON-serialized `Pcb`.
 *
 * # Arguments
 * * `content` - The `.kicad_pcb` file content as a string
 *
 * # Returns
 * JSON-serialized `Pcb` struct as JsValue, or error.
 */
export function parseKicadPcb(content: string): any;

/**
 * Parse VCode text format into a vcad IR Document (JSON).
 *
 * The VCode format is a token-efficient text representation designed
 * for ML model training and inference. See `vcad_ir::vcode` for format details.
 *
 * # Arguments
 * * `vcode` - The VCode text to parse
 *
 * # Returns
 * A JSON string representing the parsed vcad IR Document.
 *
 * # Example
 * ```javascript
 * const ir = "C 50 30 5\nY 5 10\nT 1 25 15 0\nD 0 2";
 * const doc = parseVCode(ir);
 * console.log(doc); // JSON document
 * ```
 */
export function parseVCode(vcode: string): string;

/**
 * Parse a .vcad file (JSON v0.1, VCode v0.2, or loon v0.3).
 *
 * Returns a JSON-serialized VcadFile with document, parts, and metadata.
 */
export function parseVcadFile(content: string): any;

export function plan_chat_tool(tool: string, args_json: string, doc_json: string): string;

/**
 * Process geometry with GPU acceleration.
 *
 * Computes creased normals and optionally generates LOD meshes.
 *
 * # Arguments
 * * `positions` - Flat array of vertex positions (x, y, z, ...)
 * * `indices` - Triangle indices
 * * `crease_angle` - Angle in radians for creased normal computation
 * * `generate_lod` - If true, returns multiple LOD levels
 *
 * # Returns
 * A JS array of geometry results. If `generate_lod` is true, returns
 * [full, 50%, 25%] detail levels. Otherwise returns a single mesh.
 */
export function processGeometryGpu(positions: Float32Array, indices: Uint32Array, crease_angle: number, generate_lod: boolean): Promise<any>;

/**
 * Project a triangle mesh to a 2D view.
 *
 * # Arguments
 * * `mesh_js` - Mesh data as JS object with `positions` (Float32Array) and `indices` (Uint32Array)
 * * `view_direction` - View direction: "front", "back", "top", "bottom", "left", "right", or "isometric"
 *
 * # Returns
 * A JS object containing the projected view with edges and bounds.
 */
export function projectMesh(mesh_js: any, view_direction: string): any;

/**
 * Read a DST file and return embroidery data as JSON.
 */
export function readEmbroideryDst(data: Uint8Array): string;

/**
 * Read a PES file and return embroidery data as JSON.
 *
 * Returns `{ threads, stitchPaths, stats }` as a JSON string.
 */
export function readEmbroideryPes(data: Uint8Array): string;

/**
 * Recommend smart print settings from analysis results.
 *
 * Takes a PrintAnalysis JSON and printer profile name,
 * returns recommended SliceSettings + explanations.
 */
export function recommendPrintSettings(analysis_json: string, printer_profile: string): any;

/**
 * Run the render-bake pipeline on a raw triangle mesh.
 *
 * Used by the imported-mesh path (STL / STEP drops) so meshes that arrive
 * from outside the kernel get the same post-processing as kernel-emitted
 * meshes: angle-based creased vertex normals today, tangent generation and
 * LOD baking later. Positions and indices may be duplicated (the mesh
 * becomes unindexed) so downstream consumers just upload the returned
 * arrays.
 *
 * Input is `{ positions: Float32Array, indices: Uint32Array, crease_angle_rad?: f64 }`
 * encoded as JSON. Returns `{ positions, indices, normals }` with the same
 * encoding.
 */
export function renderBakeMesh(input_json: string): string;

/**
 * Generate a section view from a triangle mesh.
 *
 * # Arguments
 * * `mesh_js` - Mesh data as JS object with `positions` (Float32Array) and `indices` (Uint32Array)
 * * `plane_json` - JSON string with plane definition: `{"origin": [x,y,z], "normal": [x,y,z], "up": [x,y,z]}`
 * * `hatch_json` - Optional JSON string with hatch pattern: `{"spacing": f64, "angle": f64}`
 *
 * # Returns
 * A JS object containing the section view with curves, hatch lines, and bounds.
 */
export function sectionMesh(mesh_js: any, plane_json: string, hatch_json?: string | null): any;

/**
 * Build an N-sided polygonal approximation of a circle as arc segments.
 * Returns a JSON array of `SketchSegment2D`.
 */
export function sketchCircleSegments(cx: number, cy: number, radius: number, segments: number): string;

/**
 * Find the segment-index closest to `(x, y)` within `tolerance`. Returns
 * `-1` if no segment is within reach.
 */
export function sketchHitTest(segments_json: string, x: number, y: number, tolerance: number): number;

/**
 * Return a plane's `{origin, xDir, yDir, normal}` as JSON. Accepts either a
 * named plane string or a custom-plane object (same shape as
 * [`WasmSketchSession`]'s constructor argument).
 */
export function sketchPlaneBasis(plane_json: string): string;

/**
 * Intersect a world-space ray with a plane and return the hit in 2D
 * sketch coordinates as `[x, y]` JSON, or the literal string `"null"` when
 * the ray is parallel to the plane.
 */
export function sketchPlaneIntersectRay(plane_json: string, ox: number, oy: number, oz: number, dx: number, dy: number, dz: number): string;

/**
 * Build the four line segments of an axis-aligned rectangle between two
 * opposite corners. Returns a JSON array of `SketchSegment2D`.
 */
export function sketchRectangleSegments(p1x: number, p1y: number, p2x: number, p2y: number): string;

/**
 * Snap a 2D point against a segment list with grid + vertex rules. Returns
 * `{x, y, snapTarget}` JSON — the snapped position plus (if a vertex snap
 * fired) the vertex that was matched.
 */
export function sketchSnap(segments_json: string, x: number, y: number, grid_enabled: boolean, grid_size: number, point_enabled: boolean, point_tolerance: number): string;

/**
 * Convert 2D sketch coordinates to a 3D world-space point, returning
 * `[x, y, z]` JSON.
 */
export function sketchToWorld(plane_json: string, sx: number, sy: number): string;

/**
 * Project a 3D world-space point onto a plane, returning 2D sketch
 * coordinates as `[x, y]` JSON.
 */
export function sketchWorldToSketch(plane_json: string, wx: number, wy: number, wz: number): string;

/**
 * Slice a mesh from vertices and indices.
 */
export function sliceMesh(vertices: Float32Array, indices: Uint32Array, settings: SlicerSettings): SliceResult;

/**
 * Slice a mesh and report progress to a JS callback.
 *
 * The callback is invoked synchronously during the WASM call as
 * `cb(stageLabel: string, current: number, total: number)`. Inside a
 * dedicated worker, the callback can safely `postMessage` to the main
 * thread — the worker thread is the one running the WASM, not the
 * main thread.
 */
export function sliceMeshWithProgress(vertices: Float32Array, indices: Uint32Array, settings: SlicerSettings, progress_cb: Function): SliceResult;

/**
 * Slice a solid.
 */
export function sliceSolid(solid: Solid, settings: SlicerSettings, segments?: number | null): SliceResult;

/**
 * Solve forward kinematics for an assembly document.
 *
 * # Arguments
 *
 * * `doc_json` - A JSON string representing a vcad Document
 *
 * # Returns
 *
 * A JsValue containing a Map of instance_id -> Transform3D.
 */
export function solveForwardKinematics(doc_json: string): any;

/**
 * Solve a TS-shaped sketch in one call.
 *
 * Takes a JSON array of `SketchSegment2D` and a JSON array of
 * `SketchConstraint`, runs the Levenberg-Marquardt solver, and returns a
 * JSON object `{ segments, converged }` where `segments` is the solved
 * segment list in the same order as the input. Segments that don't belong
 * to the constraint system (e.g. circle-as-arcs that live purely for
 * rendering) pass through unchanged.
 */
export function solveSketchSegments(segments_json: string, constraints_json: string): string;

/**
 * Get the bounding box of rendered text.
 *
 * Returns the width and height of the text in mm without creating geometry.
 * Useful for layout calculations before extruding text.
 *
 * # Arguments
 *
 * * `text` - The text string to measure
 * * `height` - Text height in mm
 * * `font` - Font name (currently only "sans-serif" supported)
 * * `letter_spacing` - Letter spacing multiplier (1.0 = normal)
 * * `line_spacing` - Line spacing multiplier (1.0 = normal)
 */
export function textBounds(text: string, height: number, font?: string | null, letter_spacing?: number | null, line_spacing?: number | null): any;

/**
 * Convert a vcad IR Document (JSON) to VCode text format.
 *
 * # Arguments
 * * `doc_json` - JSON string representing a vcad IR Document
 *
 * # Returns
 * The VCode text representation.
 *
 * # Example
 * ```javascript
 * const compact = toVCode(docJson);
 * console.log(compact); // "C 50 30 5\nY 5 10\n..."
 * ```
 */
export function toVCode(doc_json: string): string;

/**
 * Write a DST file from an embroidery pattern JSON string.
 */
export function writeEmbroideryDst(json: string): Uint8Array;

/**
 * Write a PES file from an embroidery pattern JSON string.
 */
export function writeEmbroideryPes(json: string): Uint8Array;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_physicssim_free: (a: number, b: number) => void;
    readonly __wbg_raytracer_free: (a: number, b: number) => void;
    readonly __wbg_solid_free: (a: number, b: number) => void;
    readonly __wbg_wasmannotationlayer_free: (a: number, b: number) => void;
    readonly buildPart: (a: number, b: number, c: number, d: number) => [number, number, number, number];
    readonly build_chat_system_prompt: (a: number, b: number, c: number, d: number) => [number, number];
    readonly computeCreasedNormalsGpu: (a: number, b: number, c: number, d: number, e: number) => any;
    readonly computeMeshVolume: (a: number, b: number, c: number, d: number) => number;
    readonly createDetailView: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number) => [number, number, number];
    readonly decimateMeshGpu: (a: number, b: number, c: number, d: number, e: number) => any;
    readonly deriveParts: (a: number, b: number) => [number, number, number];
    readonly documentToLoon: (a: number, b: number) => [number, number, number, number];
    readonly documentToLoonChecked: (a: number, b: number) => [number, number, number];
    readonly evalVcadSource: (a: number, b: number) => [number, number, number];
    readonly evaluateDocument: (a: number, b: number, c: number) => [number, number, number];
    readonly evaluateVCode: (a: number, b: number) => [number, number, number];
    readonly exportProjectedViewToDxf: (a: number, b: number) => [number, number, number, number];
    readonly getPartsManifest: () => [number, number];
    readonly get_anthropic_tools_json: () => [number, number];
    readonly get_kernel_version: () => [number, number];
    readonly get_tool_schemas: () => [number, number];
    readonly importStepBuffer: (a: number, b: number) => [number, number, number];
    readonly importUrdfBuffer: (a: number, b: number) => [number, number, number, number];
    readonly init: () => void;
    readonly initGpu: () => any;
    readonly isGpuAvailable: () => number;
    readonly isPhysicsAvailable: () => number;
    readonly op_chamfer: (a: number, b: number) => number;
    readonly op_circular_pattern: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number) => number;
    readonly op_fillet: (a: number, b: number) => number;
    readonly op_linear_pattern: (a: number, b: number, c: number, d: number, e: number, f: number) => number;
    readonly op_loft: (a: number, b: number, c: number) => [number, number, number];
    readonly op_revolve: (a: number, b: number, c: number, d: number, e: number, f: number, g: number) => [number, number, number];
    readonly op_shell: (a: number, b: number) => number;
    readonly op_sweep_helix: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number, l: number, m: number, n: number, o: number, p: number) => [number, number, number];
    readonly op_sweep_line: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number, l: number, m: number, n: number) => [number, number, number];
    readonly parseVCode: (a: number, b: number) => [number, number, number, number];
    readonly parseVcadFile: (a: number, b: number) => [number, number, number];
    readonly physicssim_actionDim: (a: number) => number;
    readonly physicssim_new: (a: number, b: number, c: number, d: number, e: number, f: number) => [number, number, number];
    readonly physicssim_observationDim: (a: number) => number;
    readonly physicssim_observe: (a: number) => any;
    readonly physicssim_reset: (a: number) => any;
    readonly physicssim_setMaxSteps: (a: number, b: number) => void;
    readonly physicssim_setSeed: (a: number, b: bigint) => void;
    readonly physicssim_stepPosition: (a: number, b: number, c: number) => any;
    readonly physicssim_stepTorque: (a: number, b: number, c: number) => any;
    readonly physicssim_stepVelocity: (a: number, b: number, c: number) => any;
    readonly plan_chat_tool: (a: number, b: number, c: number, d: number, e: number, f: number) => [number, number];
    readonly processGeometryGpu: (a: number, b: number, c: number, d: number, e: number, f: number) => any;
    readonly projectMesh: (a: any, b: number, c: number) => any;
    readonly raytracer_canRaytrace: (a: number) => number;
    readonly raytracer_clearScene: (a: number) => void;
    readonly raytracer_create: () => [number, number, number];
    readonly raytracer_getDebugMode: (a: number) => number;
    readonly raytracer_getEdgeDetectionEnabled: (a: number) => number;
    readonly raytracer_getFrameIndex: (a: number) => number;
    readonly raytracer_getRefineSamples: (a: number) => number;
    readonly raytracer_hasScene: (a: number) => number;
    readonly raytracer_pick: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number, l: number) => [number, number, number];
    readonly raytracer_render: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number) => any;
    readonly raytracer_resetAccumulation: (a: number) => void;
    readonly raytracer_setAO: (a: number, b: number, c: number, d: number, e: number) => void;
    readonly raytracer_setDebugMode: (a: number, b: number) => void;
    readonly raytracer_setEdgeDetection: (a: number, b: number, c: number, d: number) => void;
    readonly raytracer_setEdgeStyle: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number, l: number, m: number, n: number, o: number, p: number, q: number, r: number, s: number, t: number) => void;
    readonly raytracer_setMaterial: (a: number, b: number, c: number, d: number, e: number, f: number) => [number, number];
    readonly raytracer_setRefineSamples: (a: number, b: number) => void;
    readonly raytracer_setTheme: (a: number, b: number) => void;
    readonly raytracer_uploadSolid: (a: number, b: number) => [number, number];
    readonly renderBakeMesh: (a: number, b: number) => [number, number, number, number];
    readonly sectionMesh: (a: any, b: number, c: number, d: number, e: number) => any;
    readonly solid_boundaryEdges: (a: number, b: number) => [number, number];
    readonly solid_boundingBox: (a: number) => [number, number];
    readonly solid_centerOfMass: (a: number) => [number, number];
    readonly solid_cone: (a: number, b: number, c: number, d: number) => number;
    readonly solid_cube: (a: number, b: number, c: number) => number;
    readonly solid_cylinder: (a: number, b: number, c: number) => number;
    readonly solid_difference: (a: number, b: number) => number;
    readonly solid_empty: () => number;
    readonly solid_extrude: (a: number, b: number, c: number, d: number) => [number, number, number];
    readonly solid_extrudeWithOptions: (a: number, b: number, c: number, d: number, e: number, f: number) => [number, number, number];
    readonly solid_getMesh: (a: number, b: number) => any;
    readonly solid_horizontalSection: (a: number, b: number, c: number, d: number, e: number, f: number, g: number) => any;
    readonly solid_intersection: (a: number, b: number) => number;
    readonly solid_isEmpty: (a: number) => number;
    readonly solid_loft: (a: number, b: number, c: number) => [number, number, number];
    readonly solid_numTriangles: (a: number) => number;
    readonly solid_projectView: (a: number, b: number, c: number, d: number) => any;
    readonly solid_rotate: (a: number, b: number, c: number, d: number) => number;
    readonly solid_scale: (a: number, b: number, c: number, d: number) => number;
    readonly solid_sectionView: (a: number, b: number, c: number, d: number, e: number, f: number) => any;
    readonly solid_sphere: (a: number, b: number) => number;
    readonly solid_surfaceArea: (a: number) => number;
    readonly solid_sweepHelix: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number, l: number, m: number, n: number, o: number, p: number) => [number, number, number];
    readonly solid_sweepLine: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number, l: number, m: number, n: number) => [number, number, number];
    readonly solid_textExtrude: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number, l: number, m: number, n: number, o: number, p: number, q: number, r: number, s: number) => [number, number, number];
    readonly solid_toStepBuffer: (a: number) => [number, number, number, number];
    readonly solid_translate: (a: number, b: number, c: number, d: number) => number;
    readonly solid_union: (a: number, b: number) => number;
    readonly solid_volume: (a: number) => number;
    readonly solveForwardKinematics: (a: number, b: number) => [number, number, number];
    readonly textBounds: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number) => [number, number, number];
    readonly toVCode: (a: number, b: number) => [number, number, number, number];
    readonly wasmannotationlayer_addAlignedDimension: (a: number, b: number, c: number, d: number, e: number, f: number) => void;
    readonly wasmannotationlayer_addAngleDimension: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number) => void;
    readonly wasmannotationlayer_addDiameterDimension: (a: number, b: number, c: number, d: number, e: number) => void;
    readonly wasmannotationlayer_addHorizontalDimension: (a: number, b: number, c: number, d: number, e: number, f: number) => void;
    readonly wasmannotationlayer_addRadiusDimension: (a: number, b: number, c: number, d: number, e: number) => void;
    readonly wasmannotationlayer_addVerticalDimension: (a: number, b: number, c: number, d: number, e: number, f: number) => void;
    readonly wasmannotationlayer_annotationCount: (a: number) => number;
    readonly wasmannotationlayer_clear: (a: number) => void;
    readonly wasmannotationlayer_isEmpty: (a: number) => number;
    readonly wasmannotationlayer_new: () => number;
    readonly wasmannotationlayer_renderAll: (a: number, b: number, c: number) => any;
    readonly solid_linearPattern: (a: number, b: number, c: number, d: number, e: number, f: number) => number;
    readonly physicssim_numJoints: (a: number) => number;
    readonly solid_revolve: (a: number, b: number, c: number, d: number, e: number, f: number, g: number) => [number, number, number];
    readonly solid_canExportStep: (a: number) => number;
    readonly solid_circularPattern: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number) => number;
    readonly solid_chamfer: (a: number, b: number) => number;
    readonly solid_fillet: (a: number, b: number) => number;
    readonly solid_shell: (a: number, b: number) => number;
    readonly getCompiledModule: () => any;
    readonly __wbg_wasmdocumentengine_free: (a: number, b: number) => void;
    readonly wasmdocumentengine_add_feature: (a: number, b: number, c: number) => any;
    readonly wasmdocumentengine_can_redo: (a: number) => number;
    readonly wasmdocumentengine_can_undo: (a: number) => number;
    readonly wasmdocumentengine_compute_position_between: (a: number, b: number, c: number, d: number, e: number) => [number, number];
    readonly wasmdocumentengine_create_feature: (a: number, b: number, c: number, d: number, e: number) => any;
    readonly wasmdocumentengine_delete_feature: (a: number, b: number, c: number) => any;
    readonly wasmdocumentengine_delete_feature_by_id: (a: number, b: number, c: number) => any;
    readonly wasmdocumentengine_from_v1_json: (a: number, b: number) => [number, number, number];
    readonly wasmdocumentengine_get_document_json: (a: number) => [number, number];
    readonly wasmdocumentengine_get_ops_since: (a: number, b: number, c: number) => [number, number];
    readonly wasmdocumentengine_get_ordered_features_json: (a: number) => [number, number];
    readonly wasmdocumentengine_get_parts_json: (a: number) => [number, number];
    readonly wasmdocumentengine_get_sync_clock: (a: number) => [number, number];
    readonly wasmdocumentengine_import_ir: (a: number, b: number, c: number) => any;
    readonly wasmdocumentengine_load: (a: number, b: number) => [number, number, number];
    readonly wasmdocumentengine_merge_remote: (a: number, b: number, c: number) => any;
    readonly wasmdocumentengine_move_feature: (a: number, b: number, c: number, d: number, e: number) => any;
    readonly wasmdocumentengine_new: () => number;
    readonly wasmdocumentengine_redo: (a: number) => any;
    readonly wasmdocumentengine_rename_feature: (a: number, b: number, c: number, d: number, e: number) => any;
    readonly wasmdocumentengine_save: (a: number) => [number, number];
    readonly wasmdocumentengine_set_joint_state: (a: number, b: number, c: number, d: number) => any;
    readonly wasmdocumentengine_set_material: (a: number, b: number, c: number, d: number, e: number) => any;
    readonly wasmdocumentengine_set_param: (a: number, b: number, c: number, d: number, e: number, f: number, g: number) => any;
    readonly wasmdocumentengine_set_rotation: (a: number, b: number, c: number, d: number, e: number, f: number) => any;
    readonly wasmdocumentengine_set_scale: (a: number, b: number, c: number, d: number, e: number, f: number) => any;
    readonly wasmdocumentengine_set_translation: (a: number, b: number, c: number, d: number, e: number, f: number) => any;
    readonly wasmdocumentengine_set_visible: (a: number, b: number, c: number, d: number) => any;
    readonly wasmdocumentengine_undo: (a: number) => any;
    readonly wasmdocumentengine_update_feature: (a: number, b: number, c: number, d: number, e: number) => any;
    readonly __wbg_get_slicersettings_first_layer_height: (a: number) => number;
    readonly __wbg_get_slicersettings_infill_density: (a: number) => number;
    readonly __wbg_get_slicersettings_infill_pattern: (a: number) => number;
    readonly __wbg_get_slicersettings_layer_height: (a: number) => number;
    readonly __wbg_get_slicersettings_line_width: (a: number) => number;
    readonly __wbg_get_slicersettings_nozzle_diameter: (a: number) => number;
    readonly __wbg_get_slicersettings_support_angle: (a: number) => number;
    readonly __wbg_get_slicersettings_support_enabled: (a: number) => number;
    readonly __wbg_get_slicersettings_wall_count: (a: number) => number;
    readonly __wbg_set_slicersettings_first_layer_height: (a: number, b: number) => void;
    readonly __wbg_set_slicersettings_infill_density: (a: number, b: number) => void;
    readonly __wbg_set_slicersettings_infill_pattern: (a: number, b: number) => void;
    readonly __wbg_set_slicersettings_layer_height: (a: number, b: number) => void;
    readonly __wbg_set_slicersettings_line_width: (a: number, b: number) => void;
    readonly __wbg_set_slicersettings_nozzle_diameter: (a: number, b: number) => void;
    readonly __wbg_set_slicersettings_support_angle: (a: number, b: number) => void;
    readonly __wbg_set_slicersettings_support_enabled: (a: number, b: number) => void;
    readonly __wbg_set_slicersettings_wall_count: (a: number, b: number) => void;
    readonly __wbg_sliceresult_free: (a: number, b: number) => void;
    readonly __wbg_slicersettings_free: (a: number, b: number) => void;
    readonly analyzeForPrinting: (a: number) => [number, number, number];
    readonly checkPrintability: (a: number, b: number, c: number) => [number, number, number];
    readonly ecadBuiltinSymbols: () => [number, number, number];
    readonly ecadCheckDrc: (a: number, b: number) => [number, number, number];
    readonly ecadCheckErc: (a: number, b: number) => [number, number, number];
    readonly ecadComponentMeshes: (a: number, b: number) => [number, number, number];
    readonly ecadComputeRatsnest: (a: number, b: number, c: number, d: number) => [number, number, number];
    readonly ecadFillZones: (a: number, b: number) => [number, number, number];
    readonly ecadGenerateNetlist: (a: number, b: number) => [number, number, number];
    readonly ecadGetSymbol: (a: number, b: number) => [number, number, number];
    readonly ecadLayerZ: (a: number, b: number, c: number, d: number) => number;
    readonly ecadNetForWire: (a: number, b: number, c: number, d: number, e: number, f: number) => [number, number, number];
    readonly ecadRouteNet: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number) => [number, number, number];
    readonly ecadSnapToGridOrPin: (a: number, b: number, c: number, d: number, e: number, f: number) => [number, number, number];
    readonly estimatePrintCost: (a: number, b: number, c: number, d: number, e: number, f: number) => [number, number, number];
    readonly generate3mf: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number) => [number, number, number, number];
    readonly generate3mfWithGcode: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number) => [number, number, number, number];
    readonly generateGcode: (a: number, b: number, c: number, d: number, e: number) => [number, number, number, number];
    readonly getSlicerPrinterProfiles: () => [number, number, number];
    readonly isEcadAvailable: () => number;
    readonly parseKicadPcb: (a: number, b: number) => [number, number, number];
    readonly recommendPrintSettings: (a: number, b: number, c: number, d: number) => [number, number, number];
    readonly sliceMesh: (a: number, b: number, c: number, d: number, e: number) => [number, number, number];
    readonly sliceMeshWithProgress: (a: number, b: number, c: number, d: number, e: number, f: any) => [number, number, number];
    readonly sliceSolid: (a: number, b: number, c: number) => [number, number, number];
    readonly sliceresult_filamentGrams: (a: number) => number;
    readonly sliceresult_filamentMm: (a: number) => number;
    readonly sliceresult_getLayerPreview: (a: number, b: number) => [number, number, number];
    readonly sliceresult_layerCount: (a: number) => number;
    readonly sliceresult_printTimeSeconds: (a: number) => number;
    readonly sliceresult_statsJson: (a: number) => [number, number, number, number];
    readonly slicersettings_fromJson: (a: number, b: number) => [number, number, number];
    readonly slicersettings_new: () => number;
    readonly isSlicerAvailable: () => number;
    readonly __wbg_get_wasmcamsettings_feed_rate: (a: number) => number;
    readonly __wbg_get_wasmcamsettings_plunge_rate: (a: number) => number;
    readonly __wbg_get_wasmcamsettings_retract_z: (a: number) => number;
    readonly __wbg_get_wasmcamsettings_safe_z: (a: number) => number;
    readonly __wbg_get_wasmcamsettings_spindle_rpm: (a: number) => number;
    readonly __wbg_get_wasmcamsettings_stepdown: (a: number) => number;
    readonly __wbg_get_wasmcamsettings_stepover: (a: number) => number;
    readonly __wbg_set_wasmcamsettings_feed_rate: (a: number, b: number) => void;
    readonly __wbg_set_wasmcamsettings_plunge_rate: (a: number, b: number) => void;
    readonly __wbg_set_wasmcamsettings_retract_z: (a: number, b: number) => void;
    readonly __wbg_set_wasmcamsettings_safe_z: (a: number, b: number) => void;
    readonly __wbg_set_wasmcamsettings_spindle_rpm: (a: number, b: number) => void;
    readonly __wbg_set_wasmcamsettings_stepdown: (a: number, b: number) => void;
    readonly __wbg_set_wasmcamsettings_stepover: (a: number, b: number) => void;
    readonly __wbg_wasmcamsettings_free: (a: number, b: number) => void;
    readonly __wbg_wasmkeybindings_free: (a: number, b: number) => void;
    readonly camDropCutter: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number) => [number, number, number, number];
    readonly camExportGcode: (a: number, b: number, c: number, d: number, e: number, f: number, g: number) => [number, number, number, number];
    readonly camExportLinuxCnc: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number) => [number, number, number, number];
    readonly camGenerateCircularPocket: (a: number, b: number, c: number, d: number, e: number, f: number, g: number) => [number, number, number, number];
    readonly camGenerateContour: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number, l: number) => [number, number, number, number];
    readonly camGenerateFace: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number) => [number, number, number, number];
    readonly camGeneratePocket: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number) => [number, number, number, number];
    readonly camGenerateRoughing3d: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number) => [number, number, number, number];
    readonly camGetDefaultTools: () => [number, number, number, number];
    readonly camToolpathStats: (a: number, b: number) => [number, number, number];
    readonly digitizeSketch: (a: number, b: number, c: number, d: number) => [number, number, number, number];
    readonly digitizeText: (a: number, b: number, c: number, d: number, e: number) => [number, number, number, number];
    readonly isCamAvailable: () => number;
    readonly readEmbroideryDst: (a: number, b: number) => [number, number, number, number];
    readonly readEmbroideryPes: (a: number, b: number) => [number, number, number, number];
    readonly wasmcamsettings_fromJson: (a: number, b: number) => [number, number, number];
    readonly wasmcamsettings_new: () => number;
    readonly wasmkeybindings_chordFor: (a: number, b: number, c: number) => [number, number];
    readonly wasmkeybindings_commandsJson: (a: number) => [number, number];
    readonly wasmkeybindings_conflictsJson: (a: number, b: number, c: number) => [number, number];
    readonly wasmkeybindings_loadOverrides: (a: number, b: number, c: number) => number;
    readonly wasmkeybindings_new: () => number;
    readonly wasmkeybindings_resetAll: (a: number) => void;
    readonly wasmkeybindings_resolve: (a: number, b: number, c: number, d: number, e: number, f: number) => [number, number];
    readonly wasmkeybindings_saveOverrides: (a: number) => [number, number];
    readonly wasmkeybindings_setBinding: (a: number, b: number, c: number, d: number, e: number) => void;
    readonly writeEmbroideryDst: (a: number, b: number) => [number, number, number, number];
    readonly writeEmbroideryPes: (a: number, b: number) => [number, number, number, number];
    readonly isEmbroideryAvailable: () => number;
    readonly __wbg_wasmsketchsession_free: (a: number, b: number) => void;
    readonly sketchCircleSegments: (a: number, b: number, c: number, d: number) => [number, number, number, number];
    readonly sketchHitTest: (a: number, b: number, c: number, d: number, e: number) => [number, number, number];
    readonly sketchPlaneBasis: (a: number, b: number) => [number, number, number, number];
    readonly sketchPlaneIntersectRay: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number) => [number, number, number, number];
    readonly sketchRectangleSegments: (a: number, b: number, c: number, d: number) => [number, number, number, number];
    readonly sketchSnap: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number) => [number, number, number, number];
    readonly sketchToWorld: (a: number, b: number, c: number, d: number) => [number, number, number, number];
    readonly sketchWorldToSketch: (a: number, b: number, c: number, d: number, e: number) => [number, number, number, number];
    readonly solveSketchSegments: (a: number, b: number, c: number, d: number) => [number, number, number, number];
    readonly wasmsketchsession_addCircle: (a: number, b: number, c: number, d: number) => void;
    readonly wasmsketchsession_addConstraint: (a: number, b: number, c: number) => [number, number];
    readonly wasmsketchsession_addLine: (a: number, b: number, c: number, d: number, e: number) => void;
    readonly wasmsketchsession_addRectangle: (a: number, b: number, c: number, d: number, e: number) => void;
    readonly wasmsketchsession_cancelPending: (a: number) => void;
    readonly wasmsketchsession_clear: (a: number) => void;
    readonly wasmsketchsession_clearSelection: (a: number) => void;
    readonly wasmsketchsession_new: (a: number, b: number) => [number, number, number];
    readonly wasmsketchsession_onClick: (a: number) => [number, number];
    readonly wasmsketchsession_onCursorLeave: (a: number) => void;
    readonly wasmsketchsession_onCursorRay: (a: number, b: number, c: number, d: number, e: number, f: number, g: number) => void;
    readonly wasmsketchsession_onCursorSketch: (a: number, b: number, c: number) => void;
    readonly wasmsketchsession_onDoubleClick: (a: number) => void;
    readonly wasmsketchsession_redo: (a: number) => number;
    readonly wasmsketchsession_removeConstraint: (a: number, b: number) => void;
    readonly wasmsketchsession_setSnap: (a: number, b: number, c: number, d: number, e: number) => void;
    readonly wasmsketchsession_setTool: (a: number, b: number, c: number) => void;
    readonly wasmsketchsession_snapshot: (a: number) => [number, number, number, number];
    readonly wasmsketchsession_solve: (a: number) => number;
    readonly wasmsketchsession_toggleSelection: (a: number, b: number) => void;
    readonly wasmsketchsession_undo: (a: number) => number;
    readonly wasm_bindgen__closure__destroy__h30743bca3150d93c: (a: number, b: number) => void;
    readonly wasm_bindgen__closure__destroy__hfdadf281ff0f1c56: (a: number, b: number) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h3c7e771ac0cfa72e: (a: number, b: number, c: any, d: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__hcf7d3eaee8800b37: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h9bdf540eb7e61590: (a: number, b: number, c: any) => void;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_exn_store: (a: number) => void;
    readonly __externref_table_alloc: () => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;

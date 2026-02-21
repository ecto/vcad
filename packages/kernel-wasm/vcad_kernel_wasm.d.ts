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
     * Set the material for all faces in the scene.
     *
     * # Arguments
     * * `r`, `g`, `b` - RGB color components (0-1 range, linear)
     * * `metallic` - Metallic factor (0 = dielectric, 1 = metal)
     * * `roughness` - Roughness factor (0 = smooth/mirror, 1 = rough/diffuse)
     */
    setMaterial(r: number, g: number, b: number, metallic: number, roughness: number): void;
    /**
     * Upload a solid's BRep representation for ray tracing.
     *
     * This extracts the BRep surfaces and builds the GPU scene data.
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
 * Evaluate a loon source string and return a JSON-serialized vcad Document.
 *
 * The vcad library (types, constructors) is automatically prepended.
 * Module resolution (`[use ...]`) is not available in WASM — all code
 * must be self-contained or use the bundled vcad library.
 */
export function evalVcadSource(source: string): any;

/**
 * Evaluate compact IR and return a Solid for rendering.
 *
 * This is a convenience function that parses compact IR and evaluates
 * the geometry in a single step.
 *
 * # Arguments
 * * `compact_ir` - The compact IR text to evaluate
 *
 * # Returns
 * A Solid object that can be rendered or queried.
 */
export function evaluateCompactIR(compact_ir: string): Solid;

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
 * Generate G-code from slice result.
 */
export function generateGcode(result: SliceResult, printer_profile: string, print_temp: number, bed_temp: number): string;

/**
 * Get available printer profiles.
 */
export function getSlicerPrinterProfiles(): any;

/**
 * Get the kernel version string.
 * Use this in browser console to verify the correct WASM build is loaded:
 * `kernelWasm.get_kernel_version()` should return "2025-02-03-geom-debug"
 */
export function get_kernel_version(): string;

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
 * Parse compact IR text format into a vcad IR Document (JSON).
 *
 * The compact IR format is a token-efficient text representation designed
 * for ML model training and inference. See `vcad_ir::compact` for format details.
 *
 * # Arguments
 * * `compact_ir` - The compact IR text to parse
 *
 * # Returns
 * A JSON string representing the parsed vcad IR Document.
 *
 * # Example
 * ```javascript
 * const ir = "C 50 30 5\nY 5 10\nT 1 25 15 0\nD 0 2";
 * const doc = parseCompactIR(ir);
 * console.log(doc); // JSON document
 * ```
 */
export function parseCompactIR(compact_ir: string): string;

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
 * Slice a mesh from vertices and indices.
 */
export function sliceMesh(vertices: Float32Array, indices: Uint32Array, settings: SlicerSettings): SliceResult;

/**
 * Slice a solid.
 */
export function sliceSolid(solid: Solid, settings: SlicerSettings, segments?: number | null): SliceResult;

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
 * Convert a vcad IR Document (JSON) to compact IR text format.
 *
 * # Arguments
 * * `doc_json` - JSON string representing a vcad IR Document
 *
 * # Returns
 * The compact IR text representation.
 *
 * # Example
 * ```javascript
 * const compact = toCompactIR(docJson);
 * console.log(compact); // "C 50 30 5\nY 5 10\n..."
 * ```
 */
export function toCompactIR(doc_json: string): string;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_physicssim_free: (a: number, b: number) => void;
    readonly __wbg_raytracer_free: (a: number, b: number) => void;
    readonly __wbg_solid_free: (a: number, b: number) => void;
    readonly __wbg_wasmannotationlayer_free: (a: number, b: number) => void;
    readonly computeCreasedNormalsGpu: (a: number, b: number, c: number, d: number, e: number) => any;
    readonly createDetailView: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number) => [number, number, number];
    readonly decimateMeshGpu: (a: number, b: number, c: number, d: number, e: number) => any;
    readonly evalVcadSource: (a: number, b: number) => [number, number, number];
    readonly evaluateCompactIR: (a: number, b: number) => [number, number, number];
    readonly evaluateDocument: (a: number, b: number, c: number) => [number, number, number];
    readonly exportProjectedViewToDxf: (a: number, b: number) => [number, number, number, number];
    readonly get_kernel_version: () => [number, number];
    readonly importStepBuffer: (a: number, b: number) => [number, number, number];
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
    readonly parseCompactIR: (a: number, b: number) => [number, number, number, number];
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
    readonly processGeometryGpu: (a: number, b: number, c: number, d: number, e: number, f: number) => any;
    readonly projectMesh: (a: any, b: number, c: number) => any;
    readonly raytracer_canRaytrace: (a: number) => number;
    readonly raytracer_create: () => [number, number, number];
    readonly raytracer_getDebugMode: (a: number) => number;
    readonly raytracer_getEdgeDetectionEnabled: (a: number) => number;
    readonly raytracer_getFrameIndex: (a: number) => number;
    readonly raytracer_hasScene: (a: number) => number;
    readonly raytracer_pick: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number, l: number) => [number, number, number];
    readonly raytracer_render: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number) => any;
    readonly raytracer_resetAccumulation: (a: number) => void;
    readonly raytracer_setDebugMode: (a: number, b: number) => void;
    readonly raytracer_setEdgeDetection: (a: number, b: number, c: number, d: number) => void;
    readonly raytracer_setMaterial: (a: number, b: number, c: number, d: number, e: number, f: number) => [number, number];
    readonly raytracer_uploadSolid: (a: number, b: number) => [number, number];
    readonly sectionMesh: (a: any, b: number, c: number, d: number, e: number) => any;
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
    readonly textBounds: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number) => [number, number, number];
    readonly toCompactIR: (a: number, b: number) => [number, number, number, number];
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
    readonly solid_revolve: (a: number, b: number, c: number, d: number, e: number, f: number, g: number) => [number, number, number];
    readonly physicssim_numJoints: (a: number) => number;
    readonly solid_linearPattern: (a: number, b: number, c: number, d: number, e: number, f: number) => number;
    readonly solid_canExportStep: (a: number) => number;
    readonly solid_chamfer: (a: number, b: number) => number;
    readonly solid_fillet: (a: number, b: number) => number;
    readonly solid_shell: (a: number, b: number) => number;
    readonly solid_circularPattern: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number) => number;
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
    readonly isCamAvailable: () => number;
    readonly wasmcamsettings_fromJson: (a: number, b: number) => [number, number, number];
    readonly wasmcamsettings_new: () => number;
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
    readonly generateGcode: (a: number, b: number, c: number, d: number, e: number) => [number, number, number, number];
    readonly getSlicerPrinterProfiles: () => [number, number, number];
    readonly isSlicerAvailable: () => number;
    readonly sliceMesh: (a: number, b: number, c: number, d: number, e: number) => [number, number, number];
    readonly sliceSolid: (a: number, b: number, c: number) => [number, number, number];
    readonly sliceresult_filamentGrams: (a: number) => number;
    readonly sliceresult_filamentMm: (a: number) => number;
    readonly sliceresult_getLayerPreview: (a: number, b: number) => [number, number, number];
    readonly sliceresult_layerCount: (a: number) => number;
    readonly sliceresult_printTimeSeconds: (a: number) => number;
    readonly sliceresult_statsJson: (a: number) => [number, number, number, number];
    readonly slicersettings_fromJson: (a: number, b: number) => [number, number, number];
    readonly slicersettings_new: () => number;
    readonly ecadCheckDrc: (a: number, b: number) => [number, number, number];
    readonly ecadCheckErc: (a: number, b: number) => [number, number, number];
    readonly ecadFillZones: (a: number, b: number) => [number, number, number];
    readonly ecadGenerateNetlist: (a: number, b: number) => [number, number, number];
    readonly ecadRouteNet: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number) => [number, number, number];
    readonly isEcadAvailable: () => number;
    readonly wasm_bindgen__closure__destroy__ha8b73a36ae48e470: (a: number, b: number) => void;
    readonly wasm_bindgen__closure__destroy__h250d7189f9770b99: (a: number, b: number) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h60f25fed64173f82: (a: number, b: number, c: any, d: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h4488ad9b37e81000: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__ha8b3f1b8e67fad08: (a: number, b: number, c: any) => void;
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

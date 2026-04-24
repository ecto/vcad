/* @ts-self-types="./vcad_kernel_wasm.d.ts" */

/**
 * Physics simulation environment for robotics and RL.
 *
 * This provides a gym-style interface for simulating robot assemblies
 * with physics, joints, and collision detection.
 */
export class PhysicsSim {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        PhysicsSimFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_physicssim_free(ptr, 0);
    }
    /**
     * Get the action dimension.
     * @returns {number}
     */
    actionDim() {
        const ret = wasm.physicssim_actionDim(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * Create a new physics simulation from a vcad document JSON.
     *
     * # Arguments
     * * `doc_json` - JSON string representing a vcad IR Document
     * * `end_effector_ids` - Array of instance IDs to track as end effectors
     * * `dt` - Simulation timestep in seconds (default: 1/240)
     * * `substeps` - Number of physics substeps per step (default: 4)
     * @param {string} doc_json
     * @param {string[]} end_effector_ids
     * @param {number | null} [dt]
     * @param {number | null} [substeps]
     */
    constructor(doc_json, end_effector_ids, dt, substeps) {
        const ptr0 = passStringToWasm0(doc_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passArrayJsValueToWasm0(end_effector_ids, wasm.__wbindgen_malloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.physicssim_new(ptr0, len0, ptr1, len1, isLikeNone(dt) ? 0x100000001 : Math.fround(dt), isLikeNone(substeps) ? 0x100000001 : (substeps) >>> 0);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        this.__wbg_ptr = ret[0] >>> 0;
        PhysicsSimFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
    /**
     * Get the number of joints in the environment.
     * @returns {number}
     */
    numJoints() {
        const ret = wasm.physicssim_actionDim(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * Get the observation dimension.
     * @returns {number}
     */
    observationDim() {
        const ret = wasm.physicssim_observationDim(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * Get current observation without stepping.
     *
     * Returns observation as JSON.
     * @returns {any}
     */
    observe() {
        const ret = wasm.physicssim_observe(this.__wbg_ptr);
        return ret;
    }
    /**
     * Reset the environment to initial state.
     *
     * Returns the initial observation as JSON.
     * @returns {any}
     */
    reset() {
        const ret = wasm.physicssim_reset(this.__wbg_ptr);
        return ret;
    }
    /**
     * Set the maximum episode length.
     * @param {number} max_steps
     */
    setMaxSteps(max_steps) {
        wasm.physicssim_setMaxSteps(this.__wbg_ptr, max_steps);
    }
    /**
     * Set the random seed.
     * @param {bigint} seed
     */
    setSeed(seed) {
        wasm.physicssim_setSeed(this.__wbg_ptr, seed);
    }
    /**
     * Step the simulation with position targets.
     *
     * # Arguments
     * * `targets` - Array of position targets for each joint (degrees or mm)
     *
     * # Returns
     * Object with { observation, reward, done }
     * @param {Float64Array} targets
     * @returns {any}
     */
    stepPosition(targets) {
        const ptr0 = passArrayF64ToWasm0(targets, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.physicssim_stepPosition(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * Step the simulation with a torque action.
     *
     * # Arguments
     * * `torques` - Array of torques/forces for each joint (Nm or N)
     *
     * # Returns
     * Object with { observation, reward, done }
     * @param {Float64Array} torques
     * @returns {any}
     */
    stepTorque(torques) {
        const ptr0 = passArrayF64ToWasm0(torques, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.physicssim_stepTorque(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * Step the simulation with velocity targets.
     *
     * # Arguments
     * * `targets` - Array of velocity targets for each joint (deg/s or mm/s)
     *
     * # Returns
     * Object with { observation, reward, done }
     * @param {Float64Array} targets
     * @returns {any}
     */
    stepVelocity(targets) {
        const ptr0 = passArrayF64ToWasm0(targets, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.physicssim_stepVelocity(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
}
if (Symbol.dispose) PhysicsSim.prototype[Symbol.dispose] = PhysicsSim.prototype.free;

/**
 * GPU-accelerated ray tracer for direct BRep rendering.
 *
 * This ray tracer renders BRep surfaces directly without tessellation,
 * achieving pixel-perfect silhouettes at any zoom level.
 */
export class RayTracer {
    static __wrap(ptr) {
        ptr = ptr >>> 0;
        const obj = Object.create(RayTracer.prototype);
        obj.__wbg_ptr = ptr;
        RayTracerFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        RayTracerFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_raytracer_free(ptr, 0);
    }
    /**
     * Check if a solid can be ray traced.
     *
     * Returns true if the solid has a BRep representation.
     * @param {Solid} solid
     * @returns {boolean}
     */
    static canRaytrace(solid) {
        _assertClass(solid, Solid);
        const ret = wasm.raytracer_canRaytrace(solid.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * Create a new ray tracer.
     *
     * Requires WebGPU to be available and initialized.
     * Call `initGpu()` before calling this method.
     * @returns {RayTracer}
     */
    static create() {
        const ret = wasm.raytracer_create();
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return RayTracer.__wrap(ret[0]);
    }
    /**
     * Get the current debug render mode.
     * @returns {number}
     */
    getDebugMode() {
        const ret = wasm.raytracer_getDebugMode(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * Get whether edge detection is enabled.
     * @returns {boolean}
     */
    getEdgeDetectionEnabled() {
        const ret = wasm.raytracer_getEdgeDetectionEnabled(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * Get the current frame index for progressive rendering.
     * @returns {number}
     */
    getFrameIndex() {
        const ret = wasm.raytracer_getFrameIndex(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * Check if the ray tracer has a scene loaded.
     * @returns {boolean}
     */
    hasScene() {
        const ret = wasm.raytracer_hasScene(this.__wbg_ptr);
        return ret !== 0;
    }
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
     * @param {Float64Array} camera
     * @param {Float64Array} target
     * @param {Float64Array} up
     * @param {number} width
     * @param {number} height
     * @param {number} fov
     * @param {number} pixel_x
     * @param {number} pixel_y
     * @returns {number}
     */
    pick(camera, target, up, width, height, fov, pixel_x, pixel_y) {
        const ptr0 = passArrayF64ToWasm0(camera, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passArrayF64ToWasm0(target, wasm.__wbindgen_malloc);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passArrayF64ToWasm0(up, wasm.__wbindgen_malloc);
        const len2 = WASM_VECTOR_LEN;
        const ret = wasm.raytracer_pick(this.__wbg_ptr, ptr0, len0, ptr1, len1, ptr2, len2, width, height, fov, pixel_x, pixel_y);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0];
    }
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
     * @param {Float64Array} camera
     * @param {Float64Array} target
     * @param {Float64Array} up
     * @param {number} width
     * @param {number} height
     * @param {number} fov
     * @returns {Promise<Uint8Array>}
     */
    render(camera, target, up, width, height, fov) {
        const ptr0 = passArrayF64ToWasm0(camera, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passArrayF64ToWasm0(target, wasm.__wbindgen_malloc);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passArrayF64ToWasm0(up, wasm.__wbindgen_malloc);
        const len2 = WASM_VECTOR_LEN;
        const ret = wasm.raytracer_render(this.__wbg_ptr, ptr0, len0, ptr1, len1, ptr2, len2, width, height, fov);
        return ret;
    }
    /**
     * Reset the progressive accumulation (call when camera moves).
     */
    resetAccumulation() {
        wasm.raytracer_resetAccumulation(this.__wbg_ptr);
    }
    /**
     * Set the debug render mode.
     *
     * # Arguments
     * * `mode` - Debug mode: 0=normal, 1=normals as RGB, 2=face_id colors, 3=N·L grayscale, 4=orientation
     *
     * Call resetAccumulation() after changing mode to see immediate effect.
     * @param {number} mode
     */
    setDebugMode(mode) {
        wasm.raytracer_setDebugMode(this.__wbg_ptr, mode);
    }
    /**
     * Set edge detection settings.
     *
     * # Arguments
     * * `enabled` - Whether to show edge detection overlay
     * * `depth_threshold` - Depth discontinuity threshold (default: 0.1)
     * * `normal_threshold` - Normal angle threshold in degrees (default: 30.0)
     * @param {boolean} enabled
     * @param {number} depth_threshold
     * @param {number} normal_threshold
     */
    setEdgeDetection(enabled, depth_threshold, normal_threshold) {
        wasm.raytracer_setEdgeDetection(this.__wbg_ptr, enabled, depth_threshold, normal_threshold);
    }
    /**
     * Set the material for all faces in the scene.
     *
     * # Arguments
     * * `r`, `g`, `b` - RGB color components (0-1 range, linear)
     * * `metallic` - Metallic factor (0 = dielectric, 1 = metal)
     * * `roughness` - Roughness factor (0 = smooth/mirror, 1 = rough/diffuse)
     * @param {number} r
     * @param {number} g
     * @param {number} b
     * @param {number} metallic
     * @param {number} roughness
     */
    setMaterial(r, g, b, metallic, roughness) {
        const ret = wasm.raytracer_setMaterial(this.__wbg_ptr, r, g, b, metallic, roughness);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * Upload a solid's BRep representation for ray tracing.
     *
     * This extracts the BRep surfaces and builds the GPU scene data.
     * @param {Solid} solid
     */
    uploadSolid(solid) {
        _assertClass(solid, Solid);
        const ret = wasm.raytracer_uploadSolid(this.__wbg_ptr, solid.__wbg_ptr);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
}
if (Symbol.dispose) RayTracer.prototype[Symbol.dispose] = RayTracer.prototype.free;

/**
 * Slice result for WASM.
 */
export class SliceResult {
    static __wrap(ptr) {
        ptr = ptr >>> 0;
        const obj = Object.create(SliceResult.prototype);
        obj.__wbg_ptr = ptr;
        SliceResultFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        SliceResultFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_sliceresult_free(ptr, 0);
    }
    /**
     * Get filament weight in grams.
     * @returns {number}
     */
    get filamentGrams() {
        const ret = wasm.sliceresult_filamentGrams(this.__wbg_ptr);
        return ret;
    }
    /**
     * Get filament usage in mm.
     * @returns {number}
     */
    get filamentMm() {
        const ret = wasm.sliceresult_filamentMm(this.__wbg_ptr);
        return ret;
    }
    /**
     * Get layer data for preview.
     * @param {number} layer_index
     * @returns {any}
     */
    getLayerPreview(layer_index) {
        const ret = wasm.sliceresult_getLayerPreview(this.__wbg_ptr, layer_index);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * Get number of layers.
     * @returns {number}
     */
    get layerCount() {
        const ret = wasm.sliceresult_layerCount(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * Get estimated print time in seconds.
     * @returns {number}
     */
    get printTimeSeconds() {
        const ret = wasm.sliceresult_printTimeSeconds(this.__wbg_ptr);
        return ret;
    }
    /**
     * Get stats as JSON.
     * @returns {string}
     */
    statsJson() {
        let deferred2_0;
        let deferred2_1;
        try {
            const ret = wasm.sliceresult_statsJson(this.__wbg_ptr);
            var ptr1 = ret[0];
            var len1 = ret[1];
            if (ret[3]) {
                ptr1 = 0; len1 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred2_0 = ptr1;
            deferred2_1 = len1;
            return getStringFromWasm0(ptr1, len1);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
}
if (Symbol.dispose) SliceResult.prototype[Symbol.dispose] = SliceResult.prototype.free;

/**
 * Slicer settings for WASM.
 */
export class SlicerSettings {
    static __wrap(ptr) {
        ptr = ptr >>> 0;
        const obj = Object.create(SlicerSettings.prototype);
        obj.__wbg_ptr = ptr;
        SlicerSettingsFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        SlicerSettingsFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_slicersettings_free(ptr, 0);
    }
    /**
     * First layer height (mm).
     * @returns {number}
     */
    get first_layer_height() {
        const ret = wasm.__wbg_get_slicersettings_first_layer_height(this.__wbg_ptr);
        return ret;
    }
    /**
     * Infill density (0-1).
     * @returns {number}
     */
    get infill_density() {
        const ret = wasm.__wbg_get_slicersettings_infill_density(this.__wbg_ptr);
        return ret;
    }
    /**
     * Infill pattern (0=Grid, 1=Lines, 2=Triangles, 3=Honeycomb, 4=Gyroid).
     * @returns {number}
     */
    get infill_pattern() {
        const ret = wasm.__wbg_get_slicersettings_infill_pattern(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * Layer height (mm).
     * @returns {number}
     */
    get layer_height() {
        const ret = wasm.__wbg_get_slicersettings_layer_height(this.__wbg_ptr);
        return ret;
    }
    /**
     * Line width (mm).
     * @returns {number}
     */
    get line_width() {
        const ret = wasm.__wbg_get_slicersettings_line_width(this.__wbg_ptr);
        return ret;
    }
    /**
     * Nozzle diameter (mm).
     * @returns {number}
     */
    get nozzle_diameter() {
        const ret = wasm.__wbg_get_slicersettings_nozzle_diameter(this.__wbg_ptr);
        return ret;
    }
    /**
     * Support angle threshold.
     * @returns {number}
     */
    get support_angle() {
        const ret = wasm.__wbg_get_slicersettings_support_angle(this.__wbg_ptr);
        return ret;
    }
    /**
     * Enable support.
     * @returns {boolean}
     */
    get support_enabled() {
        const ret = wasm.__wbg_get_slicersettings_support_enabled(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * Wall count.
     * @returns {number}
     */
    get wall_count() {
        const ret = wasm.__wbg_get_slicersettings_wall_count(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * First layer height (mm).
     * @param {number} arg0
     */
    set first_layer_height(arg0) {
        wasm.__wbg_set_slicersettings_first_layer_height(this.__wbg_ptr, arg0);
    }
    /**
     * Infill density (0-1).
     * @param {number} arg0
     */
    set infill_density(arg0) {
        wasm.__wbg_set_slicersettings_infill_density(this.__wbg_ptr, arg0);
    }
    /**
     * Infill pattern (0=Grid, 1=Lines, 2=Triangles, 3=Honeycomb, 4=Gyroid).
     * @param {number} arg0
     */
    set infill_pattern(arg0) {
        wasm.__wbg_set_slicersettings_infill_pattern(this.__wbg_ptr, arg0);
    }
    /**
     * Layer height (mm).
     * @param {number} arg0
     */
    set layer_height(arg0) {
        wasm.__wbg_set_slicersettings_layer_height(this.__wbg_ptr, arg0);
    }
    /**
     * Line width (mm).
     * @param {number} arg0
     */
    set line_width(arg0) {
        wasm.__wbg_set_slicersettings_line_width(this.__wbg_ptr, arg0);
    }
    /**
     * Nozzle diameter (mm).
     * @param {number} arg0
     */
    set nozzle_diameter(arg0) {
        wasm.__wbg_set_slicersettings_nozzle_diameter(this.__wbg_ptr, arg0);
    }
    /**
     * Support angle threshold.
     * @param {number} arg0
     */
    set support_angle(arg0) {
        wasm.__wbg_set_slicersettings_support_angle(this.__wbg_ptr, arg0);
    }
    /**
     * Enable support.
     * @param {boolean} arg0
     */
    set support_enabled(arg0) {
        wasm.__wbg_set_slicersettings_support_enabled(this.__wbg_ptr, arg0);
    }
    /**
     * Wall count.
     * @param {number} arg0
     */
    set wall_count(arg0) {
        wasm.__wbg_set_slicersettings_wall_count(this.__wbg_ptr, arg0);
    }
    /**
     * Create from JSON.
     * @param {string} json
     * @returns {SlicerSettings}
     */
    static fromJson(json) {
        const ptr0 = passStringToWasm0(json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.slicersettings_fromJson(ptr0, len0);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return SlicerSettings.__wrap(ret[0]);
    }
    /**
     * Create default settings.
     */
    constructor() {
        const ret = wasm.slicersettings_new();
        this.__wbg_ptr = ret >>> 0;
        SlicerSettingsFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
}
if (Symbol.dispose) SlicerSettings.prototype[Symbol.dispose] = SlicerSettings.prototype.free;

/**
 * A 3D solid geometry object.
 *
 * Create solids from primitives, combine with boolean operations,
 * transform, and extract triangle meshes for rendering.
 */
export class Solid {
    static __wrap(ptr) {
        ptr = ptr >>> 0;
        const obj = Object.create(Solid.prototype);
        obj.__wbg_ptr = ptr;
        SolidFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        SolidFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_solid_free(ptr, 0);
    }
    /**
     * Return mesh boundary edges as a flat float array
     * `[x0, y0, z0, x1, y1, z1, ...]` with each pair of 3-component
     * positions defining one edge segment. Used by the viewport's
     * "show boundary edges" overlay to surface tessellation holes.
     *
     * Closed, manifold meshes return an empty array; each entry means
     * there's a hole in the mesh.
     * @param {number | null} [segments]
     * @returns {Float32Array}
     */
    boundaryEdges(segments) {
        const ret = wasm.solid_boundaryEdges(this.__wbg_ptr, isLikeNone(segments) ? 0x100000001 : (segments) >>> 0);
        var v1 = getArrayF32FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 4, 4);
        return v1;
    }
    /**
     * Get the bounding box as [minX, minY, minZ, maxX, maxY, maxZ].
     * @returns {Float64Array}
     */
    boundingBox() {
        const ret = wasm.solid_boundingBox(this.__wbg_ptr);
        var v1 = getArrayF64FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 8, 8);
        return v1;
    }
    /**
     * Check if the solid can be exported to STEP format.
     *
     * Returns `true` if the solid has B-rep data available for STEP export.
     * Returns `false` for mesh-only or empty solids.
     * @returns {boolean}
     */
    canExportStep() {
        const ret = wasm.raytracer_canRaytrace(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * Get the center of mass as [x, y, z].
     * @returns {Float64Array}
     */
    centerOfMass() {
        const ret = wasm.solid_centerOfMass(this.__wbg_ptr);
        var v1 = getArrayF64FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 8, 8);
        return v1;
    }
    /**
     * Chamfer all edges of the solid by the given distance.
     * @param {number} distance
     * @returns {Solid}
     */
    chamfer(distance) {
        const ret = wasm.op_chamfer(this.__wbg_ptr, distance);
        return Solid.__wrap(ret);
    }
    /**
     * Create a circular pattern of the solid around an axis.
     *
     * # Arguments
     *
     * * `axis_origin_x/y/z` - A point on the rotation axis
     * * `axis_dir_x/y/z` - Direction of the rotation axis
     * * `count` - Number of copies (including original)
     * * `angle_deg` - Total angle span in degrees
     * @param {number} axis_origin_x
     * @param {number} axis_origin_y
     * @param {number} axis_origin_z
     * @param {number} axis_dir_x
     * @param {number} axis_dir_y
     * @param {number} axis_dir_z
     * @param {number} count
     * @param {number} angle_deg
     * @returns {Solid}
     */
    circularPattern(axis_origin_x, axis_origin_y, axis_origin_z, axis_dir_x, axis_dir_y, axis_dir_z, count, angle_deg) {
        const ret = wasm.op_circular_pattern(this.__wbg_ptr, axis_origin_x, axis_origin_y, axis_origin_z, axis_dir_x, axis_dir_y, axis_dir_z, count, angle_deg);
        return Solid.__wrap(ret);
    }
    /**
     * Create a cone/frustum along Z axis.
     * @param {number} radius_bottom
     * @param {number} radius_top
     * @param {number} height
     * @param {number | null} [segments]
     * @returns {Solid}
     */
    static cone(radius_bottom, radius_top, height, segments) {
        const ret = wasm.solid_cone(radius_bottom, radius_top, height, isLikeNone(segments) ? 0x100000001 : (segments) >>> 0);
        return Solid.__wrap(ret);
    }
    /**
     * Create a box with corner at origin and dimensions (sx, sy, sz).
     * @param {number} sx
     * @param {number} sy
     * @param {number} sz
     * @returns {Solid}
     */
    static cube(sx, sy, sz) {
        const ret = wasm.solid_cube(sx, sy, sz);
        return Solid.__wrap(ret);
    }
    /**
     * Create a cylinder along Z axis with given radius and height.
     * @param {number} radius
     * @param {number} height
     * @param {number | null} [segments]
     * @returns {Solid}
     */
    static cylinder(radius, height, segments) {
        const ret = wasm.solid_cylinder(radius, height, isLikeNone(segments) ? 0x100000001 : (segments) >>> 0);
        return Solid.__wrap(ret);
    }
    /**
     * Boolean difference (self − other).
     * @param {Solid} other
     * @returns {Solid}
     */
    difference(other) {
        _assertClass(other, Solid);
        const ret = wasm.solid_difference(this.__wbg_ptr, other.__wbg_ptr);
        return Solid.__wrap(ret);
    }
    /**
     * Create an empty solid.
     * @returns {Solid}
     */
    static empty() {
        const ret = wasm.solid_empty();
        return Solid.__wrap(ret);
    }
    /**
     * Create a solid by extruding a 2D sketch profile.
     *
     * Takes a sketch profile and extrusion direction as JS objects.
     * @param {string} profile_json
     * @param {Float64Array} direction
     * @returns {Solid}
     */
    static extrude(profile_json, direction) {
        const ptr0 = passStringToWasm0(profile_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passArrayF64ToWasm0(direction, wasm.__wbindgen_malloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.solid_extrude(ptr0, len0, ptr1, len1);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return Solid.__wrap(ret[0]);
    }
    /**
     * Create a solid by extruding a 2D sketch profile with twist and/or scale.
     *
     * Takes a sketch profile, extrusion direction, twist angle (radians),
     * and scale factor at the end (1.0 = no taper).
     * @param {string} profile_json
     * @param {Float64Array} direction
     * @param {number} twist_angle
     * @param {number} scale_end
     * @returns {Solid}
     */
    static extrudeWithOptions(profile_json, direction, twist_angle, scale_end) {
        const ptr0 = passStringToWasm0(profile_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passArrayF64ToWasm0(direction, wasm.__wbindgen_malloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.solid_extrudeWithOptions(ptr0, len0, ptr1, len1, twist_angle, scale_end);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return Solid.__wrap(ret[0]);
    }
    /**
     * Fillet all edges of the solid with the given radius.
     * @param {number} radius
     * @returns {Solid}
     */
    fillet(radius) {
        const ret = wasm.op_fillet(this.__wbg_ptr, radius);
        return Solid.__wrap(ret);
    }
    /**
     * Get the triangle mesh representation.
     *
     * Returns a JS object with `positions` (Float32Array) and `indices` (Uint32Array).
     * @param {number | null} [segments]
     * @returns {any}
     */
    getMesh(segments) {
        const ret = wasm.solid_getMesh(this.__wbg_ptr, isLikeNone(segments) ? 0x100000001 : (segments) >>> 0);
        return ret;
    }
    /**
     * Generate a horizontal section view at a given Z height.
     *
     * Convenience method that creates a horizontal section plane.
     * @param {number} z
     * @param {number | null} [hatch_spacing]
     * @param {number | null} [hatch_angle]
     * @param {number | null} [segments]
     * @returns {any}
     */
    horizontalSection(z, hatch_spacing, hatch_angle, segments) {
        const ret = wasm.solid_horizontalSection(this.__wbg_ptr, z, !isLikeNone(hatch_spacing), isLikeNone(hatch_spacing) ? 0 : hatch_spacing, !isLikeNone(hatch_angle), isLikeNone(hatch_angle) ? 0 : hatch_angle, isLikeNone(segments) ? 0x100000001 : (segments) >>> 0);
        return ret;
    }
    /**
     * Boolean intersection (self ∩ other).
     * @param {Solid} other
     * @returns {Solid}
     */
    intersection(other) {
        _assertClass(other, Solid);
        const ret = wasm.solid_intersection(this.__wbg_ptr, other.__wbg_ptr);
        return Solid.__wrap(ret);
    }
    /**
     * Check if the solid is empty (has no geometry).
     * @returns {boolean}
     */
    isEmpty() {
        const ret = wasm.solid_isEmpty(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * Create a linear pattern of the solid along a direction.
     *
     * # Arguments
     *
     * * `dir_x`, `dir_y`, `dir_z` - Direction vector
     * * `count` - Number of copies (including original)
     * * `spacing` - Distance between copies
     * @param {number} dir_x
     * @param {number} dir_y
     * @param {number} dir_z
     * @param {number} count
     * @param {number} spacing
     * @returns {Solid}
     */
    linearPattern(dir_x, dir_y, dir_z, count, spacing) {
        const ret = wasm.op_linear_pattern(this.__wbg_ptr, dir_x, dir_y, dir_z, count, spacing);
        return Solid.__wrap(ret);
    }
    /**
     * Create a solid by lofting between multiple profiles.
     *
     * Takes an array of sketch profiles (minimum 2).
     * @param {string} profiles_json
     * @param {boolean | null} [closed]
     * @returns {Solid}
     */
    static loft(profiles_json, closed) {
        const ptr0 = passStringToWasm0(profiles_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.solid_loft(ptr0, len0, isLikeNone(closed) ? 0xFFFFFF : closed ? 1 : 0);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return Solid.__wrap(ret[0]);
    }
    /**
     * Get the number of triangles in the tessellated mesh.
     * @returns {number}
     */
    numTriangles() {
        const ret = wasm.solid_numTriangles(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * Project the solid to a 2D view for technical drawing.
     *
     * # Arguments
     * * `view_direction` - View direction: "front", "back", "top", "bottom", "left", "right", or "isometric"
     * * `segments` - Number of segments for tessellation (optional, default 32)
     *
     * # Returns
     * A JS object containing the projected view with edges and bounds.
     * @param {string} view_direction
     * @param {number | null} [segments]
     * @returns {any}
     */
    projectView(view_direction, segments) {
        const ptr0 = passStringToWasm0(view_direction, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.solid_projectView(this.__wbg_ptr, ptr0, len0, isLikeNone(segments) ? 0x100000001 : (segments) >>> 0);
        return ret;
    }
    /**
     * Create a solid by revolving a 2D sketch profile around an axis.
     *
     * Takes a sketch profile, axis origin, axis direction, and angle in degrees.
     * @param {string} profile_json
     * @param {Float64Array} axis_origin
     * @param {Float64Array} axis_dir
     * @param {number} angle_deg
     * @returns {Solid}
     */
    static revolve(profile_json, axis_origin, axis_dir, angle_deg) {
        const ptr0 = passStringToWasm0(profile_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passArrayF64ToWasm0(axis_origin, wasm.__wbindgen_malloc);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passArrayF64ToWasm0(axis_dir, wasm.__wbindgen_malloc);
        const len2 = WASM_VECTOR_LEN;
        const ret = wasm.solid_revolve(ptr0, len0, ptr1, len1, ptr2, len2, angle_deg);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return Solid.__wrap(ret[0]);
    }
    /**
     * Rotate the solid by angles in degrees around X, Y, Z axes.
     * @param {number} x_deg
     * @param {number} y_deg
     * @param {number} z_deg
     * @returns {Solid}
     */
    rotate(x_deg, y_deg, z_deg) {
        const ret = wasm.solid_rotate(this.__wbg_ptr, x_deg, y_deg, z_deg);
        return Solid.__wrap(ret);
    }
    /**
     * Scale the solid by (x, y, z).
     * @param {number} x
     * @param {number} y
     * @param {number} z
     * @returns {Solid}
     */
    scale(x, y, z) {
        const ret = wasm.solid_scale(this.__wbg_ptr, x, y, z);
        return Solid.__wrap(ret);
    }
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
     * @param {string} plane_json
     * @param {string | null} [hatch_json]
     * @param {number | null} [segments]
     * @returns {any}
     */
    sectionView(plane_json, hatch_json, segments) {
        const ptr0 = passStringToWasm0(plane_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        var ptr1 = isLikeNone(hatch_json) ? 0 : passStringToWasm0(hatch_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len1 = WASM_VECTOR_LEN;
        const ret = wasm.solid_sectionView(this.__wbg_ptr, ptr0, len0, ptr1, len1, isLikeNone(segments) ? 0x100000001 : (segments) >>> 0);
        return ret;
    }
    /**
     * Shell (hollow) the solid by offsetting all faces inward.
     * @param {number} thickness
     * @returns {Solid}
     */
    shell(thickness) {
        const ret = wasm.op_shell(this.__wbg_ptr, thickness);
        return Solid.__wrap(ret);
    }
    /**
     * Create a sphere centered at origin with given radius.
     * @param {number} radius
     * @param {number | null} [segments]
     * @returns {Solid}
     */
    static sphere(radius, segments) {
        const ret = wasm.solid_sphere(radius, isLikeNone(segments) ? 0x100000001 : (segments) >>> 0);
        return Solid.__wrap(ret);
    }
    /**
     * Compute the surface area of the solid.
     * @returns {number}
     */
    surfaceArea() {
        const ret = wasm.solid_surfaceArea(this.__wbg_ptr);
        return ret;
    }
    /**
     * Create a solid by sweeping a profile along a helix path.
     *
     * Takes a sketch profile and helix parameters.
     * @param {string} profile_json
     * @param {number} radius
     * @param {number} pitch
     * @param {number} height
     * @param {number} turns
     * @param {number | null} [twist_angle]
     * @param {number | null} [scale_start]
     * @param {number | null} [scale_end]
     * @param {number | null} [path_segments]
     * @param {number | null} [arc_segments]
     * @param {number | null} [orientation]
     * @returns {Solid}
     */
    static sweepHelix(profile_json, radius, pitch, height, turns, twist_angle, scale_start, scale_end, path_segments, arc_segments, orientation) {
        const ptr0 = passStringToWasm0(profile_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.solid_sweepHelix(ptr0, len0, radius, pitch, height, turns, !isLikeNone(twist_angle), isLikeNone(twist_angle) ? 0 : twist_angle, !isLikeNone(scale_start), isLikeNone(scale_start) ? 0 : scale_start, !isLikeNone(scale_end), isLikeNone(scale_end) ? 0 : scale_end, isLikeNone(path_segments) ? 0x100000001 : (path_segments) >>> 0, isLikeNone(arc_segments) ? 0x100000001 : (arc_segments) >>> 0, !isLikeNone(orientation), isLikeNone(orientation) ? 0 : orientation);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return Solid.__wrap(ret[0]);
    }
    /**
     * Create a solid by sweeping a profile along a line path.
     *
     * Takes a sketch profile and path endpoints.
     * @param {string} profile_json
     * @param {Float64Array} start
     * @param {Float64Array} end
     * @param {number | null} [twist_angle]
     * @param {number | null} [scale_start]
     * @param {number | null} [scale_end]
     * @param {number | null} [orientation]
     * @returns {Solid}
     */
    static sweepLine(profile_json, start, end, twist_angle, scale_start, scale_end, orientation) {
        const ptr0 = passStringToWasm0(profile_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passArrayF64ToWasm0(start, wasm.__wbindgen_malloc);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passArrayF64ToWasm0(end, wasm.__wbindgen_malloc);
        const len2 = WASM_VECTOR_LEN;
        const ret = wasm.solid_sweepLine(ptr0, len0, ptr1, len1, ptr2, len2, !isLikeNone(twist_angle), isLikeNone(twist_angle) ? 0 : twist_angle, !isLikeNone(scale_start), isLikeNone(scale_start) ? 0 : scale_start, !isLikeNone(scale_end), isLikeNone(scale_end) ? 0 : scale_end, !isLikeNone(orientation), isLikeNone(orientation) ? 0 : orientation);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return Solid.__wrap(ret[0]);
    }
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
     * @param {string} text
     * @param {Float64Array} origin
     * @param {Float64Array} x_dir
     * @param {Float64Array} y_dir
     * @param {Float64Array} direction
     * @param {number} height
     * @param {string | null} [font]
     * @param {string | null} [alignment]
     * @param {number | null} [letter_spacing]
     * @param {number | null} [line_spacing]
     * @returns {Solid}
     */
    static textExtrude(text, origin, x_dir, y_dir, direction, height, font, alignment, letter_spacing, line_spacing) {
        const ptr0 = passStringToWasm0(text, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passArrayF64ToWasm0(origin, wasm.__wbindgen_malloc);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passArrayF64ToWasm0(x_dir, wasm.__wbindgen_malloc);
        const len2 = WASM_VECTOR_LEN;
        const ptr3 = passArrayF64ToWasm0(y_dir, wasm.__wbindgen_malloc);
        const len3 = WASM_VECTOR_LEN;
        const ptr4 = passArrayF64ToWasm0(direction, wasm.__wbindgen_malloc);
        const len4 = WASM_VECTOR_LEN;
        var ptr5 = isLikeNone(font) ? 0 : passStringToWasm0(font, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len5 = WASM_VECTOR_LEN;
        var ptr6 = isLikeNone(alignment) ? 0 : passStringToWasm0(alignment, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len6 = WASM_VECTOR_LEN;
        const ret = wasm.solid_textExtrude(ptr0, len0, ptr1, len1, ptr2, len2, ptr3, len3, ptr4, len4, height, ptr5, len5, ptr6, len6, !isLikeNone(letter_spacing), isLikeNone(letter_spacing) ? 0 : letter_spacing, !isLikeNone(line_spacing), isLikeNone(line_spacing) ? 0 : line_spacing);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return Solid.__wrap(ret[0]);
    }
    /**
     * Export the solid to STEP format.
     *
     * # Returns
     * A byte buffer containing the STEP file data.
     *
     * # Errors
     * Returns an error if the solid has no B-rep data (e.g., mesh-only after certain operations).
     * @returns {Uint8Array}
     */
    toStepBuffer() {
        const ret = wasm.solid_toStepBuffer(this.__wbg_ptr);
        if (ret[3]) {
            throw takeFromExternrefTable0(ret[2]);
        }
        var v1 = getArrayU8FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
        return v1;
    }
    /**
     * Translate the solid by (x, y, z).
     * @param {number} x
     * @param {number} y
     * @param {number} z
     * @returns {Solid}
     */
    translate(x, y, z) {
        const ret = wasm.solid_translate(this.__wbg_ptr, x, y, z);
        return Solid.__wrap(ret);
    }
    /**
     * Boolean union (self ∪ other).
     * @param {Solid} other
     * @returns {Solid}
     */
    union(other) {
        _assertClass(other, Solid);
        const ret = wasm.solid_union(this.__wbg_ptr, other.__wbg_ptr);
        return Solid.__wrap(ret);
    }
    /**
     * Compute the volume of the solid.
     * @returns {number}
     */
    volume() {
        const ret = wasm.solid_volume(this.__wbg_ptr);
        return ret;
    }
}
if (Symbol.dispose) Solid.prototype[Symbol.dispose] = Solid.prototype.free;

/**
 * Annotation layer for dimension annotations.
 *
 * This class provides methods for creating and rendering dimension annotations
 * on 2D projected views.
 */
export class WasmAnnotationLayer {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        WasmAnnotationLayerFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_wasmannotationlayer_free(ptr, 0);
    }
    /**
     * Add an aligned dimension between two points.
     *
     * The dimension line is parallel to the line connecting the two points.
     *
     * # Arguments
     * * `x1`, `y1` - First point coordinates
     * * `x2`, `y2` - Second point coordinates
     * * `offset` - Distance from points to dimension line
     * @param {number} x1
     * @param {number} y1
     * @param {number} x2
     * @param {number} y2
     * @param {number} offset
     */
    addAlignedDimension(x1, y1, x2, y2, offset) {
        wasm.wasmannotationlayer_addAlignedDimension(this.__wbg_ptr, x1, y1, x2, y2, offset);
    }
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
     * @param {number} x1
     * @param {number} y1
     * @param {number} vx
     * @param {number} vy
     * @param {number} x2
     * @param {number} y2
     * @param {number} arc_radius
     */
    addAngleDimension(x1, y1, vx, vy, x2, y2, arc_radius) {
        wasm.wasmannotationlayer_addAngleDimension(this.__wbg_ptr, x1, y1, vx, vy, x2, y2, arc_radius);
    }
    /**
     * Add a diameter dimension for a circle.
     *
     * # Arguments
     * * `cx`, `cy` - Center of the circle
     * * `radius` - Radius of the circle
     * * `leader_angle` - Angle in radians for the leader line direction
     * @param {number} cx
     * @param {number} cy
     * @param {number} radius
     * @param {number} leader_angle
     */
    addDiameterDimension(cx, cy, radius, leader_angle) {
        wasm.wasmannotationlayer_addDiameterDimension(this.__wbg_ptr, cx, cy, radius, leader_angle);
    }
    /**
     * Add a horizontal dimension between two points.
     *
     * # Arguments
     * * `x1`, `y1` - First point coordinates
     * * `x2`, `y2` - Second point coordinates
     * * `offset` - Distance from points to dimension line (positive = above)
     * @param {number} x1
     * @param {number} y1
     * @param {number} x2
     * @param {number} y2
     * @param {number} offset
     */
    addHorizontalDimension(x1, y1, x2, y2, offset) {
        wasm.wasmannotationlayer_addHorizontalDimension(this.__wbg_ptr, x1, y1, x2, y2, offset);
    }
    /**
     * Add a radius dimension for a circle.
     *
     * # Arguments
     * * `cx`, `cy` - Center of the circle
     * * `radius` - Radius of the circle
     * * `leader_angle` - Angle in radians for the leader line direction
     * @param {number} cx
     * @param {number} cy
     * @param {number} radius
     * @param {number} leader_angle
     */
    addRadiusDimension(cx, cy, radius, leader_angle) {
        wasm.wasmannotationlayer_addRadiusDimension(this.__wbg_ptr, cx, cy, radius, leader_angle);
    }
    /**
     * Add a vertical dimension between two points.
     *
     * # Arguments
     * * `x1`, `y1` - First point coordinates
     * * `x2`, `y2` - Second point coordinates
     * * `offset` - Distance from points to dimension line (positive = right)
     * @param {number} x1
     * @param {number} y1
     * @param {number} x2
     * @param {number} y2
     * @param {number} offset
     */
    addVerticalDimension(x1, y1, x2, y2, offset) {
        wasm.wasmannotationlayer_addVerticalDimension(this.__wbg_ptr, x1, y1, x2, y2, offset);
    }
    /**
     * Get the number of annotations in the layer.
     * @returns {number}
     */
    annotationCount() {
        const ret = wasm.wasmannotationlayer_annotationCount(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * Clear all annotations from the layer.
     */
    clear() {
        wasm.wasmannotationlayer_clear(this.__wbg_ptr);
    }
    /**
     * Check if the layer has any annotations.
     * @returns {boolean}
     */
    isEmpty() {
        const ret = wasm.wasmannotationlayer_isEmpty(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * Create a new empty annotation layer.
     */
    constructor() {
        const ret = wasm.wasmannotationlayer_new();
        this.__wbg_ptr = ret >>> 0;
        WasmAnnotationLayerFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
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
     * @param {string | null} [view_json]
     * @returns {any}
     */
    renderAll(view_json) {
        var ptr0 = isLikeNone(view_json) ? 0 : passStringToWasm0(view_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len0 = WASM_VECTOR_LEN;
        const ret = wasm.wasmannotationlayer_renderAll(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
}
if (Symbol.dispose) WasmAnnotationLayer.prototype[Symbol.dispose] = WasmAnnotationLayer.prototype.free;

/**
 * CAM settings for WASM.
 */
export class WasmCamSettings {
    static __wrap(ptr) {
        ptr = ptr >>> 0;
        const obj = Object.create(WasmCamSettings.prototype);
        obj.__wbg_ptr = ptr;
        WasmCamSettingsFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        WasmCamSettingsFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_wasmcamsettings_free(ptr, 0);
    }
    /**
     * Feed rate (mm/min).
     * @returns {number}
     */
    get feed_rate() {
        const ret = wasm.__wbg_get_wasmcamsettings_feed_rate(this.__wbg_ptr);
        return ret;
    }
    /**
     * Plunge rate (mm/min).
     * @returns {number}
     */
    get plunge_rate() {
        const ret = wasm.__wbg_get_wasmcamsettings_plunge_rate(this.__wbg_ptr);
        return ret;
    }
    /**
     * Retract Z height (mm).
     * @returns {number}
     */
    get retract_z() {
        const ret = wasm.__wbg_get_wasmcamsettings_retract_z(this.__wbg_ptr);
        return ret;
    }
    /**
     * Safe Z height (mm).
     * @returns {number}
     */
    get safe_z() {
        const ret = wasm.__wbg_get_wasmcamsettings_safe_z(this.__wbg_ptr);
        return ret;
    }
    /**
     * Spindle RPM.
     * @returns {number}
     */
    get spindle_rpm() {
        const ret = wasm.__wbg_get_wasmcamsettings_spindle_rpm(this.__wbg_ptr);
        return ret;
    }
    /**
     * Stepdown distance (mm).
     * @returns {number}
     */
    get stepdown() {
        const ret = wasm.__wbg_get_wasmcamsettings_stepdown(this.__wbg_ptr);
        return ret;
    }
    /**
     * Stepover distance (mm).
     * @returns {number}
     */
    get stepover() {
        const ret = wasm.__wbg_get_wasmcamsettings_stepover(this.__wbg_ptr);
        return ret;
    }
    /**
     * Feed rate (mm/min).
     * @param {number} arg0
     */
    set feed_rate(arg0) {
        wasm.__wbg_set_wasmcamsettings_feed_rate(this.__wbg_ptr, arg0);
    }
    /**
     * Plunge rate (mm/min).
     * @param {number} arg0
     */
    set plunge_rate(arg0) {
        wasm.__wbg_set_wasmcamsettings_plunge_rate(this.__wbg_ptr, arg0);
    }
    /**
     * Retract Z height (mm).
     * @param {number} arg0
     */
    set retract_z(arg0) {
        wasm.__wbg_set_wasmcamsettings_retract_z(this.__wbg_ptr, arg0);
    }
    /**
     * Safe Z height (mm).
     * @param {number} arg0
     */
    set safe_z(arg0) {
        wasm.__wbg_set_wasmcamsettings_safe_z(this.__wbg_ptr, arg0);
    }
    /**
     * Spindle RPM.
     * @param {number} arg0
     */
    set spindle_rpm(arg0) {
        wasm.__wbg_set_wasmcamsettings_spindle_rpm(this.__wbg_ptr, arg0);
    }
    /**
     * Stepdown distance (mm).
     * @param {number} arg0
     */
    set stepdown(arg0) {
        wasm.__wbg_set_wasmcamsettings_stepdown(this.__wbg_ptr, arg0);
    }
    /**
     * Stepover distance (mm).
     * @param {number} arg0
     */
    set stepover(arg0) {
        wasm.__wbg_set_wasmcamsettings_stepover(this.__wbg_ptr, arg0);
    }
    /**
     * Create from JSON.
     * @param {string} json
     * @returns {WasmCamSettings}
     */
    static fromJson(json) {
        const ptr0 = passStringToWasm0(json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.wasmcamsettings_fromJson(ptr0, len0);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return WasmCamSettings.__wrap(ret[0]);
    }
    /**
     * Create default CAM settings.
     */
    constructor() {
        const ret = wasm.wasmcamsettings_new();
        this.__wbg_ptr = ret >>> 0;
        WasmCamSettingsFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
}
if (Symbol.dispose) WasmCamSettings.prototype[Symbol.dispose] = WasmCamSettings.prototype.free;

/**
 * CRDT-backed document engine for WASM.
 *
 * Wraps a `DocumentApi` (which wraps a `CrdtDocument`) and exposes both
 * typed mutations via `add_feature(json)` and legacy low-level CRDT methods.
 */
export class WasmDocumentEngine {
    static __wrap(ptr) {
        ptr = ptr >>> 0;
        const obj = Object.create(WasmDocumentEngine.prototype);
        obj.__wbg_ptr = ptr;
        WasmDocumentEngineFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        WasmDocumentEngineFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_wasmdocumentengine_free(ptr, 0);
    }
    /**
     * Add a feature from a JSON-serialized `FeatureInput` discriminated union.
     *
     * Example: `{"type":"Cube","size_x":10,"size_y":20,"size_z":30}`
     *
     * Returns `{ document, parts, consumedPartIds, createdFeatureId }`.
     * @param {string} input_json
     * @returns {any}
     */
    add_feature(input_json) {
        const ptr0 = passStringToWasm0(input_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.wasmdocumentengine_add_feature(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * Whether redo is available.
     * @returns {boolean}
     */
    can_redo() {
        const ret = wasm.wasmdocumentengine_can_redo(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * Whether undo is available.
     * @returns {boolean}
     */
    can_undo() {
        const ret = wasm.wasmdocumentengine_can_undo(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * Compute a FractionalIndex position between two neighbor feature IDs.
     * @param {string} before_id_json
     * @param {string} after_id_json
     * @returns {string}
     */
    compute_position_between(before_id_json, after_id_json) {
        let deferred3_0;
        let deferred3_1;
        try {
            const ptr0 = passStringToWasm0(before_id_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ptr1 = passStringToWasm0(after_id_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            const ret = wasm.wasmdocumentengine_compute_position_between(this.__wbg_ptr, ptr0, len0, ptr1, len1);
            deferred3_0 = ret[0];
            deferred3_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
        }
    }
    /**
     * Create a feature with the given kind and params (JSON string).
     *
     * Returns `{ document, parts, createdFeatureId }` as a JsValue.
     * @param {string} kind
     * @param {string} params_json
     * @returns {any}
     */
    create_feature(kind, params_json) {
        const ptr0 = passStringToWasm0(kind, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(params_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.wasmdocumentengine_create_feature(this.__wbg_ptr, ptr0, len0, ptr1, len1);
        return ret;
    }
    /**
     * Delete a feature by ID (JSON string).
     * @param {string} feature_id_json
     * @returns {any}
     */
    delete_feature(feature_id_json) {
        const ptr0 = passStringToWasm0(feature_id_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.wasmdocumentengine_delete_feature(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * Delete a feature by stable ID.
     * @param {string} stable_id
     * @returns {any}
     */
    delete_feature_by_id(stable_id) {
        const ptr0 = passStringToWasm0(stable_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.wasmdocumentengine_delete_feature_by_id(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * Load a legacy v1 JSON document and migrate to CRDT.
     * @param {string} json
     * @returns {WasmDocumentEngine}
     */
    static from_v1_json(json) {
        const ptr0 = passStringToWasm0(json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.wasmdocumentengine_from_v1_json(ptr0, len0);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return WasmDocumentEngine.__wrap(ret[0]);
    }
    /**
     * Get the materialized document as JSON.
     * @returns {string}
     */
    get_document_json() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.wasmdocumentengine_get_document_json(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Get operations since a remote clock state (JSON).
     * @param {string} remote_clock_json
     * @returns {string}
     */
    get_ops_since(remote_clock_json) {
        let deferred2_0;
        let deferred2_1;
        try {
            const ptr0 = passStringToWasm0(remote_clock_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ret = wasm.wasmdocumentengine_get_ops_since(this.__wbg_ptr, ptr0, len0);
            deferred2_0 = ret[0];
            deferred2_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * Get ordered features (for the feature tree) as JSON.
     * @returns {string}
     */
    get_ordered_features_json() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.wasmdocumentengine_get_ordered_features_json(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Get the parts list as JSON.
     * @returns {string}
     */
    get_parts_json() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.wasmdocumentengine_get_parts_json(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Get the sync clock as JSON.
     * @returns {string}
     */
    get_sync_clock() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.wasmdocumentengine_get_sync_clock(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Import IR JSON into the current document (e.g. AI-generated geometry).
     * @param {string} ir_json
     * @returns {any}
     */
    import_ir(ir_json) {
        const ptr0 = passStringToWasm0(ir_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.wasmdocumentengine_import_ir(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * Load a document from bytes.
     *
     * Auto-detects format: if CRDT (v2), loads directly; if legacy JSON (v1),
     * migrates to CRDT first.
     * @param {Uint8Array} bytes
     * @returns {WasmDocumentEngine}
     */
    static load(bytes) {
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.wasmdocumentengine_load(ptr0, len0);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return WasmDocumentEngine.__wrap(ret[0]);
    }
    /**
     * Merge remote operations (JSON array of Op).
     * @param {string} ops_json
     * @returns {any}
     */
    merge_remote(ops_json) {
        const ptr0 = passStringToWasm0(ops_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.wasmdocumentengine_merge_remote(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * Move a feature to a new position.
     * @param {string} feature_id_json
     * @param {string} position_json
     * @returns {any}
     */
    move_feature(feature_id_json, position_json) {
        const ptr0 = passStringToWasm0(feature_id_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(position_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.wasmdocumentengine_move_feature(this.__wbg_ptr, ptr0, len0, ptr1, len1);
        return ret;
    }
    /**
     * Create a new empty document engine.
     */
    constructor() {
        const ret = wasm.wasmdocumentengine_new();
        this.__wbg_ptr = ret >>> 0;
        WasmDocumentEngineFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
    /**
     * Redo the last undone action.
     * @returns {any}
     */
    redo() {
        const ret = wasm.wasmdocumentengine_redo(this.__wbg_ptr);
        return ret;
    }
    /**
     * Rename a feature.
     * @param {string} stable_id
     * @param {string} name
     * @returns {any}
     */
    rename_feature(stable_id, name) {
        const ptr0 = passStringToWasm0(stable_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.wasmdocumentengine_rename_feature(this.__wbg_ptr, ptr0, len0, ptr1, len1);
        return ret;
    }
    /**
     * Save the document to bytes.
     * @returns {Uint8Array}
     */
    save() {
        const ret = wasm.wasmdocumentengine_save(this.__wbg_ptr);
        var v1 = getArrayU8FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
        return v1;
    }
    /**
     * Set joint state.
     * @param {string} stable_id
     * @param {number} state
     * @returns {any}
     */
    set_joint_state(stable_id, state) {
        const ptr0 = passStringToWasm0(stable_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.wasmdocumentengine_set_joint_state(this.__wbg_ptr, ptr0, len0, state);
        return ret;
    }
    /**
     * Set material on a feature.
     * @param {string} stable_id
     * @param {string} material
     * @returns {any}
     */
    set_material(stable_id, material) {
        const ptr0 = passStringToWasm0(stable_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(material, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.wasmdocumentengine_set_material(this.__wbg_ptr, ptr0, len0, ptr1, len1);
        return ret;
    }
    /**
     * Set a parameter on a feature.
     * @param {string} feature_id_json
     * @param {string} key
     * @param {string} value_json
     * @returns {any}
     */
    set_param(feature_id_json, key, value_json) {
        const ptr0 = passStringToWasm0(feature_id_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(key, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passStringToWasm0(value_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len2 = WASM_VECTOR_LEN;
        const ret = wasm.wasmdocumentengine_set_param(this.__wbg_ptr, ptr0, len0, ptr1, len1, ptr2, len2);
        return ret;
    }
    /**
     * Set rotation on a feature.
     * @param {string} stable_id
     * @param {number} x
     * @param {number} y
     * @param {number} z
     * @returns {any}
     */
    set_rotation(stable_id, x, y, z) {
        const ptr0 = passStringToWasm0(stable_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.wasmdocumentengine_set_rotation(this.__wbg_ptr, ptr0, len0, x, y, z);
        return ret;
    }
    /**
     * Set scale on a feature.
     * @param {string} stable_id
     * @param {number} x
     * @param {number} y
     * @param {number} z
     * @returns {any}
     */
    set_scale(stable_id, x, y, z) {
        const ptr0 = passStringToWasm0(stable_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.wasmdocumentengine_set_scale(this.__wbg_ptr, ptr0, len0, x, y, z);
        return ret;
    }
    /**
     * Set translation on a feature.
     * @param {string} stable_id
     * @param {number} x
     * @param {number} y
     * @param {number} z
     * @returns {any}
     */
    set_translation(stable_id, x, y, z) {
        const ptr0 = passStringToWasm0(stable_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.wasmdocumentengine_set_translation(this.__wbg_ptr, ptr0, len0, x, y, z);
        return ret;
    }
    /**
     * Set visibility on a feature.
     * @param {string} stable_id
     * @param {boolean} visible
     * @returns {any}
     */
    set_visible(stable_id, visible) {
        const ptr0 = passStringToWasm0(stable_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.wasmdocumentengine_set_visible(this.__wbg_ptr, ptr0, len0, visible);
        return ret;
    }
    /**
     * Undo the last action.
     * @returns {any}
     */
    undo() {
        const ret = wasm.wasmdocumentengine_undo(this.__wbg_ptr);
        return ret;
    }
    /**
     * Update a feature with new params from a JSON-serialized `FeatureInput`.
     * @param {string} stable_id
     * @param {string} input_json
     * @returns {any}
     */
    update_feature(stable_id, input_json) {
        const ptr0 = passStringToWasm0(stable_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(input_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.wasmdocumentengine_update_feature(this.__wbg_ptr, ptr0, len0, ptr1, len1);
        return ret;
    }
}
if (Symbol.dispose) WasmDocumentEngine.prototype[Symbol.dispose] = WasmDocumentEngine.prototype.free;

export class WasmKeybindings {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        WasmKeybindingsFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_wasmkeybindings_free(ptr, 0);
    }
    /**
     * Return the effective chord (user override or default) for a command
     * id, or `None` if disabled / unbound.
     * @param {string} id
     * @returns {string | undefined}
     */
    chordFor(id) {
        const ptr0 = passStringToWasm0(id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.wasmkeybindings_chordFor(this.__wbg_ptr, ptr0, len0);
        let v2;
        if (ret[0] !== 0) {
            v2 = getStringFromWasm0(ret[0], ret[1]).slice();
            wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
        }
        return v2;
    }
    /**
     * Returns a JSON array describing every registered command. The TS UI
     * (command palette, keyboard prefs) reads this once at startup.
     *
     * Each entry is a [`CommandView`] — a flattened, owned projection of
     * `Command` that serde can serialize (the source struct uses `&'static
     * str` and a non-serializable `ModeScope` enum).
     * @returns {string}
     */
    commandsJson() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.wasmkeybindings_commandsJson(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Report binding conflicts in the given mode: pairs of commands that
     * share the same chord. Returns a JSON array for the prefs UI to
     * highlight.
     * @param {string} mode_name
     * @returns {string}
     */
    conflictsJson(mode_name) {
        let deferred2_0;
        let deferred2_1;
        try {
            const ptr0 = passStringToWasm0(mode_name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ret = wasm.wasmkeybindings_conflictsJson(this.__wbg_ptr, ptr0, len0);
            deferred2_0 = ret[0];
            deferred2_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * Load overrides previously returned by [`save_overrides`]. Malformed
     * entries are skipped — the caller never sees a parse failure for
     * stale config.
     * @param {string} json
     * @returns {boolean}
     */
    loadOverrides(json) {
        const ptr0 = passStringToWasm0(json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.wasmkeybindings_loadOverrides(this.__wbg_ptr, ptr0, len0);
        return ret !== 0;
    }
    /**
     * Construct a fresh registry with all default bindings.
     */
    constructor() {
        const ret = wasm.wasmkeybindings_new();
        this.__wbg_ptr = ret >>> 0;
        WasmKeybindingsFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
    /**
     * Clear all user overrides, restoring default bindings.
     */
    resetAll() {
        wasm.wasmkeybindings_resetAll(this.__wbg_ptr);
    }
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
     * @param {string} chord_json
     * @param {string} mode_name
     * @param {number} ctx_bits
     * @returns {string | undefined}
     */
    resolve(chord_json, mode_name, ctx_bits) {
        const ptr0 = passStringToWasm0(chord_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(mode_name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.wasmkeybindings_resolve(this.__wbg_ptr, ptr0, len0, ptr1, len1, ctx_bits);
        let v3;
        if (ret[0] !== 0) {
            v3 = getStringFromWasm0(ret[0], ret[1]).slice();
            wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
        }
        return v3;
    }
    /**
     * Serialize user overrides for persistence (e.g. localStorage).
     * @returns {string}
     */
    saveOverrides() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.wasmkeybindings_saveOverrides(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Rebind a command. Pass a JSON-encoded chord to set, or `None` to
     * clear (disabling the binding).
     * @param {string} id
     * @param {string | null} [chord_json]
     */
    setBinding(id, chord_json) {
        const ptr0 = passStringToWasm0(id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        var ptr1 = isLikeNone(chord_json) ? 0 : passStringToWasm0(chord_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len1 = WASM_VECTOR_LEN;
        wasm.wasmkeybindings_setBinding(this.__wbg_ptr, ptr0, len0, ptr1, len1);
    }
}
if (Symbol.dispose) WasmKeybindings.prototype[Symbol.dispose] = WasmKeybindings.prototype.free;

/**
 * A sketch editing session bound to JavaScript. See module docs.
 */
export class WasmSketchSession {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        WasmSketchSessionFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_wasmsketchsession_free(ptr, 0);
    }
    /**
     * Add a full circle.
     * @param {number} cx
     * @param {number} cy
     * @param {number} radius
     */
    addCircle(cx, cy, radius) {
        wasm.wasmsketchsession_addCircle(this.__wbg_ptr, cx, cy, radius);
    }
    /**
     * Add a constraint from a JSON object matching the TypeScript
     * `SketchConstraint` shape.
     * @param {string} json
     */
    addConstraint(json) {
        const ptr0 = passStringToWasm0(json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.wasmsketchsession_addConstraint(this.__wbg_ptr, ptr0, len0);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * Add a line directly (for scripted / MCP use).
     * @param {number} x1
     * @param {number} y1
     * @param {number} x2
     * @param {number} y2
     */
    addLine(x1, y1, x2, y2) {
        wasm.wasmsketchsession_addLine(this.__wbg_ptr, x1, y1, x2, y2);
    }
    /**
     * Add an axis-aligned rectangle between two corners.
     * @param {number} x1
     * @param {number} y1
     * @param {number} x2
     * @param {number} y2
     */
    addRectangle(x1, y1, x2, y2) {
        wasm.wasmsketchsession_addRectangle(this.__wbg_ptr, x1, y1, x2, y2);
    }
    /**
     * Clear pending input.
     */
    cancelPending() {
        wasm.wasmsketchsession_cancelPending(this.__wbg_ptr);
    }
    /**
     * Clear every entity and constraint.
     */
    clear() {
        wasm.wasmsketchsession_clear(this.__wbg_ptr);
    }
    /**
     * Clear the selection.
     */
    clearSelection() {
        wasm.wasmsketchsession_clearSelection(this.__wbg_ptr);
    }
    /**
     * Construct a new session on the given plane.
     *
     * `plane_json` is either a JSON string (`"XY"` / `"XZ"` / `"YZ"`) or a
     * JSON object `{ origin, xDir, yDir }` for a custom plane.
     * @param {string} plane_json
     */
    constructor(plane_json) {
        const ptr0 = passStringToWasm0(plane_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.wasmsketchsession_new(ptr0, len0);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        this.__wbg_ptr = ret[0] >>> 0;
        WasmSketchSessionFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
    /**
     * Handle a primary-button click at the current cursor position. Returns
     * a short outcome string: `"no-cursor"`, `"selection"`, `"pending"`, or
     * `"committed"`.
     * @returns {string}
     */
    onClick() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.wasmsketchsession_onClick(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Clear the cursor.
     */
    onCursorLeave() {
        wasm.wasmsketchsession_onCursorLeave(this.__wbg_ptr);
    }
    /**
     * Update the cursor from a world-space ray (e.g. camera pick ray).
     * @param {number} ox
     * @param {number} oy
     * @param {number} oz
     * @param {number} dx
     * @param {number} dy
     * @param {number} dz
     */
    onCursorRay(ox, oy, oz, dx, dy, dz) {
        wasm.wasmsketchsession_onCursorRay(this.__wbg_ptr, ox, oy, oz, dx, dy, dz);
    }
    /**
     * Update the cursor directly from 2D sketch coordinates.
     * @param {number} x
     * @param {number} y
     */
    onCursorSketch(x, y) {
        wasm.wasmsketchsession_onCursorSketch(this.__wbg_ptr, x, y);
    }
    /**
     * Handle a double-click (closes a polyline for the line tool).
     */
    onDoubleClick() {
        wasm.wasmsketchsession_onDoubleClick(this.__wbg_ptr);
    }
    /**
     * Redo the last undone mutation. Returns `true` if anything was redone.
     * @returns {boolean}
     */
    redo() {
        const ret = wasm.wasmsketchsession_redo(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * Remove the constraint at `index`.
     * @param {number} index
     */
    removeConstraint(index) {
        wasm.wasmsketchsession_removeConstraint(this.__wbg_ptr, index);
    }
    /**
     * Configure snapping behavior.
     * @param {boolean} grid_enabled
     * @param {number} grid_size
     * @param {boolean} point_enabled
     * @param {number} point_tolerance
     */
    setSnap(grid_enabled, grid_size, point_enabled, point_tolerance) {
        wasm.wasmsketchsession_setSnap(this.__wbg_ptr, grid_enabled, grid_size, point_enabled, point_tolerance);
    }
    /**
     * Change the active drawing tool. Unknown names are ignored.
     * @param {string} tool
     */
    setTool(tool) {
        const ptr0 = passStringToWasm0(tool, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        wasm.wasmsketchsession_setTool(this.__wbg_ptr, ptr0, len0);
    }
    /**
     * Return a JSON snapshot of the full session state. React can mirror
     * this into its own store on every mutation.
     * @returns {string}
     */
    snapshot() {
        let deferred2_0;
        let deferred2_1;
        try {
            const ret = wasm.wasmsketchsession_snapshot(this.__wbg_ptr);
            var ptr1 = ret[0];
            var len1 = ret[1];
            if (ret[3]) {
                ptr1 = 0; len1 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred2_0 = ptr1;
            deferred2_1 = len1;
            return getStringFromWasm0(ptr1, len1);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * Run the constraint solver. Returns `true` if it converged.
     * @returns {boolean}
     */
    solve() {
        const ret = wasm.wasmsketchsession_solve(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * Test-select or deselect a segment.
     * @param {number} segment_index
     */
    toggleSelection(segment_index) {
        wasm.wasmsketchsession_toggleSelection(this.__wbg_ptr, segment_index);
    }
    /**
     * Undo the last mutation. Returns `true` if anything was undone.
     * @returns {boolean}
     */
    undo() {
        const ret = wasm.wasmsketchsession_undo(this.__wbg_ptr);
        return ret !== 0;
    }
}
if (Symbol.dispose) WasmSketchSession.prototype[Symbol.dispose] = WasmSketchSession.prototype.free;

/**
 * Analyze a solid for 3D printing characteristics.
 *
 * Returns JSON with wall thicknesses, overhang angles, hole sizes, etc.
 * Only works on solids with BRep data (primitives, not boolean results).
 * @param {Solid} solid
 * @returns {any}
 */
export function analyzeForPrinting(solid) {
    _assertClass(solid, Solid);
    const ret = wasm.analyzeForPrinting(solid.__wbg_ptr);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

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
 * @param {string} parts_json
 * @param {string} selection_json
 * @returns {string}
 */
export function build_chat_system_prompt(parts_json, selection_json) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passStringToWasm0(parts_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(selection_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.build_chat_system_prompt(ptr0, len0, ptr1, len1);
        deferred3_0 = ret[0];
        deferred3_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
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
 * @param {string} vertices_json
 * @param {string} indices_json
 * @param {string} tool_json
 * @param {string} bounds_json
 * @param {number} resolution
 * @returns {string}
 */
export function camDropCutter(vertices_json, indices_json, tool_json, bounds_json, resolution) {
    let deferred6_0;
    let deferred6_1;
    try {
        const ptr0 = passStringToWasm0(vertices_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(indices_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passStringToWasm0(tool_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len2 = WASM_VECTOR_LEN;
        const ptr3 = passStringToWasm0(bounds_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len3 = WASM_VECTOR_LEN;
        const ret = wasm.camDropCutter(ptr0, len0, ptr1, len1, ptr2, len2, ptr3, len3, resolution);
        var ptr5 = ret[0];
        var len5 = ret[1];
        if (ret[3]) {
            ptr5 = 0; len5 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred6_0 = ptr5;
        deferred6_1 = len5;
        return getStringFromWasm0(ptr5, len5);
    } finally {
        wasm.__wbindgen_free(deferred6_0, deferred6_1, 1);
    }
}

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
 * @param {string} toolpath_json
 * @param {string} job_name
 * @param {string} tool_json
 * @param {WasmCamSettings} settings
 * @returns {string}
 */
export function camExportGcode(toolpath_json, job_name, tool_json, settings) {
    let deferred5_0;
    let deferred5_1;
    try {
        const ptr0 = passStringToWasm0(toolpath_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(job_name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passStringToWasm0(tool_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len2 = WASM_VECTOR_LEN;
        _assertClass(settings, WasmCamSettings);
        const ret = wasm.camExportGcode(ptr0, len0, ptr1, len1, ptr2, len2, settings.__wbg_ptr);
        var ptr4 = ret[0];
        var len4 = ret[1];
        if (ret[3]) {
            ptr4 = 0; len4 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred5_0 = ptr4;
        deferred5_1 = len4;
        return getStringFromWasm0(ptr4, len4);
    } finally {
        wasm.__wbindgen_free(deferred5_0, deferred5_1, 1);
    }
}

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
 * @param {string} toolpath_json
 * @param {string} job_name
 * @param {string} tool_json
 * @param {WasmCamSettings} settings
 * @param {number} program_number
 * @returns {string}
 */
export function camExportLinuxCnc(toolpath_json, job_name, tool_json, settings, program_number) {
    let deferred5_0;
    let deferred5_1;
    try {
        const ptr0 = passStringToWasm0(toolpath_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(job_name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passStringToWasm0(tool_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len2 = WASM_VECTOR_LEN;
        _assertClass(settings, WasmCamSettings);
        const ret = wasm.camExportLinuxCnc(ptr0, len0, ptr1, len1, ptr2, len2, settings.__wbg_ptr, program_number);
        var ptr4 = ret[0];
        var len4 = ret[1];
        if (ret[3]) {
            ptr4 = 0; len4 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred5_0 = ptr4;
        deferred5_1 = len4;
        return getStringFromWasm0(ptr4, len4);
    } finally {
        wasm.__wbindgen_free(deferred5_0, deferred5_1, 1);
    }
}

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
 * @param {number} cx
 * @param {number} cy
 * @param {number} radius
 * @param {number} depth
 * @param {string} tool_json
 * @param {WasmCamSettings} settings
 * @returns {string}
 */
export function camGenerateCircularPocket(cx, cy, radius, depth, tool_json, settings) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passStringToWasm0(tool_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        _assertClass(settings, WasmCamSettings);
        const ret = wasm.camGenerateCircularPocket(cx, cy, radius, depth, ptr0, len0, settings.__wbg_ptr);
        var ptr2 = ret[0];
        var len2 = ret[1];
        if (ret[3]) {
            ptr2 = 0; len2 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred3_0 = ptr2;
        deferred3_1 = len2;
        return getStringFromWasm0(ptr2, len2);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

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
 * @param {number} x
 * @param {number} y
 * @param {number} width
 * @param {number} height
 * @param {number} depth
 * @param {number} offset
 * @param {number} tab_count
 * @param {number} tab_width
 * @param {number} tab_height
 * @param {string} tool_json
 * @param {WasmCamSettings} settings
 * @returns {string}
 */
export function camGenerateContour(x, y, width, height, depth, offset, tab_count, tab_width, tab_height, tool_json, settings) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passStringToWasm0(tool_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        _assertClass(settings, WasmCamSettings);
        const ret = wasm.camGenerateContour(x, y, width, height, depth, offset, tab_count, tab_width, tab_height, ptr0, len0, settings.__wbg_ptr);
        var ptr2 = ret[0];
        var len2 = ret[1];
        if (ret[3]) {
            ptr2 = 0; len2 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred3_0 = ptr2;
        deferred3_1 = len2;
        return getStringFromWasm0(ptr2, len2);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

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
 * @param {number} min_x
 * @param {number} min_y
 * @param {number} max_x
 * @param {number} max_y
 * @param {number} depth
 * @param {string} tool_json
 * @param {WasmCamSettings} settings
 * @returns {string}
 */
export function camGenerateFace(min_x, min_y, max_x, max_y, depth, tool_json, settings) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passStringToWasm0(tool_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        _assertClass(settings, WasmCamSettings);
        const ret = wasm.camGenerateFace(min_x, min_y, max_x, max_y, depth, ptr0, len0, settings.__wbg_ptr);
        var ptr2 = ret[0];
        var len2 = ret[1];
        if (ret[3]) {
            ptr2 = 0; len2 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred3_0 = ptr2;
        deferred3_1 = len2;
        return getStringFromWasm0(ptr2, len2);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

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
 * @param {number} x
 * @param {number} y
 * @param {number} width
 * @param {number} height
 * @param {number} depth
 * @param {string} tool_json
 * @param {WasmCamSettings} settings
 * @returns {string}
 */
export function camGeneratePocket(x, y, width, height, depth, tool_json, settings) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passStringToWasm0(tool_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        _assertClass(settings, WasmCamSettings);
        const ret = wasm.camGeneratePocket(x, y, width, height, depth, ptr0, len0, settings.__wbg_ptr);
        var ptr2 = ret[0];
        var len2 = ret[1];
        if (ret[3]) {
            ptr2 = 0; len2 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred3_0 = ptr2;
        deferred3_1 = len2;
        return getStringFromWasm0(ptr2, len2);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

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
 * @param {string} height_field_json
 * @param {string} tool_json
 * @param {WasmCamSettings} settings
 * @param {number} target_z
 * @param {number} top_z
 * @param {number} stock_margin
 * @param {number} direction
 * @returns {string}
 */
export function camGenerateRoughing3d(height_field_json, tool_json, settings, target_z, top_z, stock_margin, direction) {
    let deferred4_0;
    let deferred4_1;
    try {
        const ptr0 = passStringToWasm0(height_field_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(tool_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        _assertClass(settings, WasmCamSettings);
        const ret = wasm.camGenerateRoughing3d(ptr0, len0, ptr1, len1, settings.__wbg_ptr, target_z, top_z, stock_margin, direction);
        var ptr3 = ret[0];
        var len3 = ret[1];
        if (ret[3]) {
            ptr3 = 0; len3 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred4_0 = ptr3;
        deferred4_1 = len3;
        return getStringFromWasm0(ptr3, len3);
    } finally {
        wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
    }
}

/**
 * Get default tool library.
 *
 * # Returns
 * Tool library as JSON array.
 * @returns {string}
 */
export function camGetDefaultTools() {
    let deferred2_0;
    let deferred2_1;
    try {
        const ret = wasm.camGetDefaultTools();
        var ptr1 = ret[0];
        var len1 = ret[1];
        if (ret[3]) {
            ptr1 = 0; len1 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred2_0 = ptr1;
        deferred2_1 = len1;
        return getStringFromWasm0(ptr1, len1);
    } finally {
        wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
    }
}

/**
 * Get toolpath statistics.
 *
 * # Arguments
 * * `toolpath_json` - Toolpath as JSON string
 *
 * # Returns
 * JSON object with statistics: { cutting_length, estimated_time, bounding_box }
 * @param {string} toolpath_json
 * @returns {any}
 */
export function camToolpathStats(toolpath_json) {
    const ptr0 = passStringToWasm0(toolpath_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.camToolpathStats(ptr0, len0);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Check a solid for DFM (Design for Manufacturing) printability issues.
 *
 * Returns warnings with face indices for viewport highlighting.
 * @param {Solid} solid
 * @param {string} printer_profile
 * @returns {any}
 */
export function checkPrintability(solid, printer_profile) {
    _assertClass(solid, Solid);
    const ptr0 = passStringToWasm0(printer_profile, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.checkPrintability(solid.__wbg_ptr, ptr0, len0);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

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
 * @param {Float32Array} positions
 * @param {Uint32Array} indices
 * @param {number} crease_angle
 * @returns {Promise<Float32Array>}
 */
export function computeCreasedNormalsGpu(positions, indices, crease_angle) {
    const ptr0 = passArrayF32ToWasm0(positions, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passArray32ToWasm0(indices, wasm.__wbindgen_malloc);
    const len1 = WASM_VECTOR_LEN;
    const ret = wasm.computeCreasedNormalsGpu(ptr0, len0, ptr1, len1, crease_angle);
    return ret;
}

/**
 * Compute volume of a closed triangle mesh using the divergence theorem.
 *
 * Positions are `[x, y, z, ...]` (flat f32), indices are `[i0, i1, i2, ...]`.
 * Returns volume in mm³ (same units as positions).
 * @param {Float32Array} positions
 * @param {Uint32Array} indices
 * @returns {number}
 */
export function computeMeshVolume(positions, indices) {
    const ptr0 = passArrayF32ToWasm0(positions, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passArray32ToWasm0(indices, wasm.__wbindgen_malloc);
    const len1 = WASM_VECTOR_LEN;
    const ret = wasm.computeMeshVolume(ptr0, len0, ptr1, len1);
    return ret;
}

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
 * @param {string} parent_json
 * @param {number} center_x
 * @param {number} center_y
 * @param {number} scale
 * @param {number} width
 * @param {number} height
 * @param {string} label
 * @returns {any}
 */
export function createDetailView(parent_json, center_x, center_y, scale, width, height, label) {
    const ptr0 = passStringToWasm0(parent_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passStringToWasm0(label, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len1 = WASM_VECTOR_LEN;
    const ret = wasm.createDetailView(ptr0, len0, center_x, center_y, scale, width, height, ptr1, len1);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

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
 * @param {Float32Array} positions
 * @param {Uint32Array} indices
 * @param {number} target_ratio
 * @returns {Promise<any>}
 */
export function decimateMeshGpu(positions, indices, target_ratio) {
    const ptr0 = passArrayF32ToWasm0(positions, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passArray32ToWasm0(indices, wasm.__wbindgen_malloc);
    const len1 = WASM_VECTOR_LEN;
    const ret = wasm.decimateMeshGpu(ptr0, len0, ptr1, len1, target_ratio);
    return ret;
}

/**
 * Derive parts from a Document (as JSON).
 *
 * Returns a JSON-serialized Vec<PartInfo>.
 * @param {string} doc_json
 * @returns {any}
 */
export function deriveParts(doc_json) {
    const ptr0 = passStringToWasm0(doc_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.deriveParts(ptr0, len0);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Digitize sketch segments into embroidery stitches.
 *
 * Takes a JSON array of `SketchSegment2D` (from a Sketch2D node) plus
 * stitch options, and returns an `EmbPattern` JSON string.
 * @param {string} segments_json
 * @param {string} options_json
 * @returns {string}
 */
export function digitizeSketch(segments_json, options_json) {
    let deferred4_0;
    let deferred4_1;
    try {
        const ptr0 = passStringToWasm0(segments_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(options_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.digitizeSketch(ptr0, len0, ptr1, len1);
        var ptr3 = ret[0];
        var len3 = ret[1];
        if (ret[3]) {
            ptr3 = 0; len3 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred4_0 = ptr3;
        deferred4_1 = len3;
        return getStringFromWasm0(ptr3, len3);
    } finally {
        wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
    }
}

/**
 * Digitize text into embroidery stitches.
 *
 * Converts a text string into glyph outlines, then applies the specified
 * stitch algorithm (running, satin, or fill) to produce an `EmbPattern`.
 * Returns the same JSON shape as `readEmbroideryPes`.
 * @param {string} text
 * @param {number} height
 * @param {string} options_json
 * @returns {string}
 */
export function digitizeText(text, height, options_json) {
    let deferred4_0;
    let deferred4_1;
    try {
        const ptr0 = passStringToWasm0(text, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(options_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.digitizeText(ptr0, len0, height, ptr1, len1);
        var ptr3 = ret[0];
        var len3 = ret[1];
        if (ret[3]) {
            ptr3 = 0; len3 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred4_0 = ptr3;
        deferred4_1 = len3;
        return getStringFromWasm0(ptr3, len3);
    } finally {
        wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
    }
}

/**
 * Convert a Document (as JSON) back to loon source code.
 * @param {string} doc_json
 * @returns {string}
 */
export function documentToLoon(doc_json) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passStringToWasm0(doc_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.documentToLoon(ptr0, len0);
        var ptr2 = ret[0];
        var len2 = ret[1];
        if (ret[3]) {
            ptr2 = 0; len2 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred3_0 = ptr2;
        deferred3_1 = len2;
        return getStringFromWasm0(ptr2, len2);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

/**
 * Return all builtin symbol definitions.
 *
 * # Returns
 * Array of `SymbolDef` as JsValue.
 * @returns {any}
 */
export function ecadBuiltinSymbols() {
    const ret = wasm.ecadBuiltinSymbols();
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Run Design Rule Check on a PCB layout.
 *
 * # Arguments
 * * `pcb_json` - JSON-serialized `Pcb` struct
 *
 * # Returns
 * Array of DRC violations as JsValue.
 * @param {string} pcb_json
 * @returns {any}
 */
export function ecadCheckDrc(pcb_json) {
    const ptr0 = passStringToWasm0(pcb_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.ecadCheckDrc(ptr0, len0);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Run Electrical Rule Check on a schematic sheet.
 *
 * # Arguments
 * * `sch_json` - JSON-serialized `SchematicSheet` struct
 *
 * # Returns
 * Array of ERC violations as JsValue.
 * @param {string} sch_json
 * @returns {any}
 */
export function ecadCheckErc(sch_json) {
    const ptr0 = passStringToWasm0(sch_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.ecadCheckErc(ptr0, len0);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Generate 3D component body meshes for all footprints on a PCB.
 *
 * # Arguments
 * * `pcb_json` - JSON-serialized `Pcb` struct
 *
 * # Returns
 * Array of component meshes as JsValue.
 * @param {string} pcb_json
 * @returns {any}
 */
export function ecadComponentMeshes(pcb_json) {
    const ptr0 = passStringToWasm0(pcb_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.ecadComponentMeshes(ptr0, len0);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Compute ratsnest lines for unrouted net connections.
 *
 * # Arguments
 * * `pcb_json` - JSON-serialized `Pcb` struct
 * * `netlist_json` - JSON-serialized netlist
 *
 * # Returns
 * Array of ratsnest lines as JsValue.
 * @param {string} pcb_json
 * @param {string} netlist_json
 * @returns {any}
 */
export function ecadComputeRatsnest(pcb_json, netlist_json) {
    const ptr0 = passStringToWasm0(pcb_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passStringToWasm0(netlist_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len1 = WASM_VECTOR_LEN;
    const ret = wasm.ecadComputeRatsnest(ptr0, len0, ptr1, len1);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Fill copper pour zones on the PCB.
 *
 * # Arguments
 * * `pcb_json` - JSON-serialized `Pcb` struct
 *
 * # Returns
 * Array of filled zone polygons.
 * @param {string} pcb_json
 * @returns {any}
 */
export function ecadFillZones(pcb_json) {
    const ptr0 = passStringToWasm0(pcb_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.ecadFillZones(ptr0, len0);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Generate a netlist from a schematic sheet.
 *
 * # Arguments
 * * `sch_json` - JSON-serialized `SchematicSheet` struct
 *
 * # Returns
 * Netlist as JsValue.
 * @param {string} sch_json
 * @returns {any}
 */
export function ecadGenerateNetlist(sch_json) {
    const ptr0 = passStringToWasm0(sch_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.ecadGenerateNetlist(ptr0, len0);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Look up a single builtin symbol by ID.
 *
 * # Arguments
 * * `id` - Symbol identifier (e.g. "resistor", "capacitor", "npn")
 *
 * # Returns
 * `SymbolDef` as JsValue, or null if not found.
 * @param {string} id
 * @returns {any}
 */
export function ecadGetSymbol(id) {
    const ptr0 = passStringToWasm0(id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.ecadGetSymbol(ptr0, len0);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Compute Z offset for a PCB layer.
 *
 * # Arguments
 * * `layer` - Layer name (e.g. "FCu", "BCu")
 * * `thickness` - Board thickness in mm
 * * `explosion` - Explosion factor (0 = normal, >0 = exploded)
 * @param {string} layer
 * @param {number} thickness
 * @param {number} explosion
 * @returns {number}
 */
export function ecadLayerZ(layer, thickness, explosion) {
    const ptr0 = passStringToWasm0(layer, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.ecadLayerZ(ptr0, len0, thickness, explosion);
    return ret;
}

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
 * @param {string} wire_json
 * @param {string} netlist_json
 * @param {string} components_json
 * @returns {any}
 */
export function ecadNetForWire(wire_json, netlist_json, components_json) {
    const ptr0 = passStringToWasm0(wire_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passStringToWasm0(netlist_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len1 = WASM_VECTOR_LEN;
    const ptr2 = passStringToWasm0(components_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len2 = WASM_VECTOR_LEN;
    const ret = wasm.ecadNetForWire(ptr0, len0, ptr1, len1, ptr2, len2);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

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
 * @param {string} pcb_json
 * @param {string} net
 * @param {number} start_x
 * @param {number} start_y
 * @param {number} end_x
 * @param {number} end_y
 * @param {number} width
 * @returns {any}
 */
export function ecadRouteNet(pcb_json, net, start_x, start_y, end_x, end_y, width) {
    const ptr0 = passStringToWasm0(pcb_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passStringToWasm0(net, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len1 = WASM_VECTOR_LEN;
    const ret = wasm.ecadRouteNet(ptr0, len0, ptr1, len1, start_x, start_y, end_x, end_y, width);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

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
 * @param {number} x
 * @param {number} y
 * @param {string} components_json
 * @param {number} grid
 * @param {number} threshold
 * @returns {any}
 */
export function ecadSnapToGridOrPin(x, y, components_json, grid, threshold) {
    const ptr0 = passStringToWasm0(components_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.ecadSnapToGridOrPin(x, y, ptr0, len0, grid, threshold);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Estimate print cost from volume (instant, pre-slice).
 * @param {number} volume_mm3
 * @param {number} infill_density
 * @param {number} wall_count
 * @param {number} line_width
 * @param {string} material_name
 * @returns {any}
 */
export function estimatePrintCost(volume_mm3, infill_density, wall_count, line_width, material_name) {
    const ptr0 = passStringToWasm0(material_name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.estimatePrintCost(volume_mm3, infill_density, wall_count, line_width, ptr0, len0);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Evaluate a loon source string and return a JSON-serialized vcad Document.
 *
 * The vcad library (types, constructors) is automatically prepended.
 * Module resolution (`[use ...]`) is not available in WASM — all code
 * must be self-contained or use the bundled vcad library.
 * @param {string} source
 * @returns {any}
 */
export function evalVcadSource(source) {
    const ptr0 = passStringToWasm0(source, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.evalVcadSource(ptr0, len0);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

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
 * @param {string} doc_json
 * @param {boolean} skip_clash_detection
 * @returns {any}
 */
export function evaluateDocument(doc_json, skip_clash_detection) {
    const ptr0 = passStringToWasm0(doc_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.evaluateDocument(ptr0, len0, skip_clash_detection);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

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
 * @param {string} vcode
 * @returns {Solid}
 */
export function evaluateVCode(vcode) {
    const ptr0 = passStringToWasm0(vcode, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.evaluateVCode(ptr0, len0);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return Solid.__wrap(ret[0]);
}

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
 * @param {string} view_json
 * @returns {Uint8Array}
 */
export function exportProjectedViewToDxf(view_json) {
    const ptr0 = passStringToWasm0(view_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.exportProjectedViewToDxf(ptr0, len0);
    if (ret[3]) {
        throw takeFromExternrefTable0(ret[2]);
    }
    var v2 = getArrayU8FromWasm0(ret[0], ret[1]).slice();
    wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
    return v2;
}

/**
 * Generate a 3MF file from mesh data.
 *
 * Returns the 3MF file as a byte array suitable for download or upload to a printer.
 * @param {string} name
 * @param {Float32Array} vertices
 * @param {Uint32Array} indices
 * @param {string} settings_json
 * @returns {Uint8Array}
 */
export function generate3mf(name, vertices, indices, settings_json) {
    const ptr0 = passStringToWasm0(name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passArrayF32ToWasm0(vertices, wasm.__wbindgen_malloc);
    const len1 = WASM_VECTOR_LEN;
    const ptr2 = passArray32ToWasm0(indices, wasm.__wbindgen_malloc);
    const len2 = WASM_VECTOR_LEN;
    const ptr3 = passStringToWasm0(settings_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len3 = WASM_VECTOR_LEN;
    const ret = wasm.generate3mf(ptr0, len0, ptr1, len1, ptr2, len2, ptr3, len3);
    if (ret[3]) {
        throw takeFromExternrefTable0(ret[2]);
    }
    var v5 = getArrayU8FromWasm0(ret[0], ret[1]).slice();
    wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
    return v5;
}

/**
 * Generate G-code from slice result.
 * @param {SliceResult} result
 * @param {string} printer_profile
 * @param {number} print_temp
 * @param {number} bed_temp
 * @returns {string}
 */
export function generateGcode(result, printer_profile, print_temp, bed_temp) {
    let deferred3_0;
    let deferred3_1;
    try {
        _assertClass(result, SliceResult);
        const ptr0 = passStringToWasm0(printer_profile, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.generateGcode(result.__wbg_ptr, ptr0, len0, print_temp, bed_temp);
        var ptr2 = ret[0];
        var len2 = ret[1];
        if (ret[3]) {
            ptr2 = 0; len2 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred3_0 = ptr2;
        deferred3_1 = len2;
        return getStringFromWasm0(ptr2, len2);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

/**
 * Get available printer profiles.
 * @returns {any}
 */
export function getSlicerPrinterProfiles() {
    const ret = wasm.getSlicerPrinterProfiles();
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Get the five Anthropic CRUD tool definitions
 * (`create` / `read` / `update` / `delete` / `set_material`) as a JSON
 * array, with the `create` tool's `type` enum pre-populated from the
 * kernel's tool schema list. Consumers on the web (TypeScript
 * `CommandRegistry.toAnthropicTools`) and in the TUI (`vcad_chat::
 * anthropic_tools`) render byte-identical payloads — single source of
 * truth lives in `vcad-chat::tools`.
 * @returns {string}
 */
export function get_anthropic_tools_json() {
    let deferred1_0;
    let deferred1_1;
    try {
        const ret = wasm.get_anthropic_tools_json();
        deferred1_0 = ret[0];
        deferred1_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
    }
}

/**
 * Get the kernel version string.
 * Use this in browser console to verify the correct WASM build is loaded:
 * `kernelWasm.get_kernel_version()` should return "2025-02-21-step-facebound-fix"
 * @returns {string}
 */
export function get_kernel_version() {
    let deferred1_0;
    let deferred1_1;
    try {
        const ret = wasm.get_kernel_version();
        deferred1_0 = ret[0];
        deferred1_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
    }
}

/**
 * Get tool schema definitions for all CsgOp variants.
 * Returns JSON array of ToolSchemaEntry objects.
 * @returns {string}
 */
export function get_tool_schemas() {
    let deferred1_0;
    let deferred1_1;
    try {
        const ret = wasm.get_tool_schemas();
        deferred1_0 = ret[0];
        deferred1_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
    }
}

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
 * @param {Uint8Array} data
 * @returns {any}
 */
export function importStepBuffer(data) {
    const ptr0 = passArray8ToWasm0(data, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.importStepBuffer(ptr0, len0);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Initialize the WASM module (sets up panic hook for better error messages).
 */
export function init() {
    wasm.init();
}

/**
 * Initialize the GPU context for accelerated geometry processing.
 *
 * Returns `true` if WebGPU is available and initialized, `false` otherwise.
 * This should be called once at application startup.
 * @returns {Promise<boolean>}
 */
export function initGpu() {
    const ret = wasm.initGpu();
    return ret;
}

/**
 * Check if CAM is available.
 * @returns {boolean}
 */
export function isCamAvailable() {
    const ret = wasm.isCamAvailable();
    return ret !== 0;
}

/**
 * Check if ECAD features are available in this build.
 * @returns {boolean}
 */
export function isEcadAvailable() {
    const ret = wasm.isEcadAvailable();
    return ret !== 0;
}

/**
 * Check if embroidery support is available.
 * @returns {boolean}
 */
export function isEmbroideryAvailable() {
    const ret = wasm.isCamAvailable();
    return ret !== 0;
}

/**
 * Check if GPU processing is available.
 * @returns {boolean}
 */
export function isGpuAvailable() {
    const ret = wasm.isGpuAvailable();
    return ret !== 0;
}

/**
 * Check if physics simulation is available.
 * @returns {boolean}
 */
export function isPhysicsAvailable() {
    const ret = wasm.isPhysicsAvailable();
    return ret !== 0;
}

/**
 * Check if slicer is available.
 * @returns {boolean}
 */
export function isSlicerAvailable() {
    const ret = wasm.isEcadAvailable();
    return ret !== 0;
}

/**
 * Chamfer all edges of a solid by the given distance.
 *
 * This is a standalone wrapper for lazy loading via wasmosis.
 * @param {Solid} solid
 * @param {number} distance
 * @returns {Solid}
 */
export function op_chamfer(solid, distance) {
    _assertClass(solid, Solid);
    const ret = wasm.op_chamfer(solid.__wbg_ptr, distance);
    return Solid.__wrap(ret);
}

/**
 * Create a circular pattern of a solid around an axis.
 *
 * This is a standalone wrapper for lazy loading via wasmosis.
 * @param {Solid} solid
 * @param {number} axis_origin_x
 * @param {number} axis_origin_y
 * @param {number} axis_origin_z
 * @param {number} axis_dir_x
 * @param {number} axis_dir_y
 * @param {number} axis_dir_z
 * @param {number} count
 * @param {number} angle_deg
 * @returns {Solid}
 */
export function op_circular_pattern(solid, axis_origin_x, axis_origin_y, axis_origin_z, axis_dir_x, axis_dir_y, axis_dir_z, count, angle_deg) {
    _assertClass(solid, Solid);
    const ret = wasm.op_circular_pattern(solid.__wbg_ptr, axis_origin_x, axis_origin_y, axis_origin_z, axis_dir_x, axis_dir_y, axis_dir_z, count, angle_deg);
    return Solid.__wrap(ret);
}

/**
 * Fillet all edges of a solid with the given radius.
 *
 * This is a standalone wrapper for lazy loading via wasmosis.
 * @param {Solid} solid
 * @param {number} radius
 * @returns {Solid}
 */
export function op_fillet(solid, radius) {
    _assertClass(solid, Solid);
    const ret = wasm.op_fillet(solid.__wbg_ptr, radius);
    return Solid.__wrap(ret);
}

/**
 * Create a linear pattern of a solid along a direction.
 *
 * This is a standalone wrapper for lazy loading via wasmosis.
 * @param {Solid} solid
 * @param {number} dir_x
 * @param {number} dir_y
 * @param {number} dir_z
 * @param {number} count
 * @param {number} spacing
 * @returns {Solid}
 */
export function op_linear_pattern(solid, dir_x, dir_y, dir_z, count, spacing) {
    _assertClass(solid, Solid);
    const ret = wasm.op_linear_pattern(solid.__wbg_ptr, dir_x, dir_y, dir_z, count, spacing);
    return Solid.__wrap(ret);
}

/**
 * Create a solid by lofting between multiple profiles.
 *
 * This is a standalone wrapper for lazy loading via wasmosis.
 * @param {string} profiles_json
 * @param {boolean | null} [closed]
 * @returns {Solid}
 */
export function op_loft(profiles_json, closed) {
    const ptr0 = passStringToWasm0(profiles_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.op_loft(ptr0, len0, isLikeNone(closed) ? 0xFFFFFF : closed ? 1 : 0);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return Solid.__wrap(ret[0]);
}

/**
 * Create a solid by revolving a 2D sketch profile around an axis.
 *
 * This is a standalone wrapper for lazy loading via wasmosis.
 * @param {string} profile_json
 * @param {Float64Array} axis_origin
 * @param {Float64Array} axis_dir
 * @param {number} angle_deg
 * @returns {Solid}
 */
export function op_revolve(profile_json, axis_origin, axis_dir, angle_deg) {
    const ptr0 = passStringToWasm0(profile_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passArrayF64ToWasm0(axis_origin, wasm.__wbindgen_malloc);
    const len1 = WASM_VECTOR_LEN;
    const ptr2 = passArrayF64ToWasm0(axis_dir, wasm.__wbindgen_malloc);
    const len2 = WASM_VECTOR_LEN;
    const ret = wasm.op_revolve(ptr0, len0, ptr1, len1, ptr2, len2, angle_deg);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return Solid.__wrap(ret[0]);
}

/**
 * Shell (hollow) a solid by offsetting all faces inward.
 *
 * This is a standalone wrapper for lazy loading via wasmosis.
 * @param {Solid} solid
 * @param {number} thickness
 * @returns {Solid}
 */
export function op_shell(solid, thickness) {
    _assertClass(solid, Solid);
    const ret = wasm.op_shell(solid.__wbg_ptr, thickness);
    return Solid.__wrap(ret);
}

/**
 * Create a solid by sweeping a profile along a helix path.
 *
 * This is a standalone wrapper for lazy loading via wasmosis.
 * @param {string} profile_json
 * @param {number} radius
 * @param {number} pitch
 * @param {number} height
 * @param {number} turns
 * @param {number | null} [twist_angle]
 * @param {number | null} [scale_start]
 * @param {number | null} [scale_end]
 * @param {number | null} [path_segments]
 * @param {number | null} [arc_segments]
 * @param {number | null} [orientation]
 * @returns {Solid}
 */
export function op_sweep_helix(profile_json, radius, pitch, height, turns, twist_angle, scale_start, scale_end, path_segments, arc_segments, orientation) {
    const ptr0 = passStringToWasm0(profile_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.op_sweep_helix(ptr0, len0, radius, pitch, height, turns, !isLikeNone(twist_angle), isLikeNone(twist_angle) ? 0 : twist_angle, !isLikeNone(scale_start), isLikeNone(scale_start) ? 0 : scale_start, !isLikeNone(scale_end), isLikeNone(scale_end) ? 0 : scale_end, isLikeNone(path_segments) ? 0x100000001 : (path_segments) >>> 0, isLikeNone(arc_segments) ? 0x100000001 : (arc_segments) >>> 0, !isLikeNone(orientation), isLikeNone(orientation) ? 0 : orientation);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return Solid.__wrap(ret[0]);
}

/**
 * Create a solid by sweeping a profile along a line path.
 *
 * This is a standalone wrapper for lazy loading via wasmosis.
 * @param {string} profile_json
 * @param {Float64Array} start
 * @param {Float64Array} end
 * @param {number | null} [twist_angle]
 * @param {number | null} [scale_start]
 * @param {number | null} [scale_end]
 * @param {number | null} [orientation]
 * @returns {Solid}
 */
export function op_sweep_line(profile_json, start, end, twist_angle, scale_start, scale_end, orientation) {
    const ptr0 = passStringToWasm0(profile_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passArrayF64ToWasm0(start, wasm.__wbindgen_malloc);
    const len1 = WASM_VECTOR_LEN;
    const ptr2 = passArrayF64ToWasm0(end, wasm.__wbindgen_malloc);
    const len2 = WASM_VECTOR_LEN;
    const ret = wasm.op_sweep_line(ptr0, len0, ptr1, len1, ptr2, len2, !isLikeNone(twist_angle), isLikeNone(twist_angle) ? 0 : twist_angle, !isLikeNone(scale_start), isLikeNone(scale_start) ? 0 : scale_start, !isLikeNone(scale_end), isLikeNone(scale_end) ? 0 : scale_end, !isLikeNone(orientation), isLikeNone(orientation) ? 0 : orientation);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return Solid.__wrap(ret[0]);
}

/**
 * Parse a KiCad `.kicad_pcb` file content into a JSON-serialized `Pcb`.
 *
 * # Arguments
 * * `content` - The `.kicad_pcb` file content as a string
 *
 * # Returns
 * JSON-serialized `Pcb` struct as JsValue, or error.
 * @param {string} content
 * @returns {any}
 */
export function parseKicadPcb(content) {
    const ptr0 = passStringToWasm0(content, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.parseKicadPcb(ptr0, len0);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

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
 * @param {string} vcode
 * @returns {string}
 */
export function parseVCode(vcode) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passStringToWasm0(vcode, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.parseVCode(ptr0, len0);
        var ptr2 = ret[0];
        var len2 = ret[1];
        if (ret[3]) {
            ptr2 = 0; len2 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred3_0 = ptr2;
        deferred3_1 = len2;
        return getStringFromWasm0(ptr2, len2);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

/**
 * Parse a .vcad file (JSON v0.1, VCode v0.2, or loon v0.3).
 *
 * Returns a JSON-serialized VcadFile with document, parts, and metadata.
 * @param {string} content
 * @returns {any}
 */
export function parseVcadFile(content) {
    const ptr0 = passStringToWasm0(content, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.parseVcadFile(ptr0, len0);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * @param {string} tool
 * @param {string} args_json
 * @param {string} doc_json
 * @returns {string}
 */
export function plan_chat_tool(tool, args_json, doc_json) {
    let deferred4_0;
    let deferred4_1;
    try {
        const ptr0 = passStringToWasm0(tool, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(args_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passStringToWasm0(doc_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len2 = WASM_VECTOR_LEN;
        const ret = wasm.plan_chat_tool(ptr0, len0, ptr1, len1, ptr2, len2);
        deferred4_0 = ret[0];
        deferred4_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
    }
}

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
 * @param {Float32Array} positions
 * @param {Uint32Array} indices
 * @param {number} crease_angle
 * @param {boolean} generate_lod
 * @returns {Promise<any>}
 */
export function processGeometryGpu(positions, indices, crease_angle, generate_lod) {
    const ptr0 = passArrayF32ToWasm0(positions, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passArray32ToWasm0(indices, wasm.__wbindgen_malloc);
    const len1 = WASM_VECTOR_LEN;
    const ret = wasm.processGeometryGpu(ptr0, len0, ptr1, len1, crease_angle, generate_lod);
    return ret;
}

/**
 * Project a triangle mesh to a 2D view.
 *
 * # Arguments
 * * `mesh_js` - Mesh data as JS object with `positions` (Float32Array) and `indices` (Uint32Array)
 * * `view_direction` - View direction: "front", "back", "top", "bottom", "left", "right", or "isometric"
 *
 * # Returns
 * A JS object containing the projected view with edges and bounds.
 * @param {any} mesh_js
 * @param {string} view_direction
 * @returns {any}
 */
export function projectMesh(mesh_js, view_direction) {
    const ptr0 = passStringToWasm0(view_direction, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.projectMesh(mesh_js, ptr0, len0);
    return ret;
}

/**
 * Read a DST file and return embroidery data as JSON.
 * @param {Uint8Array} data
 * @returns {string}
 */
export function readEmbroideryDst(data) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passArray8ToWasm0(data, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.readEmbroideryDst(ptr0, len0);
        var ptr2 = ret[0];
        var len2 = ret[1];
        if (ret[3]) {
            ptr2 = 0; len2 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred3_0 = ptr2;
        deferred3_1 = len2;
        return getStringFromWasm0(ptr2, len2);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

/**
 * Read a PES file and return embroidery data as JSON.
 *
 * Returns `{ threads, stitchPaths, stats }` as a JSON string.
 * @param {Uint8Array} data
 * @returns {string}
 */
export function readEmbroideryPes(data) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passArray8ToWasm0(data, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.readEmbroideryPes(ptr0, len0);
        var ptr2 = ret[0];
        var len2 = ret[1];
        if (ret[3]) {
            ptr2 = 0; len2 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred3_0 = ptr2;
        deferred3_1 = len2;
        return getStringFromWasm0(ptr2, len2);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

/**
 * Recommend smart print settings from analysis results.
 *
 * Takes a PrintAnalysis JSON and printer profile name,
 * returns recommended SliceSettings + explanations.
 * @param {string} analysis_json
 * @param {string} printer_profile
 * @returns {any}
 */
export function recommendPrintSettings(analysis_json, printer_profile) {
    const ptr0 = passStringToWasm0(analysis_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passStringToWasm0(printer_profile, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len1 = WASM_VECTOR_LEN;
    const ret = wasm.recommendPrintSettings(ptr0, len0, ptr1, len1);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

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
 * @param {any} mesh_js
 * @param {string} plane_json
 * @param {string | null} [hatch_json]
 * @returns {any}
 */
export function sectionMesh(mesh_js, plane_json, hatch_json) {
    const ptr0 = passStringToWasm0(plane_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    var ptr1 = isLikeNone(hatch_json) ? 0 : passStringToWasm0(hatch_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    var len1 = WASM_VECTOR_LEN;
    const ret = wasm.sectionMesh(mesh_js, ptr0, len0, ptr1, len1);
    return ret;
}

/**
 * Build an N-sided polygonal approximation of a circle as arc segments.
 * Returns a JSON array of `SketchSegment2D`.
 * @param {number} cx
 * @param {number} cy
 * @param {number} radius
 * @param {number} segments
 * @returns {string}
 */
export function sketchCircleSegments(cx, cy, radius, segments) {
    let deferred2_0;
    let deferred2_1;
    try {
        const ret = wasm.sketchCircleSegments(cx, cy, radius, segments);
        var ptr1 = ret[0];
        var len1 = ret[1];
        if (ret[3]) {
            ptr1 = 0; len1 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred2_0 = ptr1;
        deferred2_1 = len1;
        return getStringFromWasm0(ptr1, len1);
    } finally {
        wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
    }
}

/**
 * Find the segment-index closest to `(x, y)` within `tolerance`. Returns
 * `-1` if no segment is within reach.
 * @param {string} segments_json
 * @param {number} x
 * @param {number} y
 * @param {number} tolerance
 * @returns {number}
 */
export function sketchHitTest(segments_json, x, y, tolerance) {
    const ptr0 = passStringToWasm0(segments_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.sketchHitTest(ptr0, len0, x, y, tolerance);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return ret[0];
}

/**
 * Return a plane's `{origin, xDir, yDir, normal}` as JSON. Accepts either a
 * named plane string or a custom-plane object (same shape as
 * [`WasmSketchSession`]'s constructor argument).
 * @param {string} plane_json
 * @returns {string}
 */
export function sketchPlaneBasis(plane_json) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passStringToWasm0(plane_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.sketchPlaneBasis(ptr0, len0);
        var ptr2 = ret[0];
        var len2 = ret[1];
        if (ret[3]) {
            ptr2 = 0; len2 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred3_0 = ptr2;
        deferred3_1 = len2;
        return getStringFromWasm0(ptr2, len2);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

/**
 * Intersect a world-space ray with a plane and return the hit in 2D
 * sketch coordinates as `[x, y]` JSON, or the literal string `"null"` when
 * the ray is parallel to the plane.
 * @param {string} plane_json
 * @param {number} ox
 * @param {number} oy
 * @param {number} oz
 * @param {number} dx
 * @param {number} dy
 * @param {number} dz
 * @returns {string}
 */
export function sketchPlaneIntersectRay(plane_json, ox, oy, oz, dx, dy, dz) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passStringToWasm0(plane_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.sketchPlaneIntersectRay(ptr0, len0, ox, oy, oz, dx, dy, dz);
        var ptr2 = ret[0];
        var len2 = ret[1];
        if (ret[3]) {
            ptr2 = 0; len2 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred3_0 = ptr2;
        deferred3_1 = len2;
        return getStringFromWasm0(ptr2, len2);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

/**
 * Build the four line segments of an axis-aligned rectangle between two
 * opposite corners. Returns a JSON array of `SketchSegment2D`.
 * @param {number} p1x
 * @param {number} p1y
 * @param {number} p2x
 * @param {number} p2y
 * @returns {string}
 */
export function sketchRectangleSegments(p1x, p1y, p2x, p2y) {
    let deferred2_0;
    let deferred2_1;
    try {
        const ret = wasm.sketchRectangleSegments(p1x, p1y, p2x, p2y);
        var ptr1 = ret[0];
        var len1 = ret[1];
        if (ret[3]) {
            ptr1 = 0; len1 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred2_0 = ptr1;
        deferred2_1 = len1;
        return getStringFromWasm0(ptr1, len1);
    } finally {
        wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
    }
}

/**
 * Snap a 2D point against a segment list with grid + vertex rules. Returns
 * `{x, y, snapTarget}` JSON — the snapped position plus (if a vertex snap
 * fired) the vertex that was matched.
 * @param {string} segments_json
 * @param {number} x
 * @param {number} y
 * @param {boolean} grid_enabled
 * @param {number} grid_size
 * @param {boolean} point_enabled
 * @param {number} point_tolerance
 * @returns {string}
 */
export function sketchSnap(segments_json, x, y, grid_enabled, grid_size, point_enabled, point_tolerance) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passStringToWasm0(segments_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.sketchSnap(ptr0, len0, x, y, grid_enabled, grid_size, point_enabled, point_tolerance);
        var ptr2 = ret[0];
        var len2 = ret[1];
        if (ret[3]) {
            ptr2 = 0; len2 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred3_0 = ptr2;
        deferred3_1 = len2;
        return getStringFromWasm0(ptr2, len2);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

/**
 * Convert 2D sketch coordinates to a 3D world-space point, returning
 * `[x, y, z]` JSON.
 * @param {string} plane_json
 * @param {number} sx
 * @param {number} sy
 * @returns {string}
 */
export function sketchToWorld(plane_json, sx, sy) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passStringToWasm0(plane_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.sketchToWorld(ptr0, len0, sx, sy);
        var ptr2 = ret[0];
        var len2 = ret[1];
        if (ret[3]) {
            ptr2 = 0; len2 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred3_0 = ptr2;
        deferred3_1 = len2;
        return getStringFromWasm0(ptr2, len2);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

/**
 * Project a 3D world-space point onto a plane, returning 2D sketch
 * coordinates as `[x, y]` JSON.
 * @param {string} plane_json
 * @param {number} wx
 * @param {number} wy
 * @param {number} wz
 * @returns {string}
 */
export function sketchWorldToSketch(plane_json, wx, wy, wz) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passStringToWasm0(plane_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.sketchWorldToSketch(ptr0, len0, wx, wy, wz);
        var ptr2 = ret[0];
        var len2 = ret[1];
        if (ret[3]) {
            ptr2 = 0; len2 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred3_0 = ptr2;
        deferred3_1 = len2;
        return getStringFromWasm0(ptr2, len2);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

/**
 * Slice a mesh from vertices and indices.
 * @param {Float32Array} vertices
 * @param {Uint32Array} indices
 * @param {SlicerSettings} settings
 * @returns {SliceResult}
 */
export function sliceMesh(vertices, indices, settings) {
    const ptr0 = passArrayF32ToWasm0(vertices, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passArray32ToWasm0(indices, wasm.__wbindgen_malloc);
    const len1 = WASM_VECTOR_LEN;
    _assertClass(settings, SlicerSettings);
    const ret = wasm.sliceMesh(ptr0, len0, ptr1, len1, settings.__wbg_ptr);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return SliceResult.__wrap(ret[0]);
}

/**
 * Slice a solid.
 * @param {Solid} solid
 * @param {SlicerSettings} settings
 * @param {number | null} [segments]
 * @returns {SliceResult}
 */
export function sliceSolid(solid, settings, segments) {
    _assertClass(solid, Solid);
    _assertClass(settings, SlicerSettings);
    const ret = wasm.sliceSolid(solid.__wbg_ptr, settings.__wbg_ptr, isLikeNone(segments) ? 0x100000001 : (segments) >>> 0);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return SliceResult.__wrap(ret[0]);
}

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
 * @param {string} doc_json
 * @returns {any}
 */
export function solveForwardKinematics(doc_json) {
    const ptr0 = passStringToWasm0(doc_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.solveForwardKinematics(ptr0, len0);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Solve a TS-shaped sketch in one call.
 *
 * Takes a JSON array of `SketchSegment2D` and a JSON array of
 * `SketchConstraint`, runs the Levenberg-Marquardt solver, and returns a
 * JSON object `{ segments, converged }` where `segments` is the solved
 * segment list in the same order as the input. Segments that don't belong
 * to the constraint system (e.g. circle-as-arcs that live purely for
 * rendering) pass through unchanged.
 * @param {string} segments_json
 * @param {string} constraints_json
 * @returns {string}
 */
export function solveSketchSegments(segments_json, constraints_json) {
    let deferred4_0;
    let deferred4_1;
    try {
        const ptr0 = passStringToWasm0(segments_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(constraints_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.solveSketchSegments(ptr0, len0, ptr1, len1);
        var ptr3 = ret[0];
        var len3 = ret[1];
        if (ret[3]) {
            ptr3 = 0; len3 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred4_0 = ptr3;
        deferred4_1 = len3;
        return getStringFromWasm0(ptr3, len3);
    } finally {
        wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
    }
}

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
 * @param {string} text
 * @param {number} height
 * @param {string | null} [font]
 * @param {number | null} [letter_spacing]
 * @param {number | null} [line_spacing]
 * @returns {any}
 */
export function textBounds(text, height, font, letter_spacing, line_spacing) {
    const ptr0 = passStringToWasm0(text, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    var ptr1 = isLikeNone(font) ? 0 : passStringToWasm0(font, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    var len1 = WASM_VECTOR_LEN;
    const ret = wasm.textBounds(ptr0, len0, height, ptr1, len1, !isLikeNone(letter_spacing), isLikeNone(letter_spacing) ? 0 : letter_spacing, !isLikeNone(line_spacing), isLikeNone(line_spacing) ? 0 : line_spacing);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

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
 * @param {string} doc_json
 * @returns {string}
 */
export function toVCode(doc_json) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passStringToWasm0(doc_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.toVCode(ptr0, len0);
        var ptr2 = ret[0];
        var len2 = ret[1];
        if (ret[3]) {
            ptr2 = 0; len2 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred3_0 = ptr2;
        deferred3_1 = len2;
        return getStringFromWasm0(ptr2, len2);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

/**
 * Write a DST file from an embroidery pattern JSON string.
 * @param {string} json
 * @returns {Uint8Array}
 */
export function writeEmbroideryDst(json) {
    const ptr0 = passStringToWasm0(json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.writeEmbroideryDst(ptr0, len0);
    if (ret[3]) {
        throw takeFromExternrefTable0(ret[2]);
    }
    var v2 = getArrayU8FromWasm0(ret[0], ret[1]).slice();
    wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
    return v2;
}

/**
 * Write a PES file from an embroidery pattern JSON string.
 * @param {string} json
 * @returns {Uint8Array}
 */
export function writeEmbroideryPes(json) {
    const ptr0 = passStringToWasm0(json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.writeEmbroideryPes(ptr0, len0);
    if (ret[3]) {
        throw takeFromExternrefTable0(ret[2]);
    }
    var v2 = getArrayU8FromWasm0(ret[0], ret[1]).slice();
    wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
    return v2;
}

function __wbg_get_imports() {
    const import0 = {
        __proto__: null,
        __wbg_Error_8c4e43fe74559d73: function(arg0, arg1) {
            const ret = Error(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_Number_04624de7d0e8332d: function(arg0) {
            const ret = Number(arg0);
            return ret;
        },
        __wbg_String_8f0eb39a4a4c2f66: function(arg0, arg1) {
            const ret = String(arg1);
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_Window_cf5b693340a7c469: function(arg0) {
            const ret = arg0.Window;
            return ret;
        },
        __wbg_WorkerGlobalScope_354364d1b0bd06e5: function(arg0) {
            const ret = arg0.WorkerGlobalScope;
            return ret;
        },
        __wbg___wbindgen_boolean_get_bbbb1c18aa2f5e25: function(arg0) {
            const v = arg0;
            const ret = typeof(v) === 'boolean' ? v : undefined;
            return isLikeNone(ret) ? 0xFFFFFF : ret ? 1 : 0;
        },
        __wbg___wbindgen_debug_string_0bc8482c6e3508ae: function(arg0, arg1) {
            const ret = debugString(arg1);
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg___wbindgen_in_47fa6863be6f2f25: function(arg0, arg1) {
            const ret = arg0 in arg1;
            return ret;
        },
        __wbg___wbindgen_is_function_0095a73b8b156f76: function(arg0) {
            const ret = typeof(arg0) === 'function';
            return ret;
        },
        __wbg___wbindgen_is_null_ac34f5003991759a: function(arg0) {
            const ret = arg0 === null;
            return ret;
        },
        __wbg___wbindgen_is_object_5ae8e5880f2c1fbd: function(arg0) {
            const val = arg0;
            const ret = typeof(val) === 'object' && val !== null;
            return ret;
        },
        __wbg___wbindgen_is_string_cd444516edc5b180: function(arg0) {
            const ret = typeof(arg0) === 'string';
            return ret;
        },
        __wbg___wbindgen_is_undefined_9e4d92534c42d778: function(arg0) {
            const ret = arg0 === undefined;
            return ret;
        },
        __wbg___wbindgen_jsval_loose_eq_9dd77d8cd6671811: function(arg0, arg1) {
            const ret = arg0 == arg1;
            return ret;
        },
        __wbg___wbindgen_number_get_8ff4255516ccad3e: function(arg0, arg1) {
            const obj = arg1;
            const ret = typeof(obj) === 'number' ? obj : undefined;
            getDataViewMemory0().setFloat64(arg0 + 8 * 1, isLikeNone(ret) ? 0 : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg___wbindgen_string_get_72fb696202c56729: function(arg0, arg1) {
            const obj = arg1;
            const ret = typeof(obj) === 'string' ? obj : undefined;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg___wbindgen_throw_be289d5034ed271b: function(arg0, arg1) {
            throw new Error(getStringFromWasm0(arg0, arg1));
        },
        __wbg__wbg_cb_unref_d9b87ff7982e3b21: function(arg0) {
            arg0._wbg_cb_unref();
        },
        __wbg_activeTexture_6f9a710514686c24: function(arg0, arg1) {
            arg0.activeTexture(arg1 >>> 0);
        },
        __wbg_activeTexture_7e39cb8fdf4b6d5a: function(arg0, arg1) {
            arg0.activeTexture(arg1 >>> 0);
        },
        __wbg_attachShader_32114efcf2744eb6: function(arg0, arg1, arg2) {
            arg0.attachShader(arg1, arg2);
        },
        __wbg_attachShader_b36058e5c9eeaf54: function(arg0, arg1, arg2) {
            arg0.attachShader(arg1, arg2);
        },
        __wbg_beginComputePass_90d5303e604970cb: function(arg0, arg1) {
            const ret = arg0.beginComputePass(arg1);
            return ret;
        },
        __wbg_beginQuery_0fdf154e1da0e73d: function(arg0, arg1, arg2) {
            arg0.beginQuery(arg1 >>> 0, arg2);
        },
        __wbg_beginRenderPass_9739520c601001c3: function(arg0, arg1) {
            const ret = arg0.beginRenderPass(arg1);
            return ret;
        },
        __wbg_bindAttribLocation_5cfc7fa688df5051: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.bindAttribLocation(arg1, arg2 >>> 0, getStringFromWasm0(arg3, arg4));
        },
        __wbg_bindAttribLocation_ce78bfb13019dbe6: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.bindAttribLocation(arg1, arg2 >>> 0, getStringFromWasm0(arg3, arg4));
        },
        __wbg_bindBufferRange_009d206fe9e4151e: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.bindBufferRange(arg1 >>> 0, arg2 >>> 0, arg3, arg4, arg5);
        },
        __wbg_bindBuffer_69a7a0b8f3f9b9cf: function(arg0, arg1, arg2) {
            arg0.bindBuffer(arg1 >>> 0, arg2);
        },
        __wbg_bindBuffer_c9068e8712a034f5: function(arg0, arg1, arg2) {
            arg0.bindBuffer(arg1 >>> 0, arg2);
        },
        __wbg_bindFramebuffer_031c73ba501cb8f6: function(arg0, arg1, arg2) {
            arg0.bindFramebuffer(arg1 >>> 0, arg2);
        },
        __wbg_bindFramebuffer_7815ca611abb057f: function(arg0, arg1, arg2) {
            arg0.bindFramebuffer(arg1 >>> 0, arg2);
        },
        __wbg_bindRenderbuffer_8a2aa4e3d1fb5443: function(arg0, arg1, arg2) {
            arg0.bindRenderbuffer(arg1 >>> 0, arg2);
        },
        __wbg_bindRenderbuffer_db37c1bac9ed4da0: function(arg0, arg1, arg2) {
            arg0.bindRenderbuffer(arg1 >>> 0, arg2);
        },
        __wbg_bindSampler_96f0e90e7bc31da9: function(arg0, arg1, arg2) {
            arg0.bindSampler(arg1 >>> 0, arg2);
        },
        __wbg_bindTexture_b2b7b1726a83f93e: function(arg0, arg1, arg2) {
            arg0.bindTexture(arg1 >>> 0, arg2);
        },
        __wbg_bindTexture_ec13ddcb9dc8e032: function(arg0, arg1, arg2) {
            arg0.bindTexture(arg1 >>> 0, arg2);
        },
        __wbg_bindVertexArrayOES_c2610602f7485b3f: function(arg0, arg1) {
            arg0.bindVertexArrayOES(arg1);
        },
        __wbg_bindVertexArray_78220d1edb1d2382: function(arg0, arg1) {
            arg0.bindVertexArray(arg1);
        },
        __wbg_blendColor_1d50ac87d9a2794b: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.blendColor(arg1, arg2, arg3, arg4);
        },
        __wbg_blendColor_e799d452ab2a5788: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.blendColor(arg1, arg2, arg3, arg4);
        },
        __wbg_blendEquationSeparate_1b12c43928cc7bc1: function(arg0, arg1, arg2) {
            arg0.blendEquationSeparate(arg1 >>> 0, arg2 >>> 0);
        },
        __wbg_blendEquationSeparate_a8094fbec94cf80e: function(arg0, arg1, arg2) {
            arg0.blendEquationSeparate(arg1 >>> 0, arg2 >>> 0);
        },
        __wbg_blendEquation_82202f34c4c00e50: function(arg0, arg1) {
            arg0.blendEquation(arg1 >>> 0);
        },
        __wbg_blendEquation_e9b99928ed1494ad: function(arg0, arg1) {
            arg0.blendEquation(arg1 >>> 0);
        },
        __wbg_blendFuncSeparate_95465944f788a092: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.blendFuncSeparate(arg1 >>> 0, arg2 >>> 0, arg3 >>> 0, arg4 >>> 0);
        },
        __wbg_blendFuncSeparate_f366c170c5097fbe: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.blendFuncSeparate(arg1 >>> 0, arg2 >>> 0, arg3 >>> 0, arg4 >>> 0);
        },
        __wbg_blendFunc_2ef59299d10c662d: function(arg0, arg1, arg2) {
            arg0.blendFunc(arg1 >>> 0, arg2 >>> 0);
        },
        __wbg_blendFunc_446658e7231ab9c8: function(arg0, arg1, arg2) {
            arg0.blendFunc(arg1 >>> 0, arg2 >>> 0);
        },
        __wbg_blitFramebuffer_d730a23ab4db248e: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10) {
            arg0.blitFramebuffer(arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9 >>> 0, arg10 >>> 0);
        },
        __wbg_bufferData_1be8450fab534758: function(arg0, arg1, arg2, arg3) {
            arg0.bufferData(arg1 >>> 0, arg2, arg3 >>> 0);
        },
        __wbg_bufferData_32d26eba0c74a53c: function(arg0, arg1, arg2, arg3) {
            arg0.bufferData(arg1 >>> 0, arg2, arg3 >>> 0);
        },
        __wbg_bufferData_52235e85894af988: function(arg0, arg1, arg2, arg3) {
            arg0.bufferData(arg1 >>> 0, arg2, arg3 >>> 0);
        },
        __wbg_bufferData_98f6c413a8f0f139: function(arg0, arg1, arg2, arg3) {
            arg0.bufferData(arg1 >>> 0, arg2, arg3 >>> 0);
        },
        __wbg_bufferSubData_33eebcc173094f6a: function(arg0, arg1, arg2, arg3) {
            arg0.bufferSubData(arg1 >>> 0, arg2, arg3);
        },
        __wbg_bufferSubData_3e902f031adf13fd: function(arg0, arg1, arg2, arg3) {
            arg0.bufferSubData(arg1 >>> 0, arg2, arg3);
        },
        __wbg_buffer_26d0910f3a5bc899: function(arg0) {
            const ret = arg0.buffer;
            return ret;
        },
        __wbg_call_389efe28435a9388: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.call(arg1);
            return ret;
        }, arguments); },
        __wbg_call_4708e0c13bdc8e95: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.call(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_clearBuffer_6164fc25d22b25cc: function(arg0, arg1, arg2, arg3) {
            arg0.clearBuffer(arg1, arg2, arg3);
        },
        __wbg_clearBuffer_cfcaaf1fb2baa885: function(arg0, arg1, arg2) {
            arg0.clearBuffer(arg1, arg2);
        },
        __wbg_clearBufferfv_ac87d92e2f45d80c: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.clearBufferfv(arg1 >>> 0, arg2, getArrayF32FromWasm0(arg3, arg4));
        },
        __wbg_clearBufferiv_69ff24bb52ec4c88: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.clearBufferiv(arg1 >>> 0, arg2, getArrayI32FromWasm0(arg3, arg4));
        },
        __wbg_clearBufferuiv_8ad59a8219aafaca: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.clearBufferuiv(arg1 >>> 0, arg2, getArrayU32FromWasm0(arg3, arg4));
        },
        __wbg_clearDepth_2b109f644a783a53: function(arg0, arg1) {
            arg0.clearDepth(arg1);
        },
        __wbg_clearDepth_670099db422a4f91: function(arg0, arg1) {
            arg0.clearDepth(arg1);
        },
        __wbg_clearStencil_5d243d0dff03c315: function(arg0, arg1) {
            arg0.clearStencil(arg1);
        },
        __wbg_clearStencil_aa65955bb39d8c18: function(arg0, arg1) {
            arg0.clearStencil(arg1);
        },
        __wbg_clear_4d801d0d054c3579: function(arg0, arg1) {
            arg0.clear(arg1 >>> 0);
        },
        __wbg_clear_7187030f892c5ca0: function(arg0, arg1) {
            arg0.clear(arg1 >>> 0);
        },
        __wbg_clientWaitSync_21865feaeb76a9a5: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.clientWaitSync(arg1, arg2 >>> 0, arg3 >>> 0);
            return ret;
        },
        __wbg_colorMask_177d9762658e5e28: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.colorMask(arg1 !== 0, arg2 !== 0, arg3 !== 0, arg4 !== 0);
        },
        __wbg_colorMask_7a8dbc86e7376a9b: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.colorMask(arg1 !== 0, arg2 !== 0, arg3 !== 0, arg4 !== 0);
        },
        __wbg_compileShader_63b824e86bb00b8f: function(arg0, arg1) {
            arg0.compileShader(arg1);
        },
        __wbg_compileShader_94718a93495d565d: function(arg0, arg1) {
            arg0.compileShader(arg1);
        },
        __wbg_compressedTexSubImage2D_215bb115facd5e48: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8) {
            arg0.compressedTexSubImage2D(arg1 >>> 0, arg2, arg3, arg4, arg5, arg6, arg7 >>> 0, arg8);
        },
        __wbg_compressedTexSubImage2D_684350eb62830032: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8) {
            arg0.compressedTexSubImage2D(arg1 >>> 0, arg2, arg3, arg4, arg5, arg6, arg7 >>> 0, arg8);
        },
        __wbg_compressedTexSubImage2D_d8fbae93bb8c4cc9: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9) {
            arg0.compressedTexSubImage2D(arg1 >>> 0, arg2, arg3, arg4, arg5, arg6, arg7 >>> 0, arg8, arg9);
        },
        __wbg_compressedTexSubImage3D_16afa3a47bf1d979: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10) {
            arg0.compressedTexSubImage3D(arg1 >>> 0, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9 >>> 0, arg10);
        },
        __wbg_compressedTexSubImage3D_778008a6293f15ab: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11) {
            arg0.compressedTexSubImage3D(arg1 >>> 0, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9 >>> 0, arg10, arg11);
        },
        __wbg_configure_2414aed971d368cd: function(arg0, arg1) {
            arg0.configure(arg1);
        },
        __wbg_copyBufferSubData_a4f9815861ff0ae9: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.copyBufferSubData(arg1 >>> 0, arg2 >>> 0, arg3, arg4, arg5);
        },
        __wbg_copyBufferToBuffer_1ba67191114656a1: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.copyBufferToBuffer(arg1, arg2, arg3, arg4, arg5);
        },
        __wbg_copyBufferToTexture_878d31d479e48f28: function(arg0, arg1, arg2, arg3) {
            arg0.copyBufferToTexture(arg1, arg2, arg3);
        },
        __wbg_copyExternalImageToTexture_7878d196c0b60d39: function(arg0, arg1, arg2, arg3) {
            arg0.copyExternalImageToTexture(arg1, arg2, arg3);
        },
        __wbg_copyTexSubImage2D_417a65926e3d2490: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8) {
            arg0.copyTexSubImage2D(arg1 >>> 0, arg2, arg3, arg4, arg5, arg6, arg7, arg8);
        },
        __wbg_copyTexSubImage2D_91ebcd9cd1908265: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8) {
            arg0.copyTexSubImage2D(arg1 >>> 0, arg2, arg3, arg4, arg5, arg6, arg7, arg8);
        },
        __wbg_copyTexSubImage3D_f62ef4c4eeb9a7dc: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9) {
            arg0.copyTexSubImage3D(arg1 >>> 0, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9);
        },
        __wbg_copyTextureToBuffer_6a8fe0e90f0a663d: function(arg0, arg1, arg2, arg3) {
            arg0.copyTextureToBuffer(arg1, arg2, arg3);
        },
        __wbg_copyTextureToTexture_0a06a393d6726b4a: function(arg0, arg1, arg2, arg3) {
            arg0.copyTextureToTexture(arg1, arg2, arg3);
        },
        __wbg_createBindGroupLayout_1d93b6d41c87ba9d: function(arg0, arg1) {
            const ret = arg0.createBindGroupLayout(arg1);
            return ret;
        },
        __wbg_createBindGroup_61cd07ec9d423432: function(arg0, arg1) {
            const ret = arg0.createBindGroup(arg1);
            return ret;
        },
        __wbg_createBuffer_26534c05e01b8559: function(arg0) {
            const ret = arg0.createBuffer();
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_createBuffer_963aa00d5fe859e4: function(arg0, arg1) {
            const ret = arg0.createBuffer(arg1);
            return ret;
        },
        __wbg_createBuffer_c4ec897aacc1b91c: function(arg0) {
            const ret = arg0.createBuffer();
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_createCommandEncoder_f0e1613e9a2dc1eb: function(arg0, arg1) {
            const ret = arg0.createCommandEncoder(arg1);
            return ret;
        },
        __wbg_createComputePipeline_b9616b9fe2f4eb2f: function(arg0, arg1) {
            const ret = arg0.createComputePipeline(arg1);
            return ret;
        },
        __wbg_createFramebuffer_41512c38358a41c4: function(arg0) {
            const ret = arg0.createFramebuffer();
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_createFramebuffer_b88ffa8e0fd262c4: function(arg0) {
            const ret = arg0.createFramebuffer();
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_createPipelineLayout_56c6cf983f892d2b: function(arg0, arg1) {
            const ret = arg0.createPipelineLayout(arg1);
            return ret;
        },
        __wbg_createProgram_98aaa91f7c81c5e2: function(arg0) {
            const ret = arg0.createProgram();
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_createProgram_9b7710a1f2701c2c: function(arg0) {
            const ret = arg0.createProgram();
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_createQuerySet_c14be802adf7c207: function(arg0, arg1) {
            const ret = arg0.createQuerySet(arg1);
            return ret;
        },
        __wbg_createQuery_7988050efd7e4c48: function(arg0) {
            const ret = arg0.createQuery();
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_createRenderBundleEncoder_8e4bdffea72f8c1f: function(arg0, arg1) {
            const ret = arg0.createRenderBundleEncoder(arg1);
            return ret;
        },
        __wbg_createRenderPipeline_079a88a0601fcce1: function(arg0, arg1) {
            const ret = arg0.createRenderPipeline(arg1);
            return ret;
        },
        __wbg_createRenderbuffer_1e567f2f4d461710: function(arg0) {
            const ret = arg0.createRenderbuffer();
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_createRenderbuffer_a601226a6a680dbe: function(arg0) {
            const ret = arg0.createRenderbuffer();
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_createSampler_da6bb96c9ffaaa27: function(arg0) {
            const ret = arg0.createSampler();
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_createSampler_ef5578990df3baf7: function(arg0, arg1) {
            const ret = arg0.createSampler(arg1);
            return ret;
        },
        __wbg_createShaderModule_17f451ea25cae47c: function(arg0, arg1) {
            const ret = arg0.createShaderModule(arg1);
            return ret;
        },
        __wbg_createShader_e3ac08ed8c5b14b2: function(arg0, arg1) {
            const ret = arg0.createShader(arg1 >>> 0);
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_createShader_f2b928ca9a426b14: function(arg0, arg1) {
            const ret = arg0.createShader(arg1 >>> 0);
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_createTexture_01cc1cd2fea732d9: function(arg0, arg1) {
            const ret = arg0.createTexture(arg1);
            return ret;
        },
        __wbg_createTexture_16d2c8a3d7d4a75a: function(arg0) {
            const ret = arg0.createTexture();
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_createTexture_f9451a82c7527ce2: function(arg0) {
            const ret = arg0.createTexture();
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_createVertexArrayOES_bd76ceee6ab9b95e: function(arg0) {
            const ret = arg0.createVertexArrayOES();
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_createVertexArray_ad5294951ae57497: function(arg0) {
            const ret = arg0.createVertexArray();
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_createView_04701884291e1ccc: function(arg0, arg1) {
            const ret = arg0.createView(arg1);
            return ret;
        },
        __wbg_cullFace_39500f654c67a205: function(arg0, arg1) {
            arg0.cullFace(arg1 >>> 0);
        },
        __wbg_cullFace_e7e711a14d2c3f48: function(arg0, arg1) {
            arg0.cullFace(arg1 >>> 0);
        },
        __wbg_deleteBuffer_22fcc93912cbf659: function(arg0, arg1) {
            arg0.deleteBuffer(arg1);
        },
        __wbg_deleteBuffer_ab099883c168644d: function(arg0, arg1) {
            arg0.deleteBuffer(arg1);
        },
        __wbg_deleteFramebuffer_8de1ca41ac87cfd9: function(arg0, arg1) {
            arg0.deleteFramebuffer(arg1);
        },
        __wbg_deleteFramebuffer_9738f3bb85c1ab35: function(arg0, arg1) {
            arg0.deleteFramebuffer(arg1);
        },
        __wbg_deleteProgram_9298fb3e3c1d3a78: function(arg0, arg1) {
            arg0.deleteProgram(arg1);
        },
        __wbg_deleteProgram_f354e79b8cae8076: function(arg0, arg1) {
            arg0.deleteProgram(arg1);
        },
        __wbg_deleteQuery_ea8bf1954febd774: function(arg0, arg1) {
            arg0.deleteQuery(arg1);
        },
        __wbg_deleteRenderbuffer_096edada57729468: function(arg0, arg1) {
            arg0.deleteRenderbuffer(arg1);
        },
        __wbg_deleteRenderbuffer_0f565f0727b341fc: function(arg0, arg1) {
            arg0.deleteRenderbuffer(arg1);
        },
        __wbg_deleteSampler_c6b68c4071841afa: function(arg0, arg1) {
            arg0.deleteSampler(arg1);
        },
        __wbg_deleteShader_aaf3b520a64d5d9d: function(arg0, arg1) {
            arg0.deleteShader(arg1);
        },
        __wbg_deleteShader_ff70ca962883e241: function(arg0, arg1) {
            arg0.deleteShader(arg1);
        },
        __wbg_deleteSync_c8e4a9c735f71d18: function(arg0, arg1) {
            arg0.deleteSync(arg1);
        },
        __wbg_deleteTexture_2be78224e5584a8b: function(arg0, arg1) {
            arg0.deleteTexture(arg1);
        },
        __wbg_deleteTexture_9d411c0e60ffa324: function(arg0, arg1) {
            arg0.deleteTexture(arg1);
        },
        __wbg_deleteVertexArrayOES_197df47ef9684195: function(arg0, arg1) {
            arg0.deleteVertexArrayOES(arg1);
        },
        __wbg_deleteVertexArray_7bc7f92769862f93: function(arg0, arg1) {
            arg0.deleteVertexArray(arg1);
        },
        __wbg_depthFunc_eb3aa05361dd2eaa: function(arg0, arg1) {
            arg0.depthFunc(arg1 >>> 0);
        },
        __wbg_depthFunc_f670d4cbb9cd0913: function(arg0, arg1) {
            arg0.depthFunc(arg1 >>> 0);
        },
        __wbg_depthMask_103091329ca1a750: function(arg0, arg1) {
            arg0.depthMask(arg1 !== 0);
        },
        __wbg_depthMask_75a36d0065471a4b: function(arg0, arg1) {
            arg0.depthMask(arg1 !== 0);
        },
        __wbg_depthRange_337bf254e67639bb: function(arg0, arg1, arg2) {
            arg0.depthRange(arg1, arg2);
        },
        __wbg_depthRange_5579d448b9d7de57: function(arg0, arg1, arg2) {
            arg0.depthRange(arg1, arg2);
        },
        __wbg_destroy_35f94012e5bb9c17: function(arg0) {
            arg0.destroy();
        },
        __wbg_destroy_767d9dde1008e293: function(arg0) {
            arg0.destroy();
        },
        __wbg_destroy_c6af4226dda95dbd: function(arg0) {
            arg0.destroy();
        },
        __wbg_disableVertexAttribArray_24a020060006b10f: function(arg0, arg1) {
            arg0.disableVertexAttribArray(arg1 >>> 0);
        },
        __wbg_disableVertexAttribArray_4bac633c27bae599: function(arg0, arg1) {
            arg0.disableVertexAttribArray(arg1 >>> 0);
        },
        __wbg_disable_7fe6fb3e97717f88: function(arg0, arg1) {
            arg0.disable(arg1 >>> 0);
        },
        __wbg_disable_bd37bdcca1764aea: function(arg0, arg1) {
            arg0.disable(arg1 >>> 0);
        },
        __wbg_dispatchWorkgroupsIndirect_8b25efab93a7a433: function(arg0, arg1, arg2) {
            arg0.dispatchWorkgroupsIndirect(arg1, arg2);
        },
        __wbg_dispatchWorkgroups_c102fa81b955935d: function(arg0, arg1, arg2, arg3) {
            arg0.dispatchWorkgroups(arg1 >>> 0, arg2 >>> 0, arg3 >>> 0);
        },
        __wbg_document_ee35a3d3ae34ef6c: function(arg0) {
            const ret = arg0.document;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_done_57b39ecd9addfe81: function(arg0) {
            const ret = arg0.done;
            return ret;
        },
        __wbg_drawArraysInstancedANGLE_9e4cc507eae8b24d: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.drawArraysInstancedANGLE(arg1 >>> 0, arg2, arg3, arg4);
        },
        __wbg_drawArraysInstanced_ec30adc616ec58d5: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.drawArraysInstanced(arg1 >>> 0, arg2, arg3, arg4);
        },
        __wbg_drawArrays_075228181299b824: function(arg0, arg1, arg2, arg3) {
            arg0.drawArrays(arg1 >>> 0, arg2, arg3);
        },
        __wbg_drawArrays_2be89c369a29f30b: function(arg0, arg1, arg2, arg3) {
            arg0.drawArrays(arg1 >>> 0, arg2, arg3);
        },
        __wbg_drawBuffersWEBGL_447bc0a21f8ef22d: function(arg0, arg1) {
            arg0.drawBuffersWEBGL(arg1);
        },
        __wbg_drawBuffers_5eccfaacc6560299: function(arg0, arg1) {
            arg0.drawBuffers(arg1);
        },
        __wbg_drawElementsInstancedANGLE_6f9da0b845ac6c4e: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.drawElementsInstancedANGLE(arg1 >>> 0, arg2, arg3 >>> 0, arg4, arg5);
        },
        __wbg_drawElementsInstanced_d41fc920ae24717c: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.drawElementsInstanced(arg1 >>> 0, arg2, arg3 >>> 0, arg4, arg5);
        },
        __wbg_drawIndexedIndirect_34484fc6227c7bc8: function(arg0, arg1, arg2) {
            arg0.drawIndexedIndirect(arg1, arg2);
        },
        __wbg_drawIndexedIndirect_5a7c30bb5f1d5b67: function(arg0, arg1, arg2) {
            arg0.drawIndexedIndirect(arg1, arg2);
        },
        __wbg_drawIndexed_115af1449b52a948: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.drawIndexed(arg1 >>> 0, arg2 >>> 0, arg3 >>> 0, arg4, arg5 >>> 0);
        },
        __wbg_drawIndexed_a587cce4c317791f: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.drawIndexed(arg1 >>> 0, arg2 >>> 0, arg3 >>> 0, arg4, arg5 >>> 0);
        },
        __wbg_drawIndirect_036d71498a21f1a3: function(arg0, arg1, arg2) {
            arg0.drawIndirect(arg1, arg2);
        },
        __wbg_drawIndirect_a1d7c5e893aa5756: function(arg0, arg1, arg2) {
            arg0.drawIndirect(arg1, arg2);
        },
        __wbg_draw_5351b12033166aca: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.draw(arg1 >>> 0, arg2 >>> 0, arg3 >>> 0, arg4 >>> 0);
        },
        __wbg_draw_e2a7c5d66fb2d244: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.draw(arg1 >>> 0, arg2 >>> 0, arg3 >>> 0, arg4 >>> 0);
        },
        __wbg_enableVertexAttribArray_475e06c31777296d: function(arg0, arg1) {
            arg0.enableVertexAttribArray(arg1 >>> 0);
        },
        __wbg_enableVertexAttribArray_aa6e40408261eeb9: function(arg0, arg1) {
            arg0.enableVertexAttribArray(arg1 >>> 0);
        },
        __wbg_enable_d1ac04dfdd2fb3ae: function(arg0, arg1) {
            arg0.enable(arg1 >>> 0);
        },
        __wbg_enable_fee40f19b7053ea3: function(arg0, arg1) {
            arg0.enable(arg1 >>> 0);
        },
        __wbg_endQuery_54f0627d4c931318: function(arg0, arg1) {
            arg0.endQuery(arg1 >>> 0);
        },
        __wbg_end_0ac71677a5c1717a: function(arg0) {
            arg0.end();
        },
        __wbg_end_6f776519f1faa582: function(arg0) {
            arg0.end();
        },
        __wbg_error_7534b8e9a36f1ab4: function(arg0, arg1) {
            let deferred0_0;
            let deferred0_1;
            try {
                deferred0_0 = arg0;
                deferred0_1 = arg1;
                console.error(getStringFromWasm0(arg0, arg1));
            } finally {
                wasm.__wbindgen_free(deferred0_0, deferred0_1, 1);
            }
        },
        __wbg_error_9a7fe3f932034cde: function(arg0) {
            console.error(arg0);
        },
        __wbg_error_e98e6aadd08e0b94: function(arg0) {
            const ret = arg0.error;
            return ret;
        },
        __wbg_executeBundles_8e6c0614da2805d4: function(arg0, arg1) {
            arg0.executeBundles(arg1);
        },
        __wbg_features_1b464383ea8a7691: function(arg0) {
            const ret = arg0.features;
            return ret;
        },
        __wbg_features_e5fbbc2760867852: function(arg0) {
            const ret = arg0.features;
            return ret;
        },
        __wbg_fenceSync_c52a4e24eabfa0d3: function(arg0, arg1, arg2) {
            const ret = arg0.fenceSync(arg1 >>> 0, arg2 >>> 0);
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_finish_20711371c58df61c: function(arg0) {
            const ret = arg0.finish();
            return ret;
        },
        __wbg_finish_34b2c54329c8719f: function(arg0, arg1) {
            const ret = arg0.finish(arg1);
            return ret;
        },
        __wbg_finish_a9ab917e756ea00c: function(arg0, arg1) {
            const ret = arg0.finish(arg1);
            return ret;
        },
        __wbg_finish_e0a6c97c0622f843: function(arg0) {
            const ret = arg0.finish();
            return ret;
        },
        __wbg_framebufferRenderbuffer_850811ed6e26475e: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.framebufferRenderbuffer(arg1 >>> 0, arg2 >>> 0, arg3 >>> 0, arg4);
        },
        __wbg_framebufferRenderbuffer_cd9d55a68a2300ea: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.framebufferRenderbuffer(arg1 >>> 0, arg2 >>> 0, arg3 >>> 0, arg4);
        },
        __wbg_framebufferTexture2D_8adf6bdfc3c56dee: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.framebufferTexture2D(arg1 >>> 0, arg2 >>> 0, arg3 >>> 0, arg4, arg5);
        },
        __wbg_framebufferTexture2D_c283e928186aa542: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.framebufferTexture2D(arg1 >>> 0, arg2 >>> 0, arg3 >>> 0, arg4, arg5);
        },
        __wbg_framebufferTextureLayer_c8328828c8d5eb60: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.framebufferTextureLayer(arg1 >>> 0, arg2 >>> 0, arg3, arg4, arg5);
        },
        __wbg_framebufferTextureMultiviewOVR_16d049b41d692b91: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.framebufferTextureMultiviewOVR(arg1 >>> 0, arg2 >>> 0, arg3, arg4, arg5, arg6);
        },
        __wbg_frontFace_027e2ec7a7bc347c: function(arg0, arg1) {
            arg0.frontFace(arg1 >>> 0);
        },
        __wbg_frontFace_d4a6507ad2939b5c: function(arg0, arg1) {
            arg0.frontFace(arg1 >>> 0);
        },
        __wbg_getBindGroupLayout_4a94df6108ac6667: function(arg0, arg1) {
            const ret = arg0.getBindGroupLayout(arg1 >>> 0);
            return ret;
        },
        __wbg_getBindGroupLayout_80e803d942962f6a: function(arg0, arg1) {
            const ret = arg0.getBindGroupLayout(arg1 >>> 0);
            return ret;
        },
        __wbg_getBufferSubData_4fc54b4fbb1462d7: function(arg0, arg1, arg2, arg3) {
            arg0.getBufferSubData(arg1 >>> 0, arg2, arg3);
        },
        __wbg_getCompilationInfo_2af3ecdfeda551a3: function(arg0) {
            const ret = arg0.getCompilationInfo();
            return ret;
        },
        __wbg_getContext_2966500392030d63: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.getContext(getStringFromWasm0(arg1, arg2));
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_getContext_2a5764d48600bc43: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.getContext(getStringFromWasm0(arg1, arg2));
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_getContext_b28d2db7bd648242: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.getContext(getStringFromWasm0(arg1, arg2), arg3);
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_getContext_de810d9f187f29ca: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.getContext(getStringFromWasm0(arg1, arg2), arg3);
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_getCurrentTexture_5a79cda2ff36e1ee: function(arg0) {
            const ret = arg0.getCurrentTexture();
            return ret;
        },
        __wbg_getExtension_3c0cb5ae01bb4b17: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.getExtension(getStringFromWasm0(arg1, arg2));
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_getIndexedParameter_ca1693c768bc4934: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.getIndexedParameter(arg1 >>> 0, arg2 >>> 0);
            return ret;
        }, arguments); },
        __wbg_getMappedRange_932dd043ae22ee0a: function(arg0, arg1, arg2) {
            const ret = arg0.getMappedRange(arg1, arg2);
            return ret;
        },
        __wbg_getParameter_1ecb910cfdd21f88: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.getParameter(arg1 >>> 0);
            return ret;
        }, arguments); },
        __wbg_getParameter_2e1f97ecaab76274: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.getParameter(arg1 >>> 0);
            return ret;
        }, arguments); },
        __wbg_getPreferredCanvasFormat_de73c02773a5209e: function(arg0) {
            const ret = arg0.getPreferredCanvasFormat();
            return (__wbindgen_enum_GpuTextureFormat.indexOf(ret) + 1 || 96) - 1;
        },
        __wbg_getProgramInfoLog_2ffa30e3abb8b5c2: function(arg0, arg1, arg2) {
            const ret = arg1.getProgramInfoLog(arg2);
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_getProgramInfoLog_dbfda4b6e7eb1b37: function(arg0, arg1, arg2) {
            const ret = arg1.getProgramInfoLog(arg2);
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_getProgramParameter_43fbc6d2613c08b3: function(arg0, arg1, arg2) {
            const ret = arg0.getProgramParameter(arg1, arg2 >>> 0);
            return ret;
        },
        __wbg_getProgramParameter_92e4540ca9da06b2: function(arg0, arg1, arg2) {
            const ret = arg0.getProgramParameter(arg1, arg2 >>> 0);
            return ret;
        },
        __wbg_getQueryParameter_5d6af051438ae479: function(arg0, arg1, arg2) {
            const ret = arg0.getQueryParameter(arg1, arg2 >>> 0);
            return ret;
        },
        __wbg_getRandomValues_9c5c1b115e142bb8: function() { return handleError(function (arg0, arg1) {
            globalThis.crypto.getRandomValues(getArrayU8FromWasm0(arg0, arg1));
        }, arguments); },
        __wbg_getShaderInfoLog_9991e9e77b0c6805: function(arg0, arg1, arg2) {
            const ret = arg1.getShaderInfoLog(arg2);
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_getShaderInfoLog_9e0b96da4b13ae49: function(arg0, arg1, arg2) {
            const ret = arg1.getShaderInfoLog(arg2);
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_getShaderParameter_786fd84f85720ca8: function(arg0, arg1, arg2) {
            const ret = arg0.getShaderParameter(arg1, arg2 >>> 0);
            return ret;
        },
        __wbg_getShaderParameter_afa4a3dd9dd397c1: function(arg0, arg1, arg2) {
            const ret = arg0.getShaderParameter(arg1, arg2 >>> 0);
            return ret;
        },
        __wbg_getSupportedExtensions_57142a6b598d7787: function(arg0) {
            const ret = arg0.getSupportedExtensions();
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_getSupportedProfiles_1f728bc32003c4d0: function(arg0) {
            const ret = arg0.getSupportedProfiles();
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_getSyncParameter_7d11ab875b41617e: function(arg0, arg1, arg2) {
            const ret = arg0.getSyncParameter(arg1, arg2 >>> 0);
            return ret;
        },
        __wbg_getUniformBlockIndex_1ee7e922e6d96d7e: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.getUniformBlockIndex(arg1, getStringFromWasm0(arg2, arg3));
            return ret;
        },
        __wbg_getUniformLocation_71c070e6644669ad: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.getUniformLocation(arg1, getStringFromWasm0(arg2, arg3));
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_getUniformLocation_d06b3a5b3c60e95c: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.getUniformLocation(arg1, getStringFromWasm0(arg2, arg3));
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_get_9b94d73e6221f75c: function(arg0, arg1) {
            const ret = arg0[arg1 >>> 0];
            return ret;
        },
        __wbg_get_b3ed3ad4be2bc8ac: function() { return handleError(function (arg0, arg1) {
            const ret = Reflect.get(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_get_d8db2ad31d529ff8: function(arg0, arg1) {
            const ret = arg0[arg1 >>> 0];
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_get_with_ref_key_1dc361bd10053bfe: function(arg0, arg1) {
            const ret = arg0[arg1];
            return ret;
        },
        __wbg_gpu_87871e8f7ace8fee: function(arg0) {
            const ret = arg0.gpu;
            return ret;
        },
        __wbg_has_624cbf0451d880e8: function(arg0, arg1, arg2) {
            const ret = arg0.has(getStringFromWasm0(arg1, arg2));
            return ret;
        },
        __wbg_height_38750dc6de41ee75: function(arg0) {
            const ret = arg0.height;
            return ret;
        },
        __wbg_height_408f385de046f7e5: function(arg0) {
            const ret = arg0.height;
            return ret;
        },
        __wbg_height_87250db2be5164b9: function(arg0) {
            const ret = arg0.height;
            return ret;
        },
        __wbg_height_9a49d61734f6cf36: function(arg0) {
            const ret = arg0.height;
            return ret;
        },
        __wbg_height_aceb0c14551ea27d: function(arg0) {
            const ret = arg0.height;
            return ret;
        },
        __wbg_includes_32215c836f1cd3fb: function(arg0, arg1, arg2) {
            const ret = arg0.includes(arg1, arg2);
            return ret;
        },
        __wbg_instanceof_ArrayBuffer_c367199e2fa2aa04: function(arg0) {
            let result;
            try {
                result = arg0 instanceof ArrayBuffer;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_GpuAdapter_0731153d2b08720b: function(arg0) {
            let result;
            try {
                result = arg0 instanceof GPUAdapter;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_GpuCanvasContext_d14121c7bd72fcef: function(arg0) {
            let result;
            try {
                result = arg0 instanceof GPUCanvasContext;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_GpuDeviceLostInfo_a3677ebb8241d800: function(arg0) {
            let result;
            try {
                result = arg0 instanceof GPUDeviceLostInfo;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_GpuOutOfMemoryError_391d9a08edbfa04b: function(arg0) {
            let result;
            try {
                result = arg0 instanceof GPUOutOfMemoryError;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_GpuValidationError_f4d803c383da3c92: function(arg0) {
            let result;
            try {
                result = arg0 instanceof GPUValidationError;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_HtmlCanvasElement_3f2f6e1edb1c9792: function(arg0) {
            let result;
            try {
                result = arg0 instanceof HTMLCanvasElement;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Object_1c6af87502b733ed: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Object;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Uint8Array_9b9075935c74707c: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Uint8Array;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_WebGl2RenderingContext_4a08a94517ed5240: function(arg0) {
            let result;
            try {
                result = arg0 instanceof WebGL2RenderingContext;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Window_ed49b2db8df90359: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Window;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_invalidateFramebuffer_b17b7e1da3051745: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.invalidateFramebuffer(arg1 >>> 0, arg2);
        }, arguments); },
        __wbg_isArray_d314bb98fcf08331: function(arg0) {
            const ret = Array.isArray(arg0);
            return ret;
        },
        __wbg_isSafeInteger_bfbc7332a9768d2a: function(arg0) {
            const ret = Number.isSafeInteger(arg0);
            return ret;
        },
        __wbg_is_f29129f676e5410c: function(arg0, arg1) {
            const ret = Object.is(arg0, arg1);
            return ret;
        },
        __wbg_iterator_6ff6560ca1568e55: function() {
            const ret = Symbol.iterator;
            return ret;
        },
        __wbg_label_2082ab37d2ad170d: function(arg0, arg1) {
            const ret = arg1.label;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_length_32ed9a279acd054c: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_35a7bace40f36eac: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_9df32f7add647235: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_limits_2dd632c891786ddf: function(arg0) {
            const ret = arg0.limits;
            return ret;
        },
        __wbg_limits_f6411f884b0b2d62: function(arg0) {
            const ret = arg0.limits;
            return ret;
        },
        __wbg_lineNum_0246de1e072ffe19: function(arg0) {
            const ret = arg0.lineNum;
            return ret;
        },
        __wbg_linkProgram_6600dd2c0863bbfd: function(arg0, arg1) {
            arg0.linkProgram(arg1);
        },
        __wbg_linkProgram_be6b825cf66d177b: function(arg0, arg1) {
            arg0.linkProgram(arg1);
        },
        __wbg_log_6b5ca2e6124b2808: function(arg0) {
            console.log(arg0);
        },
        __wbg_lost_6e4d29847ce2a34a: function(arg0) {
            const ret = arg0.lost;
            return ret;
        },
        __wbg_mapAsync_37f5e03edf2e1352: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.mapAsync(arg1 >>> 0, arg2, arg3);
            return ret;
        },
        __wbg_maxBindGroups_768ca5e8623bf450: function(arg0) {
            const ret = arg0.maxBindGroups;
            return ret;
        },
        __wbg_maxBindingsPerBindGroup_057972d600d69719: function(arg0) {
            const ret = arg0.maxBindingsPerBindGroup;
            return ret;
        },
        __wbg_maxBufferSize_e237b44f19a5a62b: function(arg0) {
            const ret = arg0.maxBufferSize;
            return ret;
        },
        __wbg_maxColorAttachmentBytesPerSample_d6c7b4051d22c6d6: function(arg0) {
            const ret = arg0.maxColorAttachmentBytesPerSample;
            return ret;
        },
        __wbg_maxColorAttachments_7a18ba24c05edcfd: function(arg0) {
            const ret = arg0.maxColorAttachments;
            return ret;
        },
        __wbg_maxComputeInvocationsPerWorkgroup_b99c2f3611633992: function(arg0) {
            const ret = arg0.maxComputeInvocationsPerWorkgroup;
            return ret;
        },
        __wbg_maxComputeWorkgroupSizeX_adb26da9ed7f77f7: function(arg0) {
            const ret = arg0.maxComputeWorkgroupSizeX;
            return ret;
        },
        __wbg_maxComputeWorkgroupSizeY_cc217559c98be33b: function(arg0) {
            const ret = arg0.maxComputeWorkgroupSizeY;
            return ret;
        },
        __wbg_maxComputeWorkgroupSizeZ_66606a80e2cf2309: function(arg0) {
            const ret = arg0.maxComputeWorkgroupSizeZ;
            return ret;
        },
        __wbg_maxComputeWorkgroupStorageSize_cb6235497b8c4997: function(arg0) {
            const ret = arg0.maxComputeWorkgroupStorageSize;
            return ret;
        },
        __wbg_maxComputeWorkgroupsPerDimension_6bf550b5f21d57cf: function(arg0) {
            const ret = arg0.maxComputeWorkgroupsPerDimension;
            return ret;
        },
        __wbg_maxDynamicStorageBuffersPerPipelineLayout_c6ac20334e328b47: function(arg0) {
            const ret = arg0.maxDynamicStorageBuffersPerPipelineLayout;
            return ret;
        },
        __wbg_maxDynamicUniformBuffersPerPipelineLayout_aa8f14a74b440f01: function(arg0) {
            const ret = arg0.maxDynamicUniformBuffersPerPipelineLayout;
            return ret;
        },
        __wbg_maxSampledTexturesPerShaderStage_db7c4922cc60144a: function(arg0) {
            const ret = arg0.maxSampledTexturesPerShaderStage;
            return ret;
        },
        __wbg_maxSamplersPerShaderStage_538705fe2263e710: function(arg0) {
            const ret = arg0.maxSamplersPerShaderStage;
            return ret;
        },
        __wbg_maxStorageBufferBindingSize_32178c0f5f7f85cb: function(arg0) {
            const ret = arg0.maxStorageBufferBindingSize;
            return ret;
        },
        __wbg_maxStorageBuffersPerShaderStage_9f67e9eae0089f77: function(arg0) {
            const ret = arg0.maxStorageBuffersPerShaderStage;
            return ret;
        },
        __wbg_maxStorageTexturesPerShaderStage_57239664936031cf: function(arg0) {
            const ret = arg0.maxStorageTexturesPerShaderStage;
            return ret;
        },
        __wbg_maxTextureArrayLayers_db5d4e486c78ae04: function(arg0) {
            const ret = arg0.maxTextureArrayLayers;
            return ret;
        },
        __wbg_maxTextureDimension1D_3475085ffacabbdc: function(arg0) {
            const ret = arg0.maxTextureDimension1D;
            return ret;
        },
        __wbg_maxTextureDimension2D_7c8d5ecf09eb8519: function(arg0) {
            const ret = arg0.maxTextureDimension2D;
            return ret;
        },
        __wbg_maxTextureDimension3D_8bd976677a0f91d4: function(arg0) {
            const ret = arg0.maxTextureDimension3D;
            return ret;
        },
        __wbg_maxUniformBufferBindingSize_95b1a54e7e4a0f0f: function(arg0) {
            const ret = arg0.maxUniformBufferBindingSize;
            return ret;
        },
        __wbg_maxUniformBuffersPerShaderStage_5f475d9a453af14d: function(arg0) {
            const ret = arg0.maxUniformBuffersPerShaderStage;
            return ret;
        },
        __wbg_maxVertexAttributes_4c48ca2f5d32f860: function(arg0) {
            const ret = arg0.maxVertexAttributes;
            return ret;
        },
        __wbg_maxVertexBufferArrayStride_2233f6933ecc5a16: function(arg0) {
            const ret = arg0.maxVertexBufferArrayStride;
            return ret;
        },
        __wbg_maxVertexBuffers_c47e508cd7348554: function(arg0) {
            const ret = arg0.maxVertexBuffers;
            return ret;
        },
        __wbg_message_0762358e59db7ed6: function(arg0, arg1) {
            const ret = arg1.message;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_message_7957ab09f64c6822: function(arg0, arg1) {
            const ret = arg1.message;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_message_b163994503433c9e: function(arg0, arg1) {
            const ret = arg1.message;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_messages_da071582f72bc978: function(arg0) {
            const ret = arg0.messages;
            return ret;
        },
        __wbg_minStorageBufferOffsetAlignment_51b4801fac3a58de: function(arg0) {
            const ret = arg0.minStorageBufferOffsetAlignment;
            return ret;
        },
        __wbg_minUniformBufferOffsetAlignment_5d62a77924b2335f: function(arg0) {
            const ret = arg0.minUniformBufferOffsetAlignment;
            return ret;
        },
        __wbg_navigator_43be698ba96fc088: function(arg0) {
            const ret = arg0.navigator;
            return ret;
        },
        __wbg_navigator_4478931f32ebca57: function(arg0) {
            const ret = arg0.navigator;
            return ret;
        },
        __wbg_new_361308b2356cecd0: function() {
            const ret = new Object();
            return ret;
        },
        __wbg_new_3eb36ae241fe6f44: function() {
            const ret = new Array();
            return ret;
        },
        __wbg_new_8a6f238a6ece86ea: function() {
            const ret = new Error();
            return ret;
        },
        __wbg_new_b5d9e2fb389fef91: function(arg0, arg1) {
            try {
                var state0 = {a: arg0, b: arg1};
                var cb0 = (arg0, arg1) => {
                    const a = state0.a;
                    state0.a = 0;
                    try {
                        return wasm_bindgen__convert__closures_____invoke__h61f848611b9bfd22(a, state0.b, arg0, arg1);
                    } finally {
                        state0.a = a;
                    }
                };
                const ret = new Promise(cb0);
                return ret;
            } finally {
                state0.a = state0.b = 0;
            }
        },
        __wbg_new_dca287b076112a51: function() {
            const ret = new Map();
            return ret;
        },
        __wbg_new_dd2b680c8bf6ae29: function(arg0) {
            const ret = new Uint8Array(arg0);
            return ret;
        },
        __wbg_new_from_slice_132ef6dc5072cf68: function(arg0, arg1) {
            const ret = new Float32Array(getArrayF32FromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_from_slice_19d21922ff3c0ae6: function(arg0, arg1) {
            const ret = new Uint32Array(getArrayU32FromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_from_slice_a3d2629dc1826784: function(arg0, arg1) {
            const ret = new Uint8Array(getArrayU8FromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_no_args_1c7c842f08d00ebb: function(arg0, arg1) {
            const ret = new Function(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_with_byte_offset_and_length_aa261d9c9da49eb1: function(arg0, arg1, arg2) {
            const ret = new Uint8Array(arg0, arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_new_with_length_1763c527b2923202: function(arg0) {
            const ret = new Array(arg0 >>> 0);
            return ret;
        },
        __wbg_next_3482f54c49e8af19: function() { return handleError(function (arg0) {
            const ret = arg0.next();
            return ret;
        }, arguments); },
        __wbg_next_418f80d8f5303233: function(arg0) {
            const ret = arg0.next;
            return ret;
        },
        __wbg_now_73d406b8795c2240: function() {
            const ret = performance.now();
            return ret;
        },
        __wbg_now_a3af9a2f4bbaa4d1: function() {
            const ret = Date.now();
            return ret;
        },
        __wbg_of_f915f7cd925b21a5: function(arg0) {
            const ret = Array.of(arg0);
            return ret;
        },
        __wbg_offset_336f14c993863b76: function(arg0) {
            const ret = arg0.offset;
            return ret;
        },
        __wbg_pixelStorei_2a65936c11b710fe: function(arg0, arg1, arg2) {
            arg0.pixelStorei(arg1 >>> 0, arg2);
        },
        __wbg_pixelStorei_f7cc498f52d523f1: function(arg0, arg1, arg2) {
            arg0.pixelStorei(arg1 >>> 0, arg2);
        },
        __wbg_polygonOffset_24a8059deb03be92: function(arg0, arg1, arg2) {
            arg0.polygonOffset(arg1, arg2);
        },
        __wbg_polygonOffset_4b3158d8ed028862: function(arg0, arg1, arg2) {
            arg0.polygonOffset(arg1, arg2);
        },
        __wbg_popErrorScope_af0b22f136a861d6: function(arg0) {
            const ret = arg0.popErrorScope();
            return ret;
        },
        __wbg_prototypesetcall_bdcdcc5842e4d77d: function(arg0, arg1, arg2) {
            Uint8Array.prototype.set.call(getArrayU8FromWasm0(arg0, arg1), arg2);
        },
        __wbg_pushErrorScope_b52914ff10ba6ce3: function(arg0, arg1) {
            arg0.pushErrorScope(__wbindgen_enum_GpuErrorFilter[arg1]);
        },
        __wbg_push_8ffdcb2063340ba5: function(arg0, arg1) {
            const ret = arg0.push(arg1);
            return ret;
        },
        __wbg_queryCounterEXT_b578f07c30420446: function(arg0, arg1, arg2) {
            arg0.queryCounterEXT(arg1, arg2 >>> 0);
        },
        __wbg_querySelectorAll_1283aae52043a951: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.querySelectorAll(getStringFromWasm0(arg1, arg2));
            return ret;
        }, arguments); },
        __wbg_querySelector_c3b0df2d58eec220: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.querySelector(getStringFromWasm0(arg1, arg2));
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_queueMicrotask_0aa0a927f78f5d98: function(arg0) {
            const ret = arg0.queueMicrotask;
            return ret;
        },
        __wbg_queueMicrotask_5bb536982f78a56f: function(arg0) {
            queueMicrotask(arg0);
        },
        __wbg_queue_bea4017efaaf9904: function(arg0) {
            const ret = arg0.queue;
            return ret;
        },
        __wbg_readBuffer_9eb461d6857295f0: function(arg0, arg1) {
            arg0.readBuffer(arg1 >>> 0);
        },
        __wbg_readPixels_55b18304384e073d: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7) {
            arg0.readPixels(arg1, arg2, arg3, arg4, arg5 >>> 0, arg6 >>> 0, arg7);
        }, arguments); },
        __wbg_readPixels_6ea8e288a8673282: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7) {
            arg0.readPixels(arg1, arg2, arg3, arg4, arg5 >>> 0, arg6 >>> 0, arg7);
        }, arguments); },
        __wbg_readPixels_95b2464a7bb863a2: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7) {
            arg0.readPixels(arg1, arg2, arg3, arg4, arg5 >>> 0, arg6 >>> 0, arg7);
        }, arguments); },
        __wbg_reason_43acd39cce242b50: function(arg0) {
            const ret = arg0.reason;
            return (__wbindgen_enum_GpuDeviceLostReason.indexOf(ret) + 1 || 3) - 1;
        },
        __wbg_renderbufferStorageMultisample_bc0ae08a7abb887a: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.renderbufferStorageMultisample(arg1 >>> 0, arg2, arg3 >>> 0, arg4, arg5);
        },
        __wbg_renderbufferStorage_1bc02383614b76b2: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.renderbufferStorage(arg1 >>> 0, arg2 >>> 0, arg3, arg4);
        },
        __wbg_renderbufferStorage_6348154d30979c44: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.renderbufferStorage(arg1 >>> 0, arg2 >>> 0, arg3, arg4);
        },
        __wbg_requestAdapter_e6dcfac497cafa7a: function(arg0, arg1) {
            const ret = arg0.requestAdapter(arg1);
            return ret;
        },
        __wbg_requestDevice_03b802707d5a382c: function(arg0, arg1) {
            const ret = arg0.requestDevice(arg1);
            return ret;
        },
        __wbg_resolveQuerySet_811661fb23f3b699: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.resolveQuerySet(arg1, arg2 >>> 0, arg3 >>> 0, arg4, arg5 >>> 0);
        },
        __wbg_resolve_002c4b7d9d8f6b64: function(arg0) {
            const ret = Promise.resolve(arg0);
            return ret;
        },
        __wbg_samplerParameterf_f070d2b69b1e2d46: function(arg0, arg1, arg2, arg3) {
            arg0.samplerParameterf(arg1, arg2 >>> 0, arg3);
        },
        __wbg_samplerParameteri_8e4c4bcead0ee669: function(arg0, arg1, arg2, arg3) {
            arg0.samplerParameteri(arg1, arg2 >>> 0, arg3);
        },
        __wbg_scissor_2ff8f18f05a6d408: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.scissor(arg1, arg2, arg3, arg4);
        },
        __wbg_scissor_b870b1434a9c25b4: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.scissor(arg1, arg2, arg3, arg4);
        },
        __wbg_setBindGroup_62a3045b0921e429: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.setBindGroup(arg1 >>> 0, arg2, getArrayU32FromWasm0(arg3, arg4), arg5, arg6 >>> 0);
        },
        __wbg_setBindGroup_6c0fd18e9a53a945: function(arg0, arg1, arg2) {
            arg0.setBindGroup(arg1 >>> 0, arg2);
        },
        __wbg_setBindGroup_7f3b61f1f482133b: function(arg0, arg1, arg2) {
            arg0.setBindGroup(arg1 >>> 0, arg2);
        },
        __wbg_setBindGroup_bf767a5aa46a33ce: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.setBindGroup(arg1 >>> 0, arg2, getArrayU32FromWasm0(arg3, arg4), arg5, arg6 >>> 0);
        },
        __wbg_setBindGroup_c4aaff14063226b4: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.setBindGroup(arg1 >>> 0, arg2, getArrayU32FromWasm0(arg3, arg4), arg5, arg6 >>> 0);
        },
        __wbg_setBindGroup_f82e771dc1b69093: function(arg0, arg1, arg2) {
            arg0.setBindGroup(arg1 >>> 0, arg2);
        },
        __wbg_setBlendConstant_016723821cfb3aa4: function(arg0, arg1) {
            arg0.setBlendConstant(arg1);
        },
        __wbg_setIndexBuffer_286a40afdff411b7: function(arg0, arg1, arg2, arg3) {
            arg0.setIndexBuffer(arg1, __wbindgen_enum_GpuIndexFormat[arg2], arg3);
        },
        __wbg_setIndexBuffer_7efd0b7a40c65fb9: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.setIndexBuffer(arg1, __wbindgen_enum_GpuIndexFormat[arg2], arg3, arg4);
        },
        __wbg_setIndexBuffer_e091a9673bb575e2: function(arg0, arg1, arg2, arg3) {
            arg0.setIndexBuffer(arg1, __wbindgen_enum_GpuIndexFormat[arg2], arg3);
        },
        __wbg_setIndexBuffer_f0759f00036f615f: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.setIndexBuffer(arg1, __wbindgen_enum_GpuIndexFormat[arg2], arg3, arg4);
        },
        __wbg_setPipeline_ba92070b8ee81cf9: function(arg0, arg1) {
            arg0.setPipeline(arg1);
        },
        __wbg_setPipeline_c344f76bae58c4d6: function(arg0, arg1) {
            arg0.setPipeline(arg1);
        },
        __wbg_setPipeline_d76451c50a121598: function(arg0, arg1) {
            arg0.setPipeline(arg1);
        },
        __wbg_setScissorRect_0b6ee0852ef0b6b9: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.setScissorRect(arg1 >>> 0, arg2 >>> 0, arg3 >>> 0, arg4 >>> 0);
        },
        __wbg_setStencilReference_34fd3d59673a5a9d: function(arg0, arg1) {
            arg0.setStencilReference(arg1 >>> 0);
        },
        __wbg_setVertexBuffer_06a90dc78e1ad9c4: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.setVertexBuffer(arg1 >>> 0, arg2, arg3, arg4);
        },
        __wbg_setVertexBuffer_1540e9118b6c451d: function(arg0, arg1, arg2, arg3) {
            arg0.setVertexBuffer(arg1 >>> 0, arg2, arg3);
        },
        __wbg_setVertexBuffer_5166eedc06450701: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.setVertexBuffer(arg1 >>> 0, arg2, arg3, arg4);
        },
        __wbg_setVertexBuffer_8621784e5014065b: function(arg0, arg1, arg2, arg3) {
            arg0.setVertexBuffer(arg1 >>> 0, arg2, arg3);
        },
        __wbg_setViewport_731ad30abb13f744: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.setViewport(arg1, arg2, arg3, arg4, arg5, arg6);
        },
        __wbg_set_1eb0999cf5d27fc8: function(arg0, arg1, arg2) {
            const ret = arg0.set(arg1, arg2);
            return ret;
        },
        __wbg_set_25cf9deff6bf0ea8: function(arg0, arg1, arg2) {
            arg0.set(arg1, arg2 >>> 0);
        },
        __wbg_set_3f1d0b984ed272ed: function(arg0, arg1, arg2) {
            arg0[arg1] = arg2;
        },
        __wbg_set_6cb8631f80447a67: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = Reflect.set(arg0, arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_set_f43e577aea94465b: function(arg0, arg1, arg2) {
            arg0[arg1 >>> 0] = arg2;
        },
        __wbg_set_height_b386c0f603610637: function(arg0, arg1) {
            arg0.height = arg1 >>> 0;
        },
        __wbg_set_height_f21f985387070100: function(arg0, arg1) {
            arg0.height = arg1 >>> 0;
        },
        __wbg_set_onuncapturederror_19541466822d790b: function(arg0, arg1) {
            arg0.onuncapturederror = arg1;
        },
        __wbg_set_width_7f07715a20503914: function(arg0, arg1) {
            arg0.width = arg1 >>> 0;
        },
        __wbg_set_width_d60bc4f2f20c56a4: function(arg0, arg1) {
            arg0.width = arg1 >>> 0;
        },
        __wbg_shaderSource_32425cfe6e5a1e52: function(arg0, arg1, arg2, arg3) {
            arg0.shaderSource(arg1, getStringFromWasm0(arg2, arg3));
        },
        __wbg_shaderSource_8f4bda03f70359df: function(arg0, arg1, arg2, arg3) {
            arg0.shaderSource(arg1, getStringFromWasm0(arg2, arg3));
        },
        __wbg_size_661bddb3f9898121: function(arg0) {
            const ret = arg0.size;
            return ret;
        },
        __wbg_stack_0ed75d68575b0f3c: function(arg0, arg1) {
            const ret = arg1.stack;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_static_accessor_GLOBAL_12837167ad935116: function() {
            const ret = typeof global === 'undefined' ? null : global;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_GLOBAL_THIS_e628e89ab3b1c95f: function() {
            const ret = typeof globalThis === 'undefined' ? null : globalThis;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_SELF_a621d3dfbb60d0ce: function() {
            const ret = typeof self === 'undefined' ? null : self;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_WINDOW_f8727f0cf888e0bd: function() {
            const ret = typeof window === 'undefined' ? null : window;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_stencilFuncSeparate_10d043d0af14366f: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.stencilFuncSeparate(arg1 >>> 0, arg2 >>> 0, arg3, arg4 >>> 0);
        },
        __wbg_stencilFuncSeparate_1798f5cca257f313: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.stencilFuncSeparate(arg1 >>> 0, arg2 >>> 0, arg3, arg4 >>> 0);
        },
        __wbg_stencilMaskSeparate_28d53625c02d9c7f: function(arg0, arg1, arg2) {
            arg0.stencilMaskSeparate(arg1 >>> 0, arg2 >>> 0);
        },
        __wbg_stencilMaskSeparate_c24c1a28b8dd8a63: function(arg0, arg1, arg2) {
            arg0.stencilMaskSeparate(arg1 >>> 0, arg2 >>> 0);
        },
        __wbg_stencilMask_0eca090c4c47f8f7: function(arg0, arg1) {
            arg0.stencilMask(arg1 >>> 0);
        },
        __wbg_stencilMask_732dcc5aada10e4c: function(arg0, arg1) {
            arg0.stencilMask(arg1 >>> 0);
        },
        __wbg_stencilOpSeparate_4657523b1d3b184f: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.stencilOpSeparate(arg1 >>> 0, arg2 >>> 0, arg3 >>> 0, arg4 >>> 0);
        },
        __wbg_stencilOpSeparate_de257f3c29e604cd: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.stencilOpSeparate(arg1 >>> 0, arg2 >>> 0, arg3 >>> 0, arg4 >>> 0);
        },
        __wbg_submit_f635072bb3d05faa: function(arg0, arg1) {
            arg0.submit(arg1);
        },
        __wbg_texImage2D_087ef94df78081f0: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9) {
            arg0.texImage2D(arg1 >>> 0, arg2, arg3, arg4, arg5, arg6, arg7 >>> 0, arg8 >>> 0, arg9);
        }, arguments); },
        __wbg_texImage2D_e71049312f3172d9: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9) {
            arg0.texImage2D(arg1 >>> 0, arg2, arg3, arg4, arg5, arg6, arg7 >>> 0, arg8 >>> 0, arg9);
        }, arguments); },
        __wbg_texImage3D_bd2b0bd2cfcdb278: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10) {
            arg0.texImage3D(arg1 >>> 0, arg2, arg3, arg4, arg5, arg6, arg7, arg8 >>> 0, arg9 >>> 0, arg10);
        }, arguments); },
        __wbg_texParameteri_0d45be2c88d6bad8: function(arg0, arg1, arg2, arg3) {
            arg0.texParameteri(arg1 >>> 0, arg2 >>> 0, arg3);
        },
        __wbg_texParameteri_ec937d2161018946: function(arg0, arg1, arg2, arg3) {
            arg0.texParameteri(arg1 >>> 0, arg2 >>> 0, arg3);
        },
        __wbg_texStorage2D_9504743abf5a986a: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.texStorage2D(arg1 >>> 0, arg2, arg3 >>> 0, arg4, arg5);
        },
        __wbg_texStorage3D_e9e1b58fee218abe: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.texStorage3D(arg1 >>> 0, arg2, arg3 >>> 0, arg4, arg5, arg6);
        },
        __wbg_texSubImage2D_117d29278542feb0: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9) {
            arg0.texSubImage2D(arg1 >>> 0, arg2, arg3, arg4, arg5, arg6, arg7 >>> 0, arg8 >>> 0, arg9);
        }, arguments); },
        __wbg_texSubImage2D_19ae4cadb809f264: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9) {
            arg0.texSubImage2D(arg1 >>> 0, arg2, arg3, arg4, arg5, arg6, arg7 >>> 0, arg8 >>> 0, arg9);
        }, arguments); },
        __wbg_texSubImage2D_5d270af600a7fc4a: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9) {
            arg0.texSubImage2D(arg1 >>> 0, arg2, arg3, arg4, arg5, arg6, arg7 >>> 0, arg8 >>> 0, arg9);
        }, arguments); },
        __wbg_texSubImage2D_bd034db2e58c352c: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9) {
            arg0.texSubImage2D(arg1 >>> 0, arg2, arg3, arg4, arg5, arg6, arg7 >>> 0, arg8 >>> 0, arg9);
        }, arguments); },
        __wbg_texSubImage2D_bf72e56edeeed376: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9) {
            arg0.texSubImage2D(arg1 >>> 0, arg2, arg3, arg4, arg5, arg6, arg7 >>> 0, arg8 >>> 0, arg9);
        }, arguments); },
        __wbg_texSubImage2D_d17a39cdec4a3495: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9) {
            arg0.texSubImage2D(arg1 >>> 0, arg2, arg3, arg4, arg5, arg6, arg7 >>> 0, arg8 >>> 0, arg9);
        }, arguments); },
        __wbg_texSubImage2D_e193f1d28439217c: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9) {
            arg0.texSubImage2D(arg1 >>> 0, arg2, arg3, arg4, arg5, arg6, arg7 >>> 0, arg8 >>> 0, arg9);
        }, arguments); },
        __wbg_texSubImage2D_edf5bd70fda3feaf: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9) {
            arg0.texSubImage2D(arg1 >>> 0, arg2, arg3, arg4, arg5, arg6, arg7 >>> 0, arg8 >>> 0, arg9);
        }, arguments); },
        __wbg_texSubImage3D_1102c12a20bf56d5: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11) {
            arg0.texSubImage3D(arg1 >>> 0, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9 >>> 0, arg10 >>> 0, arg11);
        }, arguments); },
        __wbg_texSubImage3D_18d7f3c65567c885: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11) {
            arg0.texSubImage3D(arg1 >>> 0, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9 >>> 0, arg10 >>> 0, arg11);
        }, arguments); },
        __wbg_texSubImage3D_3b653017c4c5d721: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11) {
            arg0.texSubImage3D(arg1 >>> 0, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9 >>> 0, arg10 >>> 0, arg11);
        }, arguments); },
        __wbg_texSubImage3D_45591e5655d1ed5c: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11) {
            arg0.texSubImage3D(arg1 >>> 0, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9 >>> 0, arg10 >>> 0, arg11);
        }, arguments); },
        __wbg_texSubImage3D_47643556a8a4bf86: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11) {
            arg0.texSubImage3D(arg1 >>> 0, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9 >>> 0, arg10 >>> 0, arg11);
        }, arguments); },
        __wbg_texSubImage3D_59b8e24fb05787aa: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11) {
            arg0.texSubImage3D(arg1 >>> 0, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9 >>> 0, arg10 >>> 0, arg11);
        }, arguments); },
        __wbg_texSubImage3D_eff5cd6ab84f44ee: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11) {
            arg0.texSubImage3D(arg1 >>> 0, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9 >>> 0, arg10 >>> 0, arg11);
        }, arguments); },
        __wbg_then_0d9fe2c7b1857d32: function(arg0, arg1, arg2) {
            const ret = arg0.then(arg1, arg2);
            return ret;
        },
        __wbg_then_b9e7b3b5f1a9e1b5: function(arg0, arg1) {
            const ret = arg0.then(arg1);
            return ret;
        },
        __wbg_type_c0d5d83032e9858a: function(arg0) {
            const ret = arg0.type;
            return (__wbindgen_enum_GpuCompilationMessageType.indexOf(ret) + 1 || 4) - 1;
        },
        __wbg_uniform1f_b500ede5b612bea2: function(arg0, arg1, arg2) {
            arg0.uniform1f(arg1, arg2);
        },
        __wbg_uniform1f_c148eeaf4b531059: function(arg0, arg1, arg2) {
            arg0.uniform1f(arg1, arg2);
        },
        __wbg_uniform1i_9f3f72dbcb98ada9: function(arg0, arg1, arg2) {
            arg0.uniform1i(arg1, arg2);
        },
        __wbg_uniform1i_e9aee4b9e7fe8c4b: function(arg0, arg1, arg2) {
            arg0.uniform1i(arg1, arg2);
        },
        __wbg_uniform1ui_a0f911ff174715d0: function(arg0, arg1, arg2) {
            arg0.uniform1ui(arg1, arg2 >>> 0);
        },
        __wbg_uniform2fv_04c304b93cbf7f55: function(arg0, arg1, arg2, arg3) {
            arg0.uniform2fv(arg1, getArrayF32FromWasm0(arg2, arg3));
        },
        __wbg_uniform2fv_2fb47cfe06330cc7: function(arg0, arg1, arg2, arg3) {
            arg0.uniform2fv(arg1, getArrayF32FromWasm0(arg2, arg3));
        },
        __wbg_uniform2iv_095baf208f172131: function(arg0, arg1, arg2, arg3) {
            arg0.uniform2iv(arg1, getArrayI32FromWasm0(arg2, arg3));
        },
        __wbg_uniform2iv_ccf2ed44ac8e602e: function(arg0, arg1, arg2, arg3) {
            arg0.uniform2iv(arg1, getArrayI32FromWasm0(arg2, arg3));
        },
        __wbg_uniform2uiv_3030d7e769f5e82a: function(arg0, arg1, arg2, arg3) {
            arg0.uniform2uiv(arg1, getArrayU32FromWasm0(arg2, arg3));
        },
        __wbg_uniform3fv_aa35ef21e14d5469: function(arg0, arg1, arg2, arg3) {
            arg0.uniform3fv(arg1, getArrayF32FromWasm0(arg2, arg3));
        },
        __wbg_uniform3fv_c0872003729939a5: function(arg0, arg1, arg2, arg3) {
            arg0.uniform3fv(arg1, getArrayF32FromWasm0(arg2, arg3));
        },
        __wbg_uniform3iv_6aa2b0791e659d14: function(arg0, arg1, arg2, arg3) {
            arg0.uniform3iv(arg1, getArrayI32FromWasm0(arg2, arg3));
        },
        __wbg_uniform3iv_e912f444d4ff8269: function(arg0, arg1, arg2, arg3) {
            arg0.uniform3iv(arg1, getArrayI32FromWasm0(arg2, arg3));
        },
        __wbg_uniform3uiv_86941e7eeb8ee0a3: function(arg0, arg1, arg2, arg3) {
            arg0.uniform3uiv(arg1, getArrayU32FromWasm0(arg2, arg3));
        },
        __wbg_uniform4f_71ec75443e58cecc: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.uniform4f(arg1, arg2, arg3, arg4, arg5);
        },
        __wbg_uniform4f_f6b5e2024636033a: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.uniform4f(arg1, arg2, arg3, arg4, arg5);
        },
        __wbg_uniform4fv_498bd80dc5aa16ff: function(arg0, arg1, arg2, arg3) {
            arg0.uniform4fv(arg1, getArrayF32FromWasm0(arg2, arg3));
        },
        __wbg_uniform4fv_e6c73702e9a3be5c: function(arg0, arg1, arg2, arg3) {
            arg0.uniform4fv(arg1, getArrayF32FromWasm0(arg2, arg3));
        },
        __wbg_uniform4iv_375332584c65e61b: function(arg0, arg1, arg2, arg3) {
            arg0.uniform4iv(arg1, getArrayI32FromWasm0(arg2, arg3));
        },
        __wbg_uniform4iv_8a8219fda39dffd5: function(arg0, arg1, arg2, arg3) {
            arg0.uniform4iv(arg1, getArrayI32FromWasm0(arg2, arg3));
        },
        __wbg_uniform4uiv_046ee400bb80547d: function(arg0, arg1, arg2, arg3) {
            arg0.uniform4uiv(arg1, getArrayU32FromWasm0(arg2, arg3));
        },
        __wbg_uniformBlockBinding_1cf9fd2c49adf0f3: function(arg0, arg1, arg2, arg3) {
            arg0.uniformBlockBinding(arg1, arg2 >>> 0, arg3 >>> 0);
        },
        __wbg_uniformMatrix2fv_24430076c7afb5e3: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.uniformMatrix2fv(arg1, arg2 !== 0, getArrayF32FromWasm0(arg3, arg4));
        },
        __wbg_uniformMatrix2fv_e2806601f5b95102: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.uniformMatrix2fv(arg1, arg2 !== 0, getArrayF32FromWasm0(arg3, arg4));
        },
        __wbg_uniformMatrix2x3fv_a377326104a8faf4: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.uniformMatrix2x3fv(arg1, arg2 !== 0, getArrayF32FromWasm0(arg3, arg4));
        },
        __wbg_uniformMatrix2x4fv_b7a4d810e7a1cf7d: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.uniformMatrix2x4fv(arg1, arg2 !== 0, getArrayF32FromWasm0(arg3, arg4));
        },
        __wbg_uniformMatrix3fv_6f822361173d8046: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.uniformMatrix3fv(arg1, arg2 !== 0, getArrayF32FromWasm0(arg3, arg4));
        },
        __wbg_uniformMatrix3fv_b94a764c63aa6468: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.uniformMatrix3fv(arg1, arg2 !== 0, getArrayF32FromWasm0(arg3, arg4));
        },
        __wbg_uniformMatrix3x2fv_69a4cf0ce5b09f8b: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.uniformMatrix3x2fv(arg1, arg2 !== 0, getArrayF32FromWasm0(arg3, arg4));
        },
        __wbg_uniformMatrix3x4fv_cc72e31a1baaf9c9: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.uniformMatrix3x4fv(arg1, arg2 !== 0, getArrayF32FromWasm0(arg3, arg4));
        },
        __wbg_uniformMatrix4fv_0e724dbebd372526: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.uniformMatrix4fv(arg1, arg2 !== 0, getArrayF32FromWasm0(arg3, arg4));
        },
        __wbg_uniformMatrix4fv_923b55ad503fdc56: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.uniformMatrix4fv(arg1, arg2 !== 0, getArrayF32FromWasm0(arg3, arg4));
        },
        __wbg_uniformMatrix4x2fv_8c9fb646f3b90b63: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.uniformMatrix4x2fv(arg1, arg2 !== 0, getArrayF32FromWasm0(arg3, arg4));
        },
        __wbg_uniformMatrix4x3fv_ee0bed9a1330400d: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.uniformMatrix4x3fv(arg1, arg2 !== 0, getArrayF32FromWasm0(arg3, arg4));
        },
        __wbg_unmap_8c2e8131b2aaa844: function(arg0) {
            arg0.unmap();
        },
        __wbg_usage_13caa02888040e9f: function(arg0) {
            const ret = arg0.usage;
            return ret;
        },
        __wbg_useProgram_e82c1a5f87d81579: function(arg0, arg1) {
            arg0.useProgram(arg1);
        },
        __wbg_useProgram_fe720ade4d3b6edb: function(arg0, arg1) {
            arg0.useProgram(arg1);
        },
        __wbg_valueOf_3c28600026e653c4: function(arg0) {
            const ret = arg0.valueOf();
            return ret;
        },
        __wbg_value_0546255b415e96c1: function(arg0) {
            const ret = arg0.value;
            return ret;
        },
        __wbg_vertexAttribDivisorANGLE_eaa3c29423ea6da4: function(arg0, arg1, arg2) {
            arg0.vertexAttribDivisorANGLE(arg1 >>> 0, arg2 >>> 0);
        },
        __wbg_vertexAttribDivisor_744c0ca468594894: function(arg0, arg1, arg2) {
            arg0.vertexAttribDivisor(arg1 >>> 0, arg2 >>> 0);
        },
        __wbg_vertexAttribIPointer_b9020d0c2e759912: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.vertexAttribIPointer(arg1 >>> 0, arg2, arg3 >>> 0, arg4, arg5);
        },
        __wbg_vertexAttribPointer_75f6ff47f6c9f8cb: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.vertexAttribPointer(arg1 >>> 0, arg2, arg3 >>> 0, arg4 !== 0, arg5, arg6);
        },
        __wbg_vertexAttribPointer_adbd1853cce679ad: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.vertexAttribPointer(arg1 >>> 0, arg2, arg3 >>> 0, arg4 !== 0, arg5, arg6);
        },
        __wbg_videoHeight_a90b6b6ebd4132de: function(arg0) {
            const ret = arg0.videoHeight;
            return ret;
        },
        __wbg_videoWidth_4b450aa64c85eaa4: function(arg0) {
            const ret = arg0.videoWidth;
            return ret;
        },
        __wbg_viewport_174ae1c2209344ae: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.viewport(arg1, arg2, arg3, arg4);
        },
        __wbg_viewport_df236eac68bc7467: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.viewport(arg1, arg2, arg3, arg4);
        },
        __wbg_warn_f7ae1b2e66ccb930: function(arg0) {
            console.warn(arg0);
        },
        __wbg_width_5901d980713eb80b: function(arg0) {
            const ret = arg0.width;
            return ret;
        },
        __wbg_width_5f66bde2e810fbde: function(arg0) {
            const ret = arg0.width;
            return ret;
        },
        __wbg_width_75158459c067906d: function(arg0) {
            const ret = arg0.width;
            return ret;
        },
        __wbg_width_be8f36d66d37751f: function(arg0) {
            const ret = arg0.width;
            return ret;
        },
        __wbg_width_f12394c19964e4bb: function(arg0) {
            const ret = arg0.width;
            return ret;
        },
        __wbg_writeBuffer_5ca4981365eb5ac0: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.writeBuffer(arg1, arg2, arg3, arg4, arg5);
        },
        __wbg_writeTexture_246118eb2f5a1592: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.writeTexture(arg1, arg2, arg3, arg4);
        },
        __wbindgen_cast_0000000000000001: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { dtor_idx: 1294, function: Function { arguments: [NamedExternref("GPUUncapturedErrorEvent")], shim_idx: 1295, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm.wasm_bindgen__closure__destroy__h29172bbcd065953b, wasm_bindgen__convert__closures_____invoke__h409646c44a6f7cf4);
            return ret;
        },
        __wbindgen_cast_0000000000000002: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { dtor_idx: 2079, function: Function { arguments: [Externref], shim_idx: 2080, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm.wasm_bindgen__closure__destroy__h6ed7eb1128abde65, wasm_bindgen__convert__closures_____invoke__h53e1d5f5246c70f9);
            return ret;
        },
        __wbindgen_cast_0000000000000003: function(arg0) {
            // Cast intrinsic for `F64 -> Externref`.
            const ret = arg0;
            return ret;
        },
        __wbindgen_cast_0000000000000004: function(arg0) {
            // Cast intrinsic for `I64 -> Externref`.
            const ret = arg0;
            return ret;
        },
        __wbindgen_cast_0000000000000005: function(arg0, arg1) {
            // Cast intrinsic for `Ref(Slice(F32)) -> NamedExternref("Float32Array")`.
            const ret = getArrayF32FromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_cast_0000000000000006: function(arg0, arg1) {
            // Cast intrinsic for `Ref(Slice(I16)) -> NamedExternref("Int16Array")`.
            const ret = getArrayI16FromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_cast_0000000000000007: function(arg0, arg1) {
            // Cast intrinsic for `Ref(Slice(I32)) -> NamedExternref("Int32Array")`.
            const ret = getArrayI32FromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_cast_0000000000000008: function(arg0, arg1) {
            // Cast intrinsic for `Ref(Slice(I8)) -> NamedExternref("Int8Array")`.
            const ret = getArrayI8FromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_cast_0000000000000009: function(arg0, arg1) {
            // Cast intrinsic for `Ref(Slice(U16)) -> NamedExternref("Uint16Array")`.
            const ret = getArrayU16FromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_cast_000000000000000a: function(arg0, arg1) {
            // Cast intrinsic for `Ref(Slice(U32)) -> NamedExternref("Uint32Array")`.
            const ret = getArrayU32FromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_cast_000000000000000b: function(arg0, arg1) {
            // Cast intrinsic for `Ref(Slice(U8)) -> NamedExternref("Uint8Array")`.
            const ret = getArrayU8FromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_cast_000000000000000c: function(arg0, arg1) {
            // Cast intrinsic for `Ref(String) -> Externref`.
            const ret = getStringFromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_cast_000000000000000d: function(arg0) {
            // Cast intrinsic for `U64 -> Externref`.
            const ret = BigInt.asUintN(64, arg0);
            return ret;
        },
        __wbindgen_cast_000000000000000e: function(arg0, arg1) {
            var v0 = getArrayF32FromWasm0(arg0, arg1).slice();
            wasm.__wbindgen_free(arg0, arg1 * 4, 4);
            // Cast intrinsic for `Vector(F32) -> Externref`.
            const ret = v0;
            return ret;
        },
        __wbindgen_cast_000000000000000f: function(arg0, arg1) {
            var v0 = getArrayU8FromWasm0(arg0, arg1).slice();
            wasm.__wbindgen_free(arg0, arg1 * 1, 1);
            // Cast intrinsic for `Vector(U8) -> Externref`.
            const ret = v0;
            return ret;
        },
        __wbindgen_init_externref_table: function() {
            const table = wasm.__wbindgen_externrefs;
            const offset = table.grow(4);
            table.set(0, undefined);
            table.set(offset + 0, undefined);
            table.set(offset + 1, null);
            table.set(offset + 2, true);
            table.set(offset + 3, false);
        },
    };
    return {
        __proto__: null,
        "./vcad_kernel_wasm_bg.js": import0,
    };
}

function wasm_bindgen__convert__closures_____invoke__h409646c44a6f7cf4(arg0, arg1, arg2) {
    wasm.wasm_bindgen__convert__closures_____invoke__h409646c44a6f7cf4(arg0, arg1, arg2);
}

function wasm_bindgen__convert__closures_____invoke__h53e1d5f5246c70f9(arg0, arg1, arg2) {
    wasm.wasm_bindgen__convert__closures_____invoke__h53e1d5f5246c70f9(arg0, arg1, arg2);
}

function wasm_bindgen__convert__closures_____invoke__h61f848611b9bfd22(arg0, arg1, arg2, arg3) {
    wasm.wasm_bindgen__convert__closures_____invoke__h61f848611b9bfd22(arg0, arg1, arg2, arg3);
}


const __wbindgen_enum_GpuCompilationMessageType = ["error", "warning", "info"];


const __wbindgen_enum_GpuDeviceLostReason = ["unknown", "destroyed"];


const __wbindgen_enum_GpuErrorFilter = ["validation", "out-of-memory", "internal"];


const __wbindgen_enum_GpuIndexFormat = ["uint16", "uint32"];


const __wbindgen_enum_GpuTextureFormat = ["r8unorm", "r8snorm", "r8uint", "r8sint", "r16uint", "r16sint", "r16float", "rg8unorm", "rg8snorm", "rg8uint", "rg8sint", "r32uint", "r32sint", "r32float", "rg16uint", "rg16sint", "rg16float", "rgba8unorm", "rgba8unorm-srgb", "rgba8snorm", "rgba8uint", "rgba8sint", "bgra8unorm", "bgra8unorm-srgb", "rgb9e5ufloat", "rgb10a2uint", "rgb10a2unorm", "rg11b10ufloat", "rg32uint", "rg32sint", "rg32float", "rgba16uint", "rgba16sint", "rgba16float", "rgba32uint", "rgba32sint", "rgba32float", "stencil8", "depth16unorm", "depth24plus", "depth24plus-stencil8", "depth32float", "depth32float-stencil8", "bc1-rgba-unorm", "bc1-rgba-unorm-srgb", "bc2-rgba-unorm", "bc2-rgba-unorm-srgb", "bc3-rgba-unorm", "bc3-rgba-unorm-srgb", "bc4-r-unorm", "bc4-r-snorm", "bc5-rg-unorm", "bc5-rg-snorm", "bc6h-rgb-ufloat", "bc6h-rgb-float", "bc7-rgba-unorm", "bc7-rgba-unorm-srgb", "etc2-rgb8unorm", "etc2-rgb8unorm-srgb", "etc2-rgb8a1unorm", "etc2-rgb8a1unorm-srgb", "etc2-rgba8unorm", "etc2-rgba8unorm-srgb", "eac-r11unorm", "eac-r11snorm", "eac-rg11unorm", "eac-rg11snorm", "astc-4x4-unorm", "astc-4x4-unorm-srgb", "astc-5x4-unorm", "astc-5x4-unorm-srgb", "astc-5x5-unorm", "astc-5x5-unorm-srgb", "astc-6x5-unorm", "astc-6x5-unorm-srgb", "astc-6x6-unorm", "astc-6x6-unorm-srgb", "astc-8x5-unorm", "astc-8x5-unorm-srgb", "astc-8x6-unorm", "astc-8x6-unorm-srgb", "astc-8x8-unorm", "astc-8x8-unorm-srgb", "astc-10x5-unorm", "astc-10x5-unorm-srgb", "astc-10x6-unorm", "astc-10x6-unorm-srgb", "astc-10x8-unorm", "astc-10x8-unorm-srgb", "astc-10x10-unorm", "astc-10x10-unorm-srgb", "astc-12x10-unorm", "astc-12x10-unorm-srgb", "astc-12x12-unorm", "astc-12x12-unorm-srgb"];
const PhysicsSimFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_physicssim_free(ptr >>> 0, 1));
const RayTracerFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_raytracer_free(ptr >>> 0, 1));
const SliceResultFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_sliceresult_free(ptr >>> 0, 1));
const SlicerSettingsFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_slicersettings_free(ptr >>> 0, 1));
const SolidFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_solid_free(ptr >>> 0, 1));
const WasmAnnotationLayerFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_wasmannotationlayer_free(ptr >>> 0, 1));
const WasmCamSettingsFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_wasmcamsettings_free(ptr >>> 0, 1));
const WasmDocumentEngineFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_wasmdocumentengine_free(ptr >>> 0, 1));
const WasmKeybindingsFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_wasmkeybindings_free(ptr >>> 0, 1));
const WasmSketchSessionFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_wasmsketchsession_free(ptr >>> 0, 1));

function addToExternrefTable0(obj) {
    const idx = wasm.__externref_table_alloc();
    wasm.__wbindgen_externrefs.set(idx, obj);
    return idx;
}

function _assertClass(instance, klass) {
    if (!(instance instanceof klass)) {
        throw new Error(`expected instance of ${klass.name}`);
    }
}

const CLOSURE_DTORS = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(state => state.dtor(state.a, state.b));

function debugString(val) {
    // primitive types
    const type = typeof val;
    if (type == 'number' || type == 'boolean' || val == null) {
        return  `${val}`;
    }
    if (type == 'string') {
        return `"${val}"`;
    }
    if (type == 'symbol') {
        const description = val.description;
        if (description == null) {
            return 'Symbol';
        } else {
            return `Symbol(${description})`;
        }
    }
    if (type == 'function') {
        const name = val.name;
        if (typeof name == 'string' && name.length > 0) {
            return `Function(${name})`;
        } else {
            return 'Function';
        }
    }
    // objects
    if (Array.isArray(val)) {
        const length = val.length;
        let debug = '[';
        if (length > 0) {
            debug += debugString(val[0]);
        }
        for(let i = 1; i < length; i++) {
            debug += ', ' + debugString(val[i]);
        }
        debug += ']';
        return debug;
    }
    // Test for built-in
    const builtInMatches = /\[object ([^\]]+)\]/.exec(toString.call(val));
    let className;
    if (builtInMatches && builtInMatches.length > 1) {
        className = builtInMatches[1];
    } else {
        // Failed to match the standard '[object ClassName]'
        return toString.call(val);
    }
    if (className == 'Object') {
        // we're a user defined class or Object
        // JSON.stringify avoids problems with cycles, and is generally much
        // easier than looping through ownProperties of `val`.
        try {
            return 'Object(' + JSON.stringify(val) + ')';
        } catch (_) {
            return 'Object';
        }
    }
    // errors
    if (val instanceof Error) {
        return `${val.name}: ${val.message}\n${val.stack}`;
    }
    // TODO we could test for more things here, like `Set`s and `Map`s.
    return className;
}

function getArrayF32FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getFloat32ArrayMemory0().subarray(ptr / 4, ptr / 4 + len);
}

function getArrayF64FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getFloat64ArrayMemory0().subarray(ptr / 8, ptr / 8 + len);
}

function getArrayI16FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getInt16ArrayMemory0().subarray(ptr / 2, ptr / 2 + len);
}

function getArrayI32FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getInt32ArrayMemory0().subarray(ptr / 4, ptr / 4 + len);
}

function getArrayI8FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getInt8ArrayMemory0().subarray(ptr / 1, ptr / 1 + len);
}

function getArrayU16FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getUint16ArrayMemory0().subarray(ptr / 2, ptr / 2 + len);
}

function getArrayU32FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getUint32ArrayMemory0().subarray(ptr / 4, ptr / 4 + len);
}

function getArrayU8FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getUint8ArrayMemory0().subarray(ptr / 1, ptr / 1 + len);
}

let cachedDataViewMemory0 = null;
function getDataViewMemory0() {
    if (cachedDataViewMemory0 === null || cachedDataViewMemory0.buffer.detached === true || (cachedDataViewMemory0.buffer.detached === undefined && cachedDataViewMemory0.buffer !== wasm.memory.buffer)) {
        cachedDataViewMemory0 = new DataView(wasm.memory.buffer);
    }
    return cachedDataViewMemory0;
}

let cachedFloat32ArrayMemory0 = null;
function getFloat32ArrayMemory0() {
    if (cachedFloat32ArrayMemory0 === null || cachedFloat32ArrayMemory0.byteLength === 0) {
        cachedFloat32ArrayMemory0 = new Float32Array(wasm.memory.buffer);
    }
    return cachedFloat32ArrayMemory0;
}

let cachedFloat64ArrayMemory0 = null;
function getFloat64ArrayMemory0() {
    if (cachedFloat64ArrayMemory0 === null || cachedFloat64ArrayMemory0.byteLength === 0) {
        cachedFloat64ArrayMemory0 = new Float64Array(wasm.memory.buffer);
    }
    return cachedFloat64ArrayMemory0;
}

let cachedInt16ArrayMemory0 = null;
function getInt16ArrayMemory0() {
    if (cachedInt16ArrayMemory0 === null || cachedInt16ArrayMemory0.byteLength === 0) {
        cachedInt16ArrayMemory0 = new Int16Array(wasm.memory.buffer);
    }
    return cachedInt16ArrayMemory0;
}

let cachedInt32ArrayMemory0 = null;
function getInt32ArrayMemory0() {
    if (cachedInt32ArrayMemory0 === null || cachedInt32ArrayMemory0.byteLength === 0) {
        cachedInt32ArrayMemory0 = new Int32Array(wasm.memory.buffer);
    }
    return cachedInt32ArrayMemory0;
}

let cachedInt8ArrayMemory0 = null;
function getInt8ArrayMemory0() {
    if (cachedInt8ArrayMemory0 === null || cachedInt8ArrayMemory0.byteLength === 0) {
        cachedInt8ArrayMemory0 = new Int8Array(wasm.memory.buffer);
    }
    return cachedInt8ArrayMemory0;
}

function getStringFromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return decodeText(ptr, len);
}

let cachedUint16ArrayMemory0 = null;
function getUint16ArrayMemory0() {
    if (cachedUint16ArrayMemory0 === null || cachedUint16ArrayMemory0.byteLength === 0) {
        cachedUint16ArrayMemory0 = new Uint16Array(wasm.memory.buffer);
    }
    return cachedUint16ArrayMemory0;
}

let cachedUint32ArrayMemory0 = null;
function getUint32ArrayMemory0() {
    if (cachedUint32ArrayMemory0 === null || cachedUint32ArrayMemory0.byteLength === 0) {
        cachedUint32ArrayMemory0 = new Uint32Array(wasm.memory.buffer);
    }
    return cachedUint32ArrayMemory0;
}

let cachedUint8ArrayMemory0 = null;
function getUint8ArrayMemory0() {
    if (cachedUint8ArrayMemory0 === null || cachedUint8ArrayMemory0.byteLength === 0) {
        cachedUint8ArrayMemory0 = new Uint8Array(wasm.memory.buffer);
    }
    return cachedUint8ArrayMemory0;
}

function handleError(f, args) {
    try {
        return f.apply(this, args);
    } catch (e) {
        const idx = addToExternrefTable0(e);
        wasm.__wbindgen_exn_store(idx);
    }
}

function isLikeNone(x) {
    return x === undefined || x === null;
}

function makeMutClosure(arg0, arg1, dtor, f) {
    const state = { a: arg0, b: arg1, cnt: 1, dtor };
    const real = (...args) => {

        // First up with a closure we increment the internal reference
        // count. This ensures that the Rust closure environment won't
        // be deallocated while we're invoking it.
        state.cnt++;
        const a = state.a;
        state.a = 0;
        try {
            return f(a, state.b, ...args);
        } finally {
            state.a = a;
            real._wbg_cb_unref();
        }
    };
    real._wbg_cb_unref = () => {
        if (--state.cnt === 0) {
            state.dtor(state.a, state.b);
            state.a = 0;
            CLOSURE_DTORS.unregister(state);
        }
    };
    CLOSURE_DTORS.register(real, state, state);
    return real;
}

function passArray32ToWasm0(arg, malloc) {
    const ptr = malloc(arg.length * 4, 4) >>> 0;
    getUint32ArrayMemory0().set(arg, ptr / 4);
    WASM_VECTOR_LEN = arg.length;
    return ptr;
}

function passArray8ToWasm0(arg, malloc) {
    const ptr = malloc(arg.length * 1, 1) >>> 0;
    getUint8ArrayMemory0().set(arg, ptr / 1);
    WASM_VECTOR_LEN = arg.length;
    return ptr;
}

function passArrayF32ToWasm0(arg, malloc) {
    const ptr = malloc(arg.length * 4, 4) >>> 0;
    getFloat32ArrayMemory0().set(arg, ptr / 4);
    WASM_VECTOR_LEN = arg.length;
    return ptr;
}

function passArrayF64ToWasm0(arg, malloc) {
    const ptr = malloc(arg.length * 8, 8) >>> 0;
    getFloat64ArrayMemory0().set(arg, ptr / 8);
    WASM_VECTOR_LEN = arg.length;
    return ptr;
}

function passArrayJsValueToWasm0(array, malloc) {
    const ptr = malloc(array.length * 4, 4) >>> 0;
    for (let i = 0; i < array.length; i++) {
        const add = addToExternrefTable0(array[i]);
        getDataViewMemory0().setUint32(ptr + 4 * i, add, true);
    }
    WASM_VECTOR_LEN = array.length;
    return ptr;
}

function passStringToWasm0(arg, malloc, realloc) {
    if (realloc === undefined) {
        const buf = cachedTextEncoder.encode(arg);
        const ptr = malloc(buf.length, 1) >>> 0;
        getUint8ArrayMemory0().subarray(ptr, ptr + buf.length).set(buf);
        WASM_VECTOR_LEN = buf.length;
        return ptr;
    }

    let len = arg.length;
    let ptr = malloc(len, 1) >>> 0;

    const mem = getUint8ArrayMemory0();

    let offset = 0;

    for (; offset < len; offset++) {
        const code = arg.charCodeAt(offset);
        if (code > 0x7F) break;
        mem[ptr + offset] = code;
    }
    if (offset !== len) {
        if (offset !== 0) {
            arg = arg.slice(offset);
        }
        ptr = realloc(ptr, len, len = offset + arg.length * 3, 1) >>> 0;
        const view = getUint8ArrayMemory0().subarray(ptr + offset, ptr + len);
        const ret = cachedTextEncoder.encodeInto(arg, view);

        offset += ret.written;
        ptr = realloc(ptr, len, offset, 1) >>> 0;
    }

    WASM_VECTOR_LEN = offset;
    return ptr;
}

function takeFromExternrefTable0(idx) {
    const value = wasm.__wbindgen_externrefs.get(idx);
    wasm.__externref_table_dealloc(idx);
    return value;
}

let cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
cachedTextDecoder.decode();
const MAX_SAFARI_DECODE_BYTES = 2146435072;
let numBytesDecoded = 0;
function decodeText(ptr, len) {
    numBytesDecoded += len;
    if (numBytesDecoded >= MAX_SAFARI_DECODE_BYTES) {
        cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
        cachedTextDecoder.decode();
        numBytesDecoded = len;
    }
    return cachedTextDecoder.decode(getUint8ArrayMemory0().subarray(ptr, ptr + len));
}

const cachedTextEncoder = new TextEncoder();

if (!('encodeInto' in cachedTextEncoder)) {
    cachedTextEncoder.encodeInto = function (arg, view) {
        const buf = cachedTextEncoder.encode(arg);
        view.set(buf);
        return {
            read: arg.length,
            written: buf.length
        };
    };
}

let WASM_VECTOR_LEN = 0;

let wasmModule, wasm;
function __wbg_finalize_init(instance, module) {
    wasm = instance.exports;
    wasmModule = module;
    cachedDataViewMemory0 = null;
    cachedFloat32ArrayMemory0 = null;
    cachedFloat64ArrayMemory0 = null;
    cachedInt16ArrayMemory0 = null;
    cachedInt32ArrayMemory0 = null;
    cachedInt8ArrayMemory0 = null;
    cachedUint16ArrayMemory0 = null;
    cachedUint32ArrayMemory0 = null;
    cachedUint8ArrayMemory0 = null;
    wasm.__wbindgen_start();
    return wasm;
}

async function __wbg_load(module, imports) {
    if (typeof Response === 'function' && module instanceof Response) {
        if (typeof WebAssembly.instantiateStreaming === 'function') {
            try {
                return await WebAssembly.instantiateStreaming(module, imports);
            } catch (e) {
                const validResponse = module.ok && expectedResponseType(module.type);

                if (validResponse && module.headers.get('Content-Type') !== 'application/wasm') {
                    console.warn("`WebAssembly.instantiateStreaming` failed because your server does not serve Wasm with `application/wasm` MIME type. Falling back to `WebAssembly.instantiate` which is slower. Original error:\n", e);

                } else { throw e; }
            }
        }

        const bytes = await module.arrayBuffer();
        return await WebAssembly.instantiate(bytes, imports);
    } else {
        const instance = await WebAssembly.instantiate(module, imports);

        if (instance instanceof WebAssembly.Instance) {
            return { instance, module };
        } else {
            return instance;
        }
    }

    function expectedResponseType(type) {
        switch (type) {
            case 'basic': case 'cors': case 'default': return true;
        }
        return false;
    }
}

function initSync(module) {
    if (wasm !== undefined) return wasm;


    if (module !== undefined) {
        if (Object.getPrototypeOf(module) === Object.prototype) {
            ({module} = module)
        } else {
            console.warn('using deprecated parameters for `initSync()`; pass a single object instead')
        }
    }

    const imports = __wbg_get_imports();
    if (!(module instanceof WebAssembly.Module)) {
        module = new WebAssembly.Module(module);
    }
    const instance = new WebAssembly.Instance(module, imports);
    return __wbg_finalize_init(instance, module);
}

async function __wbg_init(module_or_path) {
    if (wasm !== undefined) return wasm;


    if (module_or_path !== undefined) {
        if (Object.getPrototypeOf(module_or_path) === Object.prototype) {
            ({module_or_path} = module_or_path)
        } else {
            console.warn('using deprecated parameters for the initialization function; pass a single object instead')
        }
    }

    if (module_or_path === undefined) {
        module_or_path = new URL('vcad_kernel_wasm_bg.wasm', import.meta.url);
    }
    const imports = __wbg_get_imports();

    if (typeof module_or_path === 'string' || (typeof Request === 'function' && module_or_path instanceof Request) || (typeof URL === 'function' && module_or_path instanceof URL)) {
        module_or_path = fetch(module_or_path);
    }

    const { instance, module } = await __wbg_load(await module_or_path, imports);

    return __wbg_finalize_init(instance, module);
}

export { initSync, __wbg_init as default };

/**
 * Auto-generated type definitions by wasmosis
 * Do not edit manually
 */

import type { WasmExports, WasmModule, Registry } from 'wasmosis';

export interface CoreExports extends WasmExports {
  __wbg_physicssim_free: (...args: unknown[]) => unknown;
  __wbg_raytracer_free: (...args: unknown[]) => unknown;
  __wbg_solid_free: (...args: unknown[]) => unknown;
  __wbg_wasmannotationlayer_free: (...args: unknown[]) => unknown;
  get_kernel_version: (...args: unknown[]) => unknown;
  init: (...args: unknown[]) => unknown;
  physicssim_actionDim: (...args: unknown[]) => unknown;
  physicssim_new: (...args: unknown[]) => unknown;
  physicssim_observationDim: (...args: unknown[]) => unknown;
  physicssim_observe: (...args: unknown[]) => unknown;
  physicssim_reset: (...args: unknown[]) => unknown;
  physicssim_setMaxSteps: (...args: unknown[]) => unknown;
  physicssim_setSeed: (...args: unknown[]) => unknown;
  physicssim_stepPosition: (...args: unknown[]) => unknown;
  physicssim_stepTorque: (...args: unknown[]) => unknown;
  physicssim_stepVelocity: (...args: unknown[]) => unknown;
  raytracer_canRaytrace: (...args: unknown[]) => unknown;
  raytracer_create: (...args: unknown[]) => unknown;
  raytracer_getDebugMode: (...args: unknown[]) => unknown;
  raytracer_getEdgeDetectionEnabled: (...args: unknown[]) => unknown;
  raytracer_getFrameIndex: (...args: unknown[]) => unknown;
  raytracer_hasScene: (...args: unknown[]) => unknown;
  raytracer_pick: (...args: unknown[]) => unknown;
  raytracer_render: (...args: unknown[]) => unknown;
  raytracer_resetAccumulation: (...args: unknown[]) => unknown;
  raytracer_setDebugMode: (...args: unknown[]) => unknown;
  raytracer_setEdgeDetection: (...args: unknown[]) => unknown;
  raytracer_setMaterial: (...args: unknown[]) => unknown;
  raytracer_uploadSolid: (...args: unknown[]) => unknown;
  solid_boundingBox: (...args: unknown[]) => unknown;
  solid_centerOfMass: (...args: unknown[]) => unknown;
  solid_cone: (...args: unknown[]) => unknown;
  solid_cube: (...args: unknown[]) => unknown;
  solid_cylinder: (...args: unknown[]) => unknown;
  solid_difference: (...args: unknown[]) => unknown;
  solid_empty: (...args: unknown[]) => unknown;
  solid_extrude: (...args: unknown[]) => unknown;
  solid_extrudeWithOptions: (...args: unknown[]) => unknown;
  solid_getMesh: (...args: unknown[]) => unknown;
  solid_horizontalSection: (...args: unknown[]) => unknown;
  solid_intersection: (...args: unknown[]) => unknown;
  solid_isEmpty: (...args: unknown[]) => unknown;
  solid_loft: (...args: unknown[]) => unknown;
  solid_numTriangles: (...args: unknown[]) => unknown;
  solid_projectView: (...args: unknown[]) => unknown;
  solid_rotate: (...args: unknown[]) => unknown;
  solid_scale: (...args: unknown[]) => unknown;
  solid_sectionView: (...args: unknown[]) => unknown;
  solid_sphere: (...args: unknown[]) => unknown;
  solid_surfaceArea: (...args: unknown[]) => unknown;
  solid_sweepHelix: (...args: unknown[]) => unknown;
  solid_sweepLine: (...args: unknown[]) => unknown;
  solid_textExtrude: (...args: unknown[]) => unknown;
  solid_toStepBuffer: (...args: unknown[]) => unknown;
  solid_translate: (...args: unknown[]) => unknown;
  solid_union: (...args: unknown[]) => unknown;
  solid_volume: (...args: unknown[]) => unknown;
  textBounds: (...args: unknown[]) => unknown;
  wasmannotationlayer_addAlignedDimension: (...args: unknown[]) => unknown;
  wasmannotationlayer_addAngleDimension: (...args: unknown[]) => unknown;
  wasmannotationlayer_addDiameterDimension: (...args: unknown[]) => unknown;
  wasmannotationlayer_addHorizontalDimension: (...args: unknown[]) => unknown;
  wasmannotationlayer_addRadiusDimension: (...args: unknown[]) => unknown;
  wasmannotationlayer_addVerticalDimension: (...args: unknown[]) => unknown;
  wasmannotationlayer_annotationCount: (...args: unknown[]) => unknown;
  wasmannotationlayer_clear: (...args: unknown[]) => unknown;
  wasmannotationlayer_isEmpty: (...args: unknown[]) => unknown;
  wasmannotationlayer_new: (...args: unknown[]) => unknown;
  wasmannotationlayer_renderAll: (...args: unknown[]) => unknown;
  physicssim_numJoints: (...args: unknown[]) => unknown;
  solid_linearPattern: (...args: unknown[]) => unknown;
  solid_revolve: (...args: unknown[]) => unknown;
  solid_canExportStep: (...args: unknown[]) => unknown;
  solid_chamfer: (...args: unknown[]) => unknown;
  solid_fillet: (...args: unknown[]) => unknown;
  solid_shell: (...args: unknown[]) => unknown;
  solid_circularPattern: (...args: unknown[]) => unknown;
  __wbg_get_wasmcamsettings_feed_rate: (...args: unknown[]) => unknown;
  __wbg_get_wasmcamsettings_plunge_rate: (...args: unknown[]) => unknown;
  __wbg_get_wasmcamsettings_retract_z: (...args: unknown[]) => unknown;
  __wbg_get_wasmcamsettings_safe_z: (...args: unknown[]) => unknown;
  __wbg_get_wasmcamsettings_spindle_rpm: (...args: unknown[]) => unknown;
  __wbg_get_wasmcamsettings_stepdown: (...args: unknown[]) => unknown;
  __wbg_get_wasmcamsettings_stepover: (...args: unknown[]) => unknown;
  __wbg_set_wasmcamsettings_feed_rate: (...args: unknown[]) => unknown;
  __wbg_set_wasmcamsettings_plunge_rate: (...args: unknown[]) => unknown;
  __wbg_set_wasmcamsettings_retract_z: (...args: unknown[]) => unknown;
  __wbg_set_wasmcamsettings_safe_z: (...args: unknown[]) => unknown;
  __wbg_set_wasmcamsettings_spindle_rpm: (...args: unknown[]) => unknown;
  __wbg_set_wasmcamsettings_stepdown: (...args: unknown[]) => unknown;
  __wbg_set_wasmcamsettings_stepover: (...args: unknown[]) => unknown;
  __wbg_wasmcamsettings_free: (...args: unknown[]) => unknown;
  camDropCutter: (...args: unknown[]) => unknown;
  camExportGcode: (...args: unknown[]) => unknown;
  camExportLinuxCnc: (...args: unknown[]) => unknown;
  camGenerateCircularPocket: (...args: unknown[]) => unknown;
  camGenerateContour: (...args: unknown[]) => unknown;
  camGenerateFace: (...args: unknown[]) => unknown;
  camGeneratePocket: (...args: unknown[]) => unknown;
  camGenerateRoughing3d: (...args: unknown[]) => unknown;
  camGetDefaultTools: (...args: unknown[]) => unknown;
  camToolpathStats: (...args: unknown[]) => unknown;
  isCamAvailable: (...args: unknown[]) => unknown;
  wasmcamsettings_fromJson: (...args: unknown[]) => unknown;
  wasmcamsettings_new: (...args: unknown[]) => unknown;
  __wbg_get_slicersettings_first_layer_height: (...args: unknown[]) => unknown;
  __wbg_get_slicersettings_infill_density: (...args: unknown[]) => unknown;
  __wbg_get_slicersettings_infill_pattern: (...args: unknown[]) => unknown;
  __wbg_get_slicersettings_layer_height: (...args: unknown[]) => unknown;
  __wbg_get_slicersettings_line_width: (...args: unknown[]) => unknown;
  __wbg_get_slicersettings_nozzle_diameter: (...args: unknown[]) => unknown;
  __wbg_get_slicersettings_support_angle: (...args: unknown[]) => unknown;
  __wbg_get_slicersettings_support_enabled: (...args: unknown[]) => unknown;
  __wbg_get_slicersettings_wall_count: (...args: unknown[]) => unknown;
  __wbg_set_slicersettings_first_layer_height: (...args: unknown[]) => unknown;
  __wbg_set_slicersettings_infill_density: (...args: unknown[]) => unknown;
  __wbg_set_slicersettings_infill_pattern: (...args: unknown[]) => unknown;
  __wbg_set_slicersettings_layer_height: (...args: unknown[]) => unknown;
  __wbg_set_slicersettings_line_width: (...args: unknown[]) => unknown;
  __wbg_set_slicersettings_nozzle_diameter: (...args: unknown[]) => unknown;
  __wbg_set_slicersettings_support_angle: (...args: unknown[]) => unknown;
  __wbg_set_slicersettings_support_enabled: (...args: unknown[]) => unknown;
  __wbg_set_slicersettings_wall_count: (...args: unknown[]) => unknown;
  __wbg_sliceresult_free: (...args: unknown[]) => unknown;
  __wbg_slicersettings_free: (...args: unknown[]) => unknown;
  generateGcode: (...args: unknown[]) => unknown;
  getSlicerPrinterProfiles: (...args: unknown[]) => unknown;
  isSlicerAvailable: (...args: unknown[]) => unknown;
  sliceMesh: (...args: unknown[]) => unknown;
  sliceSolid: (...args: unknown[]) => unknown;
  sliceresult_filamentGrams: (...args: unknown[]) => unknown;
  sliceresult_filamentMm: (...args: unknown[]) => unknown;
  sliceresult_getLayerPreview: (...args: unknown[]) => unknown;
  sliceresult_layerCount: (...args: unknown[]) => unknown;
  sliceresult_printTimeSeconds: (...args: unknown[]) => unknown;
  sliceresult_statsJson: (...args: unknown[]) => unknown;
  slicersettings_fromJson: (...args: unknown[]) => unknown;
  slicersettings_new: (...args: unknown[]) => unknown;
  wasm_bindgen__closure__destroy__h0cd8d7c1a52f473f: (...args: unknown[]) => unknown;
  wasm_bindgen__closure__destroy__ha8b73a36ae48e470: (...args: unknown[]) => unknown;
  wasm_bindgen__convert__closures_____invoke__h60f25fed64173f82: (...args: unknown[]) => unknown;
  wasm_bindgen__convert__closures_____invoke__hd187c9a655d7ef17: (...args: unknown[]) => unknown;
  wasm_bindgen__convert__closures_____invoke__h4488ad9b37e81000: (...args: unknown[]) => unknown;
  __wbindgen_malloc: (...args: unknown[]) => unknown;
  __wbindgen_realloc: (...args: unknown[]) => unknown;
  __wbindgen_exn_store: (...args: unknown[]) => unknown;
  __externref_table_alloc: (...args: unknown[]) => unknown;
  __wbindgen_free: (...args: unknown[]) => unknown;
  __externref_table_dealloc: (...args: unknown[]) => unknown;
  __wbindgen_start: (...args: unknown[]) => unknown;
}

export interface SweepExports extends WasmExports {
  op_loft: (...args: unknown[]) => unknown;
  op_revolve: (...args: unknown[]) => unknown;
  op_sweep_line: (...args: unknown[]) => unknown;
  op_sweep_helix: (...args: unknown[]) => unknown;
}

export interface GpuExports extends WasmExports {
  initGpu: (...args: unknown[]) => unknown;
  isGpuAvailable: (...args: unknown[]) => unknown;
  decimateMeshGpu: (...args: unknown[]) => unknown;
  processGeometryGpu: (...args: unknown[]) => unknown;
  computeCreasedNormalsGpu: (...args: unknown[]) => unknown;
}

export interface AdvancedExports extends WasmExports {
  op_shell: (...args: unknown[]) => unknown;
  op_fillet: (...args: unknown[]) => unknown;
  op_chamfer: (...args: unknown[]) => unknown;
}

export interface MlExports extends WasmExports {
  toVCode: (...args: unknown[]) => unknown;
  parseVCode: (...args: unknown[]) => unknown;
  evaluateVCode: (...args: unknown[]) => unknown;
}

export interface PatternsExports extends WasmExports {
  op_linear_pattern: (...args: unknown[]) => unknown;
  op_circular_pattern: (...args: unknown[]) => unknown;
}

export interface DraftingExports extends WasmExports {
  projectMesh: (...args: unknown[]) => unknown;
  sectionMesh: (...args: unknown[]) => unknown;
  createDetailView: (...args: unknown[]) => unknown;
  exportProjectedViewToDxf: (...args: unknown[]) => unknown;
}

export interface StepExports extends WasmExports {
  importStepBuffer: (...args: unknown[]) => unknown;
}

export interface PhysicsExports extends WasmExports {
  isPhysicsAvailable: (...args: unknown[]) => unknown;
}

export interface WasmosisRegistry extends Registry {
  load<T extends WasmExports = WasmExports>(name: string): Promise<WasmModule<T>>;
  load(name: 'core'): Promise<WasmModule<CoreExports>>;
  load(name: 'sweep'): Promise<WasmModule<SweepExports>>;
  load(name: 'gpu'): Promise<WasmModule<GpuExports>>;
  load(name: 'advanced'): Promise<WasmModule<AdvancedExports>>;
  load(name: 'ml'): Promise<WasmModule<MlExports>>;
  load(name: 'patterns'): Promise<WasmModule<PatternsExports>>;
  load(name: 'drafting'): Promise<WasmModule<DraftingExports>>;
  load(name: 'step'): Promise<WasmModule<StepExports>>;
  load(name: 'physics'): Promise<WasmModule<PhysicsExports>>;
}

/**
 * Multi-module WASM kernel loader
 *
 * Provides lazy loading of WASM modules with a unified API.
 * Core module is loaded eagerly, optional modules are loaded on demand.
 */

type WasmModule = Record<string, unknown>;

export interface KernelConfig {
  /** Base path for WASM modules */
  wasmBasePath?: string;
  /** Whether to use split modules (default: true if available) */
  useSplitModules?: boolean;
}

/**
 * WASM Kernel with lazy module loading
 */
export class Kernel {
  private core: WasmModule | null = null;
  private modules = new Map<string, Promise<WasmModule>>();
  private config: Required<KernelConfig>;

  private constructor(config: KernelConfig = {}) {
    this.config = {
      wasmBasePath: config.wasmBasePath ?? './',
      useSplitModules: config.useSplitModules ?? true,
    };
  }

  /**
   * Initialize the kernel by loading the core module
   */
  static async init(config: KernelConfig = {}): Promise<Kernel> {
    const kernel = new Kernel(config);

    if (kernel.config.useSplitModules) {
      try {
        // Try loading the split core module
        const core = await import(
          /* webpackIgnore: true */
          `${kernel.config.wasmBasePath}core/vcad_kernel_wasm_core.js`
        );
        await core.default();
        kernel.core = core;
      } catch {
        // Fall back to monolithic module
        console.log('[Kernel] Split modules not available, using monolithic');
        const mono = await import(
          /* webpackIgnore: true */
          `${kernel.config.wasmBasePath}vcad_kernel_wasm.js`
        );
        await mono.default();
        kernel.core = mono;
        kernel.config.useSplitModules = false;
      }
    } else {
      // Use monolithic module
      const mono = await import(
        /* webpackIgnore: true */
        `${kernel.config.wasmBasePath}vcad_kernel_wasm.js`
      );
      await mono.default();
      kernel.core = mono;
    }

    return kernel;
  }

  /**
   * Load a secondary module by name
   */
  private async loadModule(name: string): Promise<WasmModule> {
    // If using monolithic, all functions are already in core
    if (!this.config.useSplitModules) {
      return this.core!;
    }

    if (!this.modules.has(name)) {
      this.modules.set(
        name,
        (async () => {
          console.log(`[Kernel] Loading ${name} module...`);
          const mod = await import(
            /* webpackIgnore: true */
            `${this.config.wasmBasePath}${name}/vcad_kernel_wasm_${name}.js`
          );
          await mod.default();
          return mod;
        })()
      );
    }
    return this.modules.get(name)!;
  }

  // ===========================================================================
  // Core API (always available)
  // ===========================================================================

  get Solid() {
    return (this.core as any).Solid;
  }

  getKernelVersion(): string {
    return (this.core as any).get_kernel_version();
  }

  // ===========================================================================
  // STEP module (lazy)
  // ===========================================================================

  async importStep(data: ArrayBuffer): Promise<unknown[]> {
    const step = await this.loadModule('step');
    return (step as any).import_step(new Uint8Array(data));
  }

  async isStepAvailable(): Promise<boolean> {
    try {
      const step = await this.loadModule('step');
      return (step as any).is_step_available?.() ?? true;
    } catch {
      return false;
    }
  }

  // ===========================================================================
  // GPU module (lazy)
  // ===========================================================================

  async initGpu(): Promise<boolean> {
    const gpu = await this.loadModule('gpu');
    return (gpu as any).init_gpu?.() ?? false;
  }

  async isGpuAvailable(): Promise<boolean> {
    try {
      const gpu = await this.loadModule('gpu');
      return (gpu as any).is_gpu_available?.() ?? false;
    } catch {
      return false;
    }
  }

  async computeCreasedNormalsGpu(
    positions: Float32Array,
    indices: Uint32Array,
    creaseAngle: number
  ): Promise<Float32Array> {
    const gpu = await this.loadModule('gpu');
    return (gpu as any).compute_creased_normals_gpu(positions, indices, creaseAngle);
  }

  async decimateMeshGpu(
    positions: Float32Array,
    indices: Uint32Array,
    targetRatio: number
  ): Promise<{ positions: Float32Array; indices: Uint32Array }> {
    const gpu = await this.loadModule('gpu');
    return (gpu as any).decimate_mesh_gpu(positions, indices, targetRatio);
  }

  // ===========================================================================
  // Physics module (lazy)
  // ===========================================================================

  async isPhysicsAvailable(): Promise<boolean> {
    try {
      const physics = await this.loadModule('physics');
      return (physics as any).is_physics_available?.() ?? false;
    } catch {
      return false;
    }
  }

  // ===========================================================================
  // Drafting module (lazy)
  // ===========================================================================

  async sectionMesh(
    mesh: { positions: number[]; indices: number[] },
    plane: { origin: number[]; normal: number[]; up: number[] },
    hatch?: { spacing: number; angle: number }
  ): Promise<unknown> {
    const drafting = await this.loadModule('drafting');
    return (drafting as any).section_mesh_wasm(mesh, JSON.stringify(plane), hatch ? JSON.stringify(hatch) : undefined);
  }

  async projectMesh(
    mesh: { positions: number[]; indices: number[] },
    projectionType: string,
    options?: unknown
  ): Promise<unknown> {
    const drafting = await this.loadModule('drafting');
    return (drafting as any).project_mesh_wasm(mesh, projectionType, options ? JSON.stringify(options) : undefined);
  }

  // ===========================================================================
  // ML module (lazy)
  // ===========================================================================

  async parseCompactIr(compactIr: string): Promise<string> {
    const ml = await this.loadModule('ml');
    return (ml as any).parse_vcode(compactIr);
  }

  async toVCodeIr(docJson: string): Promise<string> {
    const ml = await this.loadModule('ml');
    return (ml as any).to_vcode(docJson);
  }

  async evaluateCompactIr(compactIr: string): Promise<unknown> {
    const ml = await this.loadModule('ml');
    return (ml as any).evaluate_vcode(compactIr);
  }

  // ===========================================================================
  // CPU Ray Tracing (in core)
  // ===========================================================================

  /**
   * Get the CpuRayTracer class for direct BRep rendering.
   * Returns null if cpu-raytrace feature is not enabled.
   */
  get CpuRayTracer() {
    return (this.core as any).CpuRayTracer ?? null;
  }
}

export default Kernel;

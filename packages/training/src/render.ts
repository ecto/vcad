/**
 * Headless rendering module for generating synthetic images from VCode.
 *
 * Uses puppeteer to render 3D geometry with Three.js in a headless browser.
 * Puppeteer is an optional dependency - install it with: npm install puppeteer
 */

// These types are inline to avoid requiring puppeteer at compile time
interface Browser {
  newPage(): Promise<Page>;
  close(): Promise<void>;
}

interface Page {
  setContent(html: string): Promise<void>;
  waitForFunction(fn: string, options?: { timeout?: number }): Promise<void>;
  evaluate<T>(fn: (...args: any[]) => T, ...args: any[]): Promise<T>;
  screenshot(options?: { type?: string }): Promise<Buffer>;
}

/** Camera view preset. */
export type ViewPreset = "isometric" | "front" | "side" | "top" | "random";

/** Render options. */
export interface RenderOptions {
  /** Image width in pixels. */
  width?: number;
  /** Image height in pixels. */
  height?: number;
  /** Camera view preset. */
  view?: ViewPreset;
  /** Background color (hex). */
  backgroundColor?: string;
  /** Part color (hex). */
  partColor?: string;
  /** Enable shadows. */
  shadows?: boolean;
}

/** Result of rendering. */
export interface RenderResult {
  /** PNG image as Buffer. */
  image: Buffer;
  /** View preset used. */
  view: ViewPreset;
  /** Render duration in ms. */
  durationMs: number;
}

/** HTML template for Three.js rendering. */
const RENDER_HTML = `
<!DOCTYPE html>
<html>
<head>
  <style>
    body { margin: 0; overflow: hidden; }
    canvas { display: block; }
  </style>
</head>
<body>
  <script type="importmap">
  {
    "imports": {
      "three": "https://cdn.jsdelivr.net/npm/three@0.160.0/build/three.module.js",
      "three/addons/": "https://cdn.jsdelivr.net/npm/three@0.160.0/examples/jsm/"
    }
  }
  </script>
  <script type="module">
    import * as THREE from 'three';

    // Globals for puppeteer access
    window.THREE = THREE;
    window.scene = null;
    window.camera = null;
    window.renderer = null;

    window.initScene = function(width, height, bgColor) {
      // Scene
      window.scene = new THREE.Scene();
      window.scene.background = new THREE.Color(bgColor);

      // Camera
      window.camera = new THREE.PerspectiveCamera(45, width / height, 0.1, 10000);

      // Renderer
      window.renderer = new THREE.WebGLRenderer({ antialias: true, preserveDrawingBuffer: true });
      window.renderer.setSize(width, height);
      window.renderer.setPixelRatio(1);
      window.renderer.shadowMap.enabled = true;
      window.renderer.shadowMap.type = THREE.PCFSoftShadowMap;
      document.body.appendChild(window.renderer.domElement);

      // Lighting
      const ambient = new THREE.AmbientLight(0xffffff, 0.5);
      window.scene.add(ambient);

      const directional = new THREE.DirectionalLight(0xffffff, 0.8);
      directional.position.set(50, 100, 50);
      directional.castShadow = true;
      directional.shadow.mapSize.width = 2048;
      directional.shadow.mapSize.height = 2048;
      window.scene.add(directional);

      const fill = new THREE.DirectionalLight(0xffffff, 0.3);
      fill.position.set(-50, 50, -50);
      window.scene.add(fill);
    };

    window.setCamera = function(view, boundingBox) {
      const center = new THREE.Vector3();
      boundingBox.getCenter(center);
      const size = new THREE.Vector3();
      boundingBox.getSize(size);
      const maxDim = Math.max(size.x, size.y, size.z);
      const distance = maxDim * 2.5;

      let position;
      switch (view) {
        case 'front':
          position = new THREE.Vector3(center.x, center.y, center.z + distance);
          break;
        case 'side':
          position = new THREE.Vector3(center.x + distance, center.y, center.z);
          break;
        case 'top':
          position = new THREE.Vector3(center.x, center.y + distance, center.z);
          break;
        case 'isometric':
        default:
          position = new THREE.Vector3(
            center.x + distance * 0.7,
            center.y + distance * 0.5,
            center.z + distance * 0.7
          );
          break;
      }

      window.camera.position.copy(position);
      window.camera.lookAt(center);
      window.camera.updateProjectionMatrix();
    };

    window.setRandomCamera = function(boundingBox) {
      const center = new THREE.Vector3();
      boundingBox.getCenter(center);
      const size = new THREE.Vector3();
      boundingBox.getSize(size);
      const maxDim = Math.max(size.x, size.y, size.z);
      const distance = maxDim * (2 + Math.random());

      // Random spherical coordinates
      const theta = Math.random() * Math.PI * 2;
      const phi = Math.PI / 6 + Math.random() * Math.PI / 3; // 30-90 degrees elevation

      const position = new THREE.Vector3(
        center.x + distance * Math.sin(phi) * Math.cos(theta),
        center.y + distance * Math.cos(phi),
        center.z + distance * Math.sin(phi) * Math.sin(theta)
      );

      window.camera.position.copy(position);
      window.camera.lookAt(center);
      window.camera.updateProjectionMatrix();
    };

    window.addMesh = function(positions, indices, color) {
      const geometry = new THREE.BufferGeometry();
      geometry.setAttribute('position', new THREE.Float32BufferAttribute(positions, 3));
      geometry.setIndex(indices);
      geometry.computeVertexNormals();

      const material = new THREE.MeshStandardMaterial({
        color: new THREE.Color(color),
        metalness: 0.1,
        roughness: 0.6,
        side: THREE.DoubleSide,
      });

      const mesh = new THREE.Mesh(geometry, material);
      mesh.castShadow = true;
      mesh.receiveShadow = true;
      window.scene.add(mesh);

      return geometry.boundingBox || new THREE.Box3().setFromObject(mesh);
    };

    window.render = function() {
      window.renderer.render(window.scene, window.camera);
    };

    window.getImageData = function() {
      return window.renderer.domElement.toDataURL('image/png');
    };

    // Signal ready
    window.sceneReady = true;
  </script>
</body>
</html>
`;

/** Renderer instance that manages a browser for batch rendering. */
export class Renderer {
  private browser: Browser | null = null;
  private page: Page | null = null;
  private initialized = false;

  /**
   * Initialize the renderer with a headless browser.
   */
  async init(): Promise<void> {
    if (this.initialized) return;

    // Dynamic import to avoid requiring puppeteer at module load
    // eslint-disable-next-line @typescript-eslint/no-var-requires
    let puppeteerModule: any;
    try {
      // @ts-ignore - puppeteer is an optional dependency
      puppeteerModule = await import("puppeteer");
    } catch {
      throw new Error(
        "puppeteer is not installed. Install it with: npm install puppeteer"
      );
    }

    this.browser = await puppeteerModule.default.launch({
      headless: "new",
      args: [
        "--no-sandbox",
        "--disable-setuid-sandbox",
        "--use-gl=angle",
        "--use-angle=metal",
        "--enable-webgl",
        "--enable-webgl2",
      ],
    }) as Browser;

    this.page = await this.browser!.newPage();
    await this.page.setContent(RENDER_HTML);

    // Wait for Three.js to load
    await this.page.waitForFunction("window.sceneReady === true", {
      timeout: 30000,
    });

    this.initialized = true;
  }

  /**
   * Render a mesh to an image.
   *
   * @param positions - Flat array of vertex positions (x,y,z,...)
   * @param indices - Triangle indices
   * @param options - Render options
   */
  async render(
    positions: number[] | Float32Array,
    indices: number[] | Uint32Array,
    options: RenderOptions = {},
  ): Promise<RenderResult> {
    if (!this.initialized || !this.page) {
      throw new Error("Renderer not initialized. Call init() first.");
    }

    const {
      width = 512,
      height = 512,
      view = "isometric",
      backgroundColor = "#f0f0f0",
      partColor = "#4a90d9",
      shadows = true,
    } = options;

    const startTime = Date.now();

    // Initialize scene
    await this.page.evaluate(
      (w: number, h: number, bg: string) => {
        (window as any).initScene(w, h, bg);
      },
      width,
      height,
      backgroundColor,
    );

    // Add mesh and get bounding box
    const posArray = Array.from(positions);
    const idxArray = Array.from(indices);

    await this.page.evaluate(
      (pos: number[], idx: number[], color: string) => {
        (window as any).addMesh(pos, idx, color);
      },
      posArray,
      idxArray,
      partColor,
    );

    // Compute bounding box and set camera
    const actualView = view === "random" ? "random" : view;
    await this.page.evaluate(
      (v: string) => {
        const bbox = new (window as any).THREE.Box3();
        (window as any).scene.traverse((obj: any) => {
          if (obj.isMesh) {
            obj.geometry.computeBoundingBox();
            const meshBbox = obj.geometry.boundingBox.clone();
            meshBbox.applyMatrix4(obj.matrixWorld);
            bbox.union(meshBbox);
          }
        });

        if (v === "random") {
          (window as any).setRandomCamera(bbox);
        } else {
          (window as any).setCamera(v, bbox);
        }
      },
      actualView,
    );

    // Render
    await this.page.evaluate(() => {
      (window as any).render();
    });

    // Get image data
    const dataUrl = await this.page.evaluate(() => {
      return (window as any).getImageData();
    });

    // Convert data URL to Buffer
    const base64Data = dataUrl.replace(/^data:image\/png;base64,/, "");
    const image = Buffer.from(base64Data, "base64");

    // Clear scene for next render
    await this.page.evaluate(() => {
      while ((window as any).scene.children.length > 0) {
        (window as any).scene.remove((window as any).scene.children[0]);
      }
    });

    return {
      image,
      view: view,
      durationMs: Date.now() - startTime,
    };
  }

  /**
   * Close the browser and clean up resources.
   */
  async close(): Promise<void> {
    if (this.browser) {
      await this.browser.close();
      this.browser = null;
      this.page = null;
      this.initialized = false;
    }
  }
}

/**
 * Render a single part to an image (convenience function).
 *
 * Creates a new renderer, renders the part, and closes the renderer.
 * For batch rendering, use the Renderer class directly.
 */
export async function renderToImage(
  positions: number[] | Float32Array,
  indices: number[] | Uint32Array,
  options: RenderOptions = {},
): Promise<Buffer> {
  const renderer = new Renderer();
  try {
    await renderer.init();
    const result = await renderer.render(positions, indices, options);
    return result.image;
  } finally {
    await renderer.close();
  }
}

/**
 * Generate multiple views of a mesh.
 */
export async function renderMultipleViews(
  renderer: Renderer,
  positions: number[] | Float32Array,
  indices: number[] | Uint32Array,
  views: ViewPreset[] = ["isometric", "front", "side", "top"],
  baseOptions: RenderOptions = {},
): Promise<Map<ViewPreset, Buffer>> {
  const results = new Map<ViewPreset, Buffer>();

  for (const view of views) {
    const result = await renderer.render(positions, indices, {
      ...baseOptions,
      view,
    });
    results.set(view, result.image);
  }

  return results;
}

# Photorealistic Rendering

## Status

| Field | Value |
|-------|-------|
| State | `in-progress` |
| Owner | @cam |

> Verified against repo 2026-07-30. Shipped: path tracing landed natively — CPU path tracer + `vcad-render --photoreal` (`crates/vcad-render/src/photoreal.rs`), HDRI environment lighting with importance sampling (`envmap.rs`, `--env`/`--env-rotation`), and a GPU viewport path tracer sharing one `bsdf.wgsl` with the CLI; video/turntable export exists via the MCP `animate`/`render_sequence`/`export_video` tools. Not shipped: cloud rendering service (LuxCoreRender/render farm), OIDN denoising, measured-material (MERL/MaterialX) import.

## Overview

Lower priority but adds polish. vcad already has a WebGPU ray tracing foundation in `vcad-kernel-raytrace` that can be extended to full path tracing rather than embedding a third-party engine.

## Current State

The existing ray tracer (`vcad-kernel-raytrace`) provides:

- WebGPU compute shaders
- Direct BRep intersection (plane, cylinder, sphere, cone, torus, NURBS)
- BVH acceleration with SAH construction
- Progressive accumulation
- Basic PBR (color, metallic, roughness)
- 31 material presets

## Strategy

Extend the existing ray tracer to full path tracing rather than embedding a third-party engine. This preserves the tight BRep integration and WebGPU pipeline while adding photorealistic capabilities.

## Path Tracing Extension

### GGX Microfacet BRDF

Disney-style PBR with physically-based light transport:

- GGX/Trowbridge-Reitz normal distribution
- Smith geometry term with height-correlated masking
- Fresnel-Schlick approximation
- Importance sampling for efficiency

### Shader Structure

```wgsl
fn trace_path(ray: Ray, rng: ptr<function, u32>) -> vec3<f32> {
    var throughput = vec3(1.0);
    var radiance = vec3(0.0);
    var current_ray = ray;

    for (var bounce = 0u; bounce < MAX_BOUNCES; bounce++) {
        let hit = trace_ray(current_ray);
        if (!hit.valid) {
            radiance += throughput * sample_environment(current_ray.direction);
            break;
        }

        let material = get_material(hit.material_id);
        radiance += throughput * material.emission;

        // Sample BRDF
        let sample = sample_ggx(hit.normal, -current_ray.direction, material, rng);
        throughput *= sample.weight;

        // Russian roulette termination
        let p = max(throughput.r, max(throughput.g, throughput.b));
        if (random_float(rng) > p) { break; }
        throughput /= p;

        current_ray = Ray(hit.position + hit.normal * EPSILON, sample.direction);
    }

    return radiance;
}
```

## Extended Material Model

### Core PBR Extensions

Beyond the current baseColor/metallic/roughness model:

| Property | Type | Description |
|----------|------|-------------|
| `ior` | f32 | Index of refraction (glass: 1.5, water: 1.33, diamond: 2.42) |
| `transmission` | f32 | Transparency amount (0-1) |
| `transmissionRoughness` | f32 | Blurry glass effect |
| `subsurfaceScattering` | f32 | SSS intensity for skin, wax, marble |
| `subsurfaceRadius` | vec3 | Per-channel scatter distance |
| `emission` | vec3 | Emissive color |
| `emissionStrength` | f32 | Emission intensity multiplier |
| `clearcoat` | f32 | Clear coat layer intensity |
| `clearcoatRoughness` | f32 | Clear coat roughness |
| `anisotropy` | f32 | Anisotropic reflection strength |
| `anisotropyRotation` | f32 | Anisotropy direction angle |
| `sheen` | f32 | Fabric sheen intensity |
| `sheenTint` | f32 | Sheen color tint toward base color |

### Texture Maps

- `baseColorMap` - Albedo texture
- `normalMap` - Tangent-space normal perturbation
- `roughnessMap` - Per-pixel roughness
- `metallicMap` - Per-pixel metallic
- `aoMap` - Ambient occlusion
- `displacementMap` - Geometry displacement (subdivision required)

### Measured Materials

Support for physically-measured material data:

- **MERL BRDF Database**: 100 isotropic materials with measured reflectance
- **MaterialX**: Industry-standard material interchange format
- **glTF extensions**: KHR_materials_transmission, KHR_materials_clearcoat, etc.

## HDRI Environment System

### Environment Map Pipeline

```
HDR/EXR File → Decode → Equirectangular Map → Importance Sampling CDF → GPU Texture
```

**Features:**

- Equirectangular HDR/EXR loading via `image` crate
- Importance sampling based on luminance for efficient lighting
- Rotation and intensity controls
- Pre-filtered mipmap chain for rough reflections (split-sum approximation)

### Environment Sources

**Bundled Presets:**

- Studio softbox (product shots)
- Outdoor overcast (neutral lighting)
- Golden hour (warm directional)
- Night city (colorful reflections)
- Pure white (CAD-style)

**External Integration:**

- [Poly Haven](https://polyhaven.com/hdris) API for 600+ free HDRIs
- User-uploaded custom environments
- Procedural sky with sun position

## Cloud Rendering Service

### Architecture

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│  vcad App   │────▶│ Render API  │────▶│ Job Queue   │────▶│ GPU Workers │
│  (Browser)  │◀────│  (Lambda)   │◀────│ (SQS/Redis) │     │ (EC2 G5)    │
└─────────────┘     └─────────────┘     └─────────────┘     └──────┬──────┘
                           │                                       │
                           │         ┌─────────────┐               │
                           └────────▶│   Results   │◀──────────────┘
                                     │    (S3)     │
                                     └─────────────┘
```

### Backend Renderer

**LuxCoreRender** for production quality:

- Unbiased path tracing
- OpenCL GPU acceleration
- Native STEP/mesh import
- Python API for automation

**Scene Export Pipeline:**

1. Export vcad document to STEP
2. Convert materials to LuxCore format
3. Package with HDRI environment
4. Submit to render farm

### API

```
POST /api/render
{
  "documentId": "uuid",
  "resolution": [1920, 1080],
  "samples": 512,
  "camera": { "position": [...], "target": [...], "fov": 45 },
  "environment": "studio_soft",
  "priority": "normal"
}
→ { "jobId": "uuid", "estimatedSeconds": 45 }

GET /api/render/:jobId
→ { "status": "rendering", "progress": 0.45, "samplesCompleted": 230 }

GET /api/render/:jobId/result
→ { "url": "https://cdn.vcad.io/renders/...", "expiresAt": "..." }

DELETE /api/render/:jobId
→ { "cancelled": true }
```

### WebSocket Progress

```typescript
const ws = new WebSocket(`wss://api.vcad.io/render/${jobId}/progress`);
ws.onmessage = (e) => {
  const { progress, preview } = JSON.parse(e.data);
  updateProgressBar(progress);
  if (preview) showPreviewImage(preview); // Low-res intermediate
};
```

### Cost Optimization

| Instance | Cost | Use Case |
|----------|------|----------|
| g5.xlarge (A10G) | ~$1.00/hr | Standard renders |
| g5.xlarge Spot | ~$0.30/hr | Non-urgent batch |
| Modal | ~$0.60/hr | Scale-to-zero, no idle cost |
| RunPod | ~$0.40/hr | Community cloud, spot-like |

**Strategies:**

- Spot instances for 60-90% savings on non-urgent jobs
- Modal/RunPod for true scale-to-zero (no minimum)
- Pre-emptible renders with checkpoint/resume
- Tiered pricing: free tier (720p, 64 SPP), pro tier (4K, unlimited)

## Denoising

### Client-Side (oidn-web)

Intel Open Image Denoise compiled to WebAssembly/WebGPU:

```typescript
import { OIDNDevice, OIDNFilter } from 'oidn-web';

const device = await OIDNDevice.create();
const filter = device.newFilter('RT');

filter.setImage('color', colorBuffer, width, height);
filter.setImage('albedo', albedoBuffer, width, height);
filter.setImage('normal', normalBuffer, width, height);
filter.setImage('output', outputBuffer, width, height);

filter.commit();
await filter.execute();
```

**Auxiliary Buffers (AOVs):**

- Albedo: Diffuse color without lighting
- Normal: World-space surface normals
- These guide the denoiser for better edge preservation

**Progressive Denoising:**

- Denoise every N samples (e.g., 8, 16, 32, 64)
- Display denoised result while accumulating more samples
- Final output at target sample count

### Server-Side

- OIDN for CPU-based denoising
- OptiX AI denoiser for NVIDIA GPUs (faster, requires CUDA)

## Animation & Export

### Turntable Animation

```typescript
interface TurntableConfig {
  axis: 'y' | 'x' | 'z';
  distance: number;
  fov: number;
  frames: number;
  resolution: [number, number];
  samples: number;
}

async function renderTurntable(doc: Document, config: TurntableConfig) {
  const frames: ImageData[] = [];
  for (let i = 0; i < config.frames; i++) {
    const angle = (i / config.frames) * Math.PI * 2;
    const camera = orbitCamera(angle, config);
    frames.push(await renderFrame(doc, camera, config));
  }
  return encodeVideo(frames);
}
```

### Video Encoding

**Primary: WebCodecs API**

```typescript
const encoder = new VideoEncoder({
  output: (chunk) => muxer.addVideoChunk(chunk),
  error: console.error,
});

encoder.configure({
  codec: 'avc1.640028', // H.264 High Profile
  width: 1920,
  height: 1080,
  framerate: 30,
  bitrate: 8_000_000,
});
```

**Fallback: ffmpeg.wasm**

For browsers without WebCodecs or advanced codec needs.

**Output Formats:**

- MP4 (H.264) - Universal compatibility
- WebM (VP9) - Better compression, web-native
- Frame sequence (PNG/EXR) - Post-production workflows

### Interactive 360 View

Multi-row turntable for full spherical coverage:

```typescript
interface Interactive360Config {
  horizontalFrames: number; // e.g., 36 (10° steps)
  verticalRows: number;     // e.g., 5 (-60° to +60°)
  resolution: [number, number];
}
```

Output compatible with WebRotate360 and similar viewers.

### High-Resolution Stills

- 4K (3840x2160) and 8K (7680x4320) output
- EXR format for HDR preservation and post-processing
- Multi-pass rendering for memory efficiency on large resolutions

## Three.js Integration

### Hybrid Rendering Mode

Combine fast rasterization with progressive path tracing:

```typescript
function HybridViewport({ document }) {
  const [rayTracedCanvas, setRayTracedCanvas] = useState<HTMLCanvasElement>();
  const [samples, setSamples] = useState(0);

  useEffect(() => {
    const tracer = new PathTracer(document);
    tracer.onProgress = (s, canvas) => {
      setSamples(s);
      setRayTracedCanvas(canvas);
    };
    tracer.start();
    return () => tracer.stop();
  }, [document]);

  return (
    <div style={{ position: 'relative' }}>
      {/* Base Three.js view - always interactive */}
      <ThreeCanvas document={document} />

      {/* Path-traced overlay - fades in as quality improves */}
      {rayTracedCanvas && (
        <canvas
          style={{
            position: 'absolute',
            opacity: Math.min(samples / 64, 1),
            pointerEvents: 'none',
          }}
          ref={(el) => el?.getContext('2d')?.drawImage(rayTracedCanvas, 0, 0)}
        />
      )}
    </div>
  );
}
```

**Behavior:**

1. Camera movement triggers Three.js rasterized view (instant)
2. When idle, path tracer starts accumulating samples
3. Overlay opacity increases as sample count grows
4. Any interaction clears overlay and returns to rasterized

## Performance Targets

| Scenario | Resolution | Samples | Time | Hardware |
|----------|------------|---------|------|----------|
| Interactive preview | 720p | 1 SPP | 16ms | WebGPU integrated |
| Progressive local | 1080p | 64 SPP | 2s | WebGPU discrete |
| High quality local | 1080p | 256 SPP | 8s | WebGPU discrete |
| Production cloud | 4K | 512 SPP | 30s | Cloud A10G |
| Animation frame | 1080p | 256 SPP | 5s | Cloud A10G |

**Optimization Techniques:**

- Tile-based rendering for cache efficiency
- Adaptive sampling (more samples in noisy regions)
- Blue noise dithering for low-sample-count appearance
- Next Event Estimation for direct lighting

## Implementation Phases

### Phase 1: Enhanced Materials (2-3 weeks)

- Add transmission, IOR, emission to material struct
- Implement clearcoat layer in shader
- Update material editor UI
- Extend 31 presets to 50+ with new properties

### Phase 2: Path Tracing Core (4-6 weeks)

- GGX importance sampling implementation
- Multi-bounce light transport
- Russian roulette termination
- Progressive sample accumulation
- Refraction for transmissive materials

### Phase 3: Environment Lighting (2-3 weeks)

- HDR/EXR texture loading
- Equirectangular to cubemap conversion
- Luminance-based importance sampling
- Environment rotation controls
- 5 bundled HDRI presets

### Phase 4: Denoising (2 weeks)

- Integrate oidn-web package
- Generate albedo/normal AOV buffers
- Progressive denoising pipeline
- Quality/performance toggle

### Phase 5: Export & Animation (3-4 weeks)

- Turntable frame generator
- WebCodecs video encoding
- ffmpeg.wasm fallback
- High-res still export
- Interactive 360 output

### Phase 6: Cloud Rendering (4-6 weeks)

- Render job API design
- LuxCoreRender worker setup
- Scene export pipeline
- Progress WebSocket
- S3 result storage
- Billing integration

### Phase 7: Material Library (ongoing)

- MERL BRDF import
- MaterialX support
- Community material sharing
- Material preview thumbnails

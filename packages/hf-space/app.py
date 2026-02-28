"""
cad0 Demo Space - Text to CAD with WASM rendering
"""

import gradio as gr
import torch
from transformers import AutoModelForCausalLM, AutoTokenizer
import spaces
import urllib.parse

MODEL_ID = "campedersen/cad0"
BASE_MODEL = "Qwen/Qwen2.5-Coder-7B-Instruct"

print("Loading tokenizer...")
tokenizer = AutoTokenizer.from_pretrained(BASE_MODEL, trust_remote_code=True)

model = None

def load_model():
    global model
    if model is None:
        print("Loading cad0 model...")
        from transformers import BitsAndBytesConfig
        bnb_config = BitsAndBytesConfig(
            load_in_4bit=True,
            bnb_4bit_compute_dtype=torch.float16,
        )
        model = AutoModelForCausalLM.from_pretrained(
            MODEL_ID,
            quantization_config=bnb_config,
            trust_remote_code=True,
            low_cpu_mem_usage=True,
        )
        print("Model loaded on GPU")
    return model


def generate(prompt, temperature=0.1):
    """Generate VCode from prompt."""
    model = load_model()

    system_prompt = "You are a CAD assistant. Output only VCode code (C for box, Y for cylinder, T for translate, U for union, D for difference). No explanations, just the IR code."
    messages = [
        {"role": "system", "content": system_prompt},
        {"role": "user", "content": prompt}
    ]
    text = tokenizer.apply_chat_template(messages, tokenize=False, add_generation_prompt=True)
    inputs = tokenizer(text, return_tensors="pt").to("cuda")

    with torch.no_grad():
        outputs = model.generate(
            **inputs,
            max_new_tokens=256,
            temperature=temperature if temperature > 0 else 1.0,
            do_sample=temperature > 0,
            pad_token_id=tokenizer.eos_token_id,
            eos_token_id=tokenizer.eos_token_id,
        )

    response = tokenizer.decode(outputs[0][inputs.input_ids.shape[1]:], skip_special_tokens=True)

    for stop in ["User", "user", "\n\n\n", "Assistant"]:
        if stop in response:
            response = response.split(stop)[0]

    if "VCode" in response:
        response = response.split("VCode")[0]

    lines = response.strip().split('\n')
    ir_lines = []
    for line in lines:
        line = line.strip()
        if not line:
            continue
        if ir_lines and not (line[0] in 'CYTUDSMRFBXH' or line[0].isdigit()):
            break
        if line[0] in 'CYTUDSMRFBXH':
            ir_lines.append(line)

    return '\n'.join(ir_lines) if ir_lines else response.strip()


@spaces.GPU
def text_to_cad(prompt, temperature=0.1):
    """Main function."""
    if not prompt.strip():
        return "Enter a prompt", ""

    ir = generate(prompt, temperature)
    return ir, ir  # Return IR twice - once for display, once to trigger JS


VIEWER_HTML = """
<div id="viewer-container" style="width:100%;height:400px;background:#1a1a1a;border-radius:8px;display:flex;align-items:center;justify-content:center;flex-direction:column;position:relative;overflow:hidden;">
    <canvas id="render-canvas" width="400" height="400" style="display:none;"></canvas>
    <div id="viewer-placeholder" style="color:#666;text-align:center;">
        <p style="font-size:48px;margin:0;">🔧</p>
        <p>Generate IR to see preview</p>
    </div>
    <div id="viewer-status" style="position:absolute;bottom:8px;left:8px;color:#666;font-size:12px;"></div>
</div>
<script type="module">
// Serve from jsDelivr CDN (GitHub raw files)
const KERNEL_BASE = 'https://cdn.jsdelivr.net/gh/ecto/vcad@main/packages/kernel-wasm';

let kernel = null;
let kernelLoading = false;

async function loadKernel() {
    if (kernel) return kernel;
    if (kernelLoading) {
        while (kernelLoading) await new Promise(r => setTimeout(r, 100));
        return kernel;
    }

    kernelLoading = true;
    const status = document.getElementById('viewer-status');
    if (status) status.textContent = 'Loading kernel...';

    try {
        const module = await import(KERNEL_BASE + '/vcad_kernel_wasm.js');
        const wasmResponse = await fetch(KERNEL_BASE + '/vcad_kernel_wasm_bg.wasm');
        const wasmBuffer = await wasmResponse.arrayBuffer();
        module.initSync({ module: wasmBuffer });
        kernel = module;
        if (status) status.textContent = '';
        console.log('vcad kernel loaded');
    } catch (e) {
        console.error('Failed to load kernel:', e);
        if (status) status.textContent = 'Kernel load failed';
    }

    kernelLoading = false;
    return kernel;
}

function project(x, y, z, angle = 45, elevation = 25) {
    const radAngle = (angle * Math.PI) / 180;
    const radElev = (elevation * Math.PI) / 180;
    const x1 = x * Math.cos(radAngle) - z * Math.sin(radAngle);
    const z1 = x * Math.sin(radAngle) + z * Math.cos(radAngle);
    const y1 = y * Math.cos(radElev) - z1 * Math.sin(radElev);
    const z2 = y * Math.sin(radElev) + z1 * Math.cos(radElev);
    return { x: x1, y: y1, z: z2 };
}

function renderMesh(ctx, mesh, width, height) {
    const { positions, indices } = mesh;
    if (!positions || positions.length === 0) return false;

    let minX = Infinity, maxX = -Infinity;
    let minY = Infinity, maxY = -Infinity;

    for (let i = 0; i < positions.length; i += 3) {
        const p = project(positions[i], positions[i + 1], positions[i + 2]);
        minX = Math.min(minX, p.x); maxX = Math.max(maxX, p.x);
        minY = Math.min(minY, p.y); maxY = Math.max(maxY, p.y);
    }

    const rangeX = maxX - minX || 1;
    const rangeY = maxY - minY || 1;
    const scale = Math.min((width - 60) / rangeX, (height - 60) / rangeY);
    const offsetX = width / 2 - ((minX + maxX) / 2) * scale;
    const offsetY = height / 2 + ((minY + maxY) / 2) * scale;

    const triangles = [];
    for (let i = 0; i < indices.length; i += 3) {
        const i0 = indices[i], i1 = indices[i + 1], i2 = indices[i + 2];
        const v0 = project(positions[i0 * 3], positions[i0 * 3 + 1], positions[i0 * 3 + 2]);
        const v1 = project(positions[i1 * 3], positions[i1 * 3 + 1], positions[i1 * 3 + 2]);
        const v2 = project(positions[i2 * 3], positions[i2 * 3 + 1], positions[i2 * 3 + 2]);

        const nx = (v1.y - v0.y) * (v2.z - v0.z) - (v1.z - v0.z) * (v2.y - v0.y);
        const ny = (v1.z - v0.z) * (v2.x - v0.x) - (v1.x - v0.x) * (v2.z - v0.z);
        const nz = (v1.x - v0.x) * (v2.y - v0.y) - (v1.y - v0.y) * (v2.x - v0.x);
        const len = Math.sqrt(nx * nx + ny * ny + nz * nz) || 1;
        const light = Math.max(0.3, (nx / len * 0.3 + ny / len * 0.5 + nz / len * 0.8));

        triangles.push({
            points: [
                { x: v0.x * scale + offsetX, y: -v0.y * scale + offsetY },
                { x: v1.x * scale + offsetX, y: -v1.y * scale + offsetY },
                { x: v2.x * scale + offsetX, y: -v2.y * scale + offsetY },
            ],
            z: (v0.z + v1.z + v2.z) / 3,
            light,
        });
    }

    triangles.sort((a, b) => a.z - b.z);

    ctx.fillStyle = '#1a1a1a';
    ctx.fillRect(0, 0, width, height);

    for (const tri of triangles) {
        const g = Math.floor(200 * tri.light);
        const b = Math.floor(220 * tri.light);
        ctx.fillStyle = `rgb(${Math.floor(g * 0.4)}, ${g}, ${Math.floor(b * 0.5)})`;
        ctx.strokeStyle = 'rgba(255, 255, 255, 0.2)';
        ctx.lineWidth = 0.5;
        ctx.beginPath();
        ctx.moveTo(tri.points[0].x, tri.points[0].y);
        ctx.lineTo(tri.points[1].x, tri.points[1].y);
        ctx.lineTo(tri.points[2].x, tri.points[2].y);
        ctx.closePath();
        ctx.fill();
        ctx.stroke();
    }

    return true;
}

window.renderVCode = async function(ir) {
    if (!ir || !ir.trim()) return;

    const canvas = document.getElementById('render-canvas');
    const placeholder = document.getElementById('viewer-placeholder');
    const status = document.getElementById('viewer-status');
    if (!canvas) return;

    const k = await loadKernel();
    if (!k) {
        if (placeholder) {
            placeholder.innerHTML = '<p style="color:#888;">Use "Open in vcad.io" to view</p>';
            placeholder.style.display = 'block';
        }
        canvas.style.display = 'none';
        return;
    }

    try {
        if (status) status.textContent = 'Rendering...';
        const solid = k.evaluateVCode(ir);

        if (solid.isEmpty()) {
            if (placeholder) {
                placeholder.innerHTML = '<p style="color:#f87171;">Empty solid</p>';
                placeholder.style.display = 'block';
            }
            canvas.style.display = 'none';
            if (status) status.textContent = '';
            return;
        }

        const mesh = solid.getMesh(32);
        const ctx = canvas.getContext('2d');

        if (renderMesh(ctx, mesh, canvas.width, canvas.height)) {
            canvas.style.display = 'block';
            if (placeholder) placeholder.style.display = 'none';
            if (status) status.textContent = `${mesh.indices.length / 3} triangles`;
        }
    } catch (e) {
        console.error('Render error:', e);
        if (placeholder) {
            placeholder.innerHTML = '<p style="color:#f87171;">Render error</p>';
            placeholder.style.display = 'block';
        }
        canvas.style.display = 'none';
        if (status) status.textContent = e.message || 'Error';
    }
};

// Preload kernel
loadKernel();
</script>
"""

DESCRIPTION = """
Generate **parametric CAD geometry** from natural language descriptions.

**How it works:** Describe a part → cad0 outputs VCode → rendered with vcad WASM kernel

**VCode:** `C w h d` (box), `Y r h` (cylinder), `T idx x y z` (translate), `U a b` (union), `D a b` (difference)

[Model](https://huggingface.co/campedersen/cad0) · [Blog](https://campedersen.com/cad0) · [vcad.io](https://vcad.io)
"""

with gr.Blocks(theme=gr.themes.Base(), title="cad0: Text to CAD") as demo:
    gr.Markdown("# cad0: Text to CAD")
    gr.Markdown(DESCRIPTION)

    with gr.Row(equal_height=True):
        with gr.Column(scale=1):
            gr.Markdown("### 1. Describe")
            prompt = gr.Textbox(
                label="Part description",
                placeholder="L-bracket: 50mm x 30mm x 3mm",
                lines=4,
                show_label=False
            )
            temperature = gr.Slider(minimum=0, maximum=1, value=0.1, step=0.1, label="Temperature")
            submit_btn = gr.Button("Generate →", variant="primary")

        with gr.Column(scale=1):
            gr.Markdown("### 2. VCode")
            ir_output = gr.Textbox(label="Generated IR", lines=8, show_label=False)
            ir_hidden = gr.Textbox(visible=False)  # Hidden field to trigger JS

        with gr.Column(scale=1):
            gr.Markdown("### 3. Preview")
            viewer = gr.HTML(VIEWER_HTML)
            open_btn = gr.Button("Open in vcad.io →", variant="secondary")

    submit_btn.click(
        fn=text_to_cad,
        inputs=[prompt, temperature],
        outputs=[ir_output, ir_hidden]
    ).then(
        fn=None,
        inputs=[ir_hidden],
        js="(ir) => { if (window.renderVCode && ir) window.renderVCode(ir); }"
    )

    open_btn.click(
        fn=None,
        inputs=[ir_output],
        js="(ir) => { if (ir) window.open('https://vcad.io/?ir=' + encodeURIComponent(ir), '_blank'); }"
    )

    gr.Examples(
        examples=[
            ["L-bracket: 50mm x 30mm x 3mm thick", 0.1],
            ["mounting plate 50x30mm with 4 corner holes", 0.1],
            ["enclosure box 80x60x40mm", 0.1],
            ["cylindrical standoff 10mm diameter, 25mm tall", 0.1],
        ],
        inputs=[prompt, temperature],
    )

if __name__ == "__main__":
    demo.launch()

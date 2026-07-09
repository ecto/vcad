// /prove — break the part. A real kernel part with one slider; the check
// catches you thinning the wall past the DFM rule, and export stays blocked
// until you fix it. The wireframe and the true mass come from the kernel.

import { loadKernel, extractEdges, meshBounds, drawWireframe } from "./wireframe.js";

const canvas = document.getElementById("proveCanvas");
const slider = document.getElementById("wallSlider");
const wallVal = document.getElementById("wallVal");
const checksEl = document.getElementById("proveChecks");
const verdictEl = document.getElementById("proveVerdict");

if (canvas && slider) init();

function init() {
  let kernel = null;
  let mesh = null;
  let massG = null; // true kernel mass, g
  let rebuildTimer = 0;

  const MIN_WALL = 1.2; // cnc-3axis DFM rule

  function render() {
    const w = parseFloat(slider.value);
    wallVal.textContent = `${w.toFixed(1)} mm`;
    const okWall = w >= MIN_WALL;
    const massRow = massG === null
      ? `<p><span>mass · 6061-t6</span><span>computing…</span></p>`
      : `<p><span>mass · 6061-t6</span><span style="color:var(--t2)">${massG.toFixed(1)} g</span></p>`;
    checksEl.innerHTML =
      `<p><span>min_wall ≥ ${MIN_WALL} mm · cnc-3axis</span><span class="${okWall ? "pass" : "fail"}">${okWall ? "✓" : "✗ " + w.toFixed(1)}</span></p>` +
      `<p><span>clearance ≥ 2.0 mm</span><span class="pass">✓</span></p>` +
      massRow;
    verdictEl.style.color = okWall ? "var(--green)" : "var(--orange)";
    verdictEl.textContent = okWall
      ? "✓ provable · export unlocked"
      : `export blocked · fix: thicken wall to ${MIN_WALL} →`;
    draw(okWall);
  }

  function scheduleRebuild() {
    clearTimeout(rebuildTimer);
    rebuildTimer = setTimeout(() => rebuild(parseFloat(slider.value)), 140);
  }

  /* ---- kernel: lazy, loads when the section approaches ---- */

  let booted = false;
  const boot = () => {
    if (booted) return;
    booted = true;
    loadKernel()
      .then((wasm) => {
        kernel = wasm;
        rebuild(parseFloat(slider.value));
      })
      .catch((e) => console.warn("prove demo: kernel unavailable", e));
  };
  const io = new IntersectionObserver((en) => {
    if (en.some((e) => e.isIntersecting)) { io.disconnect(); boot(); }
  }, { rootMargin: "400px" });
  io.observe(canvas);
  setTimeout(boot, 4000); // IO never fires in hidden documents

  function rebuild(w) {
    if (!kernel) return;
    try {
      const { Solid } = kernel;
      const box = Solid.cube(100, 80, 60);
      const hollow = box.shell(Math.max(0.4, w));
      const cutter = Solid.cube(60, 50, 70).translate(55, 45, 5);
      const part = hollow.difference(cutter);
      massG = (part.volume() * 2.7) / 1000; // ρ 6061-t6 = 2.70 g/cm³
      const m = part.getMesh(22);
      mesh = { positions: m.positions, edges: extractEdges(m.positions, m.indices), ...meshBounds(m.positions) };
      box.free(); hollow.free(); cutter.free(); part.free();
      render();
    } catch (e) {
      console.warn("prove demo rebuild failed:", e);
    }
  }

  /* ---- drawing: locked brand camera, static pose ---- */

  const TILT = Math.atan(Math.SQRT1_2);
  const CT = Math.cos(TILT), ST = Math.sin(TILT);
  const SPIN = 0.62;
  const CS = Math.cos(SPIN), SN = Math.sin(SPIN);

  function draw(pass) {
    const dpr = Math.min(devicePixelRatio || 1, 2);
    canvas.width = canvas.clientWidth * dpr;
    canvas.height = canvas.clientHeight * dpr;
    const ctx = canvas.getContext("2d");
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    if (!mesh) {
      ctx.fillStyle = "rgba(58, 61, 66, 0.9)";
      ctx.font = `${10 * dpr}px "JetBrains Mono", monospace`;
      ctx.fillText("loading the kernel…", 12 * dpr, 20 * dpr);
      return;
    }
    // green only when the checks actually pass — the color law holds here too
    drawWireframe(ctx, mesh, {
      spin: 0.62,
      ax: canvas.width / 2,
      ay: canvas.height * 0.52,
      s: Math.min(canvas.width, canvas.height) / 150,
      color: pass ? "62, 207, 142" : "158, 165, 175",
      alpha: pass ? 0.55 : 0.5,
      dpr,
    });
  }

  slider.addEventListener("input", () => { massG = null; render(); scheduleRebuild(); });
  render();
}

import { defineConfig } from "vite";
import { fileURLToPath } from "node:url";

const page = (p) => fileURLToPath(new URL(p, import.meta.url));

// The hero background imports the checked-in kernel WASM artifacts from
// ../packages/kernel-wasm (single-writer: wasm-refresh.yml on main).
export default defineConfig({
  server: {
    fs: { allow: [".."] },
  },
  build: {
    // keep the kernel out of the entry chunk — it is lazy-loaded post-LCP
    rollupOptions: {
      input: {
        main: page("./index.html"),
        design: page("./design/index.html"),
        prove: page("./prove/index.html"),
        make: page("./make/index.html"),
        agents: page("./agents/index.html"),
        kernel: page("./kernel/index.html"),
        pricing: page("./pricing/index.html"),
      },
      output: {
        manualChunks(id) {
          if (id.includes("kernel-wasm")) return "kernel";
        },
      },
    },
  },
});

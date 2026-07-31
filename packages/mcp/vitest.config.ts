import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    // Several suites drive real kernel booleans through WASM (enclosure fit,
    // integrity torr cases, receipt gates). The seam-freeze kernel does
    // strictly more conformity work per boolean than the old resolution-
    // coincidence path, which pushed those tests past vitest's 5s default on
    // CI runners while still finishing in ~2-3s locally.
    testTimeout: 30_000,
  },
});

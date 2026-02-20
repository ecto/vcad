#!/usr/bin/env node
import React from "react";
import { render } from "ink";
import { initEngineLifecycle } from "@vcad/core";
import { App } from "./App.js";

async function main() {
  await initEngineLifecycle();

  // Render the app
  const { waitUntilExit } = render(<App />);
  await waitUntilExit();
}

main().catch((err) => {
  console.error("Fatal error:", err);
  process.exit(1);
});

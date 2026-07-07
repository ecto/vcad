import { defineTool } from "eve/tools";
import { z } from "zod";

/** Fetch a vendor's playbook + manifest from the registry so the operator
 *  can see what the deterministic path expected before repairing it. */
export default defineTool({
  description:
    "Load a vendor's registry manifest and a named playbook (steps, selectors, assertions).",
  inputSchema: z.object({
    vendor: z.string().min(1),
    capability: z.enum(["quote", "order", "track", "cancel"]),
  }),
  async execute({ vendor, capability }) {
    // TODO(bringup): read packages/registry/<vendor>/manifest.json and the
    // referenced playbook; return both. Pure data, no side effects.
    throw new Error(
      `kerf bringup: registry loader not wired yet (${vendor}/${capability}) — see docs/architecture.md, Wave 0`,
    );
  },
});

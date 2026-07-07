import { defineTool } from "eve/tools";
import { z } from "zod";

/** The self-healing loop: after an agentic repair, emit the durable fix —
 *  updated steps/selectors plus a fresh DOM fixture — as a registry PR.
 *  Repairs land as patches, not one-off heroics. */
export default defineTool({
  description:
    "Propose a playbook patch (updated steps/selectors + fixture) for the vendor registry, opened as a pull request for human review.",
  inputSchema: z.object({
    vendor: z.string().min(1),
    capability: z.enum(["quote", "order", "track", "cancel"]),
    /** JSON of the revised Playbook (validated against @kerf/core before
     *  the PR opens; money-adjacent rules are enforced there). */
    playbook_json: z.string().min(2),
    /** Evidence item id of the DOM fixture captured during the repair. */
    fixture_evidence: z.string().min(1),
    summary: z.string().min(1),
  }),
  async execute({ vendor, capability }) {
    // TODO(bringup): validatePlaybook() from @kerf/core, then open the PR.
    throw new Error(
      `kerf bringup: patch pipeline not wired yet (${vendor}/${capability})`,
    );
  },
});

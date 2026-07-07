import { defineTool } from "eve/tools";
import { z } from "zod";

/**
 * The capability gate. The operator can REQUEST the buy click; it cannot
 * perform it. This tool resolves the order workflow's audit hook: the
 * auditor step re-extracts the review page, compares it against the
 * OrderIntent, and only on a pass does the workflow's own placing step
 * click. A rejected audit terminates the job (AUDIT_FAILED) — the
 * operator does not get to argue with the auditor.
 */
export default defineTool({
  description:
    "Signal that the cart is fully staged and every assertion passes; hands off to the workflow's auditor + placing steps. Does NOT place the order.",
  inputSchema: z.object({
    job_id: z.string().min(1),
    /** Evidence item id of the operator's own review-page screenshot. */
    review_evidence: z.string().min(1),
  }),
  async execute({ job_id }) {
    // TODO(bringup): resolve the order workflow's `audit-requested` hook.
    throw new Error(
      `kerf bringup: audit hook not wired yet (job ${job_id}) — see workflows/order.ts`,
    );
  },
});

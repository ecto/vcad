import { defineTool } from "eve/tools";
import { z } from "zod";

/** Suspend for a human: CAPTCHA, 2FA, login wall, payment prompt, or an
 *  L1 buy click. Surfaces the live-view URL to the principal (push + web
 *  app) and parks the job in TAKEOVER_WAIT until the human resolves. */
export default defineTool({
  description:
    "Request human takeover in the live browser view; the job suspends until the human resolves and resumes it.",
  inputSchema: z.object({
    job_id: z.string().min(1),
    reason: z.enum(["captcha", "2fa", "login", "payment_prompt", "l1_buy_click", "other"]),
    detail: z.string().optional(),
  }),
  async execute({ job_id, reason }) {
    // TODO(bringup): resolve the workflow's `takeover` hook with the
    // BrowserHost live-view URL.
    throw new Error(
      `kerf bringup: takeover hook not wired yet (job ${job_id}, ${reason})`,
    );
  },
});

import { defineAgent } from "eve";

/**
 * The kerf Tier-2 operator. It navigates and repairs; it cannot spend.
 * The buy click, card entry, and mandate checks are workflow steps that
 * exist outside this agent's toolset entirely — see instructions.md and
 * docs/architecture.md ("capability gating is workflow structure, not
 * prompting").
 */
export default defineAgent({
  model: "anthropic/claude-sonnet-5",
});

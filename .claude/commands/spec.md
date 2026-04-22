---
name: spec
description: Turn the current chat thread into a GitHub issue spec for an agent to execute
---

You are turning the current conversation into a tight, executable spec and filing it as a GitHub issue. The issue will be picked up by `.github/workflows/agent.yml` when labeled `agent:ready`, so the spec must be self-contained — the agent will not see this conversation.

## Process

1. **Draft.** Read back over the conversation so far and render a spec using the template below. Pull the Goal, Context, Constraints, and Acceptance Criteria from what has actually been discussed — do not invent requirements.

2. **Gate on missing sections.** Every section except "Out of scope" is required. If any required section cannot be filled from the conversation, stop and ask the user to clarify before drafting. Do not pad a section with "TBD" or guesses.

3. **Show the draft to the user in full.** Include the title (under 70 chars) and the complete body. Do not call `gh` yet.

4. **Require explicit approval.** Ask the user to confirm with "send it" or equivalent. If they say anything else, treat it as a revision request. Never file the issue without an explicit go — mirrors the email-approval rule in the user's global CLAUDE.md.

5. **File it.** Once approved, run:
   ```bash
   gh issue create \
     --title "<title>" \
     --label "agent:ready" \
     --body-file -
   ```
   piping the rendered body on stdin. Print the returned issue URL.

6. **Do not comment further.** The agent workflow takes over from here.

## Spec template

```markdown
## Goal
<one sentence, concrete>

## Context / Why
<what prompted this, links to prior work, related issues/PRs>

## Constraints
- <non-negotiables: perf, API stability, coord system (Z-up), etc.>
- <each constraint on its own line>

## Acceptance criteria
- [ ] <testable bullet>
- [ ] <testable bullet>

## Files likely touched
- `path/to/file.rs`
- `path/to/other.ts`

## Out of scope
- <explicit non-goals, if any>

## Verification
Run before opening the PR:
- `cargo test -p <crate>` / `cargo clippy --workspace -- -D warnings`
- `npm run build -w @vcad/<package>` / `npm test -w @vcad/<package>`
- <any manual verification the agent cannot run>
```

## Notes

- Keep the spec shorter than 60 lines. If it's longer, the task is too big — suggest splitting it into separate issues.
- Acceptance criteria must be testable. "Works correctly" is not acceptance; "`cargo test -p vcad-cli` passes with a new test for `--version`" is.
- The `Verification` section should reuse commands already in `CLAUDE.md` or `ci.yml` where possible — the agent builds on the same CI toolchain.

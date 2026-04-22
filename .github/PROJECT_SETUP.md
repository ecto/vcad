# Project board setup (one-time)

The agent workflow (`.github/workflows/agent.yml`) and the `/spec` slash command use three labels to drive a kanban board. This file documents the one-time manual setup.

## 1. Labels

Create these labels on the repo (Settings → Labels, or via `gh label create`):

| Label | Color | Purpose |
|---|---|---|
| `agent:ready` | green | Spec is complete and ready for the agent to pick up. |
| `agent:running` | yellow | Agent workflow is currently executing. Applied by the workflow. |
| `agent:failed` | red | Agent workflow errored. Applied by the workflow; needs human triage. |

```bash
gh label create agent:ready   --color 0e8a16 --description "Ready for the agent to pick up"
gh label create agent:running --color fbca04 --description "Agent workflow is executing"
gh label create agent:failed  --color b60205 --description "Agent workflow errored — human triage needed"
```

## 2. Project v2 board

Create a Project and link it to this repo:

1. Create a Project named **vcad agent queue** (user or org scope).
2. Add a **Status** field with columns: **Backlog**, **Ready**, **In Progress**, **Review**, **Done**.
3. Link the repo so new issues auto-add to the Project.

## 3. Project automations

All configurable in the Project's "Workflows" UI — no code required.

| Trigger | Effect |
|---|---|
| Item added (any issue) | Status → **Backlog** |
| Label `agent:ready` added | Status → **Ready** |
| Label `agent:running` added | Status → **In Progress** |
| Linked PR opened | Status → **Review** |
| Issue closed | Status → **Done** |
| Label `agent:failed` added | Status → **Backlog** (so it re-surfaces for triage) |

The workflow only toggles labels; the Project board handles column moves.

## 4. Secrets

Add to repo secrets (Settings → Secrets and variables → Actions):

- `ANTHROPIC_API_KEY` — required by `claude-code-action`.

`GITHUB_TOKEN` is provided automatically by Actions and has enough scope for the label/PR operations the workflow performs.

### Scope note for local `gh` usage

`gh project` commands require the `project` scope, which is not part of the default `gh auth login` set. If `gh project list` returns an empty result or fails, run:

```bash
gh auth refresh -s project,read:project
```

## 5. Smoke test

1. From a chat, run `/spec` on a trivial task and approve the draft.
2. Confirm the issue exists with `agent:ready` and the board card is in **Ready**.
3. Watch the **Agent** workflow run. Card should move to **In Progress** when `agent:running` is applied.
4. Once the PR opens, card moves to **Review**. Merging moves it to **Done**.

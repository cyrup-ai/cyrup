---
stage: new
status: done
updated: 2026-08-15 21:16
---

# FLUX_06 — The `flux` skill (pipeline docs surface)

## OBJECTIVE

Create `/Users/davidmaple/cyrup.ai/cyrup-flux/skills/flux/SKILL.md` so the pipeline's
operational docs load via `/skill:flux` and via auto-skill-loading — the cyrup equivalent of
code-puppy's co-located `_docs/` (spec [§3.3 rule 8](../flux.md)). The four reference docs
were already vendored into `skills/flux/reference/` by FLUX_01; this task writes the SKILL.md
that ties them together.

## SUBTASKS

### SUBTASK 1: Write `skills/flux/SKILL.md`

Frontmatter contract (from [`../../crates/cyrup-resources/src/skill.rs`](../../crates/cyrup-resources/src/skill.rs)):
`name` optional (falls back to the directory name — `flux` is already valid: lowercase,
≤ max length), `description` REQUIRED ("use this skill when…" — a skill with no description is
dropped). Model:
[`../../crates/cyrup-ext-subagents/resources/skills/pi-subagents/SKILL.md`](../../crates/cyrup-ext-subagents/resources/skills/pi-subagents/SKILL.md).

```markdown
---
name: flux
description: |
  Run the Flux structured development pipeline (new → ask → split → aug → exec →
  qa → tests → commit → create-pr) whose state lives in ~/.flux/<flattened-cwd>/.
  Use when the user invokes /flux/* commands, asks about flux task files, pipeline
  stages A–D, auto-pilot, or wants to resume pipeline state after a crash.
---

# Flux — structured AI dev pipeline

<Task-body content, per SUBTASK 2.>
```

### SUBTASK 2: SKILL.md body content

Compose from the vendored sources (do not invent new semantics):

1. **TL;DR** — the first sections of
   [`reference/README.md`](../../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/_docs/README.md):
   what a task file is, the `~/.flux/<flattened-dir>/` root, the core pipeline line, the
   optional `/flux/config` first-time setup.
2. **Command table** — the "Available Commands" table from
   [`reference/pipeline.md`](../../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/_docs/pipeline.md),
   plus the unified-arguments table (task-file | N | all | empty).
3. **Native commands note** — `/flux/status`, `/flux/cheatsheet`, `/flux/about` are native
   commands (Phase 2, FLUX_07/08); until then, inspect state manually:
   ``ls ~/.flux/$(printf '%s' "$(pwd -P)" | tr -cs 'a-zA-Z0-9' '-')/todo/`` (spec §3.3 rule 8).
4. **Reference pointers** — "References are relative to this file's directory": point at
   `reference/pipeline.md` (pipelines A–D), `reference/cheatsheet.md`, `reference/synopsis.md`,
   `reference/README.md`.
5. **Crash-resume + context hygiene** — from README: state survives crashes; `/clear` between
   steps is encouraged because aug output persists to disk. Adapt the `/clear` mention to
   cyrup's session model (a fresh session resumes from the same `~/.flux` state).

Keep the body tight (operational, not tutorial). No extensive documentation.

### SUBTASK 3: Verify loading

```bash
cd /tmp/flux-scratch   # any repo with the package installed
cyrup -p "/skill:flux summarize the pipeline stages"
```

The expanded text must contain the `<skill name="flux" location="…">` block (skill expansion
runs in `expand_input_text` before template expansion —
[`../../crates/cyrup-session-svc/src/session.rs`](../../crates/cyrup-session-svc/src/session.rs)
`expand_skill_command`). Also confirm the skill appears in the TUI command list with its
description.

## RESEARCH NOTES

- Skills and templates share preflight expansion: `/skill:name args` → skill block + args
  (spec §0.2). The skill is content-only; no code.
- `skills/flux/reference/` is already in place (FLUX_01 SUBTASK 3) and is NOT scanned as
  skills (only `SKILL.md` at the skill-dir root is the entry; `reference/` is plain content
  reachable by the model's file tools).
- The skill also ships in the Phase 2 crate (`resources/skills/flux/SKILL.md`, FLUX_11) —
  write it once here; FLUX_11 copies it verbatim.

## DEFINITION OF DONE

- [ ] `/skill:flux` expands to the skill block in `cyrup -p` output.
- [ ] The skill appears in the command list with its `description`.
- [ ] The body contains the command table, the unified-arguments table, the native-commands
      note with the manual-inspection one-liner, and the four `reference/` pointers.
- [ ] `cyrup list` still shows a healthy `cyrup-flux` package (no manifest warnings).

No tests to be written. No benchmarks to be written.

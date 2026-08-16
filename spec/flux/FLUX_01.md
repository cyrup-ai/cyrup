---
stage: new
status: done
updated: 2026-08-15 21:16
---

# FLUX_01 — Scaffold the `cyrup-flux` package repository

## OBJECTIVE

Create the installable prompt-template package that carries all of Phase 1, at
`/Users/davidmaple/cyrup.ai/cyrup-flux` (a NEW sibling git repo of the cyrup workspace — the
parent spec §3.3 calls for "a git repo `cyrup-flux` installable via `cyrup install git:<url>`
(and `cyrup install <local-path>` for development)"). After this task, subsequent tasks only
add content files; the install mechanics are proven once, here.

Parent spec sections: [§3.3 Phase 1](../flux.md), [§2.2 package system](../flux.md).

## SUBTASKS

### SUBTASK 1: Create the repo + directory skeleton

```bash
mkdir -p /Users/davidmaple/cyrup.ai/cyrup-flux
cd /Users/davidmaple/cyrup.ai/cyrup-flux
git init
mkdir -p prompts/flux/_docs skills/flux/reference
```

Why these paths: `prompts/` and `skills/` are the conventional resource dirs a cyrup package
exposes ([`../../crates/cyrup-resources/src/package/manifest.rs`](../../crates/cyrup-resources/src/package/manifest.rs)
— `resolve_manifest` auto-discovery). `prompts/flux/_docs/` is skipped by the recursive
scanner's `_`-prefix rule ([`../../crates/cyrup-resources/src/discovery.rs`](../../crates/cyrup-resources/src/discovery.rs)
`scan_prompt_dir`), so reference docs co-locate with the commands without registering.

### SUBTASK 2: Write `cyrup.toml` (verbatim from spec §3.3)

```toml
[package]
name = "cyrup-flux"
version = "1.0.0"
description = "Flux — structured, file-persisted AI development pipeline (new → ask → split → aug → exec → qa → tests → commit → create-pr)"

[resources]
prompts = ["prompts"]
skills = ["skills"]
```

### SUBTASK 3: Vendor the four reference docs — byte-identical, twice

```bash
SRC=/Users/davidmaple/cyrup.ai/cyrup/tmp/code-puppy/flux_bootstrap/bundled/commands/flux/_docs
DST=/Users/davidmaple/cyrup.ai/cyrup-flux
cp "$SRC/README.md" "$SRC/pipeline.md" "$SRC/cheatsheet.md" "$SRC/synopsis.md" "$DST/prompts/flux/_docs/"
cp "$SRC/README.md" "$SRC/pipeline.md" "$SRC/cheatsheet.md" "$SRC/synopsis.md" "$DST/skills/flux/reference/"
```

Source: [`../../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/_docs/`](../../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/_docs/).
Verify with `diff -r "$SRC" "$DST/prompts/flux/_docs"` — must be silent. These are reference
content, not commands; do not edit them (the cheatsheet renderer in FLUX_08 parses
`pipeline.md`, so byte-identity matters).

### SUBTASK 4: Minimal README.md

One paragraph + the install command. Nothing more (no extensive documentation):

```markdown
# cyrup-flux

Flux for cyrup — a structured, file-persisted AI development pipeline:
`new → ask → split → aug → exec → qa → tests → commit → create-pr`.
State lives in `~/.flux/<flattened-cwd>/`. Install: `cyrup install <path-to-this-repo>`
(global) or `cyrup install <path> -l` (project scope). Run `/flux/new <task>` to start.
```

### SUBTASK 5: Prove the install mechanics

```bash
cyrup install /Users/davidmaple/cyrup.ai/cyrup-flux
cyrup list
```

The package must appear in `cyrup list`. Confirm the `_docs` dir registered NOTHING:
`cyrup -p "/flux/_docs/README"` must pass the text through unexpanded (no such template), and
no `flux/*` names appear in the TUI command list yet (no templates ported — that is FLUX_02+).

## RESEARCH NOTES

- Package manifest + install machinery: [`../../crates/cyrup-resources/src/package/`](../../crates/cyrup-resources/src/package/),
  CLI verbs in [`../../crates/cyrup/src/subcommands.rs`](../../crates/cyrup/src/subcommands.rs)
  (`install`/`remove`/`update`/`list`).
- Precedence (project > global > built-in, first-wins): `ResourceSet::build` in
  [`../../crates/cyrup-resources/src/discovery.rs`](../../crates/cyrup-resources/src/discovery.rs).
- The eventual Phase 2 canonical home for this content is `crates/cyrup-ext-flux/resources/`
  (FLUX_11); this package is the Phase 1 distribution channel and later re-vendors from the
  crate (spec §3.4.1 "Bundling = single source of truth").

## DEFINITION OF DONE

- [ ] `/Users/davidmaple/cyrup.ai/cyrup-flux` is a git repo with the skeleton above and an initial commit.
- [ ] `cyrup install /Users/davidmaple/cyrup.ai/cyrup-flux` succeeds; `cyrup list` shows `cyrup-flux`.
- [ ] `diff -r` proves the four `_docs` files byte-identical to the code-puppy source (both copies).
- [ ] No `flux/*` template names resolve yet (`_docs` correctly skipped).

No tests to be written. No benchmarks to be written.

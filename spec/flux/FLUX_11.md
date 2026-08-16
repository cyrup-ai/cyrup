---
stage: new
status: done
updated: 2026-08-15 21:16
---

# FLUX_11 — Bundled resources: the crate becomes canonical (flux out of the box)

## OBJECTIVE

Move the 15 templates + `_docs/` + skill into `crates/cyrup-ext-flux/resources/` and
contribute them to every session through the `ResourcesDiscover` hook, so flux works with **no
install step** (spec [§3.4.1 "Bundling = single source of truth"](../flux.md)). After this
task the crate is the canonical home of the content; the `cyrup-flux` package re-vendors from
it (the package remains the channel for pinning/auditing/overriding).

## SUBTASKS

### SUBTASK 1: Vendor the content into the crate

```bash
PKG=/Users/davidmaple/cyrup.ai/cyrup-flux
RES=/Users/davidmaple/cyrup.ai/cyrup/crates/cyrup-ext-flux/resources
cp "$PKG"/prompts/flux/*.md "$RES/prompts/flux/"          # the 15 templates
cp "$PKG"/prompts/flux/_docs/*.md "$RES/prompts/flux/_docs/"  # 4 docs (about.md already there from FLUX_08 — keep both copies identical where they overlap)
mkdir -p "$RES/skills/flux"
cp "$PKG/skills/flux/SKILL.md" "$RES/skills/flux/SKILL.md"
cp -R "$PKG/skills/flux/reference" "$RES/skills/flux/reference"
```

`diff -r` the overlapping `_docs` files (FLUX_08 already vendored them) — must be silent.

### SUBTASK 2: `bundled_dir()` + the `ResourcesDiscover` hook

Add to the crate (lib.rs or a small `resources.rs`):

```rust
pub fn bundled_dir() -> std::path::PathBuf {
    std::env::var_os("CYRUP_FLUX_RESOURCES_DIR")
        .map(Into::into)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources"))
}
```

(The `cyrup-ext-subagents` pattern —
[`registration/resources.rs`](../../crates/cyrup-ext-subagents/src/registration/resources.rs)
`bundled_resources_dir()`.)

In `extension.rs`:

```rust
// init:
api.subscribe(&[EventKind::ResourcesDiscover]);

// on_event (returns HookOutcome directly, NOT a Result — native.rs:463):
async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
    if matches!(ev, HostEvent::ResourcesDiscover { .. }) {
        // CRITICAL (spec §0.4): contribute the prompts/ DIRECTORY, not individual files —
        // add_prompt_path loads a file by BASENAME (losing the `flux/` namespace) and a
        // directory via the recursive namespaced scanner (discovery.rs:1929-1958).
        return HookOutcome::Handled(HandledValue(serde_json::json!({
            "promptPaths": [bundled_dir().join("prompts")],
            "skillPaths":  [bundled_dir().join("skills/flux/SKILL.md")],
        })));
    }
    HookOutcome::Noop
}
```

The host loads contributions at `ResourceScope::Discovered` (rank 6 — a floor, never an
override; user/project/package `flux/*` resources still win, spec §0.4/§5.7).

### SUBTASK 3: Re-sync the package from the crate

The crate is now canonical. Replace the package's content with a copy FROM the crate (this
direction, from now on):

```bash
RES=/Users/davidmaple/cyrup.ai/cyrup/crates/cyrup-ext-flux/resources
PKG=/Users/davidmaple/cyrup.ai/cyrup-flux
rsync -a --delete "$RES/prompts/flux/" "$PKG/prompts/flux/"
rsync -a --delete "$RES/skills/flux/" "$PKG/skills/flux/"
cd "$PKG" && git add -A && git commit -m "Re-vendor from cyrup-ext-flux resources (canonical)"
```

(Delete-before-copy on the package side removes the `_docs/about.md` asymmetry if the crate
keeps it — decide once: `_docs/about.md` STAYS in the crate's `_docs/` as the render_about
source, and also ships in the package's `_docs/`; it is reference content in both places and
never registers.)

### SUBTASK 4: Build + behavioral check

```bash
cargo build -p cyrup-ext-flux && cargo build -p cyrup
cyrup remove cyrup-flux   # prove the out-of-box path with NO package installed
```

- In a scratch repo: `cyrup -p "/flux/new smoke test"` expands the bundled template (look for
  the task-creation instructions in the expanded text) — proving the directory contribution
  registered `flux/new` and not a basename `new` (spec §0.4 gotcha).
- The TUI command list shows all 15 `/flux/<step>` entries plus the three native commands.
- `/skill:flux` expands from the bundled skill.
- Reinstall the package (`cyrup install /Users/davidmaple/cyrup.ai/cyrup-flux`) and confirm no
  duplicate `flux/*` entries (first-wins precedence dedupes by normalized key —
  [`ResourceSet::build`](../../crates/cyrup-resources/src/discovery.rs)).

## RESEARCH NOTES

- `HostEvent::ResourcesDiscover` = event kind 5
  ([`event.rs`](../../crates/cyrup-ext/src/event.rs)); the aggregation seam concatenates all
  extensions' contributions ([`dispatch.rs`](../../crates/cyrup-ext/src/dispatch.rs) :316+).
- `HookOutcome`/`HandledValue`:
  [`contract.rs`](../../crates/cyrup-ext/src/contract.rs) :12, :40.
- The subagents reference implementation of this exact hook:
  [`extension.rs`](../../crates/cyrup-ext-subagents/src/extension.rs) :11013-11033 (it
  contributes FILES because flat names suit it; flux needs the namespace — directory).
- Template/skill content is edited ONLY in the crate from this task forward; FLUX_12's GAP
  sweep edits the crate copy, then re-syncs the package the same way as SUBTASK 3.

## DEFINITION OF DONE

- [ ] With the package REMOVED, all 15 `/flux/<step>` templates + `/skill:flux` resolve out of
      the box, under their namespaced names (the directory-contribution check).
- [ ] With the package REINSTALLED, no duplicates; the package content is byte-identical to
      the crate's `resources/` tree (`diff -r` silent after SUBTASK 3).
- [ ] Native commands (`/flux/status`, `/flux/cheatsheet`, `/flux/about`) and the overlay are
      unaffected; crate + binary build cleanly.

No tests to be written. No benchmarks to be written.

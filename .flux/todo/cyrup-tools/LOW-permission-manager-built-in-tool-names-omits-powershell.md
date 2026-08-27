---
stage: exec
status: in-progress
priority: MEDIUM
tool: powershell
source: QA follow-up from the powershell task
updated: 2026-08-27 18:00
---

# `BUILT_IN_TOOL_NAMES` in the permission manager still lists seven tools

[`crates/cyrup-permission-system/src/manager.rs`](../../../crates/cyrup-permission-system/src/manager.rs)
`:36` declares

```rust
const BUILT_IN_TOOL_NAMES: [&str; 7] = ["bash", "read", "write", "edit", "grep", "find", "ls"];
```

while [`crates/cyrup-tools/src/registry.rs`](../../../crates/cyrup-tools/src/registry.rs) `:21`
(`BUILTIN_NAMES`) is now `[&str; 8]` — `read, bash, powershell, edit, write, grep, find, ls`. The
set difference is exactly `{ "powershell" }`; every other name is present in both.

> **PRIORITY RAISED — LOW → MEDIUM.** The filing's "benign" claim is **half right**. It is correct
> for the dispatch consumer (`check_permission`) and I verified that arm-equivalence below. It is
> **wrong for the second consumer**, `normalize_raw_permission` (`:1076`), where the missing name
> silently **discards an explicit `deny`** and can invert an `allow`/`deny` decision. See
> "Where it is NOT benign".

## The constant has exactly two consumers

`is_built_in_tool` (`:45-47`) is `BUILT_IN_TOOL_NAMES.contains(&name)`, called from precisely two
places (`grep -n "is_built_in_tool" manager.rs` → `:46`, `:298`, `:1076`):

| # | Call site | Role |
|---|-----------|------|
| 1 | `manager.rs:298`, inside `PermissionManager::check_permission` (`:148`) | selects the "built-in path tools" arm over the extension-tool fallthrough |
| 2 | `manager.rs:1076`, inside `normalize_raw_permission` (`:1062`) | folds a **top-level shorthand key** into the `tools` record |

### Consumer 1 — dispatch. VERIFIED BENIGN.

`check_permission` reaches the built-in arm only after the `special` (`:158`), `skill` (`:182`),
`bash` (`:221`) and `mcp` (`:245`) arms have all returned. For `powershell` today, control falls
past `is_built_in_tool` at `:298` into the extension-tool arm at `:317-335`.

The two arms are provably the same computation for a tool whose input carries no path:

* **Built-in arm** (`:298-315`) — `create_action_resource_targets(normalized, input)` (`:882-887`)
  returns `vec![]` unless `path_resource_from_input` (`:873-880`) finds a non-empty `path` **or**
  `file_path` in the input. The `powershell` tool
  ([`crates/cyrup-tools/src/tools/powershell.rs`](../../../crates/cyrup-tools/src/tools/powershell.rs))
  is a `ShellTool` — its arguments are `command`/`cwd`/`timeout`, never `path`/`file_path` — so the
  target vector collapses to `["powershell"]`.
* With a **single** name, `find_by_pattern_order_for_names(patterns, ["powershell"])` (`:804-834`)
  and `find_compiled_match(patterns, "powershell")` (`:740-758`) are the same reverse
  (last-match-wins) scan over `compiled_tools` with the same trusted-`deny` floor —
  `find_match_index` is itself `for index in (0..patterns.len()).rev()`
  ([`crates/cyrup-permission-system/src/wildcard.rs`](../../../crates/cyrup-permission-system/src/wildcard.rs)
  `:127-137`), which is exactly the outer loop of `find_by_pattern_order_for_names`.
* Both arms fall back to `default_state(&resolved.layers, DefaultCategory::Tools)` and then to
  `PermissionState::Ask` (`DefaultPolicy::default()`,
  [`types.rs`](../../../crates/cyrup-permission-system/src/types.rs) `:50-61`, every category `ask`).

The **only** field that differs is `target`: the built-in arm reports `Some("powershell")` on a
match where the extension arm reports `None`. I traced every reader of
`PermissionCheckResult::target` and none of them changes a decision for this tool:

* `gate::get_pattern_approval_subject` ([`gate.rs`](../../../crates/cyrup-permission-system/src/gate.rs)
  `:150-178`) — the `Some(target)` arm strips a `"powershell:"` prefix that is not there and yields
  `"powershell"`; the `None` path falls through to `result.tool_name`, also `"powershell"`.
  **Identical**, so an "Allow Always" persists the same rule key before and after.
* `gate::format_deny_reason` (`:239-260`), `gate::hard_stop_hint` (`:229-237`),
  `gate::format_user_denied_reason` (`:263-283`) — every `target`-sensitive arm is additionally
  gated on `source == CheckSource::Mcp || tool_name == "mcp"`, false here. **Identical text.**
* `extension::audit::permission_decision_scope`
  ([`audit.rs`](../../../crates/cyrup-permission-system/src/extension/audit.rs) `:100-115`) — first
  non-empty of `target → command → path → tool_name`. `Some("powershell")` and the `tool_name`
  fallback both yield `"powershell"`. **Identical.**
* `dedup::DedupDetails::cache_key` ([`dedup.rs`](../../../crates/cyrup-permission-system/src/dedup.rs)
  `:65-88`) — the SHA-256 fingerprint gains `"target":"powershell"` in place of `"target":null` on
  a matched rule. The fingerprint is explicitly documented as needing only internal per-process
  consistency, `toolName` is in the same object so no cross-tool collision is possible, and the
  change is uniform for every `powershell` prompt. **No behavioural effect.**

So the filing's dispatch reasoning stands, and it is confirmed rather than assumed. It was also
correctly out of the powershell task's scope: `manager.rs` is a port of `permission-manager.ts`,
which is **not vendored** (`find tmp/pi -name "*permission*"` returns only
`examples/extensions/permission-gate.ts`), so there is no upstream to diff.

### Where it is NOT benign — consumer 2, `normalize_raw_permission:1076`

```rust
// Fold top-level built-in/special state keys into their category (pi `:150-163`).
if let Some(entries) = raw.as_object() {
    for (key, val) in entries {
        let Some(state) = val.as_str().and_then(PermissionState::parse) else { continue };
        if is_built_in_tool(key) {
            normalized.tools.insert(key.clone(), state);
        } else if is_special(key) {
            normalized.special.insert(key.clone(), state);
        }
    }
}
```

This is the **shorthand ingestion** path: `bash: "deny"` written as a sibling of `tools`/`bash`/
`mcp` (rather than nested under `tools`) becomes `tools["bash"] = deny`. It is how the seven listed
names get their shorthand honoured. A key that is neither a built-in nor a special is **silently
dropped** — no warning, no diagnostic.

`normalize_raw_permission` runs on **three** live layers:

* `load_global_config` (`:495`) — the trusted global layer,
* `load_project_global_config` (`:515`) — the untrusted project layer,
* `load_agent_permissions_from` (`:600`) — the **trusted agent layer**, from the `permission:` block
  of an agent markdown frontmatter.

Today a `powershell: deny` shorthand on any of those three is thrown away. The resulting state is
whatever `defaultPolicy.tools` says. **If `defaultPolicy.tools` is `allow`, an explicit, trusted
`deny` resolves to `allow`.** That is an allow/deny inversion, not a cosmetic gap.

Two aggravating details:

* The agent-frontmatter surface has **no schema at all** — it goes through
  `common::extract_frontmatter` (`common.rs:132-142`) and `common::parse_simple_yaml_map`, never
  through JSON Schema validation. So nothing warns the author.
* The JSONC config surface *is* schema'd
  ([`schemas/cyrup-permissions.schema.json`](../../../crates/cyrup-permission-system/schemas/cyrup-permissions.schema.json),
  top-level `"additionalProperties": false`), so an editor flags a top-level `powershell` there —
  but the schema is advisory: `load_global_config` parses with `jsonc::parse_ordered_config` and
  never validates, so the key is still ingested (and still dropped).

That is the real defect, and it is what makes this worth closing rather than filing as cosmetic.

## Can `BUILT_IN_TOOL_NAMES` be derived from `cyrup_tools::BUILTIN_NAMES`? Yes — and it must not be.

**The dependency is mechanically possible. There is no cycle.**

* [`crates/cyrup-permission-system/Cargo.toml`](../../../crates/cyrup-permission-system/Cargo.toml)
  `:25-71` does **not** name `cyrup-tools` today. Its cyrup edges are `cyrup-core`, `cyrup-ext`,
  `cyrup-ext-subagents`.
* [`crates/cyrup-tools/Cargo.toml`](../../../crates/cyrup-tools/Cargo.toml) `:13-32` has exactly one
  cyrup normal edge: `cyrup-core`. `cyrup-core` has **no** cyrup dependencies at all. So no normal
  path `cyrup-tools → … → cyrup-permission-system` exists and a new edge cannot close a cycle.
* `cyrup-tools` is **already in the graph**: `cyrup-ext` depends on it
  ([`crates/cyrup-ext/Cargo.toml`](../../../crates/cyrup-ext/Cargo.toml) `:23`), and
  `cyrup-permission-system` depends on `cyrup-ext`. Its dev graph already carries it twice more via
  `cyrup-session-svc` (`:34`) and `cyrup-test-support` (`:22`). Adding an edge compiles nothing new.
* `cyrup-tools` is a `[workspace.dependencies]` entry with a `version`
  ([root `Cargo.toml`](../../../Cargo.toml) `:121`), so it satisfies the `PERM-029` `cargo package`
  constraint the manifest comment at `:10-13` is protecting.

**But aliasing the two constants is the wrong fix, because they answer different questions.**

`cyrup_tools::BUILTIN_NAMES` is *"what `ToolRegistry::with_builtins` installs, in Pi's wire order"*
— its doc comment at `registry.rs:11-20` is entirely about provider-request ordering.
`BUILT_IN_TOOL_NAMES` is Pi's `permission-manager.ts` constant: *"which permission keys are
built-in-tool keys"*, i.e. which top-level shorthand folds into `tools` and which names take
`action:resource` sub-targets. The two sets coincide **today**; they are not the same predicate.

The concrete failure of an alias: `mcp` is a registered tool name in this workspace
([`crates/cyrup-mcp/src/registration.rs`](../../../crates/cyrup-mcp/src/registration.rs) `:126`
lists it in that crate's own `BUILTIN_NAMES`). The moment `mcp` — or any future name whose policy
belongs in the `mcp`/`special` category — lands in `cyrup_tools::BUILTIN_NAMES`, an alias would make
`is_built_in_tool("mcp")` true, fold a top-level `mcp: "deny"` into the **`tools`** record, and
hijack the dedicated `mcp` arm at `:245-294` through its `tool_match` fallback (`:264-273`). A
silent, wrong-category rule move is a worse failure mode than the drift being fixed. `manager.rs`'s
module doc opens with "Entirely host-independent"; a normal edge onto the concrete tool registry for
one array of `&str` gives that up for no compile-time benefit.

**Therefore: keep the literal, and make drift a hard test failure** — precisely the shape the
powershell task already established at
[`crates/cyrup-session-svc/src/builder.rs`](../../../crates/cyrup-session-svc/src/builder.rs)
`:2409-2429` (`every_builtin_is_gated_and_powershell_is_not_a_default`), which keeps
`ALL_BUILTIN_TOOLS` a literal and iterates `cyrup_tools::BUILTIN_NAMES` asserting membership in
both directions plus a length equality. A **dev**-dependency, not a normal one, so the runtime crate
stays decoupled and the test still cannot be fooled.

---

## Required implementation

Four edits. No alternatives.

### 1. `crates/cyrup-permission-system/Cargo.toml` — dev-dependency only

Append to the existing `[dev-dependencies]` block (after `async-trait`, `:99`):

```toml
# TEST-ONLY oracle for `manager.rs::built_in_tool_names_tracks_the_tool_registry`. `BUILT_IN_TOOL_NAMES`
# is Pi's `permission-manager.ts` constant (which permission KEYS are built-in-tool keys), NOT a
# mirror of the registry (which tools are INSTALLED, in wire order) — the two answer different
# questions and must stay separately stated, so this is deliberately a dev edge and `manager.rs`
# never names `cyrup_tools`. The test is what makes the sets provably equal. No new compile surface:
# `cyrup-tools` is already normal-reachable via `cyrup-ext` and dev-reachable via `cyrup-session-svc`
# + `cyrup-test-support`, with the same default `inline-images` feature.
cyrup-tools = { workspace = true }
```

Do **not** add it to `[dependencies]`.

### 2. `manager.rs:36` — the constant

Replace `:36` with:

```rust
/// The permission keys that are built-in **tool** keys (Pi `permission-manager.ts`
/// `BUILT_IN_TOOL_NAMES`): the names whose top-level shorthand folds into the `tools` record
/// ([`normalize_raw_permission`]) and which take `action:resource` sub-targets in
/// [`PermissionManager::check_permission`].
///
/// This is deliberately a LITERAL, not an alias of `cyrup_tools::BUILTIN_NAMES`. That constant
/// states which tools the registry INSTALLS and in what wire order; this one states which
/// permission keys belong to the `tools` CATEGORY. They coincide today and
/// `built_in_tool_names_tracks_the_tool_registry` fails the build if they ever stop — but an alias
/// would auto-adopt any future registry name, and a name whose policy belongs in `mcp`/`special`
/// (`mcp` is already a registered tool name in `cyrup-mcp`) would then be folded into `tools` and
/// hijack the dedicated arm at `:245`. Silent wrong-category ingestion is worse than the drift.
///
/// `powershell` sits immediately after `bash`, mirroring `cyrup_tools::BUILTIN_NAMES`' rationale
/// (`registry.rs:11-20`); order is irrelevant to `contains`, readability is not.
const BUILT_IN_TOOL_NAMES: [&str; 8] =
    ["bash", "powershell", "read", "write", "edit", "grep", "find", "ls"];
```

### 3. `manager.rs:297` — correct the now-wrong arm comment

`powershell` is not a path tool; the arm no longer contains only path tools. Replace the `:297`
comment with:

```rust
        // built-in tools. `read`/`write`/`edit`/`grep`/`find`/`ls` are path-bearing and pick up an
        // `action:resource` sub-target; `powershell` is not, so `create_action_resource_targets`
        // yields nothing for it and this arm reduces to the same single-name, last-match-wins scan
        // over `compiled_tools` (plus the same `DefaultCategory::Tools` fallback) that the
        // extension arm at `:317` runs. It is here for the `normalize_raw_permission:1076`
        // shorthand fold, which is where membership actually changes behaviour.
```

### 4. `manager.rs` `mod tests` (`:1087`) — the two tests

Add both to the existing test module. The first is the anti-drift guard; the second is the RED
proof of the actual defect. `use super::*;` at `:1090` already brings `BUILT_IN_TOOL_NAMES`,
`PermissionState`, `write` and `manager_with_global` (`:1099`) into scope.

```rust
    /// ANTI-DRIFT. `BUILT_IN_TOOL_NAMES` and `cyrup_tools::BUILTIN_NAMES` are separately stated on
    /// purpose (see the const's doc), so nothing but this test keeps them in step. Mirrors
    /// `cyrup-session-svc/src/builder.rs::every_builtin_is_gated_and_powershell_is_not_a_default`
    /// (`:2409`), which does the same both-directions check for `ALL_BUILTIN_TOOLS`.
    #[test]
    fn built_in_tool_names_tracks_the_tool_registry() {
        for name in cyrup_tools::BUILTIN_NAMES {
            assert!(
                BUILT_IN_TOOL_NAMES.contains(&name),
                "`{name}` is installed by `ToolRegistry::with_builtins` but missing from \
                 BUILT_IN_TOOL_NAMES, so `normalize_raw_permission` SILENTLY DROPS a top-level \
                 `{name}: <state>` shorthand rule instead of folding it into `tools` — an explicit \
                 deny would resolve to `defaultPolicy.tools`"
            );
        }
        // Reverse direction: nothing is treated as a built-in key that the registry never installs.
        for name in BUILT_IN_TOOL_NAMES {
            assert!(
                cyrup_tools::BUILTIN_NAMES.contains(&name),
                "BUILT_IN_TOOL_NAMES claims `{name}` is a built-in, but the registry does not \
                 install it — it belongs in the extension-tool arm, not the `tools` fold"
            );
        }
        assert_eq!(BUILT_IN_TOOL_NAMES.len(), cyrup_tools::BUILTIN_NAMES.len());
        assert!(BUILT_IN_TOOL_NAMES.contains(&"powershell"));
    }

    /// RED before the fix. The `permission:` frontmatter block has NO schema (it is parsed by
    /// `common::parse_simple_yaml_map`), so the top-level shorthand is unguarded there. Pre-fix,
    /// `is_built_in_tool("powershell")` was false, `normalize_raw_permission:1076` dropped the key,
    /// and the trusted agent-layer DENY resolved to the global `defaultPolicy.tools: allow`.
    #[test]
    fn agent_frontmatter_powershell_shorthand_is_a_tools_rule() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = manager_with_global(
            dir.path(),
            r#"{"defaultPolicy":{"tools":"allow","bash":"ask","mcp":"ask","skills":"ask"}}"#,
        );
        write(
            &dir.path().join("agents").join("coder.md"),
            "---\npermission:\n  powershell: deny\n---\nbody\n",
        );
        assert_eq!(
            m.check_permission(
                "powershell",
                &serde_json::json!({"command":"Get-ChildItem"}),
                Some("coder")
            )
            .state,
            PermissionState::Deny,
            "a top-level `powershell` shorthand must fold into the `tools` record; dropping it \
             lets `defaultPolicy.tools: allow` override an explicit trusted deny"
        );
        // MIRROR: an already-listed built-in keeps working through the identical path, so the fold
        // is set-membership only and nothing about the seven existing names changed.
        write(
            &dir.path().join("agents").join("reader.md"),
            "---\npermission:\n  read: deny\n---\nbody\n",
        );
        assert_eq!(
            m.check_permission("read", &serde_json::json!({"path":"/tmp/x"}), Some("reader"))
                .state,
            PermissionState::Deny
        );
    }
```

## Definition of done

1. `BUILT_IN_TOOL_NAMES` is `[&str; 8]` and contains `"powershell"`; `manager.rs` still does **not**
   name `cyrup_tools` outside `#[cfg(test)]`.
2. `built_in_tool_names_tracks_the_tool_registry` fails — in both directions, and on length — the
   moment either list changes without the other.
3. `agent_frontmatter_powershell_shorthand_is_a_tools_rule` is RED before edit 2 and green after.
4. `cyrup-tools` appears only under `[dev-dependencies]` in
   `crates/cyrup-permission-system/Cargo.toml`.
5. No resolved-permission change for the seven previously-listed names: the fix is a pure set
   addition, `check_permission`'s built-in and extension arms are proven equivalent for a path-less
   tool (see "Consumer 1"), and the only field that moves for `powershell` is `target`
   (`None` → `Some("powershell")` on a match), which every reader normalises back to the same value.

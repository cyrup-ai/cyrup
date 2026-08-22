---
stage: aug
status: done
updated: 2026-08-22 15:15
---

# Fix The 961 rustdoc Warnings

## Ground truth (re-verified 2026-08-22, no rebuild required)

`cargo doc --workspace --no-deps` exits 0 and emits **exactly 961 warnings**. This was re-derived
without running cargo, from the JSON diagnostics cargo already cached under
`target/debug/.fingerprint/*/output-doc-lib-*` (one NDJSON line per `rustc`/`rustdoc` diagnostic).
Regenerate the full inventory at any time with:

```bash
python3 - <<'PY'
import json, glob, os, collections
best = {}
for f in glob.glob("target/debug/.fingerprint/*/output-doc-*"):
    c = os.path.basename(f).split("output-doc-")[1]
    if c not in best or os.path.getmtime(f) > best[c][0]:
        best[c] = (os.path.getmtime(f), f)
for c, (_, f) in sorted(best.items()):
    for line in open(f, errors="replace"):
        if not line.startswith("{"):
            continue
        j = json.loads(line)
        if j.get("$message_type") != "diagnostic" or j.get("level") != "warning":
            continue
        s = ([x for x in (j.get("spans") or []) if x.get("is_primary")] or [{}])[0]
        print(f'{(j.get("code") or {}).get("code")}\t{s.get("file_name")}:{s.get("line_start")}\t{j["message"]}')
PY
```

Exact lint-code split (this is the real taxonomy; the original table was close but conflated the
per-crate `N warnings emitted` summary lines with real diagnostics):

| count | lint code | class |
|---:|---|---|
| 494 | `rustdoc::broken_intra_doc_links` | `unresolved link to X` |
| 442 | `rustdoc::private_intra_doc_links` | `public documentation for X links to private item Y` |
| 5 | `rustdoc::redundant_explicit_links` | `redundant explicit link target` |
| 1 | `rustdoc::broken_intra_doc_links` | `crate::runtime_api is both a function and a module` |
| 1 | `rustdoc::invalid_html_tags` | `unclosed HTML tag \`cause\`` |
| 18 | — | `N warnings emitted` summary lines (not defects) |

Toolchain is `stable` ([rust-toolchain.toml](../../rust-toolchain.toml)); every crate already carries
`[lints] workspace = true`, so a single `[workspace.lints.rustdoc]` table in
[Cargo.toml](../../Cargo.toml) reaches all 22 members. There is no `.cargo/config.toml` today.

---

## THE HEADLINE FINDING: 382 of the 494 broken links are ONE bug, in ONE crate

`cyrup-ext-subagents` carries 415 of the 494 unresolved links, and **382 of them are reported
against just seven parent files**:

| parent file | unresolved links |
|---|---:|
| [tui/mod.rs](../../crates/cyrup-ext-subagents/src/tui/mod.rs) | 108 |
| [background/mod.rs](../../crates/cyrup-ext-subagents/src/background/mod.rs) | 98 |
| [exec/mod.rs](../../crates/cyrup-ext-subagents/src/exec/mod.rs) | 65 |
| [spawn/mod.rs](../../crates/cyrup-ext-subagents/src/spawn/mod.rs) | 48 |
| [registration/mod.rs](../../crates/cyrup-ext-subagents/src/registration/mod.rs) | 26 |
| [discovery/mod.rs](../../crates/cyrup-ext-subagents/src/discovery/mod.rs) | 20 |
| [lib.rs](../../crates/cyrup-ext-subagents/src/lib.rs) | 17 |

None of those links are *written* in those files. They are written in the **child** modules' `//!`
headers. Here is the mechanism, proven by a controlled comparison inside this repo:

**When a module has BOTH an outer `///` doc on its `pub mod X;` declaration in the parent AND an
inner `//!` header in `X.rs`, rustdoc merges the fragments and resolves every intra-doc link in the
merged doc against the PARENT module's scope. The child's own items — including its own `pub fn`s —
stop resolving, and `super::` shifts up one level.**

Positive case (has an outer doc → breaks):
[tui/mod.rs:74-78](../../crates/cyrup-ext-subagents/src/tui/mod.rs) carries
`/// The FleetView transcript pane — Rust port of pi-subagents ...` immediately above
`pub mod fleet_transcript;`. The child
[tui/fleet_transcript.rs:9](../../crates/cyrup-ext-subagents/src/tui/fleet_transcript.rs) writes
``//! 1. **Read** ([`read_fleet_transcript`], pi `:384-404`).`` and `read_fleet_transcript` is a
`pub fn` **in that same file** at `fleet_transcript.rs:1054`. rustdoc still says
`no item named 'read_fleet_transcript' in scope`, and attributes the warning to `tui/mod.rs`.

Negative case (no outer doc → resolves):
`pub mod profiles;` at [registration/mod.rs:71](../../crates/cyrup-ext-subagents/src/registration/mod.rs)
has **no** `///` above it. Its child
[registration/profiles.rs:22](../../crates/cyrup-ext-subagents/src/registration/profiles.rs) writes
``//! [`apply_profile_to_settings_file`]`` and that link resolves silently to `profiles.rs:552`.
The only two warnings from that file are for `apply_profile`, which genuinely does not exist —
reported against `profiles.rs` itself, not against `mod.rs`.

Same crate, same toolchain, same doc shape. The only variable is the outer `///`.

`super::` behaves exactly as the model predicts: `background/mod.rs`'s warning list contains
``super::control``, ``super::ResultFile``, ``super::error::AmbiguousRunId``,
``super::parent_anchor::detached_runner_env_overlay`` — all written in `background/*.rs` children
where `super` correctly means `background`, all resolved as if `super` meant the crate root.
[tui/fleet_status.rs:7](../../crates/cyrup-ext-subagents/src/tui/fleet_status.rs) writes
``//! the full inspector ([`super::fleet`]).`` and rustdoc reports
`no item named 'fleet' in module 'cyrup_ext_subagents'` — note the module it names.

### The fix: delete the outer doc, fold its prose into the child's `//!`

There are **69** such module declarations. Do not fully-qualify 382 links; delete 69 doc comments.

| file | modules carrying an outer `///` |
|---|---|
| [lib.rs](../../crates/cyrup-ext-subagents/src/lib.rs) | `artifacts`, `native_supervisor`, `missions` (3) |
| [tui/mod.rs](../../crates/cyrup-ext-subagents/src/tui/mod.rs) | `intercom`, `render`, `events`, `notices`, `fleet_theme`, `fleet_transcript`, `fleet_state`, `fleet_status`, `fleet`, `fleet_overlay` (10) |
| [background/mod.rs](../../crates/cyrup-ext-subagents/src/background/mod.rs) | `atomic`, `control`, `cascade`, `spawn_detached`, `parent_anchor`, `reconcile`, `runner_main`, `watch`, `tracker`, `run_status`, `fleet_view`, `wait`, `resume_guidance` (13) |
| [exec/mod.rs](../../crates/cyrup-ext-subagents/src/exec/mod.rs) | `acceptance`, `agent_refinements`, `child_protocol`, `completion_guard`, `control`, `mcp_direct_tools`, `fallback`, `model_scope`, `ndjson`, `output`, `structured`, `task_intent`, `tool_call_summary`, `tool_budget`, `turn_budget`, `capability_ceiling`, `usage_budget`, `spawn_budget`, `tool_availability` (19) |
| [spawn/mod.rs](../../crates/cyrup-ext-subagents/src/spawn/mod.rs) | `depth`, `parallel`, `signal`, `worktree`, `nested_path`, `nested_events`, `chain_graph`, `intercom_target`, `dynamic_fanout` (9) |
| [registration/mod.rs](../../crates/cyrup-ext-subagents/src/registration/mod.rs) | `authority`, `guide`, `slash_commands`, `tool_description`, `cost`, `resources`, `prompt_workflows` (7) |
| [discovery/mod.rs](../../crates/cyrup-ext-subagents/src/discovery/mod.rs) | `types`, `frontmatter`, `agent_memory`, `chains`, `management`, `skills`, `merge`, `settings_write` (8) |

Mechanical transform, per module. Before —
[tui/mod.rs:74-78](../../crates/cyrup-ext-subagents/src/tui/mod.rs):

```rust
/// The FleetView transcript pane — Rust port of pi-subagents `src/tui/fleet-transcript.ts`
/// (`@v0.43.0`): the containment-checked, sanitizing, bounded transcript reader and the
/// event-list renderer behind the inspector's detail pane.
pub mod fleet_transcript;
```

After — `tui/mod.rs` keeps only the declaration:

```rust
pub mod fleet_transcript;
```

…and any prose worth keeping moves into the child's existing `//!` header. Most of these outer docs
are a one-line restatement plus "See that module's own doc for X"; that half is pure scaffolding and
should be **deleted**, not migrated. Only migrate a sentence the child header does not already say.
`fleet_transcript.rs` already opens with the same summary, so nothing migrates here.

**Do the smallest one first as a 2-minute proof.** `pub mod guide;` in
[registration/mod.rs:69](../../crates/cyrup-ext-subagents/src/registration/mod.rs) has a 2-line outer
doc. Delete it, run `cargo doc -p cyrup-ext-subagents --no-deps 2>&1 | grep -c '^warning:'`, and
confirm the count drops by exactly the number of `//!` self-links in
[registration/guide.rs](../../crates/cyrup-ext-subagents/src/registration/guide.rs). Only then do the
other 68.

### Real doc bug found in the same sweep — fix it while you are in `lib.rs`

[lib.rs:37-45](../../crates/cyrup-ext-subagents/src/lib.rs) has two module summaries concatenated
onto **one** declaration, and `pub mod jsonl;` left with no doc at all:

```rust
/// The shared, size-capped append-only JSONL primitive (R-SA-136/146) used by both
/// [`spawn::SpawnedChild`]'s child-output tee and [`background::RunPaths::events`]'s async-run
/// event log. See [`jsonl`] for the full contract.
/// The NATIVE supervisor channel (`pi-subagents/src/intercom/native-supervisor-channel.ts`): the
/// broker-free, file-backed child↔supervisor request/reply channel upstream introduced in `3ac0ef5`
/// ("Make supervisor coordination native") when it deleted the companion-recommendation surface.
pub mod native_supervisor;

pub mod jsonl;
```

Both blocks get deleted by the transform above. The `jsonl` prose is already stated verbatim by
[jsonl.rs:1-4](../../crates/cyrup-ext-subagents/src/jsonl.rs), which — having no outer doc — already
writes its links correctly qualified (``[`crate::spawn::SpawnedChild`]``,
``[`crate::background::RunPaths::events`]``). That file is the style exemplar for the whole crate.

Result: `pub mod artifacts;`, `pub mod native_supervisor;`, `pub mod missions;`, `pub mod jsonl;`,
all bare, matching the 13 declarations around them.

---

## The 442 private-item links: ONE decision, ZERO edits

This is not 442 edits and it is not a `pub`-ification exercise. rustdoc states the remedy itself in
every one of the 442 notes: *"this link will resolve properly if you pass `--document-private-items`"*.

Look at what the links actually point at:

| public item | private target | file |
|---|---|---|
| `HttpCaps` | `StreamSlot`, `StreamSlot::Eof` | [caps/http.rs](../../crates/cyrup-ext/src/caps/http.rs) |
| `request` / `request_stream` | `decode_buffered`, `decode_stream` | [caps/http.rs](../../crates/cyrup-ext/src/caps/http.rs) |
| `close_stream` | `MAX_OPEN_STREAMS`, `HTTP_POLL_IDLE_TIMEOUT` | [caps/http.rs](../../crates/cyrup-ext/src/caps/http.rs) |
| `push_tool_start` / `push_tool_update` / `push_tool_end` | `ToolRun::call_id` | [transcript.rs](../../crates/cyrup-tui/src/transcript.rs) |
| `run_rpc` | `write_pump`, `rpc_driver` | [rpc.rs](../../crates/cyrup-modes/src/rpc.rs) |
| `SessionCommand` | `dispatch`, `handle` | [rpc.rs](../../crates/cyrup-modes/src/rpc.rs) |

Every one is a *correct* cross-reference into the implementation the public item is built on.
Making `MAX_OPEN_STREAMS`, `decode_stream` and `ToolRun::call_id` `pub` to satisfy rustdoc would
export the HTTP capability's stream-decoder state machine and the transcript's private tool-run
bookkeeping as permanent API surface. That is the wrong trade. Downgrading all 442 to plain code
spans destroys navigation the author deliberately built.

### DECISION — document private items workspace-wide

These are the internals of an application workspace, not a published library façade. Note the
distribution: the two crates that *are* a consumer-facing API — `cyrup-ext-sdk` (the wasm guest SDK)
and `cyrup-sdk` — contribute **1** and **0** of the 442 respectively. The class lives entirely in
internal crates, where documenting internals is the point.

Create `.cargo/config.toml` at the repo root:

```toml
# rustdoc renders this workspace's internals, not a published library façade: the 442
# `private_intra_doc_links` warnings were all correct cross-references from a public item into the
# private machinery it is built on (e.g. `HttpCaps` -> `StreamSlot`, `run_rpc` -> `write_pump`).
# Documenting private items makes those links navigable instead of forcing either a `pub`-ification
# of internal state machines or the deletion of deliberate cross-references.
[build]
rustdocflags = ["--document-private-items"]
```

This is safe against test noise: every `mod tests` in the workspace is `#[cfg(test)]`-gated (verified
across all 21 crates; e.g. [provider.rs:197](../../crates/cyrup-provider/src/provider.rs) puts
`#[cfg(test)]` above a multi-line `#[allow(...)]` block, which is why a naive one-line grep misses
it), and `cargo doc` does not compile with `--cfg test`, so no test module reaches the rendered docs.

Because `RUSTDOCFLAGS` in the environment *overrides* `build.rustdocflags`, also pin the lint to
`allow` in the manifest so a stray env var can never turn 442 silenced links back into a wall of
warnings — see the gate below.

**Do not** add `#[doc(hidden)]`, do not widen any visibility, and do not rewrite any of the 442 links.
After this change that class is zero and no source file was touched.

---

## The residual ~109 genuinely broken links, triaged

Everything below is written in a file that is **not** one of the seven parents, so it survives the
mod-doc fix and must be fixed by hand. Triaged into the three buckets the task asked for.

### Bucket A — the doc names an API that does not exist (fix the CLAIM, not just the link)

**A1. `ToolCall::is_cancelled`** — [ctx.rs:213](../../crates/cyrup-ext-sdk/src/ctx.rs). The task's own
example, and it is real. `ToolCall` is at `ctx.rs:1608` and has fields `call_id`, `params`, `ctx`,
`signal` plus methods `new`, `signal`, `emit_update` — no `is_cancelled`. The per-call poll exists,
one hop away: `Signal::is_aborted` at `ctx.rs:1594` (`pub struct Signal` at `ctx.rs:1583`), reached via the `pub signal: Signal` field.
The prose is right; only the path is wrong.

```rust
/// per-call [`Signal::is_aborted`] poll (reached through [`ToolCall::signal`]), which is the
/// closer analog of upstream's `execute(…, signal, …)` parameter.
```

**A2. `crate::Ctx::abort_signal`** ×2 —
[descriptor.rs:196](../../crates/cyrup-ext-sdk/src/descriptor.rs) and `descriptor.rs:224`.
`abort_signal` is at `ctx.rs:779`, inside `impl Ui` (`pub struct Ui;` at `ctx.rs:749`) — **not** on
`Ctx` (`pub struct Ctx;` at `ctx.rs:54`). `Ui` is re-exported at the crate root
([lib.rs:49](../../crates/cyrup-ext-sdk/src/lib.rs)), so:

```rust
/// instead references a signal it already registered by ID via [`crate::Ui::abort_signal`] (the
```

**A3. `UserBashResult`** — [events.rs:89](../../crates/cyrup-ext-sdk/src/events.rs) says the result
*"is RETURNED via [`UserBashResult`]"*. No such type exists anywhere in the crate. The real return
channel is the handler's `Outcome` (`pub enum Outcome` at
[api.rs:69](../../crates/cyrup-ext-sdk/src/api.rs)); the registration signature at `api.rs:790` is
`Fn(UserBashEvent, &Ctx) -> Outcome`. Rewrite the claim:

```rust
/// is RETURNED as the handler's [`crate::Outcome`] (Pi `UserBashEventResult`), not carried on the
/// event.
```

**A4. `crate::events::SessionCompact`** — [descriptor.rs](../../crates/cyrup-ext-sdk/src/descriptor.rs).
The type is `SessionCompactEvent`, [events.rs:323](../../crates/cyrup-ext-sdk/src/events.rs). Renamed;
fix the link to ``[`crate::events::SessionCompactEvent`]``.

**A5. `check_agent_discovery` / `check_chain_discovery`** —
[doctor.rs:19,23](../../crates/cyrup-ext-subagents/src/registration/doctor.rs). This is the worst one
in the sweep, and it is exactly the failure class the task names: the module header enumerates
*"the six checks R-SA-131 mandates"* as (a)…(f) and names two functions that do not exist. The real
`DoctorRunner::run` (`doctor.rs:275-292`) is:

```rust
let (binary, temp_dir, config, discovery_result, catalog) = tokio::join!(
    check_binary_resolution(),
    check_temp_dir_writable(&self.async_root),
    check_config_json(&self.config_json_path),
    run_discovery_checks(&self.discovery_config),
    check_provider_catalog_freshness(
        self.provider_catalog_path.as_deref(),
        &self.discovery_config,
    ),
);
let (agents, chains, model_scope) = discovery_result;
DoctorReport { checks: vec![binary, temp_dir, config, agents, chains, catalog, model_scope] }
```

Three separate falsehoods to correct, not one link:
1. (d) and (e) are **one** function, `run_discovery_checks`, returning `(agents, chains, model_scope)`.
2. The report carries **seven** checks, not six — `doctor.rs:269` ("Run all six R-SA-131 checks") and
   the header's "six checks" are both wrong.
3. The seventh, `model_scope`, is **undocumented**. Add it as (g) or fold (d)/(e)/(g) into one
   ``[`run_discovery_checks`]`` bullet that names all three outputs.

While in this file: `doctor.rs:309` has a malformed link, `` [`VERSION_PROBE_TIMEOUT]` `` (closing
bracket and backtick transposed). It is silently not a link today. Correct it to
`` [`VERSION_PROBE_TIMEOUT`] ``.

**A6. `apply_profile`** ×2 —
[profiles.rs:47](../../crates/cyrup-ext-subagents/src/registration/profiles.rs) and `profiles.rs:78`.
The file's own comment at `profiles.rs:222-225` records the history: *"A second,
`SettingsManager`-store-based pair (`apply_profile` / `load_and_apply_profile`) used to…"* — the
function was removed and two doc links were left pointing at it. The surviving function is
`apply_profile_to_settings_file` (`profiles.rs:552`), which the same header already links correctly
at `profiles.rs:22` and `:26`. Repoint both.

### Bucket B — path correct, receiver/qualifier wrong

**B1. Bare associated items in module headers.**
[extension.rs](../../crates/cyrup-ext-subagents/src/extension.rs) links ``[`run_foreground`]`` ×2,
``[`spawn_background`]`` ×2, ``[`resolve_agent`]``. All three exist as inherent methods on
`SubagentsExtension` — `resolve_agent` at `extension.rs:1673`, `run_foreground` at `extension.rs:1883`,
`spawn_background` at `extension.rs:2422` — so a bare name never resolves from module scope. Qualify:
``[`SubagentsExtension::run_foreground`]``, ``[`SubagentsExtension::spawn_background`]``,
``[`SubagentsExtension::resolve_agent`]``.

**B2. `Self::` inside a `//!` module header.** `Self` has no meaning in a module doc.
[intercom/extension.rs:18-19](../../crates/cyrup-intercom/src/extension.rs) writes
``[`Self::clarify_channel`]`` / ``[`Self::delivery_channel`]`` / ``[`Self::steer_channel`]`` for
methods at `extension.rs:152`, `:160`, `:171`. The same file already does it right at
`extension.rs:802-803` (``[`IntercomExtension::clarify_channel`]``). Copy that form.
The five `Self::*` links in
[permission-system/extension.rs](../../crates/cyrup-permission-system/src/extension.rs)
(`new_forwarding_child`, `new_forwarding_parent`, `decide`, `on_before_agent_start`,
`should_expose_tool`) are the same defect and take the same fix.

**B3. Type not imported in the linking module.**
[host/services.rs](../../crates/cyrup-ext/src/host/services.rs) links ``[`HookOutcome`]`` ×2;
the type lives at `crate::contract::HookOutcome` (imported in
[native.rs:6](../../crates/cyrup-ext/src/native.rs), not in `services.rs`). Use the full path
``[`crate::contract::HookOutcome`]``. Same shape for `RunStatus::display_dismissed_at` in
`extension.rs` — the field is real (`background/mod.rs:952`) but needs
``[`crate::background::RunStatus::display_dismissed_at`]``.

**B4. `crate::exec::mod`** — [acceptance.rs:1773](../../crates/cyrup-ext-subagents/src/exec/acceptance.rs).
`mod` is a keyword, never an item name. The path is simply ``[`crate::exec`]``.

**B5. The one ambiguity.** [permission-system/extension.rs:1306](../../crates/cyrup-permission-system/src/extension.rs)
links ``[`crate::runtime_api`]``, which is both `pub mod runtime_api;`
([lib.rs:86](../../crates/cyrup-permission-system/src/lib.rs)) and `pub fn runtime_api()`
([runtime_api.rs:105](../../crates/cyrup-permission-system/src/runtime_api.rs)). Every use in this
file means the module (the realm-global slot). Write ``[`mod@crate::runtime_api`]`` at `:1306` and at
the two sibling occurrences, `extension.rs:398` and `extension.rs:416`, so the three agree.

### Bucket C — not a Rust path at all; escape or backtick it

**C1. `argv[0]` in prose.** [intercom_broker_cmd.rs:34](../../crates/cyrup/src/intercom_broker_cmd.rs)
and `:38`, plus [subagent_runner_cmd.rs:55](../../crates/cyrup/src/subagent_runner_cmd.rs) and `:63`,
write `argv[0]` outside backticks; rustdoc parses `[0]` as a shortcut link. Wrap the token:
`` `argv[0]` ``. (Backticks, not `\[0\]` — the surrounding text is already code-ish.)

**C2. Upstream TypeScript identifiers.** ``[`spawnBrokerIfNeeded`]``
([transport/spawn.rs](../../crates/cyrup-intercom/src/transport/spawn.rs)) is a pi symbol, not a Rust
item. Backtick it without brackets.

**C3. `#[cfg(test)]` items.** ``[`tests::the_agent_dir_resolution_matches_the_intercom_crates_table`]``
([native_supervisor.rs](../../crates/cyrup-ext-subagents/src/native_supervisor.rs)) and
``[`the_compact_description_advertises_no_verb_cyrup_cannot_dispatch`]``
([tool_description.rs](../../crates/cyrup-ext-subagents/src/registration/tool_description.rs)) name
test functions. `cargo doc` never compiles them, so no link form can ever resolve — including under
`--document-private-items`. Demote both to plain code spans.

**C4. Crates that are not dependencies of the linking crate.** ``[`cyrup_intercom`]`` and
``[`cyrup_intercom::identity::presence_name`]`` from `cyrup-ext-subagents`
([extension.rs](../../crates/cyrup-ext-subagents/src/extension.rs),
[spawn/intercom_target.rs](../../crates/cyrup-ext-subagents/src/spawn/intercom_target.rs)) —
`cyrup-intercom` is absent from
[cyrup-ext-subagents/Cargo.toml](../../crates/cyrup-ext-subagents/Cargo.toml), by design (the
dependency runs the other way: `cyrup-intercom` depends on `cyrup-ext-subagents`, see
[relay.rs:11](../../crates/cyrup-intercom/src/relay.rs)). Same for
``[`cyrup_test_support::tui::TestTerminal`]`` in
[tui/render.rs](../../crates/cyrup-ext-subagents/src/tui/render.rs) — `cyrup-test-support` is a
**dev**-dependency and is invisible to `cargo doc`. All become code spans. Do **not** add a
dependency to make a doc link resolve.

**C5. `crate::subcommands::SUBCOMMANDS`** ×5 —
[credential_print.rs:7](../../crates/cyrup/src/credential_print.rs),
[subagent_runner_cmd.rs:23,46](../../crates/cyrup/src/subagent_runner_cmd.rs),
[intercom_broker_cmd.rs](../../crates/cyrup/src/intercom_broker_cmd.rs).
`pub mod subcommands;` is present at [lib.rs:31](../../crates/cyrup/src/lib.rs), but the constant is
declared `const SUBCOMMANDS: [&str; 6]` — module-private —
at [subcommands.rs:35](../../crates/cyrup/src/subcommands.rs). Under the `--document-private-items`
decision above these resolve for free; verify after that change and only then decide whether any
remain. Do not make `SUBCOMMANDS` `pub` — nothing outside `subcommands.rs` reads it
(`main.rs:118` only names it in a comment).

---

## The last 7 warnings, each pinned to an exact line

**5 × `redundant_explicit_links`** — the label already resolves to the same item, so the explicit
`(target)` is dead weight. Delete the parenthesized target, keep the bracketed label:

| file | current | becomes |
|---|---|---|
| [run.rs:136](../../crates/cyrup/src/run.rs) | ``[`run_print`](cyrup_modes::run_print)`` | ``[`cyrup_modes::run_print`]`` |
| [relay.rs:6](../../crates/cyrup-intercom/src/relay.rs) | ``[`IntercomPayload`](cyrup_ext_subagents::tui::intercom::IntercomPayload)`` | ``[`IntercomPayload`]`` (imported at `relay.rs:11`) |
| [anthropic_messages.rs:72](../../crates/cyrup-provider/src/api/anthropic_messages.rs) | ``[`StreamOptions`](crate::StreamOptions)`` | ``[`StreamOptions`]`` (imported at `:22`) |
| [google_generative_ai.rs:277](../../crates/cyrup-provider/src/api/google_generative_ai.rs) | ``[`StreamOptions`](crate::StreamOptions)`` | ``[`StreamOptions`]`` (imported at `:27`) |
| [models_store.rs:66](../../crates/cyrup-provider/src/models_store.rs) | ``[`CancelToken`](cyrup_core::CancelToken)`` | ``[`CancelToken`]`` (imported at `:20`) |

Leave `relay.rs:4`'s ``[`DeliveryChannel`](…)`` alone — `DeliveryChannel` is *not* imported there,
so that explicit target is load-bearing. In the two `cyrup-provider` files the adjacent
``[`StreamOptions::api_options`](crate::StreamOptions::api_options)`` links (`:74` and `:280`) are
redundant by the same rule; strip them too while you are on the line.

**1 × `invalid_html_tags`** — [login.rs:168](../../crates/cyrup-config/src/login.rs):

```rust
/// { cause })` (`models.ts:441`); `withCauseDetail` appends `": <cause>"`
```

`<cause>` is parsed as an HTML tag. The literal is already quoted but not backticked — backtick it:
``appends `": <cause>"` `` → the whole `": <cause>"` inside a code span.

**1 × ambiguous link** — covered as B5 above.

---

## Regrowth gate

Add to `[workspace.lints]` in [Cargo.toml](../../Cargo.toml), directly after the existing
`[workspace.lints.clippy]` block. Every member already has `[lints] workspace = true`, so this needs
no per-crate edit:

```toml
[workspace.lints.rustdoc]
broken_intra_doc_links = "deny"
invalid_html_tags = "deny"
redundant_explicit_links = "deny"
# Resolved instead by `--document-private-items` in .cargo/config.toml: these links are deliberate
# cross-references from a public item into the private machinery it is built on, and the rendered
# docs for this workspace include private items. Pinned to `allow` so a stray RUSTDOCFLAGS in the
# environment (which overrides build.rustdocflags) cannot resurrect 442 warnings.
private_intra_doc_links = "allow"
```

`deny` — not `warn`. A warn-level lint is what produced 961 of these; the whole point is that
`cargo doc` must now fail. No separate CI job is needed: any existing workflow step that runs
`cargo doc --workspace --no-deps` becomes the gate the moment these lints are `deny`.

---

## Where the original task was wrong

1. **"Suggest working crate-by-crate, largest first, so progress is measurable"** — wrong ordering,
   and it would waste most of the effort. 382 of the 494 broken links are one systemic bug fixed by
   69 deletions in 7 files, and the entire 442-warning private class is one config line. Crate-by-crate
   would have someone hand-qualifying ``[`read_fleet_transcript`]`` into
   ``[`crate::tui::fleet_transcript::read_fleet_transcript`]`` 382 times against a *correct* link.
2. **"either those items become `pub`, or the links become plain code spans"** — a false dichotomy.
   Both options are wrong here; rustdoc's own note names the third and correct one,
   `--document-private-items`.
3. **"Consider `#![warn(rustdoc::broken_intra_doc_links)]`"** — that lint is `warn` **by default**
   (every one of the 494 diagnostics carries the note
   `` `#[warn(rustdoc::broken_intra_doc_links)]` on by default ``). Adding it changes nothing. The
   gate has to be `deny`, and it belongs in `[workspace.lints.rustdoc]`, not in 22 crate roots.
4. **The warning table double-counted.** 494 + 442 + 5 + 1 + 1 = 943; the remaining 18 of the 961 are
   rustdoc's own `N warnings emitted` summary lines. Per-crate figures in the original table are the
   summary-line values, which is why they are each one lower than the true per-crate diagnostic count
   (e.g. `cyrup-ext-subagents` 564 vs 565).
5. **The cited example is real but mis-triaged.** `ToolCall::is_cancelled` is not "an API that does
   not exist" — the capability exists as `Signal::is_aborted` one field hop away
   ([ctx.rs:1594](../../crates/cyrup-ext-sdk/src/ctx.rs)). It belongs in the *renamed* bucket. The
   genuinely-nonexistent APIs are A3 (`UserBashResult`), A5 (`check_agent_discovery` /
   `check_chain_discovery`) and A6 (`apply_profile`).

---

## Execution order

1. `.cargo/config.toml` + `[workspace.lints.rustdoc]` with `private_intra_doc_links = "allow"` and the
   other three still at `warn` (not yet `deny`). Re-run `cargo doc --workspace --no-deps`.
   Expected: 961 → ~519.
2. Proof-of-mechanism: delete the outer `///` on `pub mod guide;` in `registration/mod.rs`, re-run,
   confirm the drop.
3. Delete the remaining 68 outer module docs across the 7 files, folding non-redundant prose into each
   child's `//!` header. Fix the orphaned `jsonl`/`native_supervisor` doc block at `lib.rs:37-45`.
   Expected: ~519 → ~113.
4. Work Buckets A, B, C and the 7 pinned stragglers. Bucket A changes doc *claims*, not just links —
   `doctor.rs` needs three factual corrections. Expected: ~113 → 0.
5. Flip `broken_intra_doc_links`, `invalid_html_tags`, `redundant_explicit_links` to `deny`.
   Final `cargo doc --workspace --no-deps` must exit 0 with no output.

## Definition of done

- [ ] `cargo doc --workspace --no-deps` prints **zero** warning lines and exits 0.
- [ ] `[workspace.lints.rustdoc]` in [Cargo.toml](../../Cargo.toml) sets `broken_intra_doc_links`,
      `invalid_html_tags` and `redundant_explicit_links` to `deny`; `private_intra_doc_links` to
      `allow` with the comment explaining why.
- [ ] `.cargo/config.toml` sets `build.rustdocflags = ["--document-private-items"]` with the rationale
      comment. Deliberately reverting it and re-running `cargo doc` produces the 442
      `private_intra_doc_links` warnings and nothing else — proving that class was config, not code.
- [ ] Not one `pub`, `pub(crate)` or `#[doc(hidden)]` was added or changed anywhere. No dependency was
      added to any `Cargo.toml` to make a doc link resolve.
- [ ] No `pub mod X;` in `cyrup-ext-subagents` carries an outer `///` doc comment:
      `grep -rn -B1 '^pub mod ' crates/cyrup-ext-subagents/src --include=mod.rs --include=lib.rs | grep '///'`
      returns nothing.
- [ ] The three Bucket-A rewrites landed as corrected *claims*, not just repointed links —
      specifically `doctor.rs` no longer says "six checks", no longer names `check_agent_discovery` or
      `check_chain_discovery`, and documents the seventh (`model_scope`) check that
      `DoctorRunner::run` actually returns.

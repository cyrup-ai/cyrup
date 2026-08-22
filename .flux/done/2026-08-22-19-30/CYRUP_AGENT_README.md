---
stage: qa
status: completed
updated: 2026-08-22 21:35
---

# Add A README For cyrup-agent

## Description

`crates/cyrup-agent/` has no README. Neither does any other crate — **0 of 21** carry one, so this
is a create with no in-repo template to copy, and whatever shape it takes becomes the precedent.

The four questions the capture left open are now answered:

**Create, not update.** `crates/cyrup-agent/README.md` does not exist. Nor does
`crates/*/README.md` for any crate in the workspace.

**Audience: workspace contributors first, embedders second — not crates.io.** Fifteen in-workspace
crates import `cyrup_agent::` (cyrup-session-svc most heavily, plus cyrup-sdk, cyrup-ext,
cyrup-tui, cyrup-ext-subagents, cyrup-modes). The user-facing documentation is the mdBook under
[`docs/guide/`](../../docs/guide/introduction.md), which does **not** cover the agent loop — the
only hit for `AgentBuilder` across the whole guide is a passing mention in
`reference/troubleshooting.md`. Version is `0.0.0` and nothing is published, so this is an
orientation document for someone opening the crate, not a registry landing page.

**Add `readme = "README.md"` to the manifest.** Cargo auto-detects a README in the package root, so
the key is not strictly required, but no manifest in the workspace sets it and being explicit costs
one line. Note `publish` is deliberately **unset** for cyrup-agent — only
[`xtask`](../../xtask/Cargo.toml), [`cyrup-it`](../../crates/cyrup-it/Cargo.toml) and
[`cyrup-test-support`](../../crates/cyrup-test-support/Cargo.toml) set `publish = false`. Do not
change publish intent here; that is a separate decision.

**Document the module map — it is the single most valuable section.** The crate is 45 files and
5,326 non-test lines, and both of its module trees were created in the last day:
[`src/agent/`](../../crates/cyrup-agent/src/agent/) (14 files) and
[`src/proxy/`](../../crates/cyrup-agent/src/proxy/) (7 files). A contributor opening the crate today
has no map at all. Document it as a table of **concerns**, not a file listing that rots.

### Do not duplicate what already exists

- The root [`README.md`](../../README.md) already carries a one-line role for this crate in its
  Workspace layout table: *"the turn loop: tool execution, hooks, steering and follow-up queues,
  abort"*. The crate README expands that; it does not restate the table.
- [`src/lib.rs:1-8`](../../crates/cyrup-agent/src/lib.rs) already has a good `//!` header. Do not
  write a second, divergent description of the crate — see the Approach, which makes the README
  *become* that header rather than competing with it.
- The `arch-02` / `func-02` references throughout the crate point at documents **not present in
  this repo** (`spec/` holds only `CMDHINT_01.md` and the flux material). The README must not link
  to them; naming them as external upstream references is fine, linking is not.

## Scope

In scope: creating `crates/cyrup-agent/README.md`; wiring it in as the crate's rustdoc front page;
adding the `readme` key to `crates/cyrup-agent/Cargo.toml`; and relocating the existing `//!` header
text from `src/lib.rs` into the README so there is exactly one description of the crate.

Out of scope:
- Any behavior change, or any edit to the crate's source beyond the `lib.rs` doc header.
- READMEs for other crates. This one sets the precedent; rolling it out is separate work.
- Changing `publish`, `version`, or any other manifest field.
- Fixing the crate's 6 existing rustdoc warnings — owned by the queued **CARGO_DOC_WARNINGS** task.
  This task's only rustdoc obligation is to not *add* one.
- The mdBook. If the agent loop deserves a guide chapter, that is its own task.

## Approach

### 1. Make the README the crate's rustdoc front page

The crate has **zero doctests today** (`cargo test -p cyrup-agent --doc` → `0 passed`) and the
workspace has no `examples/` directory in any crate, so a README example would be uncompiled prose
that rots on the next API change. Wire it in instead:

```rust
// crates/cyrup-agent/src/lib.rs — replacing the current `//!` block at :1-8
#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
```

This is the prescribed path rather than a standalone README because it buys three things at once:
every ```rust block in the README becomes a compiled doctest under `cargo test`, rustdoc gains a
real front page, and the crate stops having two descriptions that can disagree. The existing header
text at `lib.rs:1-8` moves into the README's opening section — moved, not copied.

**Guard:** the README must contain no `[`Item`]` intra-doc links. Use plain code spans and relative
file links. That keeps the rustdoc warning count at exactly 6 so this does not collide with
CARGO_DOC_WARNINGS.

### 2. Write `crates/cyrup-agent/README.md`

Sections, in order:

**Title + one-paragraph what-it-is.** Carry over the substance of the current `lib.rs` header:
ordered event stream, parallel/sequential tool execution, the `Hooks` mutating seam plus the
notify-only `EventSubscriber`, steering/follow-up queues, abort/idle, managed agent state. State
that the loop is provider-agnostic — it talks to a `StreamFn`, and `ProviderStreamFn` wraps a
`cyrup_provider::Provider`.

**Quickstart** — a compiled doctest. Derive it from the real construction in
[`src/tests/settlement_latch.rs:24-45`](../../crates/cyrup-agent/src/tests/settlement_latch.rs),
made self-contained (that file's `model_ref()` comes from the test-only `support` module, so inline
it). The `faux` feature is already a dev-dependency of this crate, and doctests link dev-deps, so
this compiles as written:

```rust
use std::sync::Arc;
use cyrup_agent::{Agent, ModelRef, ProviderStreamFn, StopReason, StreamFn};
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};
use cyrup_provider::Provider;

# tokio_test::block_on(async {
let faux = Arc::new(FauxProvider::new());
faux.set_responses(vec![faux_assistant_message(vec![faux_text("hello")], StopReason::Stop)]);
let provider: Arc<dyn Provider> = faux.clone();
let stream_fn: Arc<dyn StreamFn> = Arc::new(ProviderStreamFn::new(provider));

let model = ModelRef { provider: "faux".into(), api: Some("faux".into()), model: "faux-1".into() };
let agent = Agent::builder(model, stream_fn).build();

let handle = agent.prompt("hi").await?;
let new_messages = handle.finished().await;
# Ok::<_, cyrup_agent::AgentError>(())
# });
```

If `tokio_test` is not already available, use `#[tokio::main]`-style hidden lines or mark the block
`no_run` — but prefer a genuinely executed doctest. Verify by running `cargo test -p cyrup-agent
--doc` and seeing the count go from 0 to 1.

**The loop in one pass.** Six or seven lines of prose tracing one turn: prompt → `TurnStart` →
`stream_assistant` → tool batch (parallel unless any tool is `ExecMode::Sequential`) → `TurnEnd` →
the two post-turn hooks (`prepare_next_turn`, `should_stop_after_turn`) → steering poll →
`AgentEnd`. State the ordering guarantee explicitly: all emission and hook invocation happen on the
single run task, so event order is deterministic; only tool `execute` bodies run concurrently.

**Module map.** A table of concerns over
[`src/`](../../crates/cyrup-agent/src/), naming directories and their jobs rather than every file:

| Path | Concern |
|---|---|
| `agent/` | `Agent`, `AgentBuilder`, the run context and the turn driver |
| `agent/run/` | one run's working state: turn loop, LLM boundary, tool dispatch |
| `agent/run/tools/` | preflight, finalization, and the parallel/sequential executors |
| `proxy/` | the `ProxyStreamFn` transport — wire enum, partial rebuild, SSE |
| `hooks.rs` | the mutating `Hooks` seam |
| `subscriber.rs` | the notify-only `EventSubscriber` |
| `queue.rs` | steering and follow-up queues |
| `state.rs` | managed state + the reducer |
| `loop_fn.rs` | the low-level free-function loop, for callers that do not want `Agent` |

**Extension points.** Name the four seams a downstream crate actually implements, each one line
with the trait name: `StreamFn`, `Hooks`, `EventSubscriber`, `Tool` (from cyrup-core). This is the
section a contributor needs most and the one that exists nowhere today.

**Conventions.** Two short notes: the crate is `#![forbid(unsafe_code)]` under a workspace no-panic
policy (`unwrap_used`, `expect_used`, `panic`, `indexing_slicing` are `deny`); and the port-anchor
comment style (`agent.ts:NNN`, `AGENT-0NN`, `R-02-0NN`) marks fidelity to the upstream TypeScript
implementation — say what the anchors mean so a reader stops wondering.

Keep the whole file under ~120 lines. It is a map, not a manual.

### 3. Manifest

Add to `[package]` in [`crates/cyrup-agent/Cargo.toml`](../../crates/cyrup-agent/Cargo.toml),
directly under `description`:

```toml
readme = "README.md"
```

## Definition of done

- [ ] `crates/cyrup-agent/README.md` exists, is under ~120 lines, and carries all seven sections
      above in order
- [ ] `crates/cyrup-agent/src/lib.rs` starts with `#![doc = include_str!("../README.md")]` and the
      old `//!` header block is gone (moved into the README, not duplicated)
- [ ] `grep -c '\[\`' crates/cyrup-agent/README.md` returns 0 — no intra-doc links in the README
- [ ] `cargo test -p cyrup-agent --doc` reports at least 1 passing doctest, up from 0
- [ ] `cargo test -p cyrup-agent` still reports 140 passed, 0 failed
- [ ] `cargo doc -p cyrup-agent --no-deps 2>&1 | grep -cE '^warning: (unresolved|public)'` is still
      6 — no new rustdoc warning
- [ ] `cargo clippy -p cyrup-agent --all-targets --no-deps -- -D warnings` still exits 0
- [ ] `grep -n readme crates/cyrup-agent/Cargo.toml` shows `readme = "README.md"`
- [ ] `git diff --stat` touches only `crates/cyrup-agent/README.md`, `src/lib.rs` and `Cargo.toml`

---

## QA Record — 2026-08-22 21:35

Verified complete. All nine acceptance criteria pass against the final tree: README is 98 lines
(under the ~120 target), `lib.rs` carries `#![doc = include_str!]` with no stale `//!` header, zero
intra-doc links, **1 doctest passing (up from 0)**, 140 unit tests passing, rustdoc unchanged at 6
warnings, `clippy --no-deps -D warnings` exit 0, `readme` key present, workspace build clean.

Beyond the gates, every substantive factual claim in the README was checked against the source
rather than taken on trust:

- "parallel unless the run or any individual tool asks for `ExecMode::Sequential`" matches
  `execute_tool_calls`'s `any_seq || tool_execution == Sequential` exactly.
- The truncated-batch behaviour matches `turn.rs`'s `StopReason::Length` branch.
- Subscription drop semantics, the proxy's bearer SSE transport, and the run-task / state-lock
  invariants all match their source doc comments.
- All 12 module-map paths exist, and all four named seams (`StreamFn`, `Hooks`, `EventSubscriber`,
  `Tool`) are real traits.

The doctest is executed, not merely compiled: its `assert_eq!(new_messages.len(), 2)` encodes the
loop's actual return contract, so a future API change breaks the build rather than silently rotting
the example.

**Two judgement calls that exceed the spec rather than fall short.** The README contains no relative
file links, though the Approach suggested them — correct for a dual-purpose file, since a relative
link that resolves on GitHub 404s in rustdoc and vice versa. And the quickstart shows a complete
visible `#[tokio::main] async fn main()` rather than hidden doctest lines, so the example reads as a
runnable program on GitHub.

**One criterion was wrong as authored:** the DoD says "all seven sections", but the Approach
specifies six (title, Quickstart, the loop, module map, extension points, conventions). All six are
present in order; the count was an error in the task, not a missing section.

**One cosmetic note, not a defect:** rustdoc demotes the README's `#` heading, so the crate page
renders "Crate cyrup_agent" followed by "§cyrup-agent". That title redundancy is inherent to the
`#![doc = include_str!]` pattern this task prescribed, is structurally valid, and is standard
practice across the ecosystem.

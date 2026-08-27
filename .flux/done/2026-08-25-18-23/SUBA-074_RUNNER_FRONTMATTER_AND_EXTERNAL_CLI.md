---
stage: qa
status: completed
updated: 2026-08-27 12:15
severity: high
effort: medium
subsystem: external runners / agent schema
source: docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md
item: SUBA-074
---

> **Path note.** This task lives in a subdirectory. Every flux command globs a single level (`ls -1 "$FLUX_BASE/todo/"*.md`), so `/exec`, `/aug` and `/qa` will not list it — pass the absolute path explicitly.

# SUBA-074 (stage 1) — Agent `runner:` frontmatter

## QA verdict: 9/10 — one small item outstanding

Stage 1 is **implemented correctly and is production quality**. Verified independently against
`tmp/pi-subagents` at tag `v0.57.0` and against the tree (no `git` used — files read as they exist):

- **All thirteen upstream refusal strings are verbatim.** Extracted upstream's own `throw new Error`
  templates from `git show v0.57.0:src/agents/agents.ts` and diffed them against the port's
  `format!` strings: exact match on all thirteen, including the two interpolation subtleties
  (rule 10 interpolates the ADAPTER id, not the agent name twice; rule 9 precedes rule 10, so a bad
  adapter id reports the id problem rather than the argv one — pinned by its own assertion).
- **The four contract strings and `CODE_OWNED_ADAPTER_LABEL` are exact**, the latter byte-identical
  to upstream's computed `.map(id => `'${id}'`).join(", ")`.
- **The reserved-name guard is unconditional**, matching upstream's own unconditional call at
  `agents.ts:1951` — it is not gated behind "has a runner", which is what makes it a real
  name-squat guard rather than a no-op for plain agents.
- **No regression on shipped agents**: no bundled/project agent uses a reserved selection name as
  its `name` or in `aliases`, so the new guard fails nothing that previously loaded.
- **No silent-drop path exists.** `AgentDefinition` derives only `Clone, Debug` — no `Default` — and
  there is no `..spread` construction site anywhere, so the compiler forces every literal to name
  `runner`. The two apparent production sites (`handlers.rs:53`, `lookup.rs:167`) are a return type
  and an `impl` header, not literals; `editable_base` carries the field through `.clone()`.
- **The hop-2 serde path genuinely works.** Since no test covers it (see below), it was verified
  out-of-tree by replicating the exact type definitions and serde attributes and round-tripping all
  four runner shapes plus an older config with no `runner` key. All five pass:
  `{"type":"pi"}`, `{"type":"external-cli","adapter":…,"command":…}`, the args/promptDelivery/
  capabilities shape, `{"type":"external-job","provider":…,"options":…}`, and
  backward-compatible deserialization to `None`.
- **Gates re-run independently**: 2,514 lib tests pass; `cargo doc --no-deps --lib` clean (the
  workspace pins `broken_intra_doc_links = "deny"`); zero clippy findings in `src/runner/`.
- **Two real defects were self-caught during implementation** and are correctly fixed: the
  `serde_json::Map` key-ordering bug (this workspace does not enable `preserve_order`, so a `Map`
  is a `BTreeMap` and emitted `command` before `type`, breaking the byte-stable round-trip), and a
  clippy `panic` **error** in a test module that deliberately does not allow it — resolved by
  rewriting the assertion rather than widening the module's allow list, which was the right call.
- **Scope is correct**: stage 2 is excluded, and nothing from it leaked in.

---

## The outstanding item, re-scoped by this augmentation pass

QA filed this as one gap in one test. Researching it against the tree found the gap is **three hops
wide, not one**, and that the crate **already owns a test whose entire purpose is guarding exactly
this class of loss** — it simply was not extended when `runner` joined the struct. That test is the
correct home for the fix, and using it is materially stronger than QA's original prescription.

### The hop-2 hand-off is three hops, and `runner` is unpinned at all three

A background / chain / parallel step never reaches `AgentConfig::from_agent_definition`. It goes:

```
AgentDefinition  (runner parsed from frontmatter)
  → ResolvedAgentPersona::from_agent_definition          HOP A   (agent_config.rs:263)
  → serde_json::to_string  →  runner-config.json on disk HOP B   (serialize)
  → serde_json::from_str   →  in the DETACHED process    HOP B   (deserialize)
  → ResolvedAgentPersona::to_agent_config(depth)         HOP C   (agent_config.rs:293)
  → run_sync  →  refusal_reason() gate
```

All three hops are **correct in production** — `runner: agent.runner.clone()` at HOP A,
`runner: self.runner.clone()` at HOP C, and the serde attributes at HOP B were verified working
out-of-tree during QA (all four runner shapes plus backward-compatible deserialization of an older
config carrying no `runner` key). **Nothing is broken.** What is missing is the regression guard, at
every one of the three.

### The crate already documents this exact failure mode

`exec/spawn_plan.rs:3031-3044`, the doc comment on
`the_detached_runner_persona_handoff_preserves_memory_and_tool_budget`, says it verbatim:

> *"Every field that hand-off drops is silently lost for every non-foreground run — and because both
> fields are `#[serde(default)] Option`, dropping them produces no error, no warning and no compile
> failure: `memory: None, tool_budget: None` in `to_agent_config` type-checks perfectly and leaves
> the whole rest of the suite green while `/run x --bg` quietly stops honouring the agent's
> `memory:` and `toolBudget:`."*

`runner` is now a **third** `#[serde(default)] Option` field on that same hand-off with that same
property. That test drives the REAL chain end to end — its own comment: *"resolve → serialize into
the runner config → deserialize in the detached runner process → rebuild the spawn input"* — and
asserts on **observable end products** (the delivered system prompt, the env overlay) rather than on
struct fields. It is the canonical home for this invariant.

### Why `runner` needs a SIBLING test, not a line added to that one

The existing fixture's frontmatter declares `toolBudget: {"hard": 5, "soft": 2}`, and `toolBudget`
is one of the fourteen `PI_ONLY_FIELDS` (`runner/mod.rs:197`). An `external-cli` runner added to
that fixture would be refused at load by `validate_external_runner_profile` — correctly, since
upstream forbids exactly that combination. So the external-runner guard must be its own test with
its own minimal, legal frontmatter (an external profile declares no Pi-only fields — that IS
upstream's rule).

The observable end product also differs, and this is the point worth getting right: for
`memory`/`toolBudget` the product is argv/env, because the child still spawns. For a non-`pi`
`runner` **there is no spawn plan at all** — the run is refused at `run_sync` before the ladder. So
the observable is the refusal itself.

---

## Required implementation

Two changes, both test-only. No production change — the production threading is already correct and
verified.

### 1. `src/exec/spawn_plan.rs` — the three-hop guard (the primary fix)

Add immediately after `the_detached_runner_persona_handoff_preserves_memory_and_tool_budget`
(`:3046`), mirroring its structure and rationale. `ResolvedAgentPersona` is already imported in that
test module (`:1413`); `sample_agent_config`/`base_opts` come from `testsupport`.

```rust
    /// SUBA-074 across the SAME detached-runner seam the test above guards.
    ///
    /// `runner` is a third `#[serde(default)] Option` field on that hand-off, so it carries the
    /// identical silent-loss property the test above documents: dropping it at any of the three
    /// hops type-checks perfectly, raises nothing, and leaves the suite green while every
    /// background / chain / parallel run of an external-runner profile quietly spawns a
    /// full-capability native child — the exact defect SUBA-074 exists to close, reintroduced on
    /// the one path the foreground refusal test cannot see.
    ///
    /// This drives the REAL hand-off — parse → persona → JSON → persona → `AgentConfig` — and
    /// asserts the observable end product. For `memory`/`toolBudget` that product is argv/env,
    /// because the child still spawns; for a non-`pi` runner there is NO spawn plan, because the
    /// run is refused before the ladder, so the refusal IS the product.
    ///
    /// The fixture declares no Pi-only field, which is upstream's own rule for an external profile
    /// (`agents.ts:1864-1871`) and is why this cannot simply be folded into the test above, whose
    /// fixture declares `toolBudget:`.
    #[test]
    fn the_detached_runner_persona_handoff_preserves_the_runner_profile() {
        let def = crate::discovery::frontmatter::parse_agent_file(
            "---\nname: reviewer\ndescription: Reviews\nrunner: {\"type\": \"external-cli\", \"adapter\": \"claude-code\", \"command\": \"claude\"}\n---\n\n- You are the REVIEWER persona.\n",
            crate::discovery::types::AgentSource::User,
            std::path::Path::new("reviewer.md"),
        )
        .expect("an external-cli profile declaring no Pi-only field must load");
        assert!(def.runner.is_some(), "HOP 0: the frontmatter parse must yield a runner");

        // The hand-off, verbatim: resolve → serialize into the runner config → deserialize in the
        // detached runner process → rebuild the spawn input.
        let persona = ResolvedAgentPersona::from_agent_definition(&def);
        assert_eq!(persona.runner, def.runner, "HOP A: from_agent_definition dropped the runner");

        let encoded = serde_json::to_string(&persona).expect("persona serializes");
        let decoded: ResolvedAgentPersona =
            serde_json::from_str(&encoded).expect("runner config deserializes");
        assert_eq!(decoded.runner, def.runner, "HOP B: the JSON round-trip dropped the runner");

        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };
        let agent = decoded.to_agent_config(depth);
        assert_eq!(agent.runner, def.runner, "HOP C: to_agent_config dropped the runner");

        // The observable end product: the rebuilt config refuses, so a background/chain/parallel
        // step declines the profile exactly as the foreground path does.
        let reason = agent
            .runner
            .as_ref()
            .and_then(crate::runner::AgentRunnerConfig::refusal_reason)
            .expect("a non-`pi` runner must refuse after surviving the hand-off");
        assert!(reason.contains("runner.type='external-cli'"), "{reason}");
        assert!(reason.contains("adapter 'claude-code'"), "{reason}");
        assert!(reason.contains("full-capability native child"), "{reason}");
    }
```

### 2. `src/exec/agent_config.rs` — make two existing test names true again

Both fixtures carry `runner: None`, and with `#[serde(skip_serializing_if = "Option::is_none")]`
that omits the field entirely — so each test's name now claims more than it checks.

**`resolved_agent_persona_round_trips_through_json_preserving_every_field`** (`:669`) — the fixture
at `:670`. Replace its `runner: None,` with a populated, non-`Pi` value so "every field" is true.
Use a newtype variant deliberately: `Pi` is a **unit** variant and exercises a different
internally-tagged serde code path from `ExternalCli`/`ExternalJob`, so `Pi` alone would leave the
riskier path uncovered.

```rust
            tool_budget: None,
            runner: Some(crate::runner::AgentRunnerConfig::ExternalCli(
                crate::runner::ExternalCliRunner {
                    adapter: Some("claude-code".to_string()),
                    command: "claude".to_string(),
                    args: Vec::new(),
                    prompt_delivery_stdin: false,
                    capabilities: None,
                },
            )),
```

**`to_agent_config_stamps_the_live_depth_and_reproduces_the_persona`** (`:698`) — the fixture at
`:699`. Same replacement, **plus** an assertion in its block (which currently enumerates fifteen
`assert_eq!`s over the fields that "reach the execution-ready config verbatim" and names no runner):

```rust
        assert_eq!(cfg.runner, persona.runner);
```

---

## Definition of done

- [ ] `the_detached_runner_persona_handoff_preserves_the_runner_profile` exists in
      `exec/spawn_plan.rs` and pins all three hops plus the refusal as the observable end product
- [ ] Both `exec/agent_config.rs` persona fixtures carry a populated **non-`Pi`** runner, and
      `to_agent_config_stamps_the_live_depth_and_reproduces_the_persona` asserts `cfg.runner`
- [ ] `cargo test -p cyrup-ext-subagents` passes — expect **2,515** (2,514 + the one new test)
- [ ] `cargo clippy -p cyrup-ext-subagents --all-targets` reports no NEW finding (the baseline is
      2 lib + 5 lib-test warnings; note `exec/agent_config.rs`'s test module allows
      `unwrap_used`/`expect_used` but check whether it allows `panic` before using `panic!`)

No production change is required or wanted. Everything else in SUBA-074 stage 1 is complete and
was verified in the QA pass recorded above — do not redo it.

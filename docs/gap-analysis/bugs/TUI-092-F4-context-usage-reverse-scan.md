# TUI-092-F4 — `context_usage`: reverse branch scan, not a full context build

> **Part of** [`TUI-092-progressive-lockup.md`](TUI-092-progressive-lockup.md) (the umbrella audit).
> The only defect that lives outside `crates/cyrup-tui` — a pure accessor rewrite in
> `cyrup-session-svc`.
>
> **Kind** `cyrup-original` · **Severity** high · **Effort** S · **Phase driven** 3 (every turn's
> frame stalls a little more — the finding that most directly matches *"slowly over time …
> regardless of content"*)

## Coordinates with

Nothing. The TUI call sites ([`refresh_context_usage`](../../../crates/cyrup-tui/src/app.rs#L5847),
[`ingest_session_event`](../../../crates/cyrup-tui/src/app.rs#L5799)) are unchanged — the cost dies
at the accessor. `messages()` itself stays: the RPC and replay paths genuinely need the list.

### Caller inventory of `AgentSession::context_usage` (verified in-tree)

Exactly two callers, both of which keep compiling untouched because the signature
(`pub async fn context_usage(&self) -> crate::state::ContextUsage`) does not change:

1. [`AgentSession::stats_context_usage`](../../../crates/cyrup-session-svc/src/session.rs#L4044)
   (session.rs:4045) — the **hot path**. Reached from
   [`App::refresh_context_usage`](../../../crates/cyrup-tui/src/app.rs#L5847) on the TUI run-loop
   task, gated by [`context_usage_may_have_moved`](../../../crates/cyrup-tui/src/app.rs#L6406),
   which fires on **six** event kinds: `MessageEnd`, `AgentEnd`, `CompactionEnd`, `ModelChanged`,
   `SessionStart`, `SessionReplaced`.
2. [`SessionCommand::GetContextUsage`](../../../crates/cyrup-session-svc/src/command.rs#L190)
   (`C::GetContextUsage => O::ContextUsage(self.context_usage().await)`) — the RPC/command seam.
   It silently gets the same O(branch-depth) improvement.

Two same-named neighbours that are **not** this accessor and must not be confused with it:

* [`AgentSession::state_view`](../../../crates/cyrup-session-svc/src/session.rs#L4133) inlines its
  own last-assistant `find_map` over the `messages()` list it already built for `message_count` —
  it pays no *extra* context build and is out of scope.
* [`HostServices::context_usage`](../../../crates/cyrup-session-svc/src/host_services.rs#L1256) is a
  different method on a different type — a pushed-value `serde_json::Value` read for extension
  guests (`ctx.getContextUsage()`), with no session walk behind it. Untouched.

---

## Evidence

* [`App::ingest_session_event`](../../../crates/cyrup-tui/src/app.rs#L5799) calls
  `refresh_context_usage(session).await` on the six event kinds listed by
  [`context_usage_may_have_moved`](../../../crates/cyrup-tui/src/app.rs#L6406) — including
  **every** `MessageEnd` — inline on the run-loop task, before the frame.
* [`refresh_context_usage`](../../../crates/cyrup-tui/src/app.rs#L5847) →
  [`AgentSession::stats_context_usage`](../../../crates/cyrup-session-svc/src/session.rs#L4044) →
  [`context_usage()`](../../../crates/cyrup-session-svc/src/session.rs#L4117) →
  [`messages()`](../../../crates/cyrup-session-svc/src/session.rs#L3929) →
  `manager.lock().await.build_context()` —
  [`SessionManager::build_context`](../../../crates/cyrup-session/src/manager.rs#L737) walks the
  whole branch path and `build_context_messages(&path)` (defined in
  [`cyrup-session/src/context.rs:151`](../../../crates/cyrup-session/src/context.rs#L151), called from
  [`manager.rs:766`](../../../crates/cyrup-session/src/manager.rs#L766)) **constructs the full
  `Vec<Message>`**, converting and cloning every message on the branch, tool payloads included —
  only to `.rev().find_map()` the last assistant and drop the rest.
* `stats_context_usage` then runs
  [`has_post_compaction_usage`](../../../crates/cyrup-session-svc/src/session.rs#L4080), a second
  scan (that one, to its credit, already clone-free over `entries()`).

### Cost anatomy of one `context_usage()` call today

Each stage was read in-tree; the per-event price is the sum:

1. [`messages()`](../../../crates/cyrup-session-svc/src/session.rs#L3929) =
   `manager.lock().await.build_context().messages` — one async lock, then the full build.
2. [`build_context`](../../../crates/cyrup-session/src/manager.rs#L737) =
   [`branch_path(None)`](../../../crates/cyrup-session/src/manager.rs#L627) (cheap: O(branch-depth)
   parent-link walk of `&Entry` references) **plus** a forward pass over the path for
   thinking-level/model state **plus** `build_context_messages(&path)`.
3. [`build_context_messages`](../../../crates/cyrup-session/src/context.rs#L151) locates the latest
   compaction, reconstructs the kept window, and calls
   [`push_as_message`](../../../crates/cyrup-session/src/context.rs#L59) for **every** entry in it.
   The `KnownEntry::Message` arm delegates to
   [`AgentMessage::push_llm`](../../../crates/cyrup-session/src/agent_message.rs#L188) — the
   `convertToLlm` flatten — which clones every content block (text, tool calls, tool results) into
   freshly allocated `Message`s; `Compaction`/`BranchSummary`/`CustomMessage` arms additionally
   allocate their formatted wrapper strings.
4. Back in `context_usage`, the code reverses that `Vec`, pattern-matches the first assistant, and
   **drops the entire vector** — every clone from stage 3 was pure waste for this caller.

**Cost shape.** CPU/event ∝ session history, on the run-loop task, after every assistant message.
Session-age-correlated by construction: turn *n* stalls the events arm by O(n).

**Verified in the tree:** `build_context_messages` is *defined* in
[`cyrup-session/src/context.rs:151`](../../../crates/cyrup-session/src/context.rs#L151) (called from
[`manager.rs:766`](../../../crates/cyrup-session/src/manager.rs#L766)), not `manager.rs`.
`has_post_compaction_usage` at
[`session.rs:4080`](../../../crates/cyrup-session-svc/src/session.rs#L4080) already walks `entries()`
clone-free. `Message` is already in scope at the top of
[`session.rs:14`](../../../crates/cyrup-session-svc/src/session.rs#L14) (the `cyrup_core::{…}`
group), so the rewritten body needs only the two `cyrup_session` imports shown — no new top-level
`use`.

---

## Research — what upstream Pi actually does (verified against the local mirror)

The fix's shape is not an invention; it is how Pi's own `getContextUsage` answers the same
question. Read and pinned from the local pi-mono mirror
(`/Users/davidmaple/cyrup.ai/pi`, `v0.84.2-4-gb1efcf7d7`); the exact v0.83.0 text — the release
cyrup's docstrings pin with `@v0.83.0` — is frozen at
[`tmp/pi-reference/agent-session-v0.83.0-getContextUsage.excerpt.ts`](../../../tmp/pi-reference/agent-session-v0.83.0-getContextUsage.excerpt.ts)
(`agent-session.ts:3164-3216` @v0.83.0; identical structure at `:3174-3236` @v0.84.2):

```ts
getContextUsage(): ContextUsage | undefined {
    const model = this.model;
    if (!model) return undefined;
    const contextWindow = model.contextWindow ?? 0;
    if (contextWindow <= 0) return undefined;
    // After compaction, the last assistant usage reflects pre-compaction context size…
    const branchEntries = this.sessionManager.getBranch();      // ← branch refs, NOT a context build
    const latestCompaction = getLatestCompactionEntry(branchEntries);
    if (latestCompaction) {
        const compactionIndex = branchEntries.lastIndexOf(latestCompaction);
        let hasPostCompactionUsage = false;
        for (let i = branchEntries.length - 1; i > compactionIndex; i--) { … }  // ← reverse scan
        if (!hasPostCompactionUsage) return { tokens: null, contextWindow, percent: null };
    }
    const estimate = estimateContextTokens(this.messages);
    …
}
```

Two facts fall out of the upstream source:

1. **The compaction-trust scan runs over `sessionManager.getBranch()` — branch entries by
   reference — never over a rebuilt `Vec<Message>`.** cyrup's
   [`has_post_compaction_usage`](../../../crates/cyrup-session-svc/src/session.rs#L4080) already
   mirrors this loop (backward direction, `aborted`/`error` exclusion, four-field usage sum). The
   rewrite below extends the same shape to the last-assistant lookup, via cyrup's equivalent of
   `getBranch()`: [`branch_path`](../../../crates/cyrup-session/src/manager.rs#L627).
2. **The token-count semantic differs, and that difference is LOCKED, not a bug to fix here.** Pi
   computes `estimateContextTokens(this.messages)` (a chars/4 estimate over the live message list);
   cyrup deliberately derives occupancy from the last assistant's **billed** `usage`
   ([`ContextUsage::from_last_assistant`](../../../crates/cyrup-session-svc/src/state.rs#L285),
   design note at [`state.rs:262-265`](../../../crates/cyrup-session-svc/src/state.rs#L262)). This
   task changes only **how the last assistant is found**, never what is computed from it. Do not
   "upgrade" to `estimateContextTokens` — that is a semantic change with its own tradeoffs and is
   out of scope.

---

## FIX — answer the question with a reverse scan, not a context build

The manager already exposes everything needed:
[`branch_path`](../../../crates/cyrup-session/src/manager.rs#L627) yields `Vec<&Entry>`
(references, no clones — an O(branch-depth) parent-link walk from the leaf, returned root→leaf) and
[`ContextUsage::from_last_assistant`](../../../crates/cyrup-session-svc/src/state.rs#L285) consumes
`Option<&AssistantMessage>`. Rewrite `context_usage` in
[`cyrup-session-svc/src/session.rs`](../../../crates/cyrup-session-svc/src/session.rs#L4117) as the
**only** implementation (this is prescriptive, not one of several options):

```rust
/// Context-window occupancy from the last assistant turn (Pi `getContextUsage`,
/// agent-session.ts:2977).
pub async fn context_usage(&self) -> crate::state::ContextUsage {
    use cyrup_session::entry::{Entry, KnownEntry};
    use cyrup_session::AgentMessage;
    let guard = self.manager.lock().await;
    // The last assistant ON THE ACTIVE BRANCH, by parent-link walk — the same answer
    // `messages().await.iter().rev().find_map(..)` gave, without building or cloning the
    // branch's whole message list to get it (TUI-092 F4).
    let last = guard.branch_path(None).into_iter().rev().find_map(|e| match e {
        Entry::Known(KnownEntry::Message {
            message: AgentMessage::Core(Message::Assistant(a)), ..
        }) => Some(a),
        _ => None,
    });
    // Pi `getContextUsage`: `const model = this.model; if (!model) return undefined;`
    // (agent-session.ts:3165-3166) and `if (contextWindow <= 0) return undefined;` (:3168-3169).
    // cyrup's return type is non-optional, so the modelless case degrades to a zero window,
    // which `from_last_assistant` already renders as fraction 0.0 — the same "unknown
    // occupancy" the TUI shows for an undefined usage.
    let window = { Self::lock(&self.compaction_model).as_ref().map_or(0, |m| m.context_window) };
    crate::state::ContextUsage::from_last_assistant(last, window)
}
```

One lock, one O(branch-depth) pointer walk, **zero** message clones.

### Why this compiles exactly as written (all four points verified in-tree)

* **Import shadowing is required, and the pattern is established.** session.rs:12 has
  `use cyrup_agent::{Agent, AgentMessage};` at top level, so an unqualified `AgentMessage` in this
  file means the *agent* crate's enum. The function body's inner `use cyrup_session::AgentMessage;`
  shadows it with the *session* crate's union
  ([`agent_message.rs:106`](../../../crates/cyrup-session/src/agent_message.rs#L106), re-exported at
  `cyrup_session::AgentMessage`) — the identical inner-`use` pattern
  [`has_post_compaction_usage`](../../../crates/cyrup-session-svc/src/session.rs#L4080) already uses
  at session.rs:4084-4086. `Entry`/`KnownEntry` come from
  [`cyrup_session::entry`](../../../crates/cyrup-session/src/entry.rs#L53) (also re-exported at the
  crate root, but the `entry::` path matches the neighbour). `Message` needs no import — it is in
  the top-level `cyrup_core::{…}` group at session.rs:14.
* **The match shape is real.** [`KnownEntry::Message { base, message }`](../../../crates/cyrup-session/src/entry.rs#L54)
  carries `message: AgentMessage`, and `AgentMessage::Core(Message)` is the first variant
  ([`agent_message.rs:106-108`](../../../crates/cyrup-session/src/agent_message.rs#L106)). Matching
  `&Entry` with default binding modes binds `a: &AssistantMessage`, so
  `last: Option<&AssistantMessage>` — exactly
  [`from_last_assistant(last: Option<&AssistantMessage>, context_window: u64)`](../../../crates/cyrup-session-svc/src/state.rs#L285)'s
  first parameter.
* **Borrows are sound.** `branch_path(None) -> Vec<&Entry>` borrows `guard`; `last` borrows from
  those entries; `guard` is alive for the whole body and is still in scope when
  `from_last_assistant(last, window)` consumes the reference. No clone, no lifetime extension.
* **No lock-order hazard.** The body holds the async `manager` guard while taking
  `compaction_model`, a `std::sync::Mutex` leaf lock taken through the poisoning-tolerant helper at
  session.rs:684. Every one of the ~15 `compaction_model` acquisition sites in this file
  (session.rs:1212, 1581, 1935, 2144, 3190, 3548, 3575, 4127, 4140, 4467, 4622, 4747, 4875, …)
  clones or reads a field and drops the guard immediately — none holds it across an `.await` and
  none acquires `manager` while holding it — so no lock-order cycle can exist. The
  `Self::lock(…)` temporary dies at the end of the `let window = …;` statement, before
  `from_last_assistant` runs.

### Equivalence argument — the reverse branch scan returns the same assistant

The old code scanned the **compaction-windowed built context**; the new code scans the **full
active branch**. Case analysis:

* **No compaction on the branch** — the built context *is* the branch's message list. Identical
  answer.
* **Compaction with a post-compaction assistant** — the built context's tail is the post-compaction
  messages, so its last assistant is the branch's last assistant. Identical answer.
* **Compaction with no post-compaction assistant** — the built context's kept window is a
  *contiguous suffix* of the pre-compaction path ending at the compaction entry
  ([`build_context_messages`](../../../crates/cyrup-session/src/context.rs#L151): from
  `first_kept_entry_id` to the compaction), so its last assistant is the branch's last
  pre-compaction assistant. Identical answer — **except** in the narrow sub-case where the kept
  window contains zero assistants while an earlier pre-compaction assistant exists: the old code
  returned `None` (→ `used_tokens: 0`), the new code returns that assistant's usage. The divergence
  is unreachable on the footer path — `stats_context_usage`'s
  [`has_post_compaction_usage`](../../../crates/cyrup-session-svc/src/session.rs#L4080) guard returns
  `false` in exactly this situation and answers `{tokens: None, percent: None}` without ever
  reading `used_tokens` — and on the `GetContextUsage` command path the new answer matches the
  accessor's own docstring ("occupancy from the last assistant turn") instead of reporting a
  phantom `0`. Same or strictly more correct in every case, and the three footer-observable states
  (`percent: Some`, `percent: None`, `None`-for-no-window) are byte-identical because the guard
  logic is untouched.

---

## Definition of done

* **No per-event work walks history into allocations.** The `context_usage` body contains no call
  to `messages()`, `build_context()`, or `build_context_messages`; it takes exactly one
  `manager.lock().await`, performs one `branch_path(None)` reverse scan, and lets only a
  `&AssistantMessage` flow into `ContextUsage::from_last_assistant` — zero message clones.
* **The `MessageEnd`/`AgentEnd` footer path builds no `Vec<Message>`.** Following
  [`refresh_context_usage`](../../../crates/cyrup-tui/src/app.rs#L5847) →
  [`stats_context_usage`](../../../crates/cyrup-session-svc/src/session.rs#L4044) →
  `context_usage`, per-event work is bounded by two O(branch-depth) reference-only scans (the new
  last-assistant scan plus the pre-existing clone-free `has_post_compaction_usage`), each under its
  own short manager lock.
* **Signatures and neighbours are untouched and compiling.** `context_usage`'s signature is
  unchanged, so [`SessionCommand::GetContextUsage`](../../../crates/cyrup-session-svc/src/command.rs#L190)
  keeps compiling as-is; [`state_view`](../../../crates/cyrup-session-svc/src/session.rs#L4133)
  (which legitimately needs `messages()` for the RPC `get_state` and `message_count`) is unchanged
  and still compiles; the crate builds with no new warnings at the edited site.

## Do not touch

* `messages()` / `raw_context_messages()` / `build_context` / `build_context_messages` — the RPC
  `get_state`, replay, and `raw_context_messages` (the `/resume`/`/fork`/`/import` seed path) all
  genuinely need the full list. Only the `context_usage` accessor changes.
* [`has_post_compaction_usage`](../../../crates/cyrup-session-svc/src/session.rs#L4080) — already
  clone-free over `entries()`; do not fuse it into the new scan or change its lock acquisition.
* [`AgentMessage::push_llm`](../../../crates/cyrup-session/src/agent_message.rs#L188) /
  [`push_as_message`](../../../crates/cyrup-session/src/context.rs#L59) — the flatten itself is
  correct; only this caller stops paying for it.
* [`HostServices::context_usage`](../../../crates/cyrup-session-svc/src/host_services.rs#L1256) —
  same name, different receiver (extension-guest pushed value); unrelated.
* The last-assistant-**usage** semantic of
  [`ContextUsage::from_last_assistant`](../../../crates/cyrup-session-svc/src/state.rs#L285) — Pi's
  `estimateContextTokens` alternative was considered and rejected above; adopting it is a different
  task.

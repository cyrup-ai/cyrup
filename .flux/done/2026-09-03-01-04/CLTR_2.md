---
stage: qa
status: completed
updated: 2026-09-03 23:20
aug_against: main 2cfff0f + CLTR_1 (branch 3a5784a) — UNCAPPED sweep of every crate incl. tests; every count exact
---

# CLTR_2 — `AppRole` closed enum; `Custom` reserved for extension messages (F2)

OBJECTIVE: Replace `AgentMessage::App { role: String, .. }` + the `APP_MESSAGE_ROLES` const array
+ string compares with a closed `AppRole { BashExecution, BranchSummary, CompactionSummary }`, and
collapse the two representations of a bash execution (live `Custom{kind:"bashExecution"}` vs
resumed `App{role:"bashExecution"}`) into one. **Wire bytes unchanged** (boundary B1). Source:
`.flux/research/CORE_LOOP_TYPE_REVIEW.md` §3 F2, §6 step 2.

> **READ §0 FIRST.** CLTR_1's exec showed the plan's crate inventories under-count. This file's
> sweep was uncapped across all 23 crates including tests. Three things the previous version got
> wrong or missed are corrected below.

---

## 0. What the sweep found — corrections

**0.1 The blast radius is smaller than CLTR_1's, and it is now exact.** Non-test source that names
the variant, the const, or an app-role string *as a Rust value*: **`cyrup-agent/src/event.rs`,
`cyrup-agent/src/lib.rs`, `cyrup-session-svc/src/{hooks.rs, event.rs, session/bash.rs}`.** That is
all. Every other hit is a wire/JSON literal that must stay a string: `cyrup-session/src/agent_message.rs`
(the session's OWN enum's hand-written serde — boundary B4), `cyrup-config` (`branchSummary.reserveTokens`
settings keys, unrelated), `cyrup-tui` tests and `cyrup-modes` tests (JSON assertions), and every
`App {` in `cyrup-tui` is the TUI's own `struct App`, not this variant. **TUI, subagents, modes,
config, session: untouched.**

**0.2 The deferred-flush path is a SECOND bash site the previous version missed.**
[`session/bash.rs:271`](../../crates/cyrup-session-svc/src/session/bash.rs) —
`if let AgentMessage::Custom { payload, .. } = &msg` in `flush_pending_bash_messages` — destructures
the message `:221` built. Once `:221` builds an `App`, `:271` must match `App`, and the payload's
type changes from `serde_json::Value` (`Custom.payload`) to `serde_json::Map` (`App.payload`), so
the `Value` handed to `append_bash_message(msg, &payload)` (`:279`) and on to
`append_custom_message("bashExecution", content: Value, …)` (`cyrup-session/src/manager/append.rs:88-94`)
must be re-wrapped as `Value::Object(map.clone())`. The persisted bytes are identical.

**0.3 `raw_role_tag` has exactly ONE caller** ([`event.rs:436`](../../crates/cyrup-session-svc/src/event.rs)),
and that caller can only ever see the three app roles (the `Core` and `Custom` arms matched first).
Returning `AppRole` from a function that also names `user`/`assistant`/`toolResult`/`custom` would
be a lie; the prescriptive path is a new `app_role_of(&Raw) -> Option<AppRole>` off
[`MessageRole`](../../crates/cyrup-session/src/agent_message.rs) (`:118-126`, seven variants) and
**delete `raw_role_tag`**.

**0.4 No exhaustive `AppRole` match is needed at the LLM boundary — correct the old DoD.** The
`App { payload, .. }` arm at [`hooks.rs:89-95`](../../crates/cyrup-session-svc/src/hooks.rs)
deserializes the stored wire object into the session's `Raw` union and calls `push_llm`, which
dispatches on the role *inside the payload*. The compile-time guarantee lives at construction
(`AppRole`, no `From<String>`) and at deserialization (`AppRole::parse` in the gate). Adding a
`match role { … }` there would restate a dispatch the payload already performs.

**0.5 No other producer routes an app-role message through `MessageEnd`.** `subscriber.rs:172-184`'s
`Custom` persist arm is reached only via `MessageEnd`; live bash bypasses it
(`bash.rs:287` persists directly). `compaction.rs:483`, `forking.rs:61,392`, `builder.rs:847` are
comments. No `App` persist arm is needed (matches the research's resolved question 2).

---

## 1. The enum and the variant (SUBTASK1 — `cyrup-agent/src/event.rs`)

What: replace [`event.rs:79-85`](../../crates/cyrup-agent/src/event.rs) (the doc + `APP_MESSAGE_ROLES`) with:
```rust
/// The three declaration-merged coding-agent roles [`AgentMessage::App`] carries
/// (`coding-agent/src/core/messages.ts:68-77` @v0.83.0, minus `custom`, which has its own arm).
/// Closed: an `App` with any other role is unrepresentable, and deserialization of any other
/// unknown role still fails exactly as before — pi's union is closed over the merged set too.
///
/// No `From<String>` / `From<&str>`: parsing is the one fallible door, `parse`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AppRole { BashExecution, BranchSummary, CompactionSummary }

impl AppRole {
    pub const ALL: [AppRole; 3] = [AppRole::BashExecution, AppRole::BranchSummary, AppRole::CompactionSummary];
    /// The pi `role` tag — the exact wire string.
    pub const fn as_str(self) -> &'static str {
        match self { Self::BashExecution => "bashExecution", Self::BranchSummary => "branchSummary", Self::CompactionSummary => "compactionSummary" }
    }
    /// The deserialize gate. `None` for every other tag, including the four typed roles.
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.as_str() == s)
    }
}
```
and change the variant (`:71-76`) to `App { role: AppRole, payload: serde_json::Map<String, Value> }`,
updating its doc line `:72` ("The pi `role` discriminant …") to name `AppRole`. Delete
`APP_MESSAGE_ROLES`. Update the doc references at `:79`, `:146-147`.
Where: `crates/cyrup-agent/src/event.rs`. Why: `App { role: "anything" }` constructs and
serializes today (`:138`) but can never deserialize (`:179`).

## 2. The hand-written serde impl — B1, do NOT derive (SUBTASK2 — same file)

What: **serialize `:138` unchanged** — `App { payload, .. } => payload.serialize(serializer)`.
Deserialize `:178-186`: replace `&& APP_MESSAGE_ROLES.contains(&role)` with a `let Some(role) =
AppRole::parse(role)` binding and drop the `role.to_string()` at `:181`:
```rust
if let Some(role) = v.get("role").and_then(Value::as_str).and_then(AppRole::parse) {
    let Value::Object(payload) = v else { return Err(D::Error::custom("agent message must be a JSON object")); };
    return Ok(AgentMessage::App { role, payload });
}
```
`event.rs:93`'s reason for the hand-written impl (the duplicate-`role`-key bug) stands; edit, never
replace with a derive. Why: the gate becomes the enum's own parser.

## 3. Re-exports (SUBTASK3)

What: [`cyrup-agent/src/lib.rs:25`](../../crates/cyrup-agent/src/lib.rs) —
`pub use event::{AgentEvent, AgentMessage, ToolResultMessage, APP_MESSAGE_ROLES};` →
`pub use event::{AgentEvent, AgentMessage, AppRole, ToolResultMessage};`.
[`cyrup-sdk/src/lib.rs`](../../crates/cyrup-sdk/src/lib.rs) re-exports no `AgentMessage` by name
(`:72` lists only stream types; `:109` aliases the whole crate) while `handle.rs:425` returns
`Vec<cyrup_agent::AgentMessage>` — add `pub use cyrup_agent::{AgentMessage, AppRole};` beside `:72`
so SDK users can match on it without the module alias.

## 4. `cyrup-session-svc` — delete the overload (SUBTASK4)

**4.1 [`hooks.rs`](../../crates/cyrup-session-svc/src/hooks.rs).** Delete the overload comment
`:57-74` and `app_role_payload` `:101-124` entirely. The `Custom` arm `:75-85` becomes only pi's
`case "custom"`:
```rust
AgentMessage::Custom { payload, timestamp, .. } => {
    out.push(cyrup_session::agent_message::custom_to_message(payload, timestamp.unwrap_or(0)));
}
```
The `App` arm `:89-95` is **unchanged** (§0.4). Update the doc at `:30` ("The three
[`AgentMessage::App`] roles …") to note `Custom` is now only ever an extension `customType`.

**4.2 [`event.rs`](../../crates/cyrup-session-svc/src/event.rs).** Replace `raw_role_tag` (`:453-466`)
with:
```rust
/// The app role of a raw context message, iff it is one of the three declaration-merged roles.
/// `None` for `user`/`assistant`/`toolResult`/`custom`, which never reach the `App` arm.
fn app_role_of(m: &cyrup_session::agent_message::AgentMessage) -> Option<AppRole> {
    use cyrup_session::agent_message::MessageRole;
    match m.role() {
        MessageRole::BashExecution => Some(AppRole::BashExecution),
        MessageRole::BranchSummary => Some(AppRole::BranchSummary),
        MessageRole::CompactionSummary => Some(AppRole::CompactionSummary),
        MessageRole::User | MessageRole::Assistant | MessageRole::ToolResult | MessageRole::Custom => None,
    }
}
```
and the construction `:434-438` becomes
`(Some(role), Ok(Value::Object(payload))) => AgentMessage::App { role, payload }` over
`(app_role_of(other), serde_json::to_value(other))`, with the existing degrade arm `:441-448` as
the `_`. Doc `:415-418` still accurate. `:400` (`App { .. } => None`) unchanged.

**4.3 [`session/bash.rs`](../../crates/cyrup-session-svc/src/session/bash.rs).** `:220-227`:
```rust
let payload = bash_message_payload(command, result, options.exclude_from_context);
let serde_json::Value::Object(map) = payload else { return; };   // bash_message_payload always builds an object
let msg = AgentMessage::App { role: AppRole::BashExecution, payload: map };
```
(`timestamp`/`details` fields are gone — the pi wire object inside `payload` is the whole message,
exactly as the resumed path stores it.) `:271-273`:
```rust
if let AgentMessage::App { payload, .. } = &msg {
    let value = serde_json::Value::Object(payload.clone());
    self.append_bash_message(msg, &value).await;
}
```
`:279-288` `append_bash_message` and its `append_custom_message("bashExecution", …)` call at
`:287` are **unchanged** — the persisted bytes are the same object. Check `bash_message_payload`
(`grep -n 'fn bash_message_payload' bash.rs`) returns a `Value::Object` — it does today; if a
`timestamp` was previously supplied only via `Custom.timestamp`, inject it into the map here so
the live and resumed objects stay identical.

## 5. Test plumbing (type only; no assertion's meaning changes)

| file:line | today | after |
|---|---|---|
| `cyrup-agent/src/tests/agent_message_role_key.rs:113-116` | `let App { ref role, .. }`; `assert_eq!(role, "compactionSummary")` | `assert_eq!(*role, AppRole::CompactionSummary)` |
| `session-svc/src/tests/integration.rs:314-319` | `match role.as_str() { "bashExecution" => …, _ => "app" }` | `role.as_str()` — the enum method; the `_ => "app"` arm is now unreachable and is deleted |
| `session-svc/src/tests/agent_transcript_raw_seed.rs:134,277` | `if role == "compactionSummary"` | `if *role == AppRole::CompactionSummary` |
| `…:348-351` | `role: "branchSummary".into()` | `role: AppRole::BranchSummary` |
| `…:481` | `assert_eq!(role, "bashExecution")` | `assert_eq!(role, AppRole::BashExecution)` |
| `…:516,526` | `if role == "branchSummary"` / `"compactionSummary"` | `if *role == AppRole::…` |
| `cyrup-agent/src/tests/support.rs:235`, `session-svc/event.rs:400`, `hooks.rs:211` | `App { .. }` | unchanged |

## 6. Definition of done

- `AppRole` exists as in §1 with `ALL`, `as_str`, `parse`; **no `From<String>`/`From<&str>`**.
- `AgentMessage::App.role: AppRole`; `APP_MESSAGE_ROLES`, `app_role_payload`, `raw_role_tag` are gone.
- `bash.rs` constructs `App` at `:221` **and** matches `App` at `:271`; `append_custom_message("bashExecution", …)` at `:287` untouched.
- The `Custom` arm of `coding_agent_convert_to_llm` is only `custom_to_message`.
- **Wire byte-identical**: serialize an `App` and a `Custom` before and after — identical; every
  fixture under `cyrup-agent/src/tests/` and `cyrup-session-svc/src/tests/` carrying an app role
  deserializes; `cyrup-session/src/agent_message.rs` (B4) and both WIT files untouched.
- `cyrup-tui`, `cyrup-ext-subagents`, `cyrup-modes`, `cyrup-config`, `cyrup-session` **untouched** (§0.1).
- `cargo check --workspace --all-targets --features test-fixtures` green;
  `cargo test -p cyrup-agent -p cyrup-session-svc` green; clippy exits 0 with no warning from a changed file.

## Research notes

Research §3 F2 (resolved question 2 → §0.5 here), §2 boundaries B1/B4. `prompt_runtime.rs:1002-1007`
matches `Custom.kind` against its own allowlist and is unaffected. The session bridge at
`session-svc/src/event.rs:423-430` (`Custom{kind,payload}` ↔ `Raw::Custom{custom_type,content}`)
stays the one legitimate conversion site.

No tests to be written — another team owns tests. No benchmarks to be written.

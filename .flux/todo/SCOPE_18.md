---
stage: qa
status: completed
updated: 2026-09-14 06:00
---

# SCOPE_18 — the foreground child steer handle (`ForegroundChildControl.steer`)

> Created 2026-09-12 during [`WORKFLOW_7`](WORKFLOW_7.md)'s augment, which found this unowned.
> It is the **second orphan** in this family, the same shape as the async-workflow orphan
> [`WORKFLOW_1`](WORKFLOW_1.md) §4 records — and it was found by that file's own §5 rule #3
> (*"does every 'deferred to X' row have an X that ACCEPTS it?"*).
>
> **Re-augmented 2026-09-13 against the live tree @ `f8bec9ee`.** §0.5 below lists the five
> corrections that pass made. Two of them change what gets built, and one of those two is a
> **dead-drop defect**: the delivery arm as previously specified would have written the steer
> request into a queue that **nobody drains on the foreground path**.

OBJECTIVE: make a live foreground child steerable — port pi's `ForegroundChildControl.steer`
(`shared/types.ts:2127`), the two propagation assignments
[`WORKFLOW_6`](WORKFLOW_6.md) silently dropped, and the delivery arm
[`WORKFLOW_7`](WORKFLOW_7.md) §1.6 deliberately left on upstream's refusal.

**Depends on WORKFLOW_6** (`ForegroundChildEntry`, `active_children` — **LANDED**, verified
in-tree) and **WORKFLOW_7** (the resolver and the routing that reach this handle — **NOT LANDED**;
`extension/executor/workflow_steering.rs` does not exist and `control_is_live_in_workflow` has no
definition anywhere in the crate).

---

## §0.0 — ⚠⚠ RE-AUGMENT 2026-09-14 — **MOST OF THIS TASK HAS ALREADY LANDED**, in a narrower shape

> Verified against the live tree at `/home/user/cyrup`, branch `claude/subagents-scope` (cut from
> `main` @ `d53763b`), read file by file. **Nothing below removes a requirement.** Where the tree
> already satisfies a requirement the citation is corrected and the section is marked ✅; where the
> tree does something *different* from what a section prescribes, BOTH are recorded side by side and
> the divergence is called out; where a requirement is genuinely still open it is marked ❌ and is
> the real remaining work.

### 0.0.1 Environment corrections that apply to EVERY citation in this file

| the file says | the truth here |
|---|---|
| `/home/d0m17bw/workspace/cyrup` | **`/home/user/cyrup`**. Every cyrup path below is relative to `crates/cyrup-ext-subagents/src/`. |
| `/home/d0m17bw/workspace/pi-subagents` @ `57278d82` | **ABSENT from this machine.** There is no pi checkout anywhere. **Every `pi …:NNN` citation in this document is UNVERIFIED by this pass** — treat pi line numbers as indicative, not as addresses. The pi *semantics* quoted in-file are preserved verbatim and are still the contract. |
| `[WORKFLOW_6](WORKFLOW_6.md)`, `WORKFLOW_7.md`, `WORKFLOW_8.md`, `WORKFLOW_12.md`, `WORKFLOW_13.md` | **None of these files exist anywhere under `.flux/`** (`todo/`, `done/`, `backlog/`, `review/`, `research/`). Every such link in this document is dangling. Their **code** landed; their task files did not travel to this repo. `.flux/todo/` holds `WORKFLOW_17`–`WORKFLOW_21` only. |
| `cyrup @ f8bec9ee` | Superseded. `main` has moved a long way: WORKFLOW_17–21 landed (the whole `workflowScript` runtime surface — live `runs.status`, the child-stop registry, `runs.host`, workflow `state.get/set`, live emit forwarding), plus `9aeba76` (process-group probe rewrite in `workflows/host_command.rs`, boxed `WorkflowScriptError`). **Effectively every cyrup line number in §1–§8 has shifted.** Corrected numbers are tabulated below. |

**The tree is GREEN on this commit** (`cargo fmt --all -- --check` clean, `cargo clippy --workspace
--all-targets -- -D warnings` clean, `cargo nextest run --workspace` 9913/9913). Anything that goes
red is the implementor's.

### 0.0.2 The headline: `WORKFLOW_7` **HAS** landed, and a task the code calls **`WORKFLOW_14` — which IS this objective — has landed too**

§0's dependency note (*"WORKFLOW_7 — **NOT LANDED**; `extension/executor/workflow_steering.rs` does
not exist and `control_is_live_in_workflow` has no definition anywhere in the crate"*) is **STALE**.

* `extension/executor/workflow_steering.rs` **exists**, 773 lines: `control_is_live_in_workflow`
  (`:84-95`), `active_workflow_error` (`:109-149`), `resolve_workflow_foreground_steering_target`
  (`:157-245`), `steer_workflow_foreground` (`:253-337`), plus its own test module from `:340`.
* `WORKFLOW_13` landed: a workflow owns a **real run directory**, and that is where the landed design
  roots the steer control tree.
* The in-tree docs attribute the delivery work to **`WORKFLOW_14`** by name
  (`workflow_steering.rs:14`, `foreground_control.rs:160`, `foreground.rs:142/160/900/1015`,
  `requests.rs:188`, `foreground_actions/steer.rs:275`). That is this task's objective, shipped.

**⚠ But it shipped NARROWED, and the narrowing changes what this document's OBJECTIVE means:**

> The landed design gives a steer handle to a **foreground WORKFLOW child only**, rooted in the
> **workflow's own run directory** (WORKFLOW_13). It does **not** give one to a **plain (non-workflow)
> foreground SINGLE run**, and it does **not** build the `fg-<run_id>/` scratch control tree §1
> prescribes. A plain foreground run still receives `None` for all three `RunOptions` steer paths and
> is still unsteerable.

That is a deliberate, documented choice (`foreground.rs:897-904`: *"G90 … still holds for a
foreground SINGLE run … WORKFLOW_14 narrowed it … `None` here now means 'not a workflow child', not
'impossible'"*), and it is consistent with §0.6's scope boundary — §0.6 already says the only routes
to `child.steer` in pi require a workflow. **So §1.1/§1.3/§1.4/§2's "unconditional `Some`" are now a
DECISION, not a defect: do the plain-run half only if it is still wanted, and say so explicitly.
Do not assume it, and do not silently drop it either.**

### 0.0.3 Per-section verdict

| § | requirement | verdict | where it actually is |
|---|---|---|---|
| 1.1 | `foreground_run_control_dir` in `background/artifact_roots.rs` | ❌ **NOT BUILT** — zero hits workspace-wide. Not needed by the landed (workflow-rooted) design. | `attempt_scratch_dir` is `artifact_roots.rs:325` (`_in` at `:333`); `cwd_key` at `:246`. Spec's `:325-337` / `:246` ✅ still correct. |
| 1.2 | `request_direct_steer` in `background/control.rs` | ✅ **BUILT, different name & home** — `ForegroundChildSteerHandle::deliver`, `extension/executor/foreground_control.rs:97-127`. §0.4's dead-drop defect is correctly avoided and explicitly documented. | see 0.0.4 |
| 1.3 | thread the path through `RunChannels` / `ForegroundRunOptionsInput` | ✅ **BUILT, different seam** — the *handle* rides `ForegroundRunRequest::workflow_steer`, not a path on `RunChannels`. | see 0.0.5 |
| 1.4 | teardown of `fg-<run_id>/` in `settle_foreground_run` | ❌ **NOT BUILT**, and **not applicable** under the landed design (no per-run leaf exists; the tree is inside the workflow's own run dir). | `settle_foreground_run` is now `foreground.rs:1058-1095`. |
| 2 | populate the three `RunOptions` steer fields; kill the circular comment | ✅ **BUILT** (conditionally, not unconditionally) — `foreground.rs:913-920`; circular comment replaced at `:897-911`. | see 0.0.6 |
| 2 | correct the **three stale doc claims** | ❌ **ALL THREE STILL STALE.** Real, in-scope, outstanding work. | see 0.0.6 |
| 3 | `ForegroundChildSteer` type + `ForegroundChildEntry::steer` + fill + propagate | ⚠ **HALF BUILT.** Type + entry field + `register_foreground_controls` fill + `begin_foreground_child` propagation all ✅. `ForegroundControlEntry::steer` + the `sync_current_child` assignment ❌. | see 0.0.7 |
| 4 | real delivery replacing the refusal | ✅ **BUILT** — `workflow_steering.rs:253-337`. `await_steer_ack` is already `pub(crate)` and was **not** copied. | see 0.0.8 |
| 4.2 | the no-ack word is **`queued`**, never `pending` | ⚠ **DIVERGENT** — the tree ships **`pending`** on both surfaces. | see 0.0.8 |
| 4.3 | `CHILD_SESSION_NOT_RUNNING_YET` via the capability record | ❌ **NOT BUILT AT ALL.** The constant does not exist; `read_steer_capability` has **zero production callers**. §6 invariant 7 is unmet. | see 0.0.9 |
| 5 | `WorkflowRunHost::supports_steer` + `steer` | ✅ **BUILT** — `extension/executor/workflow.rs:827-829` and `:835-936`. | see 0.0.10 |
| 5.1 | select by `(parent_workflow_run_id, workflow_key, active_children non-empty)` | ⚠ **DIVERGENT** — the host resolves the index from its **own `launched` map**, never scanning `foreground_controls`. Stronger on lane identity, weaker on liveness. | see 0.0.10 |
| 5.2 | **poll** to the ack deadline | ❌ **NOT BUILT.** No loop, no 10 ms sleep, `cancel` is `_cancel`. | see 0.0.10 |
| 5.3 | four-state receipt incl. `Missed`, via a `workflow_steer_receipt` port | ❌ **NOT BUILT.** `WorkflowSteerState::Missed` is produced **nowhere** in the crate; the mapping is inline, not a named helper. | see 0.0.10 |
| 0.6 / 6.10 | plain foreground run still refused | ✅ **HOLDS** — `text.rs:116` fired at `foreground_actions/steer.rs:136`, *after* the workflow route at `:115-125`. | |
| 0.1 | *"Add a one-line note to WORKFLOW_8"* | ❌ **IMPOSSIBLE HERE** — no `WORKFLOW_8.md` exists under `.flux/`. The naming trap it warns of is real and now worse: `extension/executor/foreground_actions/steer.rs` hosts **both** `control_steer` (`:74`) **and** `await_steer_ack` (`:265`). |

### 0.0.4 §1.2 — what shipped instead of `request_direct_steer`

**File:** `extension/executor/foreground_control.rs` (the handle lives beside the entry it hangs off,
not in `background/control.rs`). `#[derive(Clone, Debug)]`, and **`pub`, not `pub(crate)`** — it is a
field type of the `pub` `ForegroundRunRequest`, re-exported at `extension/mod.rs:79`.

```rust
// foreground_control.rs:56-74 — THREE fields, not §3's five.
pub struct ForegroundChildSteerHandle {
    pub inbox_dir: std::path::PathBuf,   // carried, never re-derived (the whole point)
    pub run_dir:   std::path::PathBuf,   // the WORKFLOW's run dir; the ack half derives off it
    pub index:     usize,                // flat index WITHIN THE WORKFLOW
}

// foreground_control.rs:97-127 — the exact function §1.2 specifies, minus the `PathBuf` return.
pub(crate) async fn deliver(
    &self,
    message: &str,
    mode: Option<crate::background::control::SteerDeliveryMode>,
    source: &str,                                   // NOT `Option<&str>`
) -> Result<String, crate::error::SubagentError>    // NOT `(PathBuf, String)`
```

Every §1.2 requirement is met by it: blank-message `SubagentError::Management("steer message must not
be empty.")` (`:105-109`); `id: control::next_steer_request_id()` (`:113`) — **the private minter is
still `pub(crate)` and was NOT widened** (`background/control.rs:1241`; §1.2's "do not widen" ✅);
`mode: mode.filter(|m| *m != SteerDeliveryMode::Steer)` (`:120`); `target_index: Some(self.index)`
(`:121`); and the write goes to **`self.inbox_dir`** (`:125`), i.e. the carried
`step_steer_inbox_dir` value — **never `steer_requests_dir`**. §6 invariant 4 ✅. The doc at `:40-52`
records that an earlier revision *did* commit exactly §0.4's dead-drop bug, which is independent
confirmation that §0.4 was right.

**⚠ Note the two-namespace hazard the spec did not name:** `handle.index` is the child's flat index
**within the workflow**; the key into `ForegroundControlEntry::active_children` is its index **within
its own foreground run** (always `0`). The landed code documents this at `workflow_steering.rs:285-289`
and `foreground_control.rs:66-73`. §1.1's "index is always 0" reasoning applies only to the latter.

**Corrected `background/control.rs` citations — §8's whole block is shifted:**

| symbol | §8 says | **now** |
|---|---|---|
| `control_inbox_dir` | `:618` | `:618` ✅ |
| `SteerRequest` / the id-ordering contract | `:1128-1138` / `:1140-1149` | `struct` at `:1132` |
| `steer_requests_dir` | `:1165` | **`:1186`** |
| `step_steer_inbox_dir` | `:1174` | **`:1195`** |
| `write_steer_request_to_dir` | `:1196` | **`:1217`** |
| `next_steer_request_id` (**still private/`pub(crate)`**) | `:1217` | **`:1241`** |
| `request_async_steer_with_mode` | `:1258` | **`:1279`** |
| `enqueue_step_steer` | `:1293` | **`:1314`** |
| `consume_steer_requests` | `:1354` | **`:1375`** (`consume_steer_requests_from_dir` at `:1334`) |
| `SteerDeliveryMode` | `:1376` | **`:1397`** (`as_str` `:1427`, `next` `:1438`) |
| `MAX_STEER_QUEUE_SIZE` | `:1430` | **`:1451`** |
| `SteerAckState` | `:1448-1458` | **`:1469-1480`**, `as_str` → `"delivered"`/`"queued"`/`"failed"` at **`:1485-1491`** |
| `SteerAck` | `:1476` | **`:1497`** |
| `SteerCapability` | `:1506` | **`:1527`** |
| `steer_capability_path` | `:1530` | **`:1551`** |
| **`steer_acks_dir` (PLURAL)** | `:1536` | **`:1557`** — §0.5 correction #2 **STANDS**: there is still no `steer_ack_dir` function; the singular name is only the `RunOptions` field (`exec/agent_config.rs:611`) and the env var. |
| `read_steer_capability` | `:1671` | **`:1692`** |
| `take_steer_acks` | `:1706` | **`:1727`** |

### 0.0.5 §1.3 — the seam that was actually used

`RunChannels` (`foreground.rs:75-96`) did **not** gain a `fg_control_dir`. The handle itself rides the
request instead, which satisfies §1.3's real goal (**one value, both sides**) more directly:

* `ForegroundRunRequest::workflow_steer: Option<ForegroundChildSteerHandle>` — `extension/executor/requests.rs:205`, doc at `:188-204` (*"ONE field feeds BOTH halves … Deriving both from one value is what makes the two sides incapable of disagreeing about the index."*).
* Destructured in `run_foreground_impl` at `foreground.rs:302`.
* → `ForegroundRunOptionsInput::workflow_steer: Option<&'a ForegroundChildSteerHandle>` (`foreground.rs:142-147`), passed at `:342`.
* → `ForegroundControlIdentity::workflow_steer` (`foreground.rs:160-164`), passed at `:355` with the comment *"The SAME handle the run options above were derived from — one value, both sides."*
* Minted in exactly one place: `WorkflowRunHost::child_steer_handle(index)` — `extension/executor/workflow.rs:270-276` — called by `launch` (`:696`, `workflow_steer: Some(self.child_steer_handle(index))`) and by `steer` (`:882`). Its doc names this as the single-construction-point invariant.

**Corrected `foreground.rs` structural citations:**

| item | §8 says | **now** |
|---|---|---|
| `RunChannels` | (implied `:612-710`) | `:75-96` |
| `ForegroundRunOptionsInput` | `:96-140` / `:103-140` | **`:105-151`** (`workflow_steer` at `:142-147`) |
| `ForegroundControlIdentity` | — | **`:153-166`** (`workflow_steer` at `:160-164`) |
| `run_foreground_impl` | `:365-380` for the scratch note | **fn at `:267`**; the *"why the scratch root is NOT deleted"* comment is at **`:390-403`** |
| `resolve_run_channels` | `:612-710` | **`:626`** |
| `build_foreground_run_options` | (§2 `:865-872`) | **`:749`**; the steer fields at **`:913-920`** |
| `child_index: Some(0)` | `:832` | **`:884`** |
| `register_foreground_controls` | `:903`, fill at `:949` | **`:951`**, fill at **`:1019`** |
| `settle_foreground_run` | `:1003-1040` | **`:1058-1095`** (the `std::sync::Mutex` section §1.4 warns about is **`:1069-1086`**, and it still holds no `.await` — the rule's home is `extension/executor/mod.rs:150-151`, not `:145-147`) |
| the `foreground_controls` map | `mod.rs:134` | **`extension/executor/mod.rs:138`** |

### 0.0.6 §2 — what shipped, and the three docs that are STILL WRONG

The circular G90 comment is **gone**. `foreground.rs:897-911` now carries the real reason, and the
three fields at **`:913-920`** are:

```rust
steer_inbox_dir: workflow_steer.map(|h| h.inbox_dir.clone()),
steer_ack_dir:   workflow_steer.map(|h| crate::background::control::steer_acks_dir(&h.run_dir, h.index)),
steer_capability_path:
                 workflow_steer.map(|h| crate::background::control::steer_capability_path(&h.run_dir, h.index)),
```

`steer_acks_dir` **plural** ✅ (§0.5 correction #2 confirmed live). `ALL THREE OR NONE` is stated and
enforced by deriving from one `Option` (`:906-912`). §2's unconditional `Some(control::…(input.fg_control_dir, 0))`
form did **not** ship and cannot, because there is no `fg_control_dir`.

**❌ STILL OPEN — §7 DoD bullet 4. All three stale doc claims survive verbatim and are now
demonstrably false for a foreground WORKFLOW child:**

1. **`exec/agent_config.rs:584-585`** — *"`None` on the foreground path (no async run directory exists), matching upstream's own `if (input.steerInboxDir)` guard."* (field `steer_inbox_dir` at `:586`; `steer_ack_dir` at `:598`; `steer_capability_path` at `:611`.)
2. **`prompt_runtime.rs:1409-1411`** — *"`Some` only when the parent handed this child a [`STEER_INBOX_ENV`] path — i.e. only for a background/async child, which is the only kind that has an async run directory to steer through."* The first clause stays; the parenthetical is false.
3. **`exec/spawn_plan.rs:1304-1305`** — *"Absent on the foreground path, so a foreground child is byte-identical to before."* **and `:1318-1321`** — *"a foreground child — which has no run directory and therefore neither path — is byte-identical to before."* (The env overlay itself is `:1300-1341`; `STEER_INBOX_ENV` write at `:1311-1314`, `STEER_CAPABILITY_ENV` at `:1327-1330`, `STEER_ACK_DIR_ENV` at `:1337-1340`.)

**Creation is confirmed still not this task's job:** `exec/mod.rs` does `create_dir_all` for the
scratch root at **`:1101-1102`**, the inbox at **`:1117-1119`**, the acks at **`:1127-1129`**, and the
capability's parent at **`:1131-1136`** — unconditionally, best-effort, on every path. §1.3's "do not
add a fourth `create_dir_all`" ✅ still correct. (§8's `:1101-1135` is close enough; use `:1101-1136`.)

### 0.0.7 §3 — three quarters built; name the missing quarter as a DECISION

* ✅ `ForegroundChildEntry::steer: Option<ForegroundChildSteerHandle>` — `foreground_control.rs:164`
  (`interrupt` is now at **`:154`**, not `:43`; the struct starts at `:131`).
* ✅ Filled by `register_foreground_controls` at `foreground.rs:1019`: `steer: identity.workflow_steer.cloned()`.
* ✅ `begin_foreground_child` (`foreground_control.rs:188-197`) — the child is MOVED in, so the field
  travels with it, exactly as §3 predicted the caller-side guard would work.
* ✅ The test fixtures §3 flagged already carry the field: `base_entry()` at **`:251-273`** and
  `child()` at **`:275-296`** (`steer: None` at `:294`). §8's `:132-171` is stale. A **third** fixture
  the spec did not know about must also be kept in sync: `workflow_steering.rs:361-389`
  (`control_entry`) and `:391-…` (`one_active_child(steer: Option<ForegroundChildSteerHandle>)`).
* ❌ **`ForegroundControlEntry` has NO `steer` field** — `extension/executor/notices.rs:20-97`; its
  fields are `interrupt` `:23`, `current_agent` `:26`, `current_index` `:28`,
  `current_activity_state` `:34`, `mode` `:38`, `description` `:41`, `current_tool` `:44`,
  `current_path` `:46`, `turn_count` `:48`, `tool_count` `:50`, `tokens` `:52`, `started_at` `:57`,
  `updated_at` `:62`, `session_id` `:68`, `parent_workflow_run_id` **`:73`** ✅, `workflow_key`
  **`:78`** (§8 said `:79`), `cwd` **`:83`** ✅, `session_name` `:87`, `active_children` **`:95`** ✅.
* ❌ **`sync_current_child` (`foreground_control.rs:171-183`) does NOT assign `steer`** — the eleven
  assignments are `current_agent`/`session_name`/`current_index`/`description`/
  `current_activity_state`/`current_tool`/`current_path`/`turn_count`/`tool_count`/`tokens`/
  `interrupt`. §0.2's dropped line `control.steer = child.steer` (pi `:60`) is **still dropped.**
  The omission list §0.2 points at is now the doc at `:167-170`, and `steer` is still not on it.

**⚠ Mechanism note the implementor needs:** under the landed design **nothing would read**
`ForegroundControlEntry::steer`. Every consumer reaches the handle through the child map —
`workflow_steering.rs:291` (`target.control.active_children.get(&index)`) then `:301`
(`child.steer.as_ref()`). Adding the mirror today ships an unread field, which trips §7's own
*"no dead-code warnings"* bullet and the crate's stated policy of not shipping ports ahead of their
first call site (`foreground_control.rs:4-9` does exactly that for `finishForegroundChild`).
**So: either (a) record the decision explicitly in the `sync_current_child` doc — "pi's
`ForegroundRunControl.steer` mirror (`shared/types.ts:2187`) has no cyrup reader; every consumer goes
through `active_children`" — closing §0.2 by documentation rather than by code, or (b) land the mirror
together with its first reader.** §6 invariant 5 is currently **half met**: `begin_foreground_child`
propagates, `sync_current_child` has nowhere to propagate to.

### 0.0.8 §4 — built; two divergences to rule on

`steer_workflow_foreground` — `extension/executor/workflow_steering.rs:253-337`:

```rust
pub(crate) async fn steer_workflow_foreground(
    &self,
    workflow_run_id: &RunId,
    message: &str,
    mode: Option<crate::background::control::SteerDeliveryMode>,
    index: Option<usize>,
    async_root: &Path,
) -> Result<String, String>
```

* index defaulting (pi `:133-137`) at `:267-284` — `active_children` is a `BTreeMap`, so `.keys()` is already sorted. ✅
* `"Foreground run '{}' child {index} is not live."` at **`:291-296`**. ✅
* `"Foreground run '{}' child {index} does not support steering."` at **`:302-307`** — §6 invariant 6 ✅, kept as the fallback exactly as §4.1 required.
* delivery at **`:313-317`**: `handle.deliver(message, mode, "workflow-steer-action")`. **§4.1's `Some("steer-action")` is not what shipped** — the two live source strings are `"workflow-steer-action"` (tool surface) and `"workflow-script-steer"` (`workflow.rs:884`, script surface). Keep them distinguishable.
* ack at **`:324`**: `Self::await_steer_ack(&handle.run_dir, &request_id, Some(handle.index)).await`.

**✅ `await_steer_ack` is ALREADY `pub(crate)` and was NOT copied** — but its home moved, and §4.1's
⚠ and §8's citation are both wrong now:

| item | §8 says | **now** |
|---|---|---|
| `control_steer` | `extension/executor/control.rs:529` | **`extension/executor/foreground_actions/steer.rs:74`** |
| the plain-foreground refusal | `control.rs:574` | **`foreground_actions/steer.rs:136`** (reached only after the workflow route at `:115-125`) |
| `await_steer_ack` (**private — must become `pub(crate)`**) | `control.rs:690-710` | **`foreground_actions/steer.rs:265-271` — already `pub(crate)`. Requirement satisfied; do nothing.** |
| — | — | **NEW:** `await_steer_ack_within(run_dir, request_id, index, budget)` at **`:280-300`**, added by WORKFLOW_14 so `runs.steer`'s `ackTimeoutMs` is honoured instead of silently retargeted. |
| `STEER_ACK_TIMEOUT` (3 s) | `text.rs:177` | `text.rs:177` ✅ (`STEER_ACK_POLL_INTERVAL` at `:182`) |
| `STEER_FOREGROUND_RUN_REFUSAL` | `text.rs:116` | `text.rs:116` ✅ |

**⚠ DIVERGENCE 1 — §4.2's receipt table and §6 invariant 8 are NOT what shipped.**
`workflow_steering.rs:325-332` renders

```rust
let state = match outcome.as_ref() { None => "pending", Some(ack) => ack.state.as_str() };
let text = format!("Steering {state} for workflow {} child {index} (request {request_id}).",
                   target.control_run_id);
```

so the no-ack word is **`pending`**, and the sentence names *"workflow {run} child {index}"*, not
§4.2's *"foreground run {id}"*. The landed code argues for this at `:334-336` (it deliberately keeps
the async arm's classification). **§4.2 forbids exactly this** (*"this surface uses `queued` … Do not
unify the two and do not invent a fourth word"*), and §6 invariant 8 says *"never `pending`"*. The same
word also appears on the script surface (`workflow.rs:912`, `delivery_status: Some("pending")` under
`WorkflowSteerState::Queued`, and `workflow.rs:933` `.map_or("pending", …)` for the per-target state).
**Rule on this explicitly**: either change the two sites to `"queued"` (and the §4.2 sentence shape),
or amend §4.2/§6.8 with the reason. Do not leave the spec and the tree disagreeing in silence.

**⚠ DIVERGENCE 2 — the `Failed` convention is now split on purpose, and the spec should record it.**
The TOOL surface returns a `Failed` ack as `Err` (`workflow_steering.rs:333-336`); the SCRIPT surface
returns it as `Ok(receipt)` with `WorkflowSteerResult::error` set (`workflow.rs:900-909` explains
why: `Err` throws in the guest and discards the request id). Two surfaces, two conventions — both
correct for their caller. Preserve both.

### 0.0.9 §4.3 — ❌ **the single largest piece of real work left**

* `CHILD_SESSION_NOT_RUNNING_YET` **does not exist anywhere in the crate** (grep: zero hits).
* **`control::read_steer_capability` (`background/control.rs:1692`) has ZERO production callers.**
  Its only two callers in the whole workspace are tests:
  `crates/cyrup-ext-subagents/src/tests/steer_delivery_integration.rs:675` and `:753`.
* So the three-way distinction §4.3 calls *"the whole of §4.3"* is **not made**. A child that has not
  yet reached its runtime (`None`) and a child whose host cannot inject at all (`Some(c)` with
  `!c.supported`) are today indistinguishable from each other and from a slow child: both simply fall
  through to `await_steer_ack`'s 3 s timeout and report `pending`.
* **§6 invariant 7 is UNMET**, and §5.2's poll has nothing to poll on until this lands — the two are
  a single unit of work, not two.
* The capability record itself is intact and still says what §4.3 relies on: `SteerCapability` at
  `control.rs:1527` (with its `pid`), `steer_capability_path` at `:1551`, and the child publishes it
  from `prompt_runtime.rs` (`STEER_CAPABILITY_ENV` read at `:2345`, threaded onto the runtime at
  `:2393`). §4.3's republish-on-every-`activate` claim should be re-checked against
  `prompt_runtime.rs`'s `publish_capability` before it is relied on — §8's `:334-345` is stale
  (the child-side env reads are now `:2332-2345`, not `:2336-2349`).

### 0.0.10 §5 — built, but by a different mechanism; three sub-requirements unmet

`impl WorkflowScriptHost for WorkflowRunHost` — `extension/executor/workflow.rs:540`. **§8's `:228` /
`launch :229` / `status :388` are all stale**, and the impl is no longer two methods:

| method | line | note |
|---|---|---|
| `launch` | `:541` | still blocking — §5.2's premise holds |
| `status` | `:744` | |
| `supports_steer` → `true` | **`:827-829`** | ✅ §7 bullet 6 |
| `steer` | **`:835-936`** | ✅ real |
| `supports_host` → `true` | `:963` | WORKFLOW_19, landed since this spec |
| `host_command` | `:970` | WORKFLOW_19 |
| `child_steer_handle` (private) | `:270-276` | the single mint point |
| `self.workflow_run_id` | **`:198`** (§8 said `:118`) | |

Engine side re-verified: trait `WorkflowScriptHost` at `workflows/scripted/engine.rs:122`; the
`supports_steer` default (`false`) at **`:160-162`**; the default refusal *"Workflow steering is
unavailable in this host."* at **`:172`**; the live capability gate `shared.host.supports_steer()` at
**`:1126`** and the same refusal at **`:1145`**. (§8's `:160-173` / `:1089` / `:1106-1108` are stale.)
`workflows/scripted/types.rs` numbers are **unchanged and correct**: `WorkflowSteerMode` `:71`,
`WorkflowSteerOptions` `:83`, **`WorkflowSteerState` `:99-108` (four variants — §0.5 correction #4
STANDS)**, `WorkflowSteerTarget` `:113-121`, `WorkflowSteerResult` `:126-146`.

**⚠ DIVERGENCE — §5.1's predicate did not ship, and what did ship is different in kind.**
`steer` never touches `foreground_controls`. It resolves the index from the host's own launch ledger
(`workflow.rs:846-856`):

```rust
let index = {
    let launched = self.launched.lock().unwrap_or_else(PoisonError::into_inner);
    launched.get(key).map(|identity| identity.index)
        .ok_or_else(|| format!("runs.steer('{key}') names no launched child in this workflow."))?
};
```

This is **stronger** than §5.1's three-term find on the two identity terms — a `WorkflowRunHost` is
constructed for exactly one workflow, and `launched` is keyed by lane key, so
`(parent_workflow_run_id, workflow_key)` hold by construction — and **weaker** on the third: the
`active_children` non-empty term, i.e. *is the child still live*, is not checked at all. (A
three-term scan of that exact shape **does** exist in this file, at `workflow.rs:491`, used by
`status`: `control.parent_workflow_run_id.as_ref() == Some(&self.workflow_run_id) && …` — that is the
pattern to reuse if §5.1's liveness term is reinstated.) There is also an extra guard §5 did not ask
for and which is worth keeping: an explicit `options.index` naming a different child is **refused**
(`:860-869`), never silently retargeted.

**❌ §5.2 — there is NO poll.** No deadline, no loop, no `Math.min(10, …)` sleep; the `cancel`
parameter is bound as `_cancel` and never read. One shot, then `await_steer_ack_within` (`:892-898`,
budget from `options.ack_timeout_ms` or `STEER_ACK_TIMEOUT`) times out. A lane that is registered but
not yet spawned therefore gets `Queued`/`pending`, not a retry. §6 invariant 9's second half is unmet.

**❌ §5.3 — the receipt is three-state in practice.** `WorkflowSteerState::Missed` is **produced
nowhere in the crate**; its only non-definition occurrence is the engine's trace mapping at
`engine.rs:1167`. The mapping is inline at `workflow.rs:910-922` — there is **no `workflow_steer_receipt`
helper**, so §7's bullet naming one is unmet as written. The receipt construction at `:924-935` does
fill `targets` with a single `WorkflowSteerTarget { index: u32::try_from(index).unwrap_or(u32::MAX),
state, reason }` (§5.3 ✅) and constructs **every field explicitly** — §5.2's ⚠ was right and remains
right: **`WorkflowSteerResult` still has no `Default` derive** (`types.rs:126`), so any new
construction site must spell all six fields out.

### 0.0.11 §6 invariant scoreboard, as of this pass

| # | status | evidence |
|---|---|---|
| 1 | ✅ **for a workflow child only** | `foreground.rs:913-920` → `spawn_plan.rs:1300-1341` |
| 2 | **N/A** — no `fg-<run_id>` tree exists | `resume_tracking` at `extension/executor/status.rs:26`, its `read_dir` at `:32` ✅ (citation still good) |
| 3 | **N/A** (follows from 2) | the scratch-root rationale survives at `foreground.rs:390-403` |
| 4 | ✅ | `foreground_control.rs:125` writes `self.inbox_dir`. `route_steer_requests` at `background/runner_main/control_watcher.rs:334`; the *"silently dead again"* invariant at **`:765-768`** (§8 said `:762-769`) |
| 5 | ⚠ **HALF** | `begin_foreground_child` ✅ `:188-197`; `sync_current_child` ❌ `:171-183` (no target field) |
| 6 | ✅ | `workflow_steering.rs:302-307` |
| 7 | ❌ **UNMET** | §0.0.9 |
| 8 | ⚠ **DIVERGENT** (`pending`, not `queued`) | `workflow_steering.rs:325`, `workflow.rs:912`/`:933` |
| 9 | ⚠ **HALF** | selection by `launched` not by the three-term find; no poll; no `Missed` |
| 10 | ✅ | `text.rs:116` fired at `foreground_actions/steer.rs:136` |
| 11 | ✅ | the four gates live in `active_workflow_error`, `workflow_steering.rs:109-149` — session present `:121-122`, controller registry `:126-128`, status read `:133-141`, `SessionGate::Strict` `:145-151` |

### 0.0.12 §7 Definition of done — what is left

| bullet | status |
|---|---|
| `foreground_run_control_dir` + `RunChannels`/`ForegroundRunOptionsInput` threading | ❌ open **as a decision** (§0.0.2): the landed design does not need it; build it only if the plain-run half is still in scope |
| `request_direct_steer` writing into `step_steer_inbox_dir`, `target_index` pinned, id from the private minter | ✅ **done** as `ForegroundChildSteerHandle::deliver` |
| three steer paths populated; circular comment replaced | ✅ **done** (conditionally on workflow) |
| the three stale doc claims corrected | ❌ **OPEN — real work, in scope, uncontroversial** |
| `ForegroundChildSteer` type, both `steer` fields, fill, propagation, fixtures compile | ⚠ **3/4 done**; `ForegroundControlEntry::steer` + `sync_current_child` open as a decision (§0.0.7) |
| `await_steer_ack` `pub(crate)` (not copied) + real delivery + retryable unpublished capability + no-handle refusal | ⚠ visibility ✅, delivery ✅, refusal ✅, **retryable capability ❌ (§0.0.9)** |
| `supports_steer()` true; selection; poll; four states via `workflow_steer_receipt` | ⚠ `supports_steer` ✅, delivery ✅, **selection divergent, poll ❌, `Missed` ❌, helper ❌** |
| plain foreground still refused; WORKFLOW_7's four gates unchanged | ✅ |
| `fg-<run_id>` removed on settle, outside the lock | **N/A** under the landed design |
| `cargo clippy --workspace --all-targets` exits 0, no new `allow`s, no dead code | ✅ **today** — the baseline is green; keep it so. ⚠ This bullet is what makes §0.0.7's "don't ship an unread field" argument binding. |

### 0.0.13 Verification commands for the implementor (read-only)

```
rg -n 'foreground_run_control_dir|request_direct_steer|CHILD_SESSION_NOT_RUNNING_YET' crates/cyrup-ext-subagents   # expect: nothing
rg -n 'WorkflowSteerState::Missed' crates/cyrup-ext-subagents                                                       # expect: engine.rs:1167 only
rg -n 'read_steer_capability' crates/cyrup-ext-subagents                                                            # expect: definition + 2 test callers
```

Format only what is touched: `cargo fmt -p cyrup-ext-subagents`. `cargo fmt --all` is a repo-wide
no-op today and must stay one.

---

## §0 — Why this task exists, and why it is smaller than it looks

> ⚠ **2026-09-14:** read [§0.0](#00--%EF%B8%8F%EF%B8%8F-re-augment-2026-09-14) FIRST. WORKFLOW_7 **has** landed and
> `extension/executor/workflow_steering.rs` **exists** — the dependency note above is stale. Most of
> §1–§5 is already in the tree, in a narrower, workflow-rooted shape. §0.0.3 has the per-section verdict.

### 0.1 Nobody owned it

Three tasks look like the owner. None is:

| task | what its "steer" actually is | evidence |
|---|---|---|
| [`WORKFLOW_8`](WORKFLOW_8.md) SUBTASK2 | `async-steering-action.ts` — a **background** run | `asyncDir`-gated at [`:45`/`:52`](../../../workspace/pi-subagents/src/runs/foreground/async-steering-action.ts), `reconcileAsyncRun` at `:71`, `requestAsyncSteer` at `:149`. A run with no directory cannot enter it. |
| [`WORKFLOW_13`](WORKFLOW_13.md) §3.2 | `WorkflowScriptHost::steer` on a **second, async** host | *"flip `supports_steer()` and fill `steer`"* — on the async host only. cyrup's FOREGROUND `WorkflowRunHost` ([`workflow.rs:228`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/workflow.rs)) implements exactly two methods — `launch` (`:229`) and `status` (`:388`) — so `engine.rs:160-173`'s default refusal stays in force after W13 ships. |
| [`WORKFLOW_12`](WORKFLOW_12.md) `:113` | `op_runs_steer` — the V8 op binding | the isolate seam, not the delivery. |

⚠ **The naming trap:** WORKFLOW_8's subtask file is literally `foreground_actions/steer.rs`.
`foreground_actions/` names pi's **source directory** (`src/runs/foreground/`), not foreground runs.
Add a one-line note to that effect in WORKFLOW_8 so the next reader does not re-lose a week here.

### 0.2 How the orphan formed — two dropped lines

WORKFLOW_6 ported [`foreground-control.ts`](../../../workspace/pi-subagents/src/runs/foreground/foreground-control.ts)
`:39-60` (`syncCurrentChild`) and `:101-137` (`beginForegroundChild`). Both `steer` assignments sit
**inside** those exact ranges:

```ts
	control.steer = child.steer;                 // :60  — last line of syncCurrentChild
	if (input.steer) child.steer = input.steer;  // :128 — inside beginForegroundChild
```

WORKFLOW_6 §1.2 enumerated what it deliberately omitted — *"`inputTokens`/`outputTokens`/`window`/
`windowPeak`/`model`/`thinking`/`lastActivityAt`/`currentToolStartedAt`/`detach`"* — and **`steer`
is not in that list.** It was dropped silently, not by decision. That is the whole defect. The
omission list is still on the live `sync_current_child` doc comment at
[`foreground_control.rs:48-51`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/foreground_control.rs);
`steer` must be added to the ported body, not to that list.

### 0.3 The transport already exists end to end — with ONE hop missing

A foreground child is **a spawned `cyrup` process going through the same code as a background
one**:

```
drive_foreground_run_sync (paths.rs:300)
  └─> crate::exec::run_sync                         ← the SAME entry the background runner uses
        ├─ exec/mod.rs:1117-1135  create_dir_all(steer_inbox_dir | steer_ack_dir | capability parent)
        ├─ spawn_plan.rs:1300-1340 env[CYRUP_SUBAGENT_STEER_INBOX / _ACK_DIR / _CAPABILITY]
        └─ child: prompt_runtime.rs:2336-2349 reads the three env vars
             └─ SteeringInbox → injects each message into its live model turn (`:583`)
                             → writes SteerAck (`:326`) and SteerCapability (`:334`)
```

Every hop above is generic. The child side has **no** foreground/background distinction — it reads
three env vars and attaches a watcher. `SteerAck`/`SteerCapability`/`SteerAckState`
(`control.rs:1476`, `:1506`, `:1448`) are equally generic.

The reason the feature is dead on the foreground path is **three hard-coded `None`s**:

```rust
// foreground.rs:865-872 — the justification is CIRCULAR
// G90: a FOREGROUND single run has no async run directory and therefore no steer
// inbox — pi supplies `steerInboxDir` only from the background runner, and
// `control_steer` refuses a foreground run outright for exactly this reason.
steer_inbox_dir: None,
// SUBA-049: same reason, for the return half — a foreground run has no run directory,
// so there is nowhere to write an acknowledgment or a capability record.
steer_ack_dir: None,
steer_capability_path: None,
```

The refusal is justified by the absence and the absence by the refusal. Neither premise is a
mechanism constraint: *"a foreground run has no run directory"* is true of the **async** run tree
only, and cyrup already gives every run's `cwd` a scratch root
([`attempt_scratch_dir`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/artifact_roots.rs),
`artifact_roots.rs:325-337`), created by `exec::run_sync` at `exec/mod.rs:1101`.

### 0.4 ⚠ THE MISSING HOP — this is the defect the previous augment did not catch

**On the background path, the parent does NOT write into the child's inbox. A RUNNER routes it
there.** Two directories, two hops:

```
parent:  request_async_steer_with_mode(run_dir, …)        control.rs:1258
             └─ writes → steer_requests_dir(run_dir)      control.rs:1165  = <run_dir>/control/steer-requests/
RUNNER:  route_steer_requests(…)                          runner_main/control_watcher.rs:334
             ├─ consume_steer_requests(run_dir)           control.rs:1354   (drains the run-level queue)
             └─ enqueue_step_steer(run_dir, index, req)   control.rs:1293  → step_steer_inbox_dir(run_dir, index)
CHILD:   SteeringInbox::flush → consume_steer_requests_from_dir(self.dir)   prompt_runtime.rs:583
```

`control_watcher.rs:762-769` states the agreement as a named invariant: *"`route_steer_requests`
writes an accepted request into `control::step_steer_inbox_dir(run_dir, index)`; `run_single` hands
the child `steer_inbox_for(index)`. If those two ever diverge … the feature is silently dead again,
with no test failing."*

**A foreground run has no runner process.** `drive_foreground_run_sync` spawns the child directly;
nothing in this process drains `<fg_control_dir>/control/steer-requests/`. So reusing
`control_steer`'s `request_async_steer_with_mode` (as §4 previously implied) writes the request into
a **dead drop**: the file lands on disk, no one moves it, the child never sees it,
`await_steer_ack` times out, and the tool honestly reports `queued` forever. That is a silent
regression of exactly the shape `control_watcher.rs:762` warns about.

**The fix is one new `pub` helper (§1.2): the parent writes DIRECTLY into the per-child inbox**,
because on this path the parent IS the router. `enqueue_step_steer` already performs that write —
it just needs a minted id and a request to carry, and `next_steer_request_id` is private
(`control.rs:1217`).

### 0.5 Corrections this re-augment makes to the previous text

| # | previous claim | truth | impact |
|---|---|---|---|
| 1 | *"reuse `control_steer`'s existing ack wait and request write"* | the request write targets a queue **no foreground process drains** (§0.4) | **behavioural — the feature would ship dead** |
| 2 | `control::steer_ack_dir(&fg_control_dir, 0)` | **no such function.** It is `control::steer_acks_dir` (plural, `control.rs:1536`). Only the `RunOptions` FIELD is singular (`agent_config.rs:599`). | **compile error as written** |
| 3 | §5: *"reusing WORKFLOW_7's `control_is_live_in_workflow`"* | upstream's `steerWorkflowChildByKey` uses a **different** predicate (`subagent-executor.ts:4491-4493`): `parentWorkflowRunId && workflowKey && activeChildren.size`. `controlIsLiveInWorkflow` (`workflow-foreground-steering.ts:32-36`) compares `sessionId`, **not** `workflowKey` — it cannot select a lane. | **behavioural — wrong child, or none** |
| 4 | §4/§6: *"pi's three receipts … never a fourth state"* | true of the **tool** receipt. `WorkflowSteerState` (`scripted/types.rs:99-108`) has **four**: `Queued`/`Delivered`/`Missed`/`Failed`, and `Missed` is load-bearing in §5 (`:4512`, `:4531`, `:4534`, `:4537`). | §5 was underspecified |
| 5 | §1: *"It does have a scratch dir"* (per-run) | `attempt_scratch_dir(cwd)` is **per-`cwd`, not per-run** (`artifact_roots.rs:333-337`) — shared by every run in that directory. The `fg-<run_id>/` leaf IS the per-run partition, and that is precisely why it must be created and removed by this task. | clarification |

Also verified-still-true: every `None` at `foreground.rs:868/871/872`, the circular comment at
`:865-867`, the stale docs at `agent_config.rs:583-586` and `prompt_runtime.rs:1409-1412`, the
*"Absent on the foreground path"* comment at `spawn_plan.rs:1304-1305`, and
`ForegroundChildEntry`'s single `interrupt` handle at `foreground_control.rs:43`.

### 0.6 Scope boundary — what stays refused

A **plain** (non-workflow) foreground run addressed by `action:"steer"` stays refused with
[`STEER_FOREGROUND_RUN_REFUSAL`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/tool/text.rs)
(`text.rs:116`, fired at `control.rs:574`). That is upstream-faithful: pi refuses the same case at
`subagent-executor.ts:3217`, and the **only** routes to `child.steer` in pi are
`resolveWorkflowForegroundSteeringTarget` (requires `parentWorkflowRunId`,
`workflow-foreground-steering.ts:47`) and `steerWorkflowChildByKey` (requires a `workflowRunId` +
lane key, `:4491`). Do not widen it.

pi's third consumer — the FleetView TUI's `actions.steer` (`tui/fleet.ts:1395`) — is **out of
scope**; cyrup's fleet TUI has no steer action to wire it to yet.

---

## §1 SUBTASK1 — a control tree for a foreground run, and a direct-drop request writer

> ⚠ **2026-09-14 — §0.0.4/§0.0.5.** §1.2 is **BUILT** as `ForegroundChildSteerHandle::deliver`
> (`extension/executor/foreground_control.rs:97-127`), not as `request_direct_steer`; §0.4's dead-drop
> defect is correctly avoided. §1.3 is **BUILT** via `ForegroundRunRequest::workflow_steer`
> (`requests.rs:205`), not via a path on `RunChannels`. §1.1 and §1.4 are **NOT BUILT** and are not
> needed by the landed design — they are the plain-(non-workflow)-foreground half, which is now an
> explicit decision, not a defect. Every `background/control.rs` line number below is shifted; the
> correction table is in §0.0.4.

### 1.1 `foreground_run_control_dir`

**File:** [`background/artifact_roots.rs`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/artifact_roots.rs),
beside `attempt_scratch_dir` (`:325`).

```rust
/// `<temp_root_dir>/scratch/<cwd_key>/fg-<run_id>/` — the run-directory-shaped root a FOREGROUND
/// run's steer control tree hangs off. The `control/…` leaves below it are appended by
/// [`crate::background::control::step_steer_inbox_dir`] and friends, which is why this returns the
/// run root and NOT the `control/` directory itself.
///
/// Why not the async run tree: `resume_tracking` (`extension/executor/status.rs:26-32`) does a
/// `read_dir` over the whole async root and treats every entry as a run to reconcile, so a
/// foreground run with a directory there would surface as a status-less async run. Why the
/// per-`cwd` scratch root: it is already `run_sync`'s own working area (`exec/mod.rs:1101`),
/// already keyed by [`cwd_key`], already outside the project tree, and already disposable.
///
/// ⚠ [`attempt_scratch_dir`] is per-`cwd`, NOT per-run — every run in a directory shares it. The
/// `fg-<run_id>` leaf IS the per-run partition, which is exactly why §1.4's teardown must remove
/// this path and never its parent.
///
/// Layout mirrors the background tree byte for byte so `step_steer_inbox_dir`/`steer_acks_dir`/
/// `steer_capability_path` (`background/control.rs:1174`, `:1536`, `:1530`) are reused UNCHANGED —
/// each takes a `run_dir` and appends `control/…` via `control_inbox_dir` (`:618`), and none of
/// them knows or cares that this one is not under the async root.
#[must_use]
pub fn foreground_run_control_dir(cwd: &Path, run_id: &RunId) -> PathBuf {
    attempt_scratch_dir(cwd).join(format!("fg-{}", run_id.as_str()))
}
```

Resulting paths, for `index = 0`:

```
<temp>/scratch/<cwd_key>/fg-<run_id>/control/steer-targets/0/        ← inbox   (the child polls this)
<temp>/scratch/<cwd_key>/fg-<run_id>/control/steer-acks/0/           ← acks    (the child writes here)
<temp>/scratch/<cwd_key>/fg-<run_id>/control/steer-capabilities/0.json ← capability
```

Index is always `0`: a foreground SINGLE run has exactly one child at flat index 0, which is the
same fact `register_foreground_controls` already encodes (`foreground.rs:949`,
`begin_foreground_child` with `index: 0`) and the same default the child itself falls back to
(`prompt_runtime.rs:2347-2349`, `CHILD_INDEX_ENV` → `unwrap_or(0)` — and `foreground.rs:832` does
set `child_index: Some(0)`, so the two agree explicitly rather than by luck).

### 1.2 `request_direct_steer` — the parent-as-router write (§0.4)

**File:** [`background/control.rs`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/control.rs),
immediately after `enqueue_step_steer` (`:1293`), where both halves it fuses already live.

```rust
/// Write one steer request STRAIGHT into a child's own inbox, minting the id the caller needs to
/// wait for the acknowledgment.
///
/// This is [`request_async_steer_with_mode`] and [`enqueue_step_steer`] fused, with the runner hop
/// removed — because on the FOREGROUND path there is no runner. `drive_foreground_run_sync` spawns
/// the child in-process (`extension/executor/paths.rs:300`); nothing drains
/// [`steer_requests_dir`], so a request written there is a dead drop that the child never sees and
/// the parent's ack wait can only ever time out on. The parent IS the router here, so it writes
/// where the router would have written: [`step_steer_inbox_dir`], the exact directory
/// `run_sync` handed the child as `CYRUP_SUBAGENT_STEER_INBOX` (`exec/spawn_plan.rs:1306`).
///
/// `target_index` is PINNED to `index`, exactly as [`enqueue_step_steer`] pins it: a request
/// sitting in a per-child inbox is addressed, and the child's own validator reads the field back.
///
/// [CYRUP-DELTA: a second producer for an existing directory, not a second channel. The request
/// shape, the file name, the ordering rule and the inbox path are all pi's — only the hop that
/// would have moved the file is elided, because on this path the two hops collapse to one.]
///
/// # Errors
///
/// [`SubagentError::Management`] for an empty message (upstream's `steer message must not be
/// empty.`), or [`SubagentError::Spawn`] for an I/O failure.
pub async fn request_direct_steer(
    run_dir: &Path,
    index: usize,
    message: &str,
    mode: Option<SteerDeliveryMode>,
    source: Option<&str>,
) -> Result<(PathBuf, String), SubagentError> {
    let message = message.trim();
    if message.is_empty() {
        return Err(SubagentError::Management(
            "steer message must not be empty.".to_string(),
        ));
    }
    let request = SteerRequest {
        kind: "steer".to_string(),
        id: next_steer_request_id(),
        ts: crate::time::now_epoch_millis(),
        message: message.to_string(),
        // `Steer` is normalised OFF the wire, matching upstream's conditional spread — see
        // `SteerRequest::mode`'s own doc (`:1140-1149`).
        mode: mode.filter(|m| *m != SteerDeliveryMode::Steer),
        target_index: Some(index),
        source: source.map(str::to_string),
    };
    let id = request.id.clone();
    let path = write_steer_request_to_dir(&step_steer_inbox_dir(run_dir, index), &request).await?;
    Ok((path, id))
}
```

**Do not** widen `next_steer_request_id`'s visibility and **do not** rebuild a request in
`workflow_steering.rs`: the monotonic-sequence tiebreak documented at `control.rs:1128-1138` is the
only thing that makes same-millisecond guidance deterministically ordered, and a second minting site
is a second place for it to drift.

### 1.3 Wiring it into `run_foreground_impl`

The path must be computed **once**, before `build_foreground_run_options`, and reach three places:
the `RunOptions` (§2), the control entry (§3), and teardown (§1.4). The natural seam is
[`resolve_run_channels`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/foreground.rs)
(`:612-710`), which already mints `run_id` and already returns a `RunChannels` bundle every later
stage reads — add `fg_control_dir: PathBuf` to it and compute it beside `session_dir` (`:695-697`).
`ForegroundRunOptionsInput` (`:103-140`) then gains a borrowed `fg_control_dir: &'a Path`, matching
its existing owned/borrowed convention (borrowed = the caller still needs it, which is true here
because §1.4 tears it down).

Creation is **not** this task's job: `exec::run_sync` already does `create_dir_all` for all three
paths (`exec/mod.rs:1117-1135`), unconditionally and best-effort, on every path including this one.
Do not add a fourth `create_dir_all`.

### 1.4 Teardown

Remove the tree in [`settle_foreground_run`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/foreground.rs)
(`:1003-1040`), after the control entry is dropped.

⚠ `settle_foreground_run(&self, run_id, control_notifier)` does **not** take a `cwd` — and it must
not grow one. The entry it removes at `:1018` already carries it: `ForegroundControlEntry::cwd`
(`notices.rs:83-84`), populated from `ForegroundControlIdentity` at `foreground.rs:945`. Derive the
path from the entry being dropped:

```rust
            if let Some(entry) = controls.remove(run_id.as_str()) {
                tracing::debug!( /* … unchanged … */ );
                // WORKFLOW_14 §1.4 — the steer control tree is per-RUN (`fg-<run_id>/` under the
                // per-`cwd` scratch root), so it is removed with the run and never with its
                // parent, which every other run in this directory shares. Derived from the entry's
                // OWN `cwd` rather than a new parameter: that field exists for exactly this kind
                // of reader (`notices.rs:83-84`).
                //
                // Best-effort by construction. The tree is disposable — it holds consumed request
                // files, drained acks and one capability record — and a removal failure must never
                // turn a settled run into a failed one. The scratch root is scoped-temp, so the
                // worst case of a leak is an empty directory the OS reaps.
                if let Some(cwd) = entry.cwd.as_deref() {
                    let dir = crate::background::foreground_run_control_dir(cwd, run_id);
                    let _ = tokio::fs::remove_dir_all(&dir).await;
                }
            }
```

⚠ The block at `:1013-1031` is a `std::sync::Mutex` critical section. `remove_dir_all` is `.await`,
so it must be issued **after** the lock guard is dropped — bind the `cwd` inside the block, close
the scope, then remove. Holding a `std::sync::Mutex` across an `.await` is exactly what the
`foreground_controls` doc (`mod.rs:145-147`) says never happens.

Do **not** extend this to the scratch root itself — `run_foreground_impl:365-380` carries a long
comment explaining why an earlier revision's `remove_dir_all` of the whole scratch dir was a defect
(it discarded the per-attempt stdout tee that `cyrup-it`'s integration tests read back). This
removal is strictly the `fg-<run_id>/` leaf.

## §2 SUBTASK2 — populate the three `RunOptions` fields

> ⚠ **2026-09-14 — §0.0.6.** The three fields **are populated** (`foreground.rs:913-920`) and the
> circular comment **is gone** (`:897-911`) — but conditionally, `Some` iff the child is a workflow
> child, since there is no `fg_control_dir`. `steer_acks_dir` plural confirmed (now `control.rs:1557`).
> **The three stale doc claims below are ALL STILL STALE** and are genuine open work:
> `exec/agent_config.rs:584-585`, `prompt_runtime.rs:1409-1411`, `exec/spawn_plan.rs:1304-1305` + `:1318-1321`.

**File:** [`foreground.rs:865-872`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/foreground.rs),
in `build_foreground_run_options`.

```rust
            // WORKFLOW_14 — the pre-task comment here was CIRCULAR: it justified the absence with
            // `control_steer`'s refusal and the refusal with the absence. Neither premise is a
            // mechanism constraint. `run_sync` creates all three of these
            // (`exec/mod.rs:1117-1135`) and `spawn_plan` hands them to the child as
            // `CYRUP_SUBAGENT_STEER_INBOX`/`_ACK_DIR`/`_CAPABILITY` (`spawn_plan.rs:1300-1340`) on
            // EVERY path — a foreground child is a spawned `cyrup` process exactly like a
            // background one, and the child-side `SteeringInbox` has no foreground/background
            // distinction at all (`prompt_runtime.rs:2336-2349`).
            //
            // The tree is NOT under the async run root: `resume_tracking` reads that root
            // directory-wise (`status.rs:26-32`) and would surface a foreground run there as a
            // status-less async run. See `foreground_run_control_dir` (`artifact_roots.rs`).
            //
            // Index 0: a foreground SINGLE run has exactly one child, the same fact
            // `register_foreground_controls` encodes with `begin_foreground_child { index: 0 }`
            // (`:949`) and `child_index: Some(0)` (`:832`) hands the child.
            steer_inbox_dir: Some(control::step_steer_inbox_dir(input.fg_control_dir, 0)),
            steer_ack_dir: Some(control::steer_acks_dir(input.fg_control_dir, 0)),
            steer_capability_path: Some(control::steer_capability_path(input.fg_control_dir, 0)),
```

⚠ `steer_acks_dir` is **plural** (`control.rs:1536`). There is no `steer_ack_dir` function; the
singular name belongs only to the `RunOptions` field (`agent_config.rs:599`) and the env var
(`prompt_runtime.rs:2344`). Correction #2 of §0.5.

**Three doc claims become false with this change and must be corrected in the same commit** — all
three are load-bearing documentation a reader will trust over the code:

1. [`agent_config.rs:583-586`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/exec/agent_config.rs)
   — *"`None` on the foreground path (no async run directory exists), matching upstream's own
   `if (input.steerInboxDir)` guard."* Becomes: `Some` on every path; the guard survives for
   embedders that supply no control tree.
2. [`prompt_runtime.rs:1409-1412`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/prompt_runtime.rs)
   — *"`Some` only when the parent handed this child a `STEER_INBOX_ENV` path — i.e. only for a
   background/async child, which is the only kind that has an async run directory to steer
   through."* The parenthetical is now simply wrong; the first clause stays.
3. [`spawn_plan.rs:1304-1305`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/exec/spawn_plan.rs)
   — *"Absent on the foreground path, so a foreground child is byte-identical to before."* and the
   matching *"a foreground child — which has no run directory and therefore neither path — is
   byte-identical to before"* at `:1319-1320`.

## §3 SUBTASK3 — the handle on `ForegroundChildEntry`

> ⚠ **2026-09-14 — §0.0.7.** Built as `ForegroundChildSteerHandle` (`foreground_control.rs:56-74`) with
> **three** fields (`inbox_dir`/`run_dir`/`index`) — no `ack_dir`, no `capability_path`.
> `ForegroundChildEntry::steer` ✅ `:164`, filled at `foreground.rs:1019` ✅, propagated by
> `begin_foreground_child` ✅. **`ForegroundControlEntry::steer` and the `sync_current_child`
> assignment are STILL MISSING** — and under the landed design nothing would read them, so close
> §0.2 by documenting the decision or land the mirror with its first reader. Fixture line numbers are
> now `:251-296`, plus a third fixture at `workflow_steering.rs:361-…`.

**File:** [`foreground_control.rs:43`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/foreground_control.rs),
next to `interrupt`.

pi's shape (`shared/types.ts:2127`) is
`steer?: (input: ForegroundSteerInput) => Promise<ForegroundSteerOutcome>` — a closure, because pi's
foreground child is an **in-process `ChildSession`** whose `steer`/`followUp` methods are direct
calls (`runs/shared/child-session.ts:324-325`, closed over at
`subagent-executor.ts:3859-3870`). cyrup's child is a **spawned process**, so its equivalent of that
in-process session handle IS the file protocol it was handed. Data, not a trait object: no `dyn`,
no new implementor, no second transport.

```rust
    /// pi `ForegroundChildControl.steer` (`shared/types.ts:2127`) — this child's live steer
    /// channel. `Some` once the parent gave the run a control tree (WORKFLOW_14 §1); `None` keeps
    /// upstream's own `if (!child.steer)` refusal reachable
    /// (`workflow-foreground-steering.ts:144`) for a child registered without one.
    ///
    /// The three paths ARE the handle. pi closes over a function because its child is an
    /// in-process `ChildSession` (`runs/shared/child-session.ts:324`); cyrup's child is a spawned
    /// `cyrup` process, so the channel is the file protocol it was handed at spawn — the same
    /// three paths `build_foreground_run_options` put on its `RunOptions` and `spawn_plan` put in
    /// its environment.
    pub(crate) steer: Option<ForegroundChildSteer>,
```

```rust
/// pi's `ForegroundSteerInput`/`ForegroundSteerOutcome` pair (`shared/types.ts:2130-2140`),
/// expressed as the addresses the existing protocol already uses.
///
/// Cloned into `ForegroundControlEntry` by `sync_current_child`, so three `PathBuf`s and a `usize`
/// rather than an `Arc`: the clone happens once per control event on a struct that is already
/// `Clone`, and an `Arc` here would buy indirection without removing a single allocation that
/// matters.
#[derive(Clone)]
pub(crate) struct ForegroundChildSteer {
    /// `control/steer-targets/<index>/` — where a request is DROPPED (§1.2); the exact directory
    /// the child was handed as `CYRUP_SUBAGENT_STEER_INBOX`.
    pub(crate) inbox_dir: PathBuf,
    /// `control/steer-acks/<index>/` — where the child ANSWERS (`prompt_runtime.rs:326`).
    pub(crate) ack_dir: PathBuf,
    /// `control/steer-capabilities/<index>.json` — whether the child can be steered AT ALL, and
    /// under which pid. This is what distinguishes "has not booted yet" from "cannot be steered",
    /// which is the whole of §4.3.
    pub(crate) capability_path: PathBuf,
    /// The flat child index — always `0` for a foreground SINGLE run (§1.1).
    pub(crate) index: usize,
    /// The run-directory-shaped root the three paths above were derived from
    /// (`foreground_run_control_dir`). Carried rather than re-derived because
    /// `take_steer_acks`/`read_steer_capability` take a `run_dir`, not a leaf
    /// (`control.rs:1706`, `:1671`).
    pub(crate) run_dir: PathBuf,
}
```

**Restore the two dropped assignments** (§0.2), in the functions that already exist:

```rust
// `sync_current_child` (:50) — pi `foreground-control.ts:60`, the line WORKFLOW_6 dropped.
// Unconditional, exactly as upstream's is: this function's contract is "the live child's state IS
// the run's state", and a conditional here would strand a stale handle after a child swap.
entry.steer = child.steer.clone();
```

```rust
// `begin_foreground_child` (:67) — pi `:128`, `if (input.steer) child.steer = input.steer`.
// Guarded exactly as upstream guards it, so a `None` never clears an existing handle. Since the
// child is MOVED in here, the guard is on the CALLER's side in cyrup: `register_foreground_controls`
// builds `ForegroundChildEntry { steer: Some(..), .. }` and the field simply travels with it.
```

`ForegroundControlEntry` ([`notices.rs:20`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/notices.rs))
gains the matching `steer: Option<ForegroundChildSteer>` so `sync_current_child` has somewhere to
sync it to — pi's `ForegroundRunControl.steer` (`shared/types.ts:2187`, declared as
`steer?: ForegroundChildControl["steer"]`). Both existing test fixtures that build the struct
literally — `foreground_control.rs`'s `base_entry()`/`child()` (`:132-171`) — need the new field.

`register_foreground_controls` (`foreground.rs:903`) fills it at `:949`, from the same
`fg_control_dir` §1.3 threaded in:

```rust
                    index: 0,
                    // WORKFLOW_14 §3 — the same three paths `build_foreground_run_options` put on
                    // this run's `RunOptions`, derived from the ONE `fg_control_dir` resolved in
                    // `resolve_run_channels`. Never re-derived here: two derivations of the same
                    // path is exactly the divergence `control_watcher.rs:762-769` names as the way
                    // this feature dies silently.
                    steer: Some(ForegroundChildSteer {
                        inbox_dir: control::step_steer_inbox_dir(fg_control_dir, 0),
                        ack_dir: control::steer_acks_dir(fg_control_dir, 0),
                        capability_path: control::steer_capability_path(fg_control_dir, 0),
                        index: 0,
                        run_dir: fg_control_dir.to_path_buf(),
                    }),
```

## §4 SUBTASK4 — real delivery, replacing WORKFLOW_7 §1.6's refusal

> ⚠ **2026-09-14 — §0.0.8/§0.0.9.** This file **exists** and the delivery arm **is real**
> (`workflow_steering.rs:253-337`). `await_steer_ack` is **already `pub(crate)`** and lives at
> `extension/executor/foreground_actions/steer.rs:265` — not `control.rs:690`; no visibility change is
> needed. **Two open items:** the no-ack word shipped as **`pending`**, which §4.2 and §6.8 forbid
> (rule on it); and **§4.3 is entirely unbuilt** — `CHILD_SESSION_NOT_RUNNING_YET` does not exist and
> `read_steer_capability` has zero production callers, so §6 invariant 7 is unmet.

**File:** `extension/executor/workflow_steering.rs` — **created by WORKFLOW_7**, which has not
landed. This task replaces exactly one line of it: the arm WORKFLOW_7 §1.6 leaves at
`Foreground run '{id}' child {i} does not support steering.`

Port [`steerWorkflowForegroundTarget`'s delivery half, `:146-168`](../../../workspace/pi-subagents/src/runs/foreground/workflow-foreground-steering.ts).

### 4.1 The shape

```rust
    // pi `:142-144` — unchanged from WORKFLOW_7 §1.6, and the `index` defaulting ABOVE this point
    // (`:135-141`) is already specified and landed by that task. Do not rewrite it.
    let Some(child) = target.control.active_children.get(&index) else {
        return Err(format!("Foreground run '{run_id}' child {index} is not live."));
    };
    // pi `:144` — `ForegroundChildControl.steer` is OPTIONAL upstream and stays optional here.
    // This is the branch WORKFLOW_7 shipped as the whole arm; it survives as the fallback.
    let Some(steer) = child.steer.clone() else {
        return Err(format!(
            "Foreground run '{run_id}' child {index} does not support steering."
        ));
    };

    // pi `:146-147`. cyrup mints the id inside `request_direct_steer` rather than ahead of it,
    // because the id is also the ORDERING tiebreak (`control.rs:1128-1138`) and a second minting
    // site is a second place for that to drift.
    let message = message.trim();
    let (_, request_id) = control::request_direct_steer(
        &steer.run_dir,
        steer.index,
        message,
        mode,
        Some("steer-action"),
    )
    .await
    .map_err(|e| e.to_string())?;

    // pi `:149`'s `await child.steer(...)` resolves the outcome synchronously because its child is
    // in-process. cyrup's child is a process, so the outcome arrives as a file: the SAME
    // `await_steer_ack` + 3s `STEER_ACK_TIMEOUT` (`text.rs:177`) `control_steer` already uses
    // (`control.rs:660`, `:690`). Narrowed to this request id, which is what keeps two concurrent
    // steers from consuming each other's answers (`take_steer_acks`'s own `[CYRUP-DELTA]`,
    // `control.rs:1692-1705`).
    let outcome = Self::await_steer_ack(&steer.run_dir, &request_id, Some(steer.index)).await;
```

⚠ `await_steer_ack` is a **private** inherent method on `SubagentExecutor`
([`control.rs:690`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/control.rs)).
`workflow_steering.rs` is a sibling module, so it must become `pub(crate)` (or
`pub(in crate::extension::executor)`). Change the visibility; do **not** copy the function.

### 4.2 The receipt mapping — exact, already

cyrup's [`SteerAckState`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/control.rs)
(`control.rs:1448-1458`) has precisely pi's three states:

| pi `outcome.state` (`:150-155`) | cyrup `SteerAckState` | receipt (`:163`/`:166`/`:168`) |
|---|---|---|
| `delivered` | `Delivered` | `Steering delivered for foreground run {id} (request {rid}).` |
| `queued` | `Queued` | `Steering queued for foreground run {id} (request {rid}).` |
| `failed` | `Failed` | `Steering failed for foreground run {id} (request {rid}): {reason}` + `isError` |
| — (no ack inside the budget) | `None` | the **queued** receipt, verbatim |

The no-ack fallback is upstream's stated contract at `workflow-foreground-steering.ts:112-114`:
*"No acknowledgment leaves an honest, unaddressed queued receipt."* `control_steer` already renders
that case as the word `pending` (`control.rs:664-667`) — that is the **async** surface's own text
and is correct there; this surface uses **`queued`**, because pi's foreground receipt has no
`pending` word (`:158`: `deliveryStatus: outcome.state === "delivered" ? "delivered" : "queued"`).
Do not unify the two and do not invent a fourth word.

### 4.3 `CHILD_SESSION_NOT_RUNNING_YET` — the state pi has and this port must reproduce

pi's foreground steer closure has a fourth branch the receipt table hides
([`subagent-executor.ts:3860-3861`](../../../workspace/pi-subagents/src/runs/foreground/subagent-executor.ts)):

```ts
steer: async (input) => {
    if (!childSessionControls) return { state: "failed", reason: CHILD_SESSION_NOT_RUNNING_YET };
```

`CHILD_SESSION_NOT_RUNNING_YET = "Child session is not running yet."` (`:4336`). It exists because
**the control registers before the child session does** — `beginForegroundChild` runs at `:3842`,
`onChildSession` fires at `:3899` — and `steerWorkflowChildByKey` **polls on exactly that reason**
(`:4501-4502`) rather than treating it as a failure.

cyrup's signal for the same fact is the **capability record**, which is precisely what it was built
for (`control.rs:1668-1670`: *"`None` when the child has not reached its runtime yet"*):

```rust
/// pi's `CHILD_SESSION_NOT_RUNNING_YET` (`subagent-executor.ts:4336`), resolved from the child's
/// published capability instead of an in-process session handle.
///
/// Three distinguishable facts, and collapsing any two of them is how this surface lies:
///   None                     → the child has not reached its runtime yet → pi's "not running yet"
///   Some(c) if !c.supported  → its host cannot inject messages at all    → a genuine `failed`
///   Some(c)                  → steerable
///
/// The `pid` on the record (`control.rs:1527`) makes a stale file from a dead process detectable;
/// this surface does not need it, but do not drop it from the read.
const CHILD_SESSION_NOT_RUNNING_YET: &str = "Child session is not running yet.";
```

Consume it **before** the request write, so the poll in §5 has something to poll on:

```rust
    match control::read_steer_capability(&steer.run_dir, steer.index).await {
        // pi `:3861` — the control exists, the child does not yet. NOT a refusal: §5 retries this
        // until the ack deadline, exactly as `steerWorkflowChildByKey` does at `:4502`.
        None => return Err(CHILD_SESSION_NOT_RUNNING_YET.to_string()),
        // pi's `catch` arm at `:3867-3869`, reached here at spawn time rather than call time
        // because cyrup's child publishes the answer instead of throwing it.
        Some(cap) if !cap.supported => {
            return Err(format!(
                "Steering failed for foreground run {run_id} (request -): child {index} cannot be steered."
            ));
        }
        Some(_) => {}
    }
```

⚠ The capability is republished on every `activate`, not only at `start`
(`prompt_runtime.rs:334-345`'s `publish_capability` doc: `set_host_services` is late-bound, so a
single publish at `session_start` would pin `supported: false` on every child). So a `None` read is
genuinely "not yet", never "never".

## §5 SUBTASK5 — `WorkflowRunHost::steer`, the second consumer

> ⚠ **2026-09-14 — §0.0.10.** `supports_steer()` is **already `true`** (`workflow.rs:827`) and `steer`
> is **already real** (`:835-936`). But it uses a **different mechanism**: it resolves the index from
> the host's own `launched` ledger (`:846-856`), never scanning `foreground_controls`. **There is NO
> poll loop** (§5.2 unbuilt, `cancel` is `_cancel`) and **`WorkflowSteerState::Missed` is produced
> nowhere in the crate** (§5.3 unbuilt, no `workflow_steer_receipt` helper). `WorkflowSteerResult`
> still has no `Default` (`types.rs:126`) — §5.2's ⚠ stands. Host impl is at `:540`, not `:228`.

**File:** [`workflow.rs:228`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/workflow.rs),
in `impl WorkflowScriptHost for WorkflowRunHost`, beside `launch` (`:229`) and `status` (`:388`).

Upstream is **`steerWorkflowChildByKey`** ([`subagent-executor.ts:4477-4541`](../../../workspace/pi-subagents/src/runs/foreground/subagent-executor.ts)),
wired as the host's `steer` at `:5710` and `:5960`. That is the port target — **not**
`resolveWorkflowForegroundSteeringTarget`.

### 5.1 ⚠ It is a DIFFERENT predicate (correction #3 of §0.5)

```ts
// :4491-4493
const control = [...state.foregroundControls.values()].find((candidate) =>
	candidate.parentWorkflowRunId === input.workflowRunId
	&& candidate.workflowKey === input.key
	&& (candidate.activeChildren?.size ?? 0) > 0);
```

versus WORKFLOW_7's `controlIsLiveInWorkflow` (`workflow-foreground-steering.ts:32-36`):

```ts
control.parentWorkflowRunId === workflowRunId && control.sessionId === sessionId && activeChildren.size > 0
```

The middle term differs: **`workflowKey` here, `sessionId` there.** Reusing WORKFLOW_7's predicate
would select *any* live child of the workflow regardless of lane — wrong child on a multi-lane
script, and `Missed` on none. cyrup's `ForegroundControlEntry` carries both fields
(`notices.rs:73`, `:79`), so write the three-term find upstream writes. The session term is
unnecessary because a `WorkflowRunHost` **is** this session's host: it was constructed for one tool
call, and `workflow_run_id` (`workflow.rs:118`) is the only workflow it can ever name.

### 5.2 ⚠ It must POLL, and that is what makes the method non-vacuous

`WorkflowRunHost::launch` **blocks until its child settles** (`workflow.rs:353` `.await`), so
`runs.steer(key, …)` is only ever reachable from a **concurrent lane** of the guest script — the
engine drives lanes on one event loop and `run_steer` hops to main via `shared.on_main`
(`engine.rs:1118-1124`). The steered lane may therefore be anywhere in its life, including
*registered but not yet spawned*. Upstream's loop (`:4490`, `:4539`) exists for exactly that, and
its comment at `:4501` says so: *"The control registers before its child session exists; keep
polling until the steer can route."*

```rust
    /// pi `options.steer` (`engine.rs:165`), implemented as `steerWorkflowChildByKey`
    /// (`subagent-executor.ts:4477`). The foreground host CAN steer now (WORKFLOW_14 §4);
    /// WORKFLOW_13 answers the same two methods on its own ASYNC host, separately.
    fn supports_steer(&self) -> bool {
        true
    }

    async fn steer(
        &self,
        key: &str,
        message: &str,
        options: WorkflowSteerOptions,
        cancel: CancelToken,
    ) -> Result<WorkflowSteerResult, String> {
        // pi `:4488-4489` — ONE budget for the whole loop, not per attempt.
        let deadline = std::time::Instant::now()
            + options
                .ack_timeout_ms
                .map_or(STEER_ACK_TIMEOUT, std::time::Duration::from_millis);
        loop {
            // pi `:4491-4493`, verbatim three terms. `.clone()`d out of the lock — every access to
            // `foreground_controls` is a short synchronous section with no `.await` inside it
            // (`mod.rs:145-147`).
            let found = { /* scan self.executor.foreground_controls for
                             (parent_workflow_run_id == self.workflow_run_id)
                             && (workflow_key == key)
                             && !active_children.is_empty(), returning (run_id, entry) */ };

            if let Some((run_id, entry)) = found {
                match self.steer_foreground_target(&run_id, &entry, message, &options).await {
                    // pi `:4502` — the ONLY retryable outcome. Anything else is the answer.
                    Err(reason) if reason == CHILD_SESSION_NOT_RUNNING_YET => {}
                    other => return Ok(workflow_steer_receipt(key, other)),
                }
            }

            // pi `:4536-4537`. cyrup has no async status file for a FOREGROUND child, so the
            // status-derived `missed` arms (`:4512`, `:4531`, `:4534`) collapse into this one —
            // the lane has no live steering route inside the budget, which is the same fact those
            // three report by three different routes.
            if cancel.is_cancelled() || std::time::Instant::now() >= deadline {
                return Ok(WorkflowSteerResult {
                    key: key.to_string(),
                    state: WorkflowSteerState::Missed,
                    error: Some(format!("Workflow child '{key}' had no live steering route.")),
                    ..Default::default()
                });
            }
            // pi `:4539` — `Math.min(10, …)`; the same 10 ms, not `STEER_ACK_POLL_INTERVAL`
            // (that one paces the ACK read inside `await_steer_ack`, a different loop).
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
```

⚠ `WorkflowSteerResult` (`scripted/types.rs:126-140`) has no `Default` derive today. Either add one
or construct every field explicitly — do not `unwrap`.

### 5.3 The FOUR-state receipt (correction #4 of §0.5)

Port `workflowSteerReceipt` (`subagent-executor.ts:4319-4334`). `WorkflowSteerState`
(`scripted/types.rs:99-108`) has four variants, and `Missed` is only reachable from §5.2's loop:

| condition | `WorkflowSteerState` | source |
|---|---|---|
| ack `Delivered` | `Delivered` | `:4325` |
| ack `Queued`, or no ack inside the budget | `Queued` | `:4325` |
| ack `Failed`, or the delivery arm errored | `Failed` | `:4323-4324` |
| no live route before the deadline / cancelled | **`Missed`** | `:4537` |

`targets` is a single-element `Vec<WorkflowSteerTarget>` (`types.rs:112-121`) carrying
`{ index, state, reason? }` — pi `:4331`. `request_id` and `delivery_status` come straight off the
ack (`:4329-4330`).

Without this subtask the feature is unreachable by a user: a workflow script's `runs.steer` returns
`"Workflow steering is unavailable in this host."` from `engine.rs:1106-1108`, gated on
`shared.host.supports_steer()` at `:1089`.

---

## §6 Behavioural contract

| # | invariant | source |
|---|---|---|
| 1 | A foreground run is handed a real `steer_inbox_dir`/`steer_ack_dir`/`steer_capability_path`, and its child receives all three env vars. | `spawn_plan.rs:1300-1340` |
| 2 | The control tree lives under the per-`cwd` run **scratch**, in a `fg-<run_id>/` leaf, never under the async root — `resume_tracking`'s `read_dir` must not see it as a status-less run. | `status.rs:26-32` |
| 3 | The `fg-<run_id>/` leaf — never its shared parent — is removed when the run settles; a removal failure never fails the run; the per-attempt stdout tee survives. | §1.4, `foreground.rs:365-380` |
| 4 | **The parent writes the request into `step_steer_inbox_dir`, not `steer_requests_dir`.** There is no runner on this path to perform the routing hop. | §0.4, `control_watcher.rs:334-400` |
| 5 | `sync_current_child` and `begin_foreground_child` propagate the handle, restoring `foreground-control.ts:60` and `:128`. | §0.2 |
| 6 | A child with no handle still refuses with upstream's `child {i} does not support steering.` | `:144` |
| 7 | A child whose capability is unpublished yields `CHILD_SESSION_NOT_RUNNING_YET` — **retryable**, not a refusal; `supported: false` is a genuine failure. | `:3861`, `:4502` |
| 8 | Delivery maps `Delivered`/`Queued`/`Failed` onto pi's three tool receipts, and a missing ack yields the honest **queued** receipt — never `pending`, never a fourth word. | `:112-114`, `:150-158` |
| 9 | `runs.steer(key, …)` selects by `(parent_workflow_run_id, workflow_key, active_children non-empty)` — **not** by session — and polls to the ack deadline before answering `Missed`. | `:4491-4493`, `:4536-4539` |
| 10 | A **plain** foreground run addressed by `action:"steer"` is still refused with `STEER_FOREGROUND_RUN_REFUSAL`. | `:3217`, `control.rs:574`, §0.6 |
| 11 | A steer addressed at a workflow in another session is still refused by WORKFLOW_7's gates — this task adds delivery, never a bypass. | WORKFLOW_7 §4 |

## §7 Definition of done

* `foreground_run_control_dir` exists in `background/artifact_roots.rs` and returns the run-root
  (not the `control/` leaf); `RunChannels`/`ForegroundRunOptionsInput` thread it through once.
* `request_direct_steer` exists in `background/control.rs`, writes into `step_steer_inbox_dir` with
  `target_index` pinned, and mints its id through the existing private `next_steer_request_id`.
* `build_foreground_run_options` populates all three steer paths using `step_steer_inbox_dir` /
  **`steer_acks_dir`** / `steer_capability_path`; the circular comment at `foreground.rs:865-867`
  is replaced with the real reason.
* The three stale doc claims (`agent_config.rs:583-586`, `prompt_runtime.rs:1409-1412`,
  `spawn_plan.rs:1304-1305` + `:1319-1320`) are corrected.
* `ForegroundChildSteer` exists; `ForegroundChildEntry::steer` / `ForegroundControlEntry::steer`
  exist, are filled by `register_foreground_controls`, and are propagated by `sync_current_child`.
  The two struct-literal test fixtures in `foreground_control.rs` compile.
* `await_steer_ack` is `pub(crate)` (not copied) and WORKFLOW_7's delivery arm performs a real
  steer returning one of pi's three receipts; an unpublished capability is retryable; a child with
  no handle still hits upstream's refusal.
* `WorkflowRunHost::supports_steer()` returns `true`; `steer` selects by
  `(parent_workflow_run_id, workflow_key, non-empty active_children)`, polls to the ack deadline,
  and maps onto all four `WorkflowSteerState` variants through a `workflow_steer_receipt` port.
* A plain foreground run is still refused; WORKFLOW_7's four gates are unchanged.
* The `fg-<run_id>/` tree is removed on settle, outside the `foreground_controls` lock guard.
* `cargo clippy --workspace --all-targets` exits 0 with no new `allow`s and no dead-code warnings.

## §8 Research notes & citations

> ⚠ **2026-09-14 — §0.0.1.** The two checkout paths below do not exist on this machine: the repo is
> `/home/user/cyrup` and there is **no pi checkout at all**, so every `pi …:NNN` citation here is
> unverified by this pass. The cyrup commit `f8bec9ee` is long superseded — **nearly every cyrup line
> number in this block is shifted.** Corrected tables: `background/control.rs` in §0.0.4,
> `extension/executor/foreground.rs` in §0.0.5, `foreground_control.rs`/`notices.rs` in §0.0.7,
> `control.rs`/`text.rs`/`foreground_actions/steer.rs` in §0.0.8, `workflow.rs`/`engine.rs`/`types.rs`
> in §0.0.10. The final line of this file (`/home/d0m17bw/.flux/…`) is a stale self-reference.

**Upstream** (`/home/d0m17bw/workspace/pi-subagents` @ `57278d82`)

* [`shared/types.ts:2127`](../../../workspace/pi-subagents/src/shared/types.ts) — `ForegroundChildControl.steer`; `:2130-2140` `ForegroundSteerInput`/`Outcome`; `:2187` `ForegroundRunControl.steer`; `:2353-2356` `ForegroundChildSessionControls`
* [`runs/foreground/foreground-control.ts:60`](../../../workspace/pi-subagents/src/runs/foreground/foreground-control.ts) and `:128` — the two assignments WORKFLOW_6 dropped; `:17` the `BeginForegroundChildInput.steer` field
* [`runs/foreground/workflow-foreground-steering.ts:32-36`](../../../workspace/pi-subagents/src/runs/foreground/workflow-foreground-steering.ts) — `controlIsLiveInWorkflow` (the **session** predicate); `:135-141` the `index` defaulting (WORKFLOW_7's, not this task's); `:142-144` the two refusals; `:146-168` the delivery + the three receipts; `:112-114` the no-ack contract
* [`runs/foreground/subagent-executor.ts:3842-3871`](../../../workspace/pi-subagents/src/runs/foreground/subagent-executor.ts) — `beginForegroundChild`'s real `steer` closure and `:3899` the `onChildSession` that arms it; `:4319-4334` `workflowSteerReceipt`; `:4336` `CHILD_SESSION_NOT_RUNNING_YET`; `:4477-4541` `steerWorkflowChildByKey` (the **key** predicate at `:4491-4493`, the poll at `:4501-4502`/`:4539`, the four `missed` arms at `:4512`/`:4531`/`:4534`/`:4537`); `:5710`/`:5960` the host wiring; `:3217` the plain-foreground refusal
* [`runs/shared/child-session.ts:324-325`](../../../workspace/pi-subagents/src/runs/shared/child-session.ts) — why pi's handle is a closure and cyrup's is a path set
* [`runs/foreground/async-steering-action.ts:45,52,71,149`](../../../workspace/pi-subagents/src/runs/foreground/async-steering-action.ts) — the `asyncDir` gating that proves WORKFLOW_8 is a different layer

**cyrup** (`/home/d0m17bw/workspace/cyrup` @ `f8bec9ee`)

* [`extension/executor/foreground.rs:865-872`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/foreground.rs) — the three `None`s and the circular comment; `:612-710` `resolve_run_channels`/`RunChannels`; `:96-140` `ForegroundRunOptionsInput`; `:903-1000` `register_foreground_controls` (`:832` `child_index: Some(0)`, `:949` `index: 0`); `:1003-1040` `settle_foreground_run`; `:365-380` why the scratch root is NOT deleted
* [`extension/executor/foreground_control.rs:20-44`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/foreground_control.rs) — `ForegroundChildEntry` (`interrupt` at `:43`); `:48-64` `sync_current_child` + its omission list; `:67-76` `begin_foreground_child`; `:132-171` the test fixtures that must gain the field
* [`extension/executor/notices.rs:20-97`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/notices.rs) — `ForegroundControlEntry` (`cwd` `:83-84`, `parent_workflow_run_id` `:73`, `workflow_key` `:79`, `active_children` `:95`)
* [`extension/executor/paths.rs:300-313`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/paths.rs) — `drive_foreground_run_sync` → `exec::run_sync`, the shared entry
* [`extension/executor/status.rs:26-32`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/status.rs) — `resume_tracking`'s `read_dir` over the async root: invariant 2's reason
* [`extension/executor/control.rs:529`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/control.rs) `control_steer`; `:574` the plain-foreground refusal; `:640-660` the async request + ack wait; `:690-710` `await_steer_ack` (**private — must become `pub(crate)`**)
* [`extension/executor/workflow.rs:228`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/workflow.rs) — the host impl with only `launch` (`:229`) and `status` (`:388`); `:118` `workflow_run_id`; `:353` the blocking launch that makes §5.2's poll necessary
* [`extension/executor/mod.rs:134`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/mod.rs) — `foreground_controls: HashMap<String, ForegroundControlEntry>` and `:145-147` the no-`.await`-under-lock rule
* [`exec/mod.rs:1101-1135`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/exec/mod.rs) — scratch + all three steer `create_dir_all`s before spawn
* [`exec/spawn_plan.rs:1300-1340`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/exec/spawn_plan.rs) — the env overlay; its two *"Absent on the foreground path"* comments are the lines this task falsifies
* [`exec/agent_config.rs:583-611`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/exec/agent_config.rs) — the three `RunOptions` fields and the stale `None on the foreground path` doc
* [`prompt_runtime.rs:2336-2349`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/prompt_runtime.rs) — the child-side read, with no foreground/background distinction; `:1409-1412` the stale doc; `:326` the ack write; `:334-345` `publish_capability`'s republish cadence; `:583` the inbox drain
* [`background/control.rs:618`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/control.rs) `control_inbox_dir`, `:1128-1138` the id-ordering contract, `:1165` `steer_requests_dir`, `:1174` `step_steer_inbox_dir`, `:1196` `write_steer_request_to_dir`, `:1217` `next_steer_request_id` (private), `:1258` `request_async_steer_with_mode`, `:1293` `enqueue_step_steer`, `:1354` `consume_steer_requests`, `:1376` `SteerDeliveryMode`, `:1430` `MAX_STEER_QUEUE_SIZE`, `:1448-1458` `SteerAckState`, `:1476` `SteerAck`, `:1506` `SteerCapability`, `:1530` `steer_capability_path`, **`:1536` `steer_acks_dir`**, `:1671` `read_steer_capability`, `:1706` `take_steer_acks`
* [`background/runner_main/control_watcher.rs:334-400`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/runner_main/control_watcher.rs) — `route_steer_requests`, the runner hop a foreground run does not have; `:762-769` the "silently dead again" invariant
* [`background/artifact_roots.rs:246`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/artifact_roots.rs) `cwd_key`, `:325-337` `attempt_scratch_dir`/`_in` (per-`cwd`, not per-run)
* [`extension/tool/text.rs:116`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/tool/text.rs) `STEER_FOREGROUND_RUN_REFUSAL`, `:177` `STEER_ACK_TIMEOUT` (3 s)
* [`workflows/scripted/engine.rs:122`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/workflows/scripted/engine.rs) the `WorkflowScriptHost` trait, `:160-173` the `supports_steer`/`steer` defaults, `:1062-1130` `run_steer` and the `:1089` capability gate
* [`workflows/scripted/types.rs:71-140`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/workflows/scripted/types.rs) — `WorkflowSteerMode`/`Options`/**`State` (four variants, `:99-108`)**/`Target`/`Result`
* [`tests/steer_delivery_integration.rs`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/tests/steer_delivery_integration.rs) — the existing end-to-end conventions for this protocol (`:150`, `:290`, `:376`, `:769-794`)

No third-party clone was required: upstream is checked out at
`/home/d0m17bw/workspace/pi-subagents`, so nothing was written to `./tmp`.

/home/d0m17bw/.flux/-home-d0m17bw-workspace-cyrup/todo/SCOPE_18.md

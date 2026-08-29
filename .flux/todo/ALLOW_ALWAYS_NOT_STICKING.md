---
stage: aug
status: done
updated: 2026-08-29 01:55
---

# PERM-034: "Allow Always" Does Not Stick

## Objective

**"Allow Always" does not stick — the same tool/command is re-prompted repeatedly within one
session.** Owner report, live use 2026-08-15, `critical`, class `port-bug`, size `M`. A user
who approves an operation permanently is asked again on the next identical call, which makes
the permission gate unusable.

The row offers two candidates. **Reading the code at HEAD kills one and narrows the other to a
single code path**, and turns up a third mechanism the row does not name — which is now the
leading hypothesis.

---

## Audit results — settled, do not re-investigate

### Every line citation in the row is stale

`extension.rs` no longer exists; it was split into `extension/`. The row's numbers all miss.
This is the same drift found on SEAM-112, filed the same day. Navigate by symbol:

| Row cites | Reality at HEAD |
| --- | --- |
| `extension.rs:1717` (write) | [`extension/prompt.rs:304`](../../crates/cyrup-permission-system/src/extension/prompt.rs) |
| `extension.rs:2023` (write) | [`extension/decide.rs:379`](../../crates/cyrup-permission-system/src/extension/decide.rs) |
| `extension.rs:1464`/`:1634`/`:2211` (read) | [`decide.rs:126`](../../crates/cyrup-permission-system/src/extension/decide.rs), `decide.rs:296`, [`agent_start.rs:198`](../../crates/cyrup-permission-system/src/extension/agent_start.rs) |
| `extension.rs:2617`/`:2705` (clear) | [`extension/native.rs:181`](../../crates/cyrup-permission-system/src/extension/native.rs), `native.rs:269` |
| `stores.rs:13` (session-only note) | correct |

### Candidate (a) — subject mismatch — is DEAD for an identical repeated call

The row suspects the recorded `subject` differs from the one derived at match time. **It
cannot**, because both sides call the same function on the same inputs.

- **Write:** both sites call `gate::get_pattern_approval_subject(check, input)` —
  `prompt.rs:304` and `decide.rs:379`.
- **Read:** `gate::apply_pattern_approval_state`
  ([`gate.rs:209`](../../crates/cyrup-permission-system/src/gate.rs)) — the single funnel for
  all three read sites — computes `let subject = get_pattern_approval_subject(&result, input)`
  and evaluates it against `[config_rule, session_rules]`.

Same function, same arguments. For bash it returns `result.command` verbatim (pi
`index.ts:817-839`), so two identical calls produce byte-identical subjects by construction.

The one residual non-determinism is narrow and is **not** the reported case: for a
path-bearing tool with no `cwd` in its input, the subject falls back to
`std::env::current_dir()`. `gate::inject_cwd` normally injects the session cwd first. That
matters only for path tools, not for the bash re-prompt in the report.

### Candidate (b) — over-eager clearing — is NARROWED to exactly one code path

`clear()` has exactly **two** triggers, and both match pi (`index.ts:2089,2123`):

- `HostEvent::SessionStart` → `native.rs:181`
- `HostEvent::SessionShutdown` → `native.rs:269`

`ResourcesDiscover` clears only `dedup`, not the approval store — correct.

And `SessionStart` **cannot fire twice for one session**:
[`lifecycle.rs:186`](../../crates/cyrup-session-svc/src/session/lifecycle.rs) latches it —
`if self.start_announced.swap(true, Ordering::SeqCst) { return; }`. A swap creates a new
session object, so one announcement each.

**There is exactly one bypass**, and the row does not mention it:
`ExtensionFacade::reload` ([`facade.rs:2142`](../../crates/cyrup-ext/src/facade.rs)) dispatches
`HostEvent::SessionStart { reason: "reload" }` **directly** through `dispatch_notify` at
`:2194`, skipping the per-session latch entirely.

Note what that path does either way: it tears down and rebuilds the whole extension set
(`bus.clear()`, `native/live/loaded` cleared, then `discover_and_load`). **So an extension
reload drops every "Allow Always" approval mid-session regardless of the clear** — the store
dies with the instance. Whether that is correct parity is a real question, but it requires a
reload to have happened.

### The mechanism the row does not name — now the leading hypothesis

**The approval store is a per-instance field, not shared state.**
[`extension/mod.rs:124`](../../crates/cyrup-permission-system/src/extension/mod.rs) declares
`session_approvals: Mutex<SessionApprovalStore>` — a plain `Mutex`, **not** an `Arc`, so it is
owned by one `PermissionSystemExtension` value and visible to nobody else. It is initialised
per construction at
[`construct.rs:197`](../../crates/cyrup-permission-system/src/extension/construct.rs).

There is more than one constructor: `construct.rs:37` `new` and `:61`
`new_forwarding_parent`, the latter built at
[`install.rs:168`](../../crates/cyrup-permission-system/src/extension/install.rs), with a
parent/child forwarding split described at `mod.rs:32` and `:157`.

**So if the approval is written on one instance and the next call is evaluated on another, the
rule is invisible and the user is re-prompted — with both halves correctly "wired", which is
exactly the paradox the row describes.** This explains the symptom without requiring either
of the row's candidates to be true.

### Ruled out on the way past

The dialog really can produce `Always`:
[`ask.rs:153`](../../crates/cyrup-permission-system/src/ask.rs) maps the selected option to
`PermissionDecisionState::Always` on exact-string equality with `APPROVE_ALWAYS_OPTION`
(`"Allow Always"`), which is the same constant it passed into `select`. Worth one line in the
log to confirm, but note a mismatch here would produce **denials**, not re-prompts, so it does
not fit the report.

---

## The work

### 1. Instrument, reproduce ONCE, read

Per `handoff/03-verification.md`: log at the write and the match, run once, read. Do not
characterise this by re-running it.

Log at exactly these four points, and **include the instance identity in every line** —
`std::ptr::from_ref(self).addr()` or an equivalent stable per-instance id — because instance
identity is the leading hypothesis and no other field distinguishes the stores:

- **write** — `prompt.rs:304` (and `decide.rs:379`): instance id, `check.tool_name`, the
  `subject`, and the rule count after the push.
- **match** — `gate.rs:209` `apply_pattern_approval_state`: the derived `subject`,
  `session_rules.len()`, and whether any rule matched.
- **read entry** — `decide.rs:126`: instance id, immediately before `get_rules()`.
- **clear** — `native.rs:181` and `:269`: instance id and the event `reason`.

Then: approve one bash command with **Allow Always**, trigger the identical command again,
stop.

### 2. Read the log against this decision tree

- **The write and the match log DIFFERENT instance ids** → the leading hypothesis is
  confirmed. The fix is to give the approval store one owner for the session: hold it as
  `Arc<Mutex<SessionApprovalStore>>` on the shared services and hand the same handle to every
  `PermissionSystemExtension` construction (`construct.rs:197`, both constructors), rather
  than constructing a fresh store per instance. Preserve the parent/child forwarding roles as
  they are — only the store's ownership changes.
- **Same instance, and a `clear` line appears between the two prompts** → candidate (b) via
  `ExtensionFacade::reload` (`facade.rs:2194`). Establish what triggered the reload and
  whether pi reloads on that event; the fix is at the trigger, not by removing the clear.
- **Same instance, no clear, and the two `subject` strings differ** → candidate (a) after all,
  in a form this reading did not predict; reconcile the derivation against pi
  `index.ts:599-610`.
- **Same instance, no clear, subjects identical, but no rule matched** → the fault is in
  `evaluate::evaluate`'s pattern matching, where a stored bash command is treated as a glob
  and fails to match itself. Look at commands containing `*`, `?` or `[`.

### 3. Fix at whichever branch the log names

Do not fix the symptom. Suppressing the second prompt without explaining where the rule went
would leave the gate silently inconsistent.

### 4. Repoint the stale citations

Update the SEAM-112-era line numbers in the `PERM-034` rows
([`10-cyrup-permission-system.md:189`](../../docs/gap-analysis/10-cyrup-permission-system.md),
[`00-residual-ledger.md:25`](../../docs/gap-analysis/00-residual-ledger.md)) to the symbols in
the drift table. A row whose stated purpose is "do not re-derive this" is worse than useless
with dead links.

---

## Out of scope

- **Persisting approvals across a restart.** The store is deliberately session-only and
  in-memory on both sides — upstream deleted `PermanentApprovalStore` in v0.8.0 (`a33ac2c`).
  Losing approvals on restart is CORRECT. Do not "fix" this by adding persistence.
- The `evaluate` ruleset ORDER (`[config, session]`, last-match-wins so session beats config)
  — verified correct at `gate.rs:209` against pi v0.8.0 `index.ts:557-579`.
- The dialog option set and its ordering (`ask.rs:99-102`), which matches pi
  `permission-dialog.ts:24-28`.

---

## Definition of done

1. One instrumented reproduction captured, with instance ids present on every logged line.
2. The decision-tree branch that fired is named, in one sentence grounded in that log rather
   than inferred from reading.
3. The fix is applied at that branch. If it is the instance-identity branch, the approval
   store has exactly one owner per session and every construction shares that handle.
4. A second live run confirms an "Allow Always" approval is not re-prompted for the identical
   call within the session.
5. Approvals still do NOT survive a restart.
6. Temporary instrumentation removed; `cargo check --workspace --all-targets` clean.
7. The `PERM-034` rows carry the corrected citations.

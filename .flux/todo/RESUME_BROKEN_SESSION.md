---
stage: qa
status: needs-rework
updated: 2026-08-29 02:26
---

# SEAM-112: /resume Produces A Broken Session — ledger citation cleanup

## QA verdict: 9/10 — the fix is correct and complete; four citations in the two ledger rows are wrong.

**The code is DONE and verified. Do not touch it.** For context only, so the remaining edits
read correctly:

- The root cause was a port divergence: pi guards the overflow-latch clear with
  `stopReason !== "error" && stopReason !== "length"` ([`agent-session.ts:678`](../../tmp/pi/packages/coding-agent/src/core/agent-session.ts))
  and the retry-counter reset with `!== "error"` alone (`:684`); the port fused the two into one
  early `return`, losing the `length` arm, so every `Length` message cleared the latch at
  `run.rs` immediately before `check_compaction` read it — making the one-shot brake at
  [`auto_compaction.rs:85-98`](../../crates/cyrup-session-svc/src/session/auto_compaction.rs)
  unreachable and the compact-and-retry cycle unbounded.
- The guard split is applied at
  [`crates/cyrup-session-svc/src/session/run.rs:285-309`](../../crates/cyrup-session-svc/src/session/run.rs).
- `cargo check --workspace --all-targets` is clean, zero warnings.

**QA verified and signed off:** the fix logic and its truth table; that all four latch access
sites in the workspace are accounted for (`run.rs:247`, `:291`, `auto_compaction.rs:85`, `:100`)
with no other consumer; that [`subscriber.rs:188`](../../crates/cyrup-session-svc/src/subscriber.rs)
dispatches `on_assistant_message_end` unconditionally for every assistant message, so a `Length`
message does reach it; that the brake arm is now reachable and terminates the run; that no
existing test scripts a `Length` assistant message; and that every in-source citation repointed
in `auto_compaction.rs` resolves exactly at HEAD.

---

## Outstanding — four wrong line numbers, both files

The two `SEAM-112` ledger rows were rewritten to correct drifted citations, and in doing so
introduced four of their own. Each points at a doc-comment line or a truncated range rather than
at the code it names. Fix exactly these four; change nothing else in either row.

### 1. [`docs/gap-analysis/08-cyrup-session-svc-and-modes.md:410`](../../docs/gap-analysis/08-cyrup-session-svc-and-modes.md) — the latch-clear line

The row reads `` `session/run.rs:278` set `overflow_recovery_attempted` to `false` immediately
BEFORE `session/auto_compaction.rs:85` read it. ``

`:278` was the pre-fix offset. At HEAD it is a line of the SEAM-112 doc comment. The guarded
clear now lives at **`run.rs:291`**.

Replace `` `session/run.rs:278` `` with `` `session/run.rs:291` (pre-fix `:278`) `` — the row is
written in past tense about the defect, so name both, and do not leave a bare offset that
resolves to a comment.

### 2. Same row — the fix range

The row reads `` **FIX (`session/run.rs:269-296`):** ``

The function is **`:269-309`** (doc comment `:269-284`, body `:285-309`). `:296` cuts off inside
the `attempt` block — i.e. it excludes the retry-counter reset that the very same sentence says
is preserved for `Length`.

Replace `` `session/run.rs:269-296` `` with `` `session/run.rs:285-309` `` (the function body; use
`:269-309` if the doc comment should be included).

### 3. Same row — the tool-result emit pair in struck candidate (1)

The row cites `` `cyrup-agent/src/agent/run/tools/exec.rs:249-250`, `:361-362` `` as the two
`MessageStart`/`MessageEnd` pairs.

`:249-250` is correct. The second is off by one: `:361` is
`let msg = AgentMessage::ToolResult(fin.message.clone());` and the pair is **`:362-363`**.

Replace `` `:361-362` `` with `` `:362-363` ``.

### 4. [`docs/gap-analysis/00-residual-ledger.md:24`](../../docs/gap-analysis/00-residual-ledger.md) — the handler line

The row reads `` `on_assistant_message_end` (`cyrup-session-svc/src/session/run.rs:275`) ``

At HEAD `:275` is a doc-comment line. The function signature is at **`:285`**.

Replace `` `cyrup-session-svc/src/session/run.rs:275` `` with
`` `cyrup-session-svc/src/session/run.rs:285` ``.

---

## Constraints

- **Do not modify any `.rs` file.** The code change is complete and verified; re-running
  `cargo check` is unnecessary for a markdown-only edit.
- Do not restructure, shorten, or re-word either ledger row beyond the four replacements above.
  Both rows are otherwise accurate — every other citation in them was checked against HEAD and
  resolves exactly.
- Do not re-open the struck candidates, and do not port `isRecoverableLength` (PARITY-GAPS
  VL-P10, drafted-and-reverted under `PROV-069`).

## Definition of done

1. The four replacements above are applied, and each new offset resolves at HEAD to the symbol
   the surrounding prose names.
2. Both table rows still parse as 5-cell markdown rows (6 pipes, no stray `|` in the text).
3. No `.rs` file is modified.

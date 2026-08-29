---
stage: qa
status: completed
updated: 2026-08-29 19:15
---

# TOOL-007 closure — DO NOT EXECUTE. The premise is false; nothing is owed.

> **Research verdict: this task should be closed without any edit.** It was filed on the belief
> that `TOOL-007` is *"still open at `medium`, `cyrup-original`"*. It is not. **`TOOL-007` closed on
> 2026-08-14 in sweep 1**, and no open item in area 04 covers this work. Executing the task as filed
> would append a closure marker to an already-closed row.
>
> The filing error was mine and it is the same one twice over: I read the item BODY (`:309`) and an
> older per-baseline table (`:97`) and never checked the authoritative status table at `:172`.

---

## 1. What the record actually says

### 1.1 `TOOL-007` is closed

[`docs/gap-analysis/04-cyrup-tools.md:177`](../../docs/gap-analysis/04-cyrup-tools.md), in the
current status table:

```
| ~~TOOL-007~~ | ~~medium~~ **CLOSED 2026-08-14** | cyrup-original | M | Protected-path write block
is on by default, has no pi analog, and `bash` bypasses it *(ships with TOOL-039)* —
**CLOSED 2026-08-14**: sweep 1 — `builder.rs:239` now sets `protect_paths: false` (the item cites
`:208` = true) and `isolation/mod.rs:11-17` is rewritten to match the wiring. All three cited facts
were stale. |
```

### 1.2 Which table is authoritative, and why the filing got it wrong

| line | what it is | says about TOOL-007 |
|---|---|---|
| `:172` | the **current** status table, preceded by five dated RECOUNT lines ending at `:162` (*"RECOUNTED 2026-08-15 … 34 rows: 30 fully closed, 4 open"*) | **CLOSED 2026-08-14** |
| `:89` | `## Status since the 1806375 / 9219dcd baseline` (`:87`) — a **historical per-baseline** table | `still-open` — true at that baseline, pinned like every other dated finding |
| `:309` | the item **body** | retains its original pre-closure finding text |

The body retaining pre-closure text is **correct**, not an omission. Compare closed items:
`TOOL-039` (`:209`) and `TOOL-006` (`:295`) are both closed in the table and carry **no** marker in
their `##` heading. Heading markers appear only for a different shape — `TOOL-M01` (`:786`,
*"FILED AND CLOSED"* in one pass) and `TOOL-042` (`:816`, *"REOPENED … PARTIALLY CLOSED"*).
`TOOL-007` matches the `TOOL-039`/`TOOL-006` shape exactly.

### 1.3 The `mod.rs:3-6` limb the filing asked to check — already closed, before this branch

The filing said to verify whether
[`crates/cyrup-tools/src/isolation/mod.rs:3-6`](../../crates/cyrup-tools/src/isolation/mod.rs) still
*"asserts the opposite of the wiring"*. Checked: it reads *"by default nothing here is in the call
path"*, which is **true** now that `protect_paths` defaults to `false`. The 2026-08-14 closure
already records this — *"`isolation/mod.rs:11-17` is rewritten to match the wiring"*. Nothing owed.

### 1.4 The full-vs-partial question the filing raised does not arise

It does not arise because there is nothing to close. And the precedent it pointed at settles the
adjacent question anyway: `TOOL-042` was **reopened** *"after 286 measured runs refuted the mechanism
its closure rested on"* (`:150`). The mechanism `TOOL-007`'s closure rested on is
`protect_paths: false` plus the `mod.rs` rewrite — **both still true at HEAD**. Nothing refutes the
closure, so nothing reopens.

The `bash`-bypass limb never blocked the closure and does not now: `ADR-0003` D6 accepts it, and the
closure note does not rest on it.

### 1.5 Nothing open covers this work

The four open rows in the current table are `TOOL-022`, `TOOL-015`, `TOOL-017` and `TOOL-042`. None
concerns protected paths, `ProtectedFs`, or `.env` handling.

### 1.6 The pi-extension finding has no home in this area

Area 04's scope line (`:3`) measures against **`pi/packages/coding-agent/src/core/tools/`**.
`examples/extensions/protected-paths.ts` is outside that reference, so the finding that pi ships an
opt-in protected-path extension is not an area-04 gap. It is already recorded where it belongs —
in the code, in `isolation/mod.rs`, `builder.rs` and `tests/isolation.rs`, corrected on this branch
in `a3c1abe` and its predecessor.

---

## 2. Why the code change needs no ledger entry

`a3c1abe` narrowed the matcher (component-equality → name-plus-dot, so the dotenv family is covered)
and rooted `ProtectedFs` at the session cwd. Both moved cyrup **closer** to pi, inside territory the
ledger already closed.

The gap analysis is a ledger of **gaps against pi**. A post-closure improvement to a closed item
opens no gap and narrows none that is tracked, so it gets no row. The change is documented where
this repo documents implementation: in the code, with citations to pi's source — which is where the
QA that reviewed `a3c1abe` verified it.

---

## 3. Definition of done

This task is complete when it is **archived unexecuted**, having confirmed all five facts below.
Confirm them by reading; change nothing.

1. `04-cyrup-tools.md:177` carries `**CLOSED 2026-08-14**` for `TOOL-007`.
2. The current status table is the one at `:172`, and its latest recount line (`:162`) reports
   `34 rows: 30 fully closed, 4 open`.
3. Those 4 open rows are `TOOL-022`, `TOOL-015`, `TOOL-017`, `TOOL-042` — none protected-path.
4. `isolation/mod.rs:3-6` reads *"by default nothing here is in the call path"*, which is true at
   `protect_paths: false`.
5. `TOOL-007`'s body heading (`:309`) carries no closure marker, matching `TOOL-039` (`:209`) and
   `TOOL-006` (`:295`).

## 4. Out of scope — do not do any of this

- **Do not append a closure marker to `TOOL-007`.** It is already closed; a second marker would
  misdate the record.
- **Do not reopen `TOOL-007`.** Its closure mechanism is intact (§1.4).
- **Do not edit `docs/gap-analysis/04-cyrup-tools.md` at all** — not the `:89` historical table, not
  `:315`, not `:1055`. All are pinned dated findings.
- **Do not edit `docs/adr/ADR-0003-bash-scope.md`.** `docs/adr/README.md:73` — an ADR is superseded,
  never amended — and ADR-0003's decisions (D5, D6, no CLI flag) are unchanged by this work.
- **Do not file the pi-extension finding as a new area-04 item.** Out of that area's stated
  reference (§1.6). If it is ever filed, it belongs to whichever area owns `cyrup-ext`, and that is
  a separate decision, not a rider on this task.
- **Do not touch any file under `crates/`.**

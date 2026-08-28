---
title: "CYRUP-DELTA capability gap at crates/cyrup-tools/src/isolation/mod.rs:12"
priority: MEDIUM
crate: cyrup-tools
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: aug
status: done
updated: 2026-08-28 02:09
---

# Capability gap: `crates/cyrup-tools/src/isolation/mod.rs:12`

Classified a **capability gap** — a caller can observe a difference — by the audit
that reviewed all 87 `CYRUP-DELTA` markers against pi at `e8682309`.

**This was never authorized as an accepted divergence.** The marker was written by an
agent. Nobody decided it was acceptable. It is filed here so it is a decision rather
than an artifact.

> **Direction note.** Unlike the other ten gaps in this batch, this one is in the **ADD**
> direction: cyrup has a guardrail the marker claims pi lacks. "Close it" would therefore
> mean **removing or weakening a security control**. That is a real cost, and it is *not*
> the presumptive answer here. Read the research below before ruling: the premise the
> marker and ADR-0003 both rest on turns out to be **factually wrong**.

---

## RESEARCH (augmentation pass, 2026-08-28) — reference tree `./tmp/pi` @ `e8682309`

### Finding 1 (load-bearing) — the marker's central claim is false: **pi has a protected-path concept, and it protects the same three paths**

The marker says "pi has no protected-path concept anywhere." The ADR the marker cites says
the same thing three times, and its own reversal clause says overturning D5 "would
additionally require pi to grow a protected-path concept." All of that is contradicted by
the pinned tree.

`tmp/pi/packages/coding-agent/examples/extensions/protected-paths.ts` @ `e8682309`, in full
(30 lines):

```ts
export default function (pi: ExtensionAPI) {
	const protectedPaths = [".env", ".git/", "node_modules/"];

	pi.on("tool_call", async (event, ctx) => {
		if (event.toolName !== "write" && event.toolName !== "edit") return undefined;

		const path = event.input.path as string;
		const isProtected = protectedPaths.some((p) => path.includes(p));

		if (isProtected) {
			if (ctx.hasUI) ctx.ui.notify(`Blocked write to protected path: ${path}`, "warning");
			return { block: true, reason: `Path "${path}" is protected` };
		}
		return undefined;
	});
}
```

It is not stray sample code. It is a **documented, catalogued** extension:
`tmp/pi/packages/coding-agent/docs/extensions.md:2944` —
`| protected-paths.ts | Block writes to specific paths | on("tool_call") |` — in the
"Events & Gates" section of the Examples Reference table, alongside `permission-gate.ts`.

And pi's extension loader **auto-discovers** it. `tmp/pi/packages/coding-agent/src/core/extensions/loader.ts:779-784`
scans project-local `<cwd>/.pi/extensions/` and global `<agentDir>/extensions/`;
`docs/extensions.md:113-119` documents the four discovery shapes
(`~/.pi/agent/extensions/*.ts`, `~/.pi/agent/extensions/*/index.ts`, `.pi/extensions/*.ts`,
`.pi/extensions/*/index.ts`). A pi user drops that file in a directory and gets the guard —
no flag, no rebuild, no code change to pi.

**So the true shape of this gap is not "cyrup added a guardrail pi lacks."** It is: *both
projects ship an opt-in protected-path guard, off by default, over the same three names
(`.env`, `.git/`, `node_modules/`), covering the same two tools, blind to `bash` in both
cases — and they differ in seam, enablement mechanism, and match precision.* That is a much
smaller divergence than the marker describes, and it removes the strongest argument for
"close it by deletion."

### Finding 2 — cyrup already ships a near-exact port of pi's example, and it is dead code slated for deletion

`crates/cyrup-tools/src/isolation/policy.rs:194-205`:

```rust
pub fn protected_path_rule(paths: ProtectedPaths) -> Rule {
    Rule::when(move |tool, input| {
        matches!(tool, "write" | "edit")
            && input.get("path").and_then(Value::as_str)
                .is_some_and(|p| paths.is_protected(Path::new(p)))
    })
    .deny("write to protected path denied")
}
```

Same seam as pi (the `tool_call` gate), same two tool names, same `path` argument, same
`{ block, reason }` contract (`PolicyDecision::Block`, `policy.rs:23-32`, documented at
`policy.rs:4-6` as mirroring pi's `{ block, reason } | { input } | undefined`).

`grep -rn 'protected_path_rule' --include=*.rs crates/` returns **three** hits, all
re-exports (`isolation/mod.rs:21`, `isolation/mod.rs:46`, `lib.rs:43`) plus its own
definition and unit test (`policy.rs:284-306`). **Zero production consumers.**
ADR-0003 D7 (`docs/adr/ADR-0003-bash-scope.md:246-252`) explicitly says these helpers
"are **not** to be wired … they remain where the backlog put them, in `PARITY-GAPS` §5's
deletion candidates."

That is the awkward part of the record: **cyrup's one artifact that matches pi's actual
shape is the one scheduled for deletion**, while the one with no pi analogue (the fs-seam
decorator) is the one that was kept. Whichever way David rules, this inversion should be
resolved deliberately.

And cyrup's gate is real, not aspirational: `crates/cyrup-it/tests/permission/gate_integration.rs:108`
(`gate_blocks_a_deny_rule_through_before_tool_call`) proves a deny rule blocks through the
registered `before_tool_call` hook (`NativeExtension::on_event(ToolCall)`).

### Finding 3 — what pi actually does when an agent writes a protected path

**Default pi (no extension loaded):** the write happens, no predicate anywhere.
`tmp/pi/packages/coding-agent/src/core/tools/write.ts` @ `e8682309`:
`createWriteToolDefinition` at `:187`, `resolveToCwd(path, cwd)` at `:208`, `ops.mkdir(dir)`
at `:221`, `ops.writeFile(absolutePath, content)` at `:225`. No path check.
`edit.ts:332` `async execute` → `ops.writeFile(absolutePath, finalContent)` at `:371`.
Same: no predicate.
*(Note: the marker and ADR-0003 both cite "`write.ts:195-225`". At `e8682309` the accurate
anchors are `:187` / `:208` / `:225`. Cite the symbol, not the stale range.)*

**pi with the example extension loaded:** the `tool_call` gate returns
`{ block: true, reason: 'Path ".env" is protected' }`; `docs/extensions.md:774` documents
that return shape; `docs/extensions.md:2904` — "`tool_call` errors block the tool
(fail-safe)". The model sees an error tool result. Additionally, when `ctx.hasUI`, the user
gets a `notify(..., "warning")` toast — **cyrup produces no such user-visible notification**
(see Finding 5).

### Finding 4 — what cyrup actually does, and the default

Default verified. `crates/cyrup-session-svc/src/builder.rs:250` — `protect_paths: false`,
with the ADR-0003 D5 rationale in the comment at `:247-249`. The marker's parenthetical is
correct. Wiring at `builder.rs:863-870`:

```rust
let mut fs = base.fs.clone();
if cfg.confine_to_cwd { fs = Arc::new(TraversalFs::new(fs, cwd.clone())); }
if cfg.protect_paths  { fs = Arc::new(ProtectedFs::with_defaults(fs)); }
let backend = Backend { fs, proc: base.proc.clone() };
```

`proc` is passed through undecorated — the `bash` bypass the marker names, confirmed in
source, not inferred.

`ProtectedFs` (`crates/cyrup-tools/src/isolation/protected.rs:70-152`) guards
`write_in_place` (`:126-129`) and `access` under `Access::ReadWrite` (`:131-136`), returning
`error::denied(...)`. Reads, `read_stream`, `metadata`, `read_dir`, `detect_image_mime` and
`walk` pass through.

**Decorator completeness is currently sound but structurally fragile.** `FsOps`
(`crates/cyrup-tools/src/ops/mod.rs:395-458`) has exactly one mutating method
(`write_in_place`, `:437`) plus `access` (`:439`); `ProtectedFs` names both, so there is no
hole today. The decorator's own docstring at `protected.rs:107-121` records the hazard: a
Rust decorator must name every trait method, and a dropped delegation is *silent* because
the trait default and a forwarded call return the same bytes. **Any future mutating method
on `FsOps` (`remove`, `rename`, `create_dir`) silently escapes the guard.** That is a
maintenance liability the disposition should account for; it is a finding, not a descope.

Enablement surface: `SessionConfig::protect_paths` is a Rust builder field only.
ADR-0003 D5 deliberately refuses a CLI flag or `settings.json` key. So in cyrup the guard
is reachable **only by an SDK embedder recompiling**; in pi it is reachable by **any end
user** dropping a `.ts` file into `.pi/extensions/`. That asymmetry is itself caller-visible
and runs opposite to the direction the marker claims.

### Finding 5 — the caller-visible differences that actually exist (verified, enumerated)

With cyrup's flag on vs. pi's example extension loaded:

| # | Dimension | pi (`protected-paths.ts`) | cyrup (`ProtectedFs`) | Who is stricter |
|---|---|---|---|---|
| 1 | Seam | `tool_call` gate, before `execute` | `FsOps` backend seam | **cyrup** — covers *any* tool holding the fs backend, including custom/embedder tools; pi's matches on `toolName` and misses a custom writer |
| 2 | Enablement | drop a file in `.pi/extensions/` (end user) | `SessionConfig::protect_paths` builder field (Rust embedder, recompile) | **pi** — reachable without a rebuild |
| 3 | `bash` coverage | none | none | tie (both blind) |
| 4 | User notification | `ctx.ui.notify(..., "warning")` when `hasUI` | none — `Err` only | **pi** |
| 5 | Error text | `Path "X" is protected` | `write to protected path denied: X` | cosmetic, but a transcript-visible string difference |
| 6 | Match semantics | `path.includes(p)` on the **raw argument** | path-**component** equality on the path handed to the backend | see below — **it goes both ways** |

**(6) is the sharp one, and it cuts against cyrup on the case that matters most.**
`ProtectedPaths::is_protected` (`protected.rs:55-63`) matches when any *component* equals
`.env` / `.git` / `node_modules`. pi's example matches any *substring*. Verified matrix:

| path | cyrup blocks | pi blocks |
|---|---|---|
| `/w/.env` | yes | yes |
| `/w/.env.local` | **no** | yes |
| `/w/.env.production` | **no** | yes |
| `/w/.environment` | no | yes (false positive) |
| `/w/config.env` | no | yes (false positive) |
| `/w/.git/config` | yes | yes |
| `/w/.git` (the dir itself) | yes | **no** (trailing slash in pi's list) |
| `/w/node_modules/a/i.js` | yes | yes |

cyrup is more precise (no `.environment` false positive — asserted at `protected.rs:172`)
but **misses `.env.local` / `.env.production` / `.env.development`**, which in practice are
where secrets live in most JS projects. An embedder who switches `protect_paths: true`
believing `.env*` is covered gets a guard that is narrower than they think. This is a
defect in the *current form* regardless of which disposition is chosen, and it is the
single strongest argument for **reshape** over both "keep as-is" and "delete".

### Finding 6 — what ADR-0003 D5 actually decided, and who decided it

`docs/adr/ADR-0003-bash-scope.md`. **Status line, `:3`: "accepted (decided by default under
the parity rule — overridable)."** Date `:4` — 2026-08-13. It decides OQ-1
(`docs/PARITY-PLAN.md:1417-1435`), bundling `TOOL-039` + `TOOL-007` as one shell-surface
decision, and unblocked 14 items in batch 9 plus batch 10 transitively.

**"Decided by default" is not a human ruling.** There is no signature, no reviewer, no
recorded approval in the ADR, in `docs/adr/README.md:16`, or in the OQ-1 text. The status
line says the parity rule (align to pi absent instruction) supplied the answer because
nobody was there to give one — which is the exact provenance class this backlog exists to
convert into a decision. Note also `docs/adr/ADR-0008-requirement-ids-and-sdk-surface.md:3`
carries the identical status line and its Decision A (`:214-217`) rules that `R-NN-NNN` /
`func-NN` / `spec/…` tokens "carry **no authority** … may not, on its own, justify a
divergence" — so **`R-12-006` cannot be cited as a reason to keep `ProtectedFs`**, and the
`spec/` tree those tokens point at does not exist in this workspace (`:37-49`).

D5 verbatim (`ADR-0003:194-201`): flip the default to `false`; **keep** the field and the
decorator as an "embedder-only, opt-in composition point — it is dead by default, so it
costs no behaviour"; and specifically **do not** promote it to a CLI flag or settings key,
"pi has neither, and adding user-visible configuration surface pi lacks is the divergence
this rule exists to prevent."

D6 (`:203-209`) stamped the field `[CYRUP-DELTA]`, ordered the docs corrected to match the
wiring, and forbade decorating `ProcOps` to close the `bash` bypass.

**Three of D5/D6's premises do not survive Finding 1:**
1. "pi has no protected-path concept at all" (`builder.rs:182-184`, `ADR-0003:195-196`,
   `isolation/mod.rs:14-15`, `tests/isolation.rs:159-161`) — **false**; the concept exists
   at `examples/extensions/protected-paths.ts`, catalogued at `docs/extensions.md:2944`.
2. "adding user-visible configuration surface pi lacks is the divergence this rule exists to
   prevent" (`ADR-0003:198-200`, `:321-324`) — pi *does* expose a user-reachable enablement
   path (auto-discovered extensions). The parity-preserving move may be *more* user-facing
   surface, not less.
3. `ADR-0003:334-335` — reversal "would additionally require pi to grow a protected-path
   concept, or an explicit acceptance that cyrup refuses writes pi permits." pi already
   grew it. That clause is spent.

Rejected alternatives ADR-0003 already priced, which stay priced and should not be
re-litigated from scratch: decorating `ProcOps` (`:312-320` — undecidable from command text,
`sh -c 'e''cho x > .env'`); promoting to a CLI flag / settings key (`:321-324`); deleting
`ProtectedFs` outright (`:326-330` — "discards a composable, correctly-written `FsOps`
decorator that an SDK embedder can legitimately want, for no parity gain", deferred to the
`PARITY-GAPS` §5 sweep).

### Finding 7 — the audit trail on this item is itself unreliable

`docs/gap-analysis/04-cyrup-tools.md` carries `TOOL-007` twice with contradictory state:
`:97` says "still-open … `protect_paths: true` still hardcoded at `builder.rs:208`", while
`:177` says "**CLOSED 2026-08-14** … `builder.rs:239` now sets `protect_paths: false` …
**All three cited facts were stale.**" The live default is at `builder.rs:250` today —
so even the closure note's line number has since drifted. Anchor by symbol
(`impl Default for SessionConfig`), never by line, in whatever is written next.

---

## RECORD (what pi does / what cyrup does — corrected)

### What pi does
Default: no protected-path predicate — `write.ts:187/208/225` and `edit.ts:332/371`
(@ `e8682309`) write whatever absolute path they are handed.
**With the shipped, documented `protected-paths.ts` extension auto-loaded from
`.pi/extensions/` or `~/.pi/agent/extensions/`: pi blocks `write`/`edit` to `.env`, `.git/`,
`node_modules/` at the `tool_call` gate, with a UI warning.** `bash` is not covered.

### What cyrup does
Default: `protect_paths: false` (`crates/cyrup-session-svc/src/builder.rs:250`) — identical
to default pi. With an embedder setting it `true`, `ProtectedFs` blocks `write_in_place` and
writable `access` at the fs backend seam for path components `.env`/`.git`/`node_modules`;
`bash` is not covered (`proc` undecorated, `builder.rs:870`). cyrup additionally ships an
unwired gate-level equivalent of pi's extension (`policy.rs:194-205`, zero consumers).

### What a caller sees
Not "cyrup errors where pi writes." **Both are off by default and behave identically out of
the box.** With each project's guard enabled, the differences are the six rows in Finding 5
— chiefly: cyrup's is reachable only by recompiling an embedder, produces no UI warning, and
**misses `.env.local` / `.env.production`**, while covering custom tools that pi's
`toolName` check would miss.

---

## Decision required — David's call

The three dispositions, with consequences priced. **Nothing below is settled; the
augmentation pass has no authority to choose, and "keep as-is" in particular is an
acceptance that only David can grant.**

### Option A — Align to pi by *deleting* `ProtectedFs`/`ProtectedPaths`

> **This removes a security control.** State that plainly before choosing it.

What it costs: an SDK embedder who wants defense-in-depth at the backend seam loses the only
mechanism that covers *custom* tools (pi's `toolName` check cannot). ADR-0003 `:326-330`
already priced this as "no parity gain" — but that pricing assumed pi had no concept at all;
under Finding 1 the honest framing is "delete cyrup's stricter seam and keep only the gate
equivalent," which is a different trade.
What it buys: one fewer `CYRUP-DELTA`; the fs seam stops being a place a future mutating
method can silently escape (Finding 4).
Coherence requirement: if A is chosen, `policy.rs::protected_path_rule` must be **kept and
un-deleted** — it is the pi-shaped equivalent, and removing both leaves cyrup strictly
*less* capable than pi. That directly contradicts ADR-0003 D7 / `PARITY-GAPS` §5, which
must then be amended.

### Option B — Explicitly accept, with the record corrected

Keep `ProtectedFs` as the embedder-only opt-in. **The acceptance is now much easier to
justify than the marker implies**, because pi has the same feature: cyrup is not inventing
a restriction, it is implementing a shared one at a different seam.
Required work if B: correct the four sites that assert "pi has no protected-path concept"
(`crates/cyrup-session-svc/src/builder.rs:182-184`,
`crates/cyrup-tools/src/isolation/mod.rs:14-15`,
`crates/cyrup-tools/src/tests/isolation.rs:160-161`,
`docs/adr/ADR-0003-bash-scope.md:195-196` and `:334-335`) to cite
`examples/extensions/protected-paths.ts` instead; annotate the `[CYRUP-DELTA]` marker as
**authorized by David on <date>, reason: <reason>**; and decide the Finding 5 row 6 defect
separately (accepting the divergence does not make `.env.local` coverage correct).

### Option C — Reshape (the augmentation's recommendation to *consider*, not a ruling)

The divergence is defensible but the current form has three concrete problems, each
verified above: (i) `.env.local`/`.env.production` are unguarded (Finding 5 row 6);
(ii) the enablement path is narrower than pi's (recompile vs. drop-in file);
(iii) the pi-shaped artifact (`protected_path_rule`) is dead and slated for deletion while
the non-pi-shaped one is kept (Finding 2). Sub-shapes worth pricing:

- **C1 — fix the match set.** Extend `ProtectedPaths` to treat a leading-`.env` component as
  protected (`.env`, `.env.*`), keeping component-equality for `.git`/`node_modules` so the
  `.environment` false positive stays excluded. Smallest change; closes a real hole; does not
  touch parity posture.
- **C2 — move the guard to the gate, matching pi's seam.** Wire `protected_path_rule` behind
  the same `SessionConfig` field and drop `ProtectedFs` from the builder chain. Makes cyrup
  structurally identical to pi's example. Cost: loses custom-tool coverage; contradicts
  ADR-0003 D7 (which must be amended either way under Option A too).
- **C3 — warn rather than error.** Emit a warning into the tool result / UI and let the write
  proceed. **Assess with care**: it converts a control into advice, and a model that ignores
  a warning still writes `.env`. pi's example *both* warns and blocks; C3 would be stricter
  than default pi and weaker than guarded pi, i.e. a third behaviour neither project has.
  Prescribed here for completeness because the brief asked for it, not because the research
  favours it.
- **C4 — make it end-user reachable.** Expose enablement the way pi does (an extension the
  user loads) rather than a Rust field. ADR-0003 `:321-324` rejected a CLI flag/settings key
  on the premise pi lacks any user-facing path; Finding 1 voids that premise, so this option
  should be re-priced rather than inherited as rejected.

### Open questions this pass surfaced (for David, not for an agent to settle)

1. **Is `ADR-0003` D5/D6 still standing given that its factual premise is void?** It was
   "decided by default," and three of its stated reasons are now known false. Does it need
   re-deciding, or does the outcome survive on other grounds?
2. **Which artifact is cyrup's protected-path implementation** — the fs decorator or the gate
   rule? Both exist; one is slated for deletion; nobody chose.
3. **Is `PARITY-GAPS` §5's deletion of `protected_path_rule`/`dangerous_bash_rule` still
   right**, now that `protected_path_rule` is the closest thing cyrup has to a pi feature?
4. **Does the ADR-0003 D6 prohibition on decorating `ProcOps` still hold** as the reason the
   default is `false`? Its logic (undecidable from command text) is sound and independent of
   Finding 1 — but the marker uses the `bash` bypass as evidence the guard is "partial,"
   while pi's guard is equally partial. Should partiality still count against cyrup?
5. **Does `FsOps` need a mutation-audit guard** so a future `remove`/`rename` cannot silently
   escape `ProtectedFs` (Finding 4)? Independent of the disposition, if the decorator lives.

Do not silently keep option B by leaving the marker as-is; that is how this became a
backlog in the first place.

---

## Guards required, per disposition

An existing guard already pins today's behaviour and must be updated, not bypassed, by
whichever option is chosen:
`crates/cyrup-tools/src/tests/isolation.rs::protected_fs_is_fs_only_and_bash_is_never_covered`
(`:169-230`) asserts (a) undecorated `write` to `.env` succeeds, (b) with `ProtectedFs`
`write` is refused, (c) `bash 'printf ... >> .env'` reaches the file anyway. Its doc comment
(`:160-161`) contains the false "pi has no protected-path concept at all" claim and must be
corrected under every option.

- **Option A (delete).** A guard that fails before the change: a repo-level assertion that
  `ProtectedFs`/`ProtectedPaths` appear nowhere under `crates/`, plus a behavioural test that
  a `SessionConfig` with the (removed) guard requested still writes `.env` — i.e. delete
  `protected_fs_is_fs_only_and_bash_is_never_covered` case (b) and replace it with a
  parity test asserting cyrup's `write`/`edit` match `write.ts:225` unconditionally. Keep a
  test proving `protected_path_rule` still blocks through `before_tool_call`
  (mirror `crates/cyrup-it/tests/permission/gate_integration.rs:108`), since A only makes
  sense with the gate rule retained.
- **Option B (accept).** The guard is the *authorization record*, not new behaviour: a doc
  test / repo lint asserting the `[CYRUP-DELTA]` marker at
  `crates/cyrup-tools/src/isolation/mod.rs` carries an `AUTHORIZED-BY: David <date>` token,
  so an unauthorized re-introduction of the same pattern elsewhere fails CI. Plus keep
  `protected_fs_is_fs_only_and_bash_is_never_covered` with its docstring corrected to cite
  `examples/extensions/protected-paths.ts` rather than "pi has no concept".
- **Option C1 (match set).** A table test over `ProtectedPaths::is_protected` covering
  exactly the Finding 5 matrix: `.env` ✓, `.env.local` ✓ (fails today), `.env.production` ✓
  (fails today), `.environment` ✗ (must stay excluded — this is the regression risk),
  `config.env` ✗, `.git` ✓, `.git/config` ✓, `node_modules/a/i.js` ✓. Two rows fail before
  the change; one row is the guard against over-correcting into pi's substring bug.
- **Option C2 (move to gate).** An end-to-end test through `before_tool_call` (shape:
  `crates/cyrup-it/tests/permission/gate_integration.rs:108`) asserting `write` to `.env` is
  blocked with the gate rule wired and the fs decorator absent — fails today because
  `protected_path_rule` has zero production consumers. Plus a negative test that a **custom**
  tool writing `.env` is *not* blocked, making the seam change's cost executable
  documentation rather than a silent regression.
- **Option C3 (warn not error).** A test asserting the write **succeeds** and the tool result
  carries the warning text, plus an explicit assertion that `isError` is false — so the
  weakening is visible in the test name and cannot be mistaken for a block.
- **Option C4 (user-reachable enablement).** A test loading the guard through cyrup's
  extension path (not a `SessionConfig` field) and asserting the block, mirroring pi's
  auto-discovery contract (`loader.ts:779-784`).

## Definition of done

1. The gap is closed, or the marker records an explicit authorized acceptance **naming
   David and the reason** (not "decided by default").
2. If closed, a test fails without the change (see the per-option guards above).
3. The four sites asserting "pi has no protected-path concept" are corrected regardless of
   disposition — they are factually wrong at `e8682309` and are load-bearing for ADR-0003 D5.
4. No behaviour regression in the owning crate; `protected_fs_is_fs_only_and_bash_is_never_covered`
   updated rather than deleted unless Option A is chosen.
5. `docs/gap-analysis/04-cyrup-tools.md`'s duplicated `TOOL-007` rows (`:97` still-open vs
   `:177` closed) reconciled, since this item's history is cited from both.

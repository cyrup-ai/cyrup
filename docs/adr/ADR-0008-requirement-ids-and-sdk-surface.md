# ADR-0008 — Requirement ids are an index, not an authority; and the SDK is held to parity by capability, not by export list

**Status** accepted (decided by default under the parity rule — overridable)
**Date** 2026-08-13
**Decides** OQ-6 (`docs/PARITY-PLAN.md:1453-1466`), in both its halves — the `spec/` question and the
SDK-surface question it explicitly also carries
**Blocks released** Batch 2 (the "write ADR-0001 into this workspace or delete every reference to it"
line, `PARITY-PLAN.md:248-249`); batch 3's citation lint; batch 7's final shape for `PERM-009`;
batch 19's pre-flight member-list re-check (`PARITY-PLAN.md:943-944`); the disposition of
`DRIFT-047`/`VL-P5` and `PROV-031`; and the **OQ-6 membership** of `SESS-038` and `SEAM-058` — whose
own dispositions belong to ADR-0004 and to the reachability rule respectively (`AGENT-028` belongs to
OQ-2, not here)

---

## Context

### The measurement, at cyrup HEAD `72cd292`

Everything below is `rg` over `crates/` at HEAD, not restated from any document.

| scheme | occurrences | distinct | notes |
|---|---:|---:|---|
| `R-NN-NNN` | **1055** | 288 | across 235 files; 13 area prefixes `R-00`…`R-12`, heaviest `R-08` (229) |
| `arch-NN` / `func-NN` | **820** | 26 | `arch-00`…`arch-12` + `arch-08b`; `func-00`…`func-12` less `func-05` |
| `spec/…` paths | **220** | — | 201 are `spec/tui/01`…`spec/tui/07` §-refs; 14 name a `.md` file |
| `R-ARCH-<AREA>-NNN` | **74** | — | a *second*, differently-shaped scheme (`R-ARCH-TUI-003`, `R-ARCH-EXT-011`) |
| `ADR-NNNN` | **26** | 2 | `ADR-0001` ×20 (all `cyrup-tui`), `ADR-0002` ×6 (WASM serde boundary) |
| **total** | **≈2 195** | | in **380** of 815 `.rs` files, plus `Cargo.toml:1` and both `world.wit` copies |

The question as posed says "995 `R-NN-NNN`, 23 `ADR-NNNN`, 10 `spec/`". The true figures at HEAD are
larger and the vocabulary is **five schemes, not three**. That matters: any rule written for
`R-NN-NNN` alone leaves 1 140 citations un-ruled.

### Do the referenced documents exist anywhere? No — and the mechanism of loss is in this repo

1. **Not in this workspace.** No `spec/` at `crates/`, at the repo root, at
   `/Users/davidmaple/cyrup.ai/spec`, or at `/Users/davidmaple/spec`.
2. **Not in any commit, on any branch.** `git log --all --name-only --pretty=format:` over all 307
   commits yields 55 distinct non-`crates/` paths; **none** is under `spec/`. There is no deletion to
   recover — `--diff-filter=D` finds nothing because the tree was never tracked here.
3. **It was real, on a machine that is gone.** `.workflows/check-citations.py:24` hardcodes
   `WS = pathlib.Path("/home/d0m17bw/workspace")`. Commit `a9000b1`'s author line is
   `d0m17bw@test-wibey-cli-vm2.us-central1-c.c.dxsensei.internal`. Its message says the analysis
   documents "lived at the workspace root (`/home/d0m17bw/workspace/gap-analysis/`), **which is not a
   git repo** — so ~916 KB of verified parity findings were untracked and would have been lost with
   the machine." `spec/` was a sibling in that same untracked root and was **not** rescued. Root
   `README.md:64-65` states the design intent that produced this: "The authoritative design lives in a
   **separate `spec/` tree that is not vendored into this repository**; the code refers to it as
   `../spec`."
4. **Even the one rescued subtree does not resolve its own citations.** `a9000b1` rewrote exactly four
   `///` comments and left eight in-source `spec/gap-analysis/…` sites pointing at the *old*
   numbering: `crates/cyrup-agent/tests/model_boundary.rs:7` and `untracked_misses.rs:1` cite
   `03-cyrup-agent.md` (the rescued file is `docs/gap-analysis/02-cyrup-agent.md`);
   `crates/cyrup-tui/src/lib.rs:31` and `tests/tree_and_chrome.rs:3` cite `12-cyrup-tui.md` (it is
   `07-cyrup-tui.md`); `tests/assembled_render.rs:2` cites `12-cyrup-tui-audit-summary.md` and
   `cyrup-test-support/src/lib.rs:347,681` cite `spec/gap-analysis/fixtures-capture/*.ts` — neither of
   which was rescued at all.

**Finding: the documents do not exist and are not recoverable from anything reachable here.** This is
not "I looked and could not find it"; the repo's own commit message documents an untracked workspace
root on a disposable VM, and only one of its subtrees was copied out before it went away.

### But the vocabulary still works as an index

`README.md:70` claims the ids remain "a usable search index" without the tree. Tested against the
single most load-bearing id: `rg R-08-010 crates/` returns 12 sites in 6 crates —
`cyrup-ext/src/hooks.rs:3,31`, `event.rs:152`, `contract.rs:31`,
`cyrup-permission-system/src/lib.rs:4`, `extension.rs:7`, `cyrup-ext-sdk/src/api.rs:92`,
`example.rs:82`, plus four tests — and **every one means the same thing**: the `before_tool_call`
block seam. The claim holds. The index is real even though the requirement text is gone.

### How much of it is actually normative?

Of the 1055 `R-NN-NNN` occurrences, **17 lines** pair a requirement id with divergence language
(`dropped` / `not ported` / `intentionally` / `mandate` / `no analog` / `omitted`), and they cluster on
three ids: `R-09-021` (the dropped npm channel — `cyrup-resources/src/package/source.rs:71,81`,
`manifest.rs`, `cyrup/src/update_check.rs:37`, `subcommands.rs:299`, `tests/resources.rs`),
`R-06-003` (`cyrup-session/src/prompt/builder.rs`, "mandates it"), `R-02-027`
(`cyrup-agent/src/event.rs`). Add `ADR-0001` ×20 and `ADR-0002` ×6 and the normative set is **~45
lines out of ~2 195 citations — about 2 %.** The other 98 % are provenance annotations of the form
"this code realises requirement X", which are exactly what the index is made of.

Two sites in the tree have already reached this conclusion unprompted and written it down:
`cyrup-tui/src/startup.rs:20-22` ("Earlier revisions headed this section 'Deliberate divergences
(ADR-0001)'. No ADR document exists in this workspace, so that citation asserted an authority nothing
here can verify, and it read as permission to stop. It was not."), and `cyrup-tui/src/app.rs:1281`
("**Divergence from pi — UNPORTED (the `ADR-0001` it once cited does not exist…)**").

### Citations also drift silently, because nothing can check them

`cyrup-session-svc/src/builder.rs:147` cites `custom_tools` to "sdk.ts:71,384". At v0.83.0
`customTools?: ToolDefinition[];` is at **:73** and its consumption at **:383**
(`git -C pi show v0.83.0:packages/coding-agent/src/core/sdk.ts | grep -n customTools`). Both legs are
off. `.workflows/check-citations.py`'s own docstring names the deeper version of this: "a BARE
`index.ts:1447` is correct when the citing code ports v0.7.1 and wrong when it ports v0.8.0."

And the disease has reached the argument against itself. Seven places — `PARITY-PLAN.md:479`,
`05-cyrup-config-and-resources.md:284`, `07-cyrup-tui.md:722`, `:754`, `:1098`,
`10-cyrup-permission-system.md:132` and `:475` — cite "**README:208-212**" as the authority for "an
unverifiable in-source claim is not a decision of record". `docs/gap-analysis/README.md:208-212` is
about **censusing upstream baselines**; it says nothing of the kind. The one anchor everyone reaches
for to reject unverifiable citations is itself an unverifiable citation that has drifted.

> **Canonical anchors, re-resolved by reading `README.md` at HEAD, and agreed with ADR-0001** (which
> found the same drift independently). There are **three** rules, not two, and each has exactly one
> address: **`:130-135`** — severity is never held down by an unverifiable justification;
> **`:268-273`** — a code comment invoking an `R-NN-NNN` id or an ADR to justify a divergence is an
> unverifiable claim, not a decision of record; **`:274-276`** — there is no "accepted divergence"
> category. Every batch quoting any of the three re-anchors to these. An earlier draft of this ADR
> cited `:133-135` / `:271-277`; `:271-277` straddles two bullets and is superseded by the pair above.

### `PERM-009` — the premise is false: there is no citation to produce

OQ-6 asks whether `spec/` "mandates `PERM-009`'s bash bypass", and the assignment describes the item
as held down "by an `R-NN-NNN` mandate". Read at HEAD, that is not what is there.
`crates/cyrup-permission-system/src/extension.rs` contains exactly **two** citation tokens in 2 200+
lines: `R-08-010` at `:7` (module doc, the `tool_call` seam) and the bare word "mandate" at `:1631`.
The bash branch at **`:1651-1653`** is justified by a prose phrase — "(the gate re-checks each — the
mandate-directed analog of the read/skills bypass)" — that names **no requirement id, no ADR and no
`spec/` path**. It is not an unverifiable citation; it is an unsourced assertion. There is nothing to
produce.

The two-sided read settles it without reference to any spec:

- **upstream, ported baseline** — `pi-permission-system` `src/index.ts:2049-2075` **@v0.7.1**:
  `shouldExposeTool` has one post-deny branch — `if (toolName === "read" && hasAllowedSkills(...))
  return true;` at `:2070-2072` — then `return false` at `:2074`. No bash branch.
- **upstream, latest** — same file `:1791-1816` **@v0.8.0**: byte-for-byte the same single branch
  (only `permanentApprovals` is dropped from the `applyPatternApprovalState` call). No bash branch.
- **cyrup** — `extension.rs:1645-1655`: the `read`/skills branch at `:1648-1650`, then an **extra**
  `if tool_name == "bash" && mgr.get_bash_permissions(agent_name).any_allow() { return true; }` at
  `:1651-1653`.

And the parenthetical that made it look benign is **false on the code's own text**. pi's read bypass
*is* paired with a re-gate, and pi says so inline at `index.ts:2067-2069` @v0.7.1: "expose read anyway
so the agent can read skill files. **The tool_call handler will restrict reads to skill paths only.**"
cyrup's bash branch has no such counterpart, and
`manager.rs:205-215` resolves a bash *command* rule **above** the tool-level state ("command rules
OUTRANK the tool-level `bash` fallback"), so under `tools.bash: deny` + `bash: {"git status": allow}`
the tool stays exposed, `find_compiled_match` returns the command-level `allow`, and `git status`
**executes**. The one sentence holding the item down asserted the exact safety property the code does
not have.

### The SDK half — the four items are not one class

`PARITY-GAPS.md:831` groups `VL-P5`/`DRIFT-047`, `SESS-038`, `SEAM-058` and `PROV-031` as "all
embedder-facing with no user-visible symptom in the cyrup binary". Read individually, they are not:

- **`VL-P5`/`DRIFT-047` is not embedder-only.** `PARITY-GAPS.md:788` records that `packages/telemetry`
  entered the port's **runtime** dependency closure at v0.84.1, and `DRIFT-050`
  (`12-upstream-drift-pi-core.md:11`) is a user-facing behaviour: `CYRUP_TELEMETRY=` empty is an
  explicit OFF upstream and a silent no-op here. This is ordinary in-scope port work.
- **`SESS-038` and `SEAM-058` are not SDK items at all; they are unreachable upstream.**
  `00-residual-ledger.md:284`: nothing in `coding-agent/src` or `agent/src` imports
  `session-backends/sqlite-node`. `:285`: at v0.84.1 `git grep experimentalCli` matches only the file
  itself and its test. They are trackers under the project's *existing* reachability rule, which has
  nothing to do with SDKs.
- **`PROV-031` is the only genuine SDK/crate-boundary item**, and
  `01-cyrup-core-and-provider.md:593-601` already records that **no behaviour is lost** — login/logout
  live at `cyrup-config/src/login.rs:784`, the auth check at `:360`, availability filtering at
  `cyrup-session-svc/src/session.rs:2726-2728`. What is lost is the crate boundary and two duplicated
  availability filters that can drift.

Four files asked the same question because each hit a *different* problem and reached for the same
label. The question needs re-posing before it can be answered.

### What the SDK surfaces actually are, both sides

**pi @v0.83.0.** The embedder surface is `packages/coding-agent/src/index.ts` — 401 lines, 34 `export`
statements over 31 modules. `core/sdk.ts` (398 lines) is *one of them*: `CreateAgentSessionOptions`
(`:37-83`), `CreateAgentSessionResult` (`:86-93` — `session`, `extensionsResult`,
`modelFallbackMessage`), `createAgentSession` (`:169`), and a re-export block at `:96-126` ending in
`withFileMutationQueue` plus nine tool factories under the comment "Tool factories (for custom cwd)"
(`:114-126`). `index.ts` additionally exports the TUI component classes, the theme API
(`Theme`/`highlightCode`/`initTheme`), `InteractiveMode`, `RpcClient`, `main()`, `getShellConfig`, the
trust store, the compaction API and the package manager.

**cyrup at HEAD.** `crates/cyrup-sdk` is 923 lines over 5 files (`client.rs` 288, `error.rs` 44,
`handle.rs` 450, `lib.rs` 114, `prelude.rs` 27) and depends on **6** of the 18 workspace crates
(`Cargo.toml:15-20`: core, session-svc, modes, agent, provider, session; `cyrup-ext` is a *dev*-dep
only, `:29-31`). `lib.rs` re-exports those six as modules (`:110-115`), the seam types (`:76-83`), the
multi-session runtime types (`:86-89`) and three run-mode helpers (`:99-102`).

Three concrete, checkable results of that shape:

1. **`sdk.ts:114-126`'s own re-export block is entirely absent.**
   `rg 'cyrup_tools|create_read|with_file_mutation' crates/cyrup-sdk/src/lib.rs` → **zero**. An
   embedder wanting pi's documented "read-only tool set over a custom cwd" must add a direct
   `cyrup-tools` dependency — which `lib.rs:7` says the crate exists to prevent ("re-exports the
   load-bearing types so embedders need not depend on internal crates"). That is a self-contradiction
   at HEAD and **no gap item names it**. It is also free to fix: `cyrup-session-svc/Cargo.toml:29`
   already depends on `cyrup-tools`, so `cyrup-sdk` already links it transitively — a direct
   dependency adds no crate to the graph.
2. **`CreateAgentSessionResult`'s other two members are not on the ergonomic handle.**
   `handle.rs` exposes 27 methods and neither `model_fallback_message` nor the extensions result. Both
   are *reachable* — `session.agent_session().model_fallback_message()`
   (`cyrup-session-svc/src/session.rs:3196-3198`, correctly cited to "Pi `modelFallbackMessage`,
   sdk.ts:91") and extension load errors via `startup_diagnostics` (`builder.rs:926-928`). So this is
   ergonomics, not capability.
3. **Everything else `index.ts` exports exists in the workspace**, on `cyrup-tui`, `cyrup-config`,
   `cyrup-tools`, `cyrup-resources` or `cyrup-modes` — reachable, just not from `cyrup-sdk`.

The mechanism difference is real and forced: **pi ships one npm package; cyrup ships 18 crates.** A
literal 1:1 re-export list would make `cyrup-sdk` depend on `cyrup-tui` and therefore on ratatui and
crossterm, which is worse for every embedder that does not draw a terminal. But the mechanism
difference costs behaviour in exactly one place — `sdk.ts:114-126` — and under the standing rule that
cost stays on the backlog as work.

---

## Decision

### A. Requirement ids, ADR ids and `spec/` paths carry **no authority**. They are an index.

1. **A citation is provenance, never justification.** An `R-NN-NNN`, `R-ARCH-<AREA>-NNN`, `ADR-NNNN`,
   `arch-NN`, `func-NN` or `spec/…` token may not, on its own, justify a divergence, hold a severity
   down, close a gap item, gate a code review, or answer "is X in scope?". Where one is the *only*
   support for a divergence, the divergence is an **open gap item at its own user-visible severity**
   until it is either ported or re-justified from a two-sided read. This generalises the ruling the
   repair pass already applied to `TUI-019` and makes it the standing rule for all ~2 195.
2. **Do not mass-delete them.** The index works (the `R-08-010` test above) and it is the only
   surviving map of a design nobody can read. Deleting ~2 195 tokens to solve a problem caused by ~45
   would destroy the map to remove the misleading signposts.
3. **Quarantine the ~2 %, not the 98 %.** Add `cargo xtask lint-citations` in batch 3 (which already
   creates `crates/xtask`). It fails the build on any line that carries a citation token **and** a
   divergence marker (`not ported`, `unported`, `dropped`, `deliberate`, `intentional`, `mandate`,
   `no analog`, `divergence`, `out of scope`, `CYRUP-DELTA`) unless the same doc-comment block also
   names a file that exists under `docs/adr/` or `docs/gap-analysis/`. Seed it with a reviewed
   allow-list carrying one reason per entry — ~45 lines at HEAD, of which `R-09-021` (5 sites) and
   `ADR-0001` (20 sites) are the two clusters. The lint's job is to stop the class from growing, not
   to clear the backlog in one commit.

   **Interaction with ADR-0002's `CYRUP-DELTA` rule, settled here so the two lints do not fight.**
   `docs/adr/ADR-0002-extension-io-is-serde.md` rule 7 *mandates* a `CYRUP-DELTA` note wherever a
   mechanism-forced omission is deferred, and `CYRUP-DELTA` is on this lint's divergence-marker list.
   A conforming note therefore carries **both** halves and passes both lints: the tagged two-sided
   upstream citation ADR-0002 requires (`pi v0.83.0 types.ts:334`) **and** the owning
   `docs/adr/…` path or `docs/gap-analysis/` item id this rule requires. A `CYRUP-DELTA` that names
   only the pi line fails `lint-citations` — correctly, because an unowned delta is exactly the
   unverifiable claim §A.1 forbids. The two checks ship as one `cargo xtask lint-citations` pass.
4. **The `R-NN-NNN` and `R-ARCH-*` namespaces are CLOSED.** Read-only, historical, index-only. Never
   mint a new one. To cite a requirement from now on, an author does exactly one of two things:
   - **cite a decision** — write it into `docs/adr/ADR-NNNN-<slug>.md` in this repository and cite it
     by that path; or
   - **cite the code** — name the upstream repo, **tag** and `file:line` on both sides, tag mandatory
     (`pi-permission-system v0.7.1 index.ts:2049-2075`), per `check-citations.py`'s docstring.

   A bare untagged upstream `file:line` is a defect, not a citation — `builder.rs:147` above is why.
5. **`docs/adr/` is this workspace's decision of record**, and this batch's eleven files are its first
   entries. **This batch re-uses the numbers `ADR-0001` and `ADR-0002`**, which the 26 in-source
   tokens already spend — `docs/adr/ADR-0001-tui-substrate.md` and
   `docs/adr/ADR-0002-extension-io-is-serde.md` exist in this directory as of this writing, on the
   same two subjects as the lost originals (the TUI substrate; extension I/O crossing as serde).
   That is a live ambiguity, not a hypothetical one, and it must be closed **per site, by reading**,
   never by assuming the numbers mean the same thing:

   - Where the in-source token's claim **matches what the new ADR actually decides**, the citation now
     resolves. Re-point it explicitly — `docs/adr/ADR-0001-tui-substrate.md`, path and all, not the
     bare token — so the next reader can `cat` it. The bare form `ADR-0001` is forbidden going
     forward precisely because it is now ambiguous between a lost document and a present one.
   - Where the new ADR decides something **different from, or narrower than, what the comment
     asserts** — and `cyrup-tui/src/app.rs:1281` and `startup.rs:20-22` are two sites that already
     record their token as unbacked — the comment is **wrong**, not merely stale. Strike the citation
     and either port the behaviour or file it as a gap item at its own severity.

   Do **not** re-create the lost `spec/architecture` documents to make the tokens resolve. A citation
   is repaired by re-reading the code it annotates, never by manufacturing the authority it invokes.
   Whichever batch next touches each of the 26 sites in `cyrup-tui`, `cyrup-ext`, `cyrup-ext-sdk` and
   `cyrup-session` does this triage for that file; the lint in §A.3 keeps the class from growing
   meanwhile.
6. **Fix the eight stale `spec/gap-analysis/…` paths** to their `docs/gap-analysis/` equivalents under
   the current numbering, in the batch that owns each file. `12-cyrup-tui.md` → `07-cyrup-tui.md`,
   `03-cyrup-agent.md` → `02-cyrup-agent.md`. Where the target was never rescued
   (`12-cyrup-tui-audit-summary.md`, `spec/gap-analysis/fixtures-capture/*.ts`), replace the path with
   a one-line statement of what the reader needs, since the file is gone — the two fixture `_note`
   blocks in `cyrup-test-support/fixtures/pi/` already carry full reproduction instructions inline and
   need only the dead path struck.
7. **Correct `README.md:64-71`.** It presents `../spec` as a currently-available tree. Rewrite it to
   say the tree is lost, that the ids remain as a grep index with no authority, and to point at
   `docs/adr/`. Leaving it as-is is what produces the next reader who thinks the citations are
   checkable.

### B. `PERM-009`: delete the branch. There is no mandate, and no branch is held open for one.

Delete `crates/cyrup-permission-system/src/extension.rs:1651-1653` and the false parenthetical at
`:1631`, leaving `should_expose_tool` byte-equivalent in behaviour to `index.ts:1791-1816` @v0.8.0.
`PERM-009` stays **critical** and closes on deletion.

OQ-6's "produce `spec/`, then re-implement in the re-gating shape" option is **struck, not deferred**:
no requirement id is cited at that site, so there is no document that could be produced to revive it.
If a maintainer later wants a bash bypass, it is a **new feature filed against upstream behaviour**
with its own ADR and its own re-gating `tool_call` handler — never a restoration of this branch.

### C. SDK-surface parity: in scope by **capability**, not by export list.

The rule an implementer follows, without re-deriving anything:

> **Every capability pi exports from `packages/coding-agent/src/index.ts` at the ported tag must
> exist, be `pub`, and be reachable from a published workspace crate. Which crate it lives on is a
> documented mechanism difference and costs nothing. `cyrup-sdk` must additionally re-export
> everything `core/sdk.ts` itself re-exports, because that is a named file the port claims to have
> ported and it fits without dependency inversion. A capability that exists nowhere is an ordinary
> gap item, graded on its user-visible consequence like everything else — SDK-only reach is not a
> severity discount, and it is not a scope exemption.**

**Boundary against ADR-0004, stated so the two rules cannot be read into conflict.** The capability
set this rule ranges over is what **`packages/coding-agent/src/index.ts`** exports — pi's *product*
surface. It is **not** what `packages/agent/src/index.ts` exports, which is the separately-published
`@earendil-works/pi-agent-core` SDK; `docs/adr/ADR-0004-agent-harness-scope.md` rules that package's
`harness/**` subtree out of scope on the measured ground that no line of it reaches the `pi` binary.
Nothing in §C pulls a harness symbol back in: coding-agent's `index.ts` exports none of them, and its
one importer (`src/server/create-harness.ts`) is on no export path. If pi ever moves a harness symbol
onto coding-agent's `index.ts`, that is ADR-0004's tripwire firing, not this rule widening.

Concretely:

- **`cyrup-sdk` takes a direct `cyrup-tools` dependency and re-exports `sdk.ts:114-126`** — the file
  lock/mutation-queue primitive plus the tool constructors for `read`/`bash`/`edit`/`write`/`grep`/
  `find`/`ls` and the coding / read-only sets. No new crate enters the graph
  (`cyrup-session-svc/Cargo.toml:29` already pulls it). File this as a new item in area 08; **S**;
  severity **low** (an embedder can work around it with a direct dependency today, at the cost of the
  encapsulation `lib.rs:7` promises).
- **Surface `model_fallback_message()` and the extension-load result on `cyrup_sdk::Session`**
  (`handle.rs`), mirroring `CreateAgentSessionResult`'s three members. **S**, **low** — both are
  already reachable one hop down.
- **`cyrup-sdk` does NOT re-export `cyrup-tui`, `cyrup-config` or `cyrup-resources` as modules.**
  Doing so inverts the dependency graph and drags ratatui into every embedding. This is the stated
  mechanism difference; its behavioural cost is zero because each capability is `pub` on its own
  workspace crate and an embedder can depend on that crate directly. Record it in `lib.rs`'s module
  doc as a mechanism difference carrying this ADR's path — not as an "accepted divergence", of which
  there is no category.
- **`PROV-031` stays open at `low`**, unchanged. Its own body already establishes that no behaviour is
  lost; what it fixes is a crate boundary and two duplicated availability filters. It is now correctly
  classified rather than parked behind a scope question.
- **`VL-P5` / `DRIFT-047` are reclassified out of "SDK surface" and into ordinary port work.**
  Telemetry is inside the runtime dependency closure at v0.84.1 and `DRIFT-050` is user-facing.
  `DRIFT-047` keeps its `duplicate-of: VL-P5` marker and is resolved by extending `VL-P5`, per
  `12-upstream-drift-pi-core.md:756`.
- **`SESS-038` and `SEAM-058` leave OQ-6's dependent list**, because neither is an SDK-scope
  question — both are **upstream-reachability** trackers. What this ADR settles is the *reason*, not
  the disposition:
  - **`SEAM-058` remains a tracker** on that reason. Its escalation trigger is unchanged and already
    written: pi's `main()` referencing `experimentalCli`. ADR-0006 re-verified it as **not fired** at
    pi HEAD `581d75a89`.
  - **`SESS-038`'s disposition is ADR-0004's, not this ADR's**, and ADR-0004 **closes it as out of
    scope** on a two-sided measurement this ADR did not make (no package in pi's monorepo depends on
    `@earendil-works/pi-session-backend-sqlite-node`). An earlier draft of this ADR said "tracker
    held"; that is superseded — `docs/adr/ADR-0004-agent-harness-scope.md` owns the call, and
    `03-cyrup-session.md:124` required it be answered with `AGENT-028`, which only OQ-2 could do.

  Both `## Trackers` rows must be re-worded to cite reachability rather than the SDK question, so
  nobody re-opens OQ-6 to move them.
- **`AGENT-028` is NOT settled here.** It is the agent-harness scope question (OQ-2) and only its
  telemetry leg overlapped this one. Leave it to OQ-2's ADR.

---

## Consequences

**Batch by batch.**

- **Batch 2** — the line "Write ADR-0001 into this workspace or delete every reference to it"
  (`PARITY-PLAN.md:248-249`) is **replaced**: do neither. `docs/adr/ADR-0001-tui-substrate.md` now
  exists from this same batch, so the 20 in-source `ADR-0001` sites get §A.5's per-site triage —
  re-point where the new ADR really decides the claim, strike where it does not — instead of a
  blanket write-or-delete. The batch's *Verified by* line
  (`PARITY-PLAN.md:259-260` — "`rg 'ADR-0001|spec/architecture|R-[0-9]{2}-[0-9]{3}' crates/` returns
  only references that resolve to a readable file") is **unachievable and is withdrawn**; substitute:
  *`cargo xtask lint-citations` passes, its allow-list has one reason per entry, and no line outside
  the allow-list pairs a citation token with a divergence marker.*
- **Batch 3** — gains `cargo xtask lint-citations` (§A.3), which also carries ADR-0002 rule 7's
  `CYRUP-DELTA` conformance check. Small: it is a line-oriented regex pass over `crates/**`, not a
  `syn` parse. **Batch 3 is the collision point of five ADRs in this batch** — it also gains
  `cargo xtask upstream-watch` and the `check-citations.py` repair (ADR-0006), the two
  `--target *-pc-windows-msvc` check gates (ADR-0007), and the `CYRUP_SHELL` repo-guard test
  (ADR-0003 D8(2)). The consolidated list is in `docs/adr/README.md`; size batch 3 against all of it,
  not against this line alone.
- **Batch 7** — `PERM-009` deletes cleanly with no held-open follow-up. The batch's own hedge at
  `PARITY-PLAN.md:480-483` ("If the mandate is later produced, the correct shape is still not the
  current one…") is **struck**: no mandate is cited at the site, so there is nothing to produce.
- **Batch 19** — the pre-flight at `PARITY-PLAN.md:943-944` ("If OQ-6 comes back 'out of scope',
  re-check the member list before starting") resolves to **no member-list change**. `cyrup-ext-sdk`
  stays in `members` (`Cargo.toml:20`) and out of `default-members`, for the reason stated at
  `Cargo.toml:25-27` — it is a wasm32-wasip2 guest crate, not a host-graph member. Proceed as
  planned; the 0.5.0 SDK migration note is still required.
- **Every batch** — a reviewer may now reject a diff whose only justification for a divergence is a
  citation token, by pointing at this ADR.

**Ledger changes** (severity / kind / scope, for the gap-analysis update pass):

| id | change |
|---|---|
| `PERM-009` | **critical held.** Scope narrowed: the "produce the mandate" branch is struck; the fix is deletion of `extension.rs:1651-1653` plus the false parenthetical at `:1631`, full stop. Its *Taken on trust* caveat (`10-cyrup-permission-system.md:475`) is resolved — the site cites no id at all. |
| `TUI-019` | `medium` **held**, and the ADR-0001 strike is now permanent policy rather than a one-item repair. Its scope decision belongs to OQ-3, not here. |
| `SESS-038` | **Not an SDK question** — remove it from OQ-6's dependents; its *reason* is unreachable-upstream. Its **disposition is ADR-0004's**, which closes it out of scope. This ADR does not hold it open. |
| `SEAM-058` | tracker **held** on the unreachable-upstream reason; remove it from OQ-6's dependents. Trigger re-verified not-fired by ADR-0006. |
| `DRIFT-047` / `VL-P5` | **kind change**: out of "SDK surface, decision pending" and into ordinary in-scope port work; `DRIFT-047` keeps `duplicate-of: VL-P5`. Severity unchanged pending the `DRIFT-050` pairing. |
| `PROV-031` | `low` **held**; scope confirmed as crate-boundary + de-duplication. No longer blocked on a scope answer. |
| `AGENT-028` | **untouched here.** Belongs to OQ-2. |
| `PB-7` | its "cannot be checked from this workspace" caveat on `R-09-021` (`PARITY-GAPS.md:828`) is resolved by rule: the id carries no authority, so the npm-channel drop is an undecided question to be settled on its own merits, not a decided one. `R-09-021`'s 5 sites go on the lint allow-list until then. |
| `CFG-048` | its `migrations.rs:9-10` justification is an in-source "intentionally NOT ported" comment with no citation — already ruled unverifiable at `05-cyrup-config-and-resources.md:284`, now covered by the general rule and by the lint. |

**New work this creates that no item covers** — three items, all new, to be filed in the update pass:

1. **area 08** — `cyrup-sdk` does not re-export `sdk.ts:114-126` (the mutation queue + nine tool
   factories) and cannot, lacking a `cyrup-tools` dependency, while `lib.rs:7` promises embedders need
   no internal-crate dependency. `low` · **S**.
2. **area 08** — `cyrup_sdk::Session` surfaces neither member of `CreateAgentSessionResult`
   (`sdk.ts:86-93`) beyond `session` itself. `low` · **S**.
3. **area 05 / cross-cutting** — the citation lint, its allow-list, the §A.5 triage of the 26
   `ADR-000N` sites, the eight stale `spec/gap-analysis/` paths, the `README.md:64-71` correction, and
   the seven stale "`README:208-212`" anchors re-pointed at the canonical three
   (`docs/gap-analysis/README.md:130-135` / `:268-273` / `:274-276`). `low` · **M** as one unit; it is
   98 % mechanical.

---

## Rejected alternatives

**Delete all ~2 195 citations.** Cost: destroys the only surviving map of the design. The `R-08-010`
test proves the index resolves — 12 sites, 6 crates, one coherent meaning. A reader cross-cutting the
permission seam today greps one id; afterwards they read 815 files. It also costs a ~2 195-line diff
touching 380 files, which cannot be reviewed and will collide with all thirty batches. Buys only the
removal of ~45 misleading lines, which §A.3 removes at 2 % of the cost.

**Reconstruct `spec/` from the code.** Cost: it would be written *from* the implementation, so every
requirement would be true by construction — a specification that can never fail. That is worse than no
specification, because it launders the port's current behaviour into an authority. It also cannot be
done: `spec/tui/01`…`07` alone is cited 201 times at §-level granularity nothing in-tree records, and
`arch-08b`, `func-05`'s absence and the `R-ARCH-*` scheme show the structure is not recoverable from
the citations either.

**Treat citations as authoritative "until disproven".** Cost: this is the status quo that produced the
problem. It held `TUI-019` at `low` for months (`docs/gap-analysis/README.md:273`), it held
`PERM-009` at `medium` behind a sentence asserting a re-gate the code does not perform, and it is
unfalsifiable by
construction — nobody can disprove a document nobody can read. It also inverts the burden of proof
against the project's own two-sided-evidence rule.

**Rule that `spec/` is authoritative but simply unavailable, and block on recovering it.** Cost: it
blocks batches 2, 3, 7 and 19 on a machine that the repo's own commit message says is gone, and the
maintainer has explicitly asked not to be blocked. There is also nothing to recover: the workspace
root was not a git repo, so no history of it exists anywhere.

**Declare SDK-surface parity out of scope.** Cost: it would exempt `sdk.ts:114-126` — a named file in
the port's own claimed scope, fixable in an afternoon with no new crate in the graph — and it would
create the "accepted divergence" category the project explicitly does not have
(`docs/gap-analysis/README.md:274-276`). It would also mis-file three of its four supposed members:
telemetry is runtime, and two are unreachable-upstream trackers.

**Hold `cyrup-sdk` to a literal 1:1 with `index.ts`'s 34 export statements.** Cost: `cyrup-sdk` would
have to re-export `cyrup-tui`'s ~40 component types and the theme API, taking a hard dependency on
ratatui and crossterm for every embedder — including headless ones, which is most of them. It would
also re-export `main()`, making a library that runs a CLI. This is the case where cargo genuinely
forces a different mechanism; §C ports the behaviour and states the mechanism difference with its
reason, as the standing rule requires.

**Answer only the `R-NN-NNN` half, as OQ-6 literally asks.** Cost: leaves 1 140 citations across four
other schemes (`arch-NN`/`func-NN` 820, `spec/…` 220, `R-ARCH-*` 74, `ADR-NNNN` 26) un-ruled, and the
next reader re-opens the same question under a different token. The 26 `ADR-NNNN` sites are the *most*
normative of the five schemes, so the literal reading would miss the worst offenders.

---

## How to reverse this

> *"The `spec/` tree still exists — here it is — and its requirements are binding on the port."*

To act on that, the tree must be **checked into this repository** (a path someone can `cat`), pinned
to the commit the code was written against, and reconciled: for each of the ~45 normative citations,
either the requirement is confirmed and the divergence closes with a `docs/adr/` entry recording it,
or it is not and the divergence stays open. Producing the tree alone does not reverse this ADR — the
lint stays either way, because §A's rule is about *checkability*, not about whether a particular
document happens to be true.

Two narrower reversals, each independent:

- *"Restore the bash bypass"* (§B) requires a stated user-visible reason, an ADR, and a `tool_call`
  handler that re-gates execution to the allow-listed commands — pi's read/skills shape
  (`index.ts:2068-2070`), not `manager.rs:205-215`'s command-first precedence. Producing a document
  is not sufficient; the current shape is wrong under any mandate.
- *"The SDK tracks a narrower contract than the product"* (§C) requires naming the contract in a
  document in this repository and accepting, in writing, the cost: `sdk.ts:114-126` stays unported and
  `lib.rs:7`'s no-internal-dependency promise is withdrawn from the crate's own docs.

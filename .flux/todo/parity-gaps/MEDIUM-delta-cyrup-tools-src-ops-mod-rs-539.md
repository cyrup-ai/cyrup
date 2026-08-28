---
title: "CYRUP-DELTA capability gap at crates/cyrup-tools/src/ops/mod.rs:539"
priority: MEDIUM
crate: cyrup-tools
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: aug
status: done
updated: 2026-08-28 02:12
---

# Capability gap: `crates/cyrup-tools/src/ops/mod.rs:539`

Classified a **capability gap** — a caller can observe a difference — by the audit
that reviewed all 87 `CYRUP-DELTA` markers against pi at `e8682309`.

**This was never authorized as an accepted divergence.** The marker was written by an
agent. Nobody decided it was acceptable. It is filed here so it is a decision rather
than an artifact.

## What pi does

pi exposes `BashOperations` as a PUBLIC extension type, and an extension returns one from a
`user_bash` handler via `UserBashEventResult.operations` or from `BashToolOptions.operations`.
`executeBash` resolves `options?.operations ?? createLocalBashOperations({ shellPath })` on every
invocation, so a JS extension can redirect command execution to SSH / a container / a remote host.

## What cyrup does

The host-side trait exists and the consumer side is wired (`BashOptions::operations` ->
`execute_bash`), but there is no WIT round-trip: `crates/cyrup-ext/wit/world.wit`'s `on-user-bash`
returns a `hook-outcome` only; there is no registration import and no keyed dispatch export, so a
WASM guest has nothing callable to register. `crates/cyrup-ext/src/lib.rs` states this openly
(DRIFT-004 / SEAM-015). NOTE: the marker is tagged `[CYRUP-DELTA, mechanism]` — that tag is wrong.

## What a caller sees

CONFIRMED capability gap. An extension author porting a pi `user_bash` handler that returns
`operations` gets: the JSON key survives into `UserBashReduction::Handled`, and then nothing
happens — the command runs on the local host shell. Only in-host Rust callers can supply a backend.

---

# AUGMENTATION (stage: aug, 2026-08-28)

Reference tree `./tmp/pi` at **`e8682309`**, which is pi **v0.84.3**
(`tmp/pi/packages/coding-agent/package.json:3`). Every pi and cyrup symbol below was opened at that
commit / at cyrup HEAD. Anchors are by SYMBOL first, line second, because the line numbers move.

## 0. Which trait? — the brief's `FsOps` is wrong, twice over

The brief called this `FsOps`. It is **`BashOperations`** and the task file was already right about
that. Verified:

- `crates/cyrup-tools/src/ops/mod.rs:539` is the `[CYRUP-DELTA, mechanism]` line, and it sits in the
  doc comment of **`pub trait BashOperations`** (`ops/mod.rs:554`).
- `pub trait FsOps` is a *different* trait in the same file (`ops/mod.rs:394`). It carries its own,
  unrelated `[CYRUP-DELTA, mechanism only]` at `ops/mod.rs:115` (pi reads exactly
  `IMAGE_TYPE_SNIFF_BYTES` from a file). `FsOps` is a session-lifetime backend, not an
  extension-returnable one.
- `pub trait ProcOps` (`ops/mod.rs:464`) is the third; `ops/mod.rs:517-528` states the split
  deliberately — `ProcOps` is the construction-time backend, `BashOperations` is the **per-call
  override an extension supplies**.

The `FsOps` naming is not merely a typo: it points at a *different mechanism*, and correcting it
narrows the gap materially. See §3.

## 1. What pi actually lets an extension do here, at `e8682309`

**The interface** — `packages/coding-agent/src/core/tools/bash.ts`:

- JSDoc `:59-62` — *"Pluggable operations for the bash tool. Override these to delegate command
  execution to remote systems (for example SSH)."*
- `export interface BashOperations` `:63-81`. **ONE member**, `exec` `:71-80`:
  `exec: (command: string, cwd: string, options: { onData: (data: Buffer) => void; signal?:
  AbortSignal; timeout?: number; env?: NodeJS.ProcessEnv }) => Promise<{ exitCode: number | null }>`
- `createLocalShellOperations(shellName, resolveShellConfig)` `:84-150` — the local implementation.
- `createLocalBashOperations(options?: { shellPath?: string })` `:158-160` — the wrapper pi
  **exports to extensions** (`src/index.ts:290`; the type itself at `:280`) for the
  wrap-then-delegate case. Its own JSDoc `:153-157` names the use case: *"useful for extensions that
  intercept user_bash and still want pi's standard local shell behavior while wrapping or rewriting
  commands."*
- `BashToolOptions.operations` `:198-200`; `createBashTool(cwd, options?)` `:536`.

**The extension-facing return** — `packages/coding-agent/src/core/extensions/types.ts:1117-1122`:

```ts
export interface UserBashEventResult {
	/** Custom operations to use for execution */
	operations?: BashOperations;
	/** Full replacement: extension handled execution, use this result */
	result?: BashResult;
}
```

subscribed at `types.ts:1278` (`on(event: "user_bash", …)`).

**Where it is consumed** — three sites, all verified:

- `agent-session.ts:2953-2985` `executeBash(command, onChunk?, options?)`; the resolution is
  **`agent-session.ts:2972`**: `options?.operations ?? createLocalBashOperations({ shellPath })`,
  handed to `executeBashWithOperations` (`bash-executor.ts:50-56`).
- `rpc-mode.ts:565-583` — `case "bash"`: short-circuits on `eventResult?.result`, otherwise
  `operations: eventResult?.operations` at **`:581`**.
- `interactive-mode.ts:6471-6494` — the `!`/`!!` path, `{ excludeFromContext, operations:
  eventResult?.operations }` at **`:6494`**.

**What a real extension actually overrides.** All three shipped pi examples are in the pinned tree
under `packages/coding-agent/examples/extensions/`, and all three do the same two things:

| example | `BashOperations` factory | `user_bash` handler | ALSO re-registers built-in tools |
|---|---|---|---|
| `ssh.ts` | `createRemoteBashOps` `:81` | `:203-206` → `{ operations: … }` | `read`/`write`/`edit`/`bash` at `:126-181` |
| `sandbox/index.ts` | `createSandboxedBashOps` `:132` | `:229-231` | `bash` at `:223` |
| `gondolin/index.ts` | `createGondolinBashOps` `:324` | `:517-519` | `read`/`write`/`edit`/`bash`/`ls`/`find` at `:448-503` |

So the concrete override is: **one method**, `exec`, that shells out to `ssh`/a sandbox/a VM instead
of spawning locally, streams combined stdout+stderr back through `onData` as raw `Buffer`s, honours
an `AbortSignal`, and returns an exit code (`null` = killed).

## 2. Verified state of the cyrup side

- **Host-side trait: complete.** `cyrup_tools::ops::BashOperations` (`ops/mod.rs:554-570`),
  `BashExecOptions` (`ops/mod.rs:507-513`: `on_data: &mut dyn FnMut(&[u8])`, `cancel`, `timeout`,
  `env`, `env_remove`), `LocalBashOperations` (`ops/mod.rs:581`, `::new` / `::with_proc`).
- **Consumer side: complete.** `cyrup_session_svc::BashOptions::operations:
  Option<Arc<dyn cyrup_tools::ops::BashOperations>>` (`cyrup-session-svc/src/bash.rs:99`);
  `run_bash(..., operations, ...)` takes the `??` whole at `bash.rs:141`/`:185-186`;
  `AgentSession::execute_bash` resolves it at `cyrup-session-svc/src/session/bash.rs:115-124`;
  `execute_bash_with_user_event` (`session/bash.rs:167`) forwards it.
- **Reduction carries the key.** `ExtensionHost::emit_user_bash` →
  `UserBashReduction::Handled(Value)` (`cyrup-ext/src/facade.rs:57`, `:968`), pinned by
  `cyrup-ext/src/tests/payload_and_seam_parity.rs:993`
  `user_bash_reduction_carries_the_operations_half_not_only_the_result_half`.
- **Nothing fills it.** `emit_user_bash_event` (`session/bash.rs:191`) reads only `"result"`;
  `cyrup-modes/src/rpc/mod.rs:1109` passes a literal `operations: None`;
  `cyrup-session-svc/src/command.rs:148` likewise.
- **The world.** `on-user-bash: func(command, exclude-from-context, cwd) -> hook-outcome` at
  `crates/cyrup-ext/wit/world.wit:351` (the task file said `:345-346` — drift, and it is one line).
  `interface registration` `:502-531` has no `register-bash-operations`. `interface events` has no
  bash dispatch export.
- **The design is already written.** `crates/cyrup-ext/src/lib.rs:78-120` is the DRIFT-004 /
  SEAM-015 register entry and it drafts the exact round-trip. §5 below corrects it in three places
  and adds the two costs it does not name.

**Version note (ADR-0002 is stale on this point):** the world is `package cyrup:ext@0.8.0`
(`world.wit:63`) and `HOST_WORLD = "cyrup:ext@0.8"` (`cyrup-ext/src/manifest.rs:233`). ADR-0002 was
written at `0.4.0` and frames the work as "batch 19, the single `0.5.0` bump"; four minors have
shipped since. Do not schedule off the ADR's batch numbers.

## 3. FINDING — the gap is materially narrower than the marker claims

The marker says: *"A pi extension that transparently ran the user's shell over SSH is not
expressible as a cyrup WASM extension at all."* **That is an overstatement, and David should not
make an accept/close decision on it.**

Verified: **a cyrup WASM guest can already register a tool named `bash` and it overrides the
built-in by name.**

- `ExtensionRegistry::active_tools` (`cyrup-ext/src/registry.rs:1162-1183`): *"Merge a base tool set
  (built-ins) with extension tools; extension tools override by name (R-08-012/014)."* Pinned by
  `cyrup-ext/src/tests/native_dispatch.rs:473` `a08_4_registered_tool_overrides_builtin_read`.
- The SDK ships the descriptor for it: `cyrup_ext_sdk::tool_factory::bash_descriptor(cwd)`
  (`cyrup-ext-sdk/src/tool_factory.rs:19-34`), whose own doc says *"The author supplies the executor
  (the guest runs `ctx.exec(...)` against its granted exec capability)."*
- The guest has real execution capability: `interface exec` (`world.wit:799-802`) and `interface
  proc` (`world.wit:817-838`: `spawn`/`write-stdin`/`read-stdout`/`read-stderr`/`poll-exit`/`kill`),
  both behind the `capabilities.exec` grant.

So the **agent-loop half** of all three pi examples — the `registerTool({...localBash, execute})`
half, which is the majority of each file — **is expressible in cyrup today**. What is *not*
expressible is the `user_bash` half: the interactive `!` / `!!` command and the JSON-RPC `bash`
command.

Two residual asymmetries even on the expressible half, both worth stating because they are the
honest version of "already supported":

1. pi's guest can **wrap and delegate** (`createLocalBashOperations` is exported to extensions,
   `index.ts:290`). cyrup exposes `LocalBashOperations` to in-host Rust only, so a cyrup guest
   overriding `bash` must reimplement the whole tool — truncation, output accumulation, ANSI
   stripping — rather than wrap it. That is a separate, smaller gap; see Open Question 3.
2. pi's fs-side seams (`ReadOperations` `read.ts:49`, `WriteOperations` `write.ts:31`,
   `EditOperations` `edit.ts:96`, `LsOperations` `ls.ts:37`, `GrepOperations` `grep.ts:56`,
   `FindOperations` `find.ts:55`) are supplied **only** through tool-factory options, never through
   an extension event result — `grep -rn 'operations' extensions/types.ts` returns exactly one hit,
   `:1119`, and it is `BashOperations`. So they need **no** WIT round-trip; the tool-name override
   above already covers them. **This is why the brief's `FsOps` framing was wrong twice: wrong
   trait, and a mechanism that is already closed.**

**Consequence for the decision:** the true cost of NOT closing is not "no ssh extensions". It is
narrower and sharper — see §7.

## 4. Why ADR-0002 forbids returning a trait impl, and why it is NOT the blocker

`docs/adr/ADR-0002-extension-io-is-serde.md` (accepted 2026-08-13).

**The decision:** *"Every value that crosses the extension boundary crosses as a value, not as a
reference — on both the WASM guest tier and the native built-in tier."*

The mechanical reason, stated in the ADR's Context: *"A WASM Component Model instance has no shared
address space with the host, no shared allocator, and no way to hold a Rust `&mut` across a call.
Nothing about this is a preference."* pi's extension surface is **aliasing** (jiti loads TS modules
into the host process and hands them live objects, `loader.ts:411/:419/:472`); cyrup's cannot be.

The rules that bind this item:

- **Rule 4** — *"Where pi passes or returns a function, port it as a WIT export plus, where the
  function is invoked with host-owned state, a matching import. Registration splits into
  `register-X(key)` (import) + a keyed dispatch export."* `BashOperations` is an object with one
  method, i.e. a function; this is rule 4 verbatim.
- **Rule 6** — a live signal becomes a poll, never a blocking await.
- **Rule 7** — *"The encoding is never a licence to drop a field."* Any serde-representable pi
  argument, field or return **is in scope and must be carried**; only a genuinely non-representable
  thing may be *re-shaped, and re-shaped, not omitted*.
- **Rule 9** — a new export bumps the `HOST_WORLD` minor; added imports are additive.

**The key finding for David: closing this does NOT require revisiting ADR-0002.**

1. Rule 4 already **mandates** the shape. Rule 7 already **forbids** the current omission. The ADR
   is not permitting the gap; the gap is out of compliance with it.
2. Reversing the ADR would not even help. Its rejected alternative **A** ("give the world WIT
   `resource` types so a guest can hold a host object") is the wrong direction for this item:
   `BashOperations` is a **guest-owned** object the **host** calls. Resources let a guest hold a
   *host* object. Taking alternative A would leave this item exactly where it is.
3. Rejected alternative **B** (embed a JS runtime) is refused on a stated project constraint
   (Rust-only, no JS/TS runtime dependency), not on effort.
4. Rejected alternative **D** ("accept the omissions the encoding makes awkward") is the category
   this backlog exists to prevent, and the ADR's own words for its cost apply here verbatim: *"Each
   fails **silently**, keyed on user configuration."*

So: **this is a compliance debt against an already-accepted architecture, not an architecture
question.** The only genuinely architectural sub-question is Cost 2 in §6 (live vs. batched output),
and that is a smaller question than ADR-0002.

## 5. What closure requires — the concrete shape

`crates/cyrup-ext/src/lib.rs:93-105` already drafts this. It is right in outline. **Three
corrections and two additions**, all forced by symbols verified above.

### 5a. WIT — `crates/cyrup-ext/wit/world.wit` (and the byte-identical SDK copy)

**Import**, into `interface registration` (next to `register-markdown-transformer`, `world.wit:517`):

```wit
// pi `UserBashEventResult.operations` (extensions/types.ts:1117-1122 @v0.84.3). Argument-less
// because upstream keeps at most one per handler result; the host reaches the backend through the
// `bash-operations-exec` EXPORT — this import only declares that this guest HAS one.
register-bash-operations: func();
```

**Export**, into `interface events`:

```wit
bash-operations-exec: func(call-id: string, command: string, cwd: string,
                           env-json: string, timeout-ms: option<u64>) -> result<exit-status, string>;
```

**Correction 1 — the draft omits `timeout`.** pi's options bag is `{onData, signal, timeout, env}`
(`bash.ts:71-80`) and `BashExecOptions` (`ops/mod.rs:507-513`) carries `timeout` *and* `env_remove`.
Rule 7 makes both mandatory. `env-json` must carry the additive `env` **and** the `env_remove`
deletion list: pi materializes the whole environment (`env: env ?? getShellEnv()`, `bash.ts:102`) so
it expresses deletion by omission, while cyrup inherits and must name it — `ops/mod.rs:503-506`
states exactly this and the seam must not lose it.

**Correction 2 — the return cannot be a bare `s32`.** `BashOperations::exec` returns
`Result<ExitStatus, ToolError>` and `ops/mod.rs:557-563` is explicit that cyrup deliberately keeps
`ExitStatus::Killed` (cancel) and `ExitStatus::TimedOut` distinguishable where pi collapses both to
`exitCode: null`. Collapsing them at the WIT seam would re-introduce the divergence the trait was
written to avoid. Add a fixed-shape `variant exit-status { code(s32), killed, timed-out }` to
`interface types` — ADR-0002 rule 1 puts fixed-shape control values in real WIT records/variants,
and `exec-result` (`world.wit`, `interface types`) is the existing precedent.

**Imports for the streaming/cancel halves:**

```wit
// pi `onData: (data: Buffer) => void` (bash.ts:73). RAW combined stdout+stderr.
emit-bash-output:   func(call-id: string, chunk: list<u8>);
// pi `signal?: AbortSignal` (bash.ts:74) as the rule-6 poll.
is-bash-cancelled:  func(call-id: string) -> bool;
```

**Correction 3 — `chunk` must be `list<u8>`, not the `chunk-json: string` of
`host-tool.emit-update` (`world.wit:913`).** pi's `onData` takes a `Buffer`; the sanitizing (ANSI
strip, CR normalize) happens in the **caller's** wrapper, `bash-executor.ts:78-102`, never in the
backend — and `ops/mod.rs:492-495` ports that contract deliberately (`on_data: &mut dyn
FnMut(&[u8])`). Routing bytes through a JSON string forces a lossy UTF-8 coercion at the seam and
moves sanitization to the wrong side.

**ABI:** the export is new, so `HOST_WORLD` goes `cyrup:ext@0.8` → `cyrup:ext@0.9`
(`cyrup-ext/src/manifest.rs:233`), per rule 9 and the world's own bump policy note
(`world.wit:43-62`). The three imports alone would be additive and free — see the partial ladder,
§8, and why shipping them alone is worthless.

### 5b. Host — `crates/cyrup-ext`

A `GuestBashOperations { ext: Arc<LiveExtension>, … }` implementing
`cyrup_tools::ops::BashOperations`, dispatching exactly the way `LiveExtension::execute_tool`
(`host/live.rs:1416-1445`) does: take `self.inner.lock().await`, `store.set_epoch_deadline`,
`guest.arm_epoch_deadline_estimate`, bind the call's `CancelToken` so `is-bash-cancelled` reads live
state, and drop the binding under a guard. **Copy `ToolCallBinding` (`host/live.rs:1298-1313`) as a
`BashCallBinding`** — its doc records EXT-M06, where a dropped/cancelled call left its queued chunks
for the *next* call to drain. The same bug is reachable here by construction; do not re-derive it.

### 5c. Wiring — `crates/cyrup-session-svc`

`emit_user_bash_event` (`session/bash.rs:191`) reads `"operations"` off
`UserBashReduction::Handled`, and when the owning extension registered one, constructs the proxy and
fills `BashOptions::operations` (`bash.rs:99`). `execute_bash_with_user_event` (`session/bash.rs:167`)
then sets one field — exactly as its own doc predicts at `session/bash.rs:164-166`: *"Once it exists
this wrapper sets one field."* Same for `cyrup-modes/src/rpc/mod.rs:1109` and
`cyrup-session-svc/src/command.rs:148`, whose `operations: None` becomes the real value.

### 5d. Guest SDK — `crates/cyrup-ext-sdk` (must land in the same change)

Three files, and the template is `register_markdown_transformer`, which is the most recent instance
of this exact pattern:

- `src/api.rs` — an author-facing `BashOperations` trait + `register_bash_operations(&mut self, …)`
  (mirror `api.rs:732`) + the dispatch entry point (mirror `transform_markdown`, `api.rs:738`).
- `src/guest.rs` — call `registration::register_bash_operations()` in the init sweep (mirror
  `guest.rs:160`) and add the export body (mirror `guest.rs:323`).
- `src/macros.rs` — the wit-bindgen shim (mirror `macros.rs:95-99`).

Both `wit/world.wit` copies stay byte-identical; `cyrup-ext/src/tests/wit_world_sync.rs` already
enforces that.

## 6. The two costs the existing design note does NOT name

These are why this is an **M**, not the **S** the register entry implies.

### Cost 1 — the 5-second epoch budget

`WASM_EPOCH_BUDGET_TICKS = 1000` (`cyrup-ext/src/facade.rs:2135`) × `epoch::DEFAULT_TICK = 5ms`
(`cyrup-ext/src/host/epoch.rs:19`) ≈ **5 s per guest call**, after which the guest traps with
`Trap::Interrupt` → `EpochTimeout`. A user `!` command is unbounded — `cargo build` is not 5 seconds.

The escape is real but conditional: `GuestState::note_dialog_wait` (`host/services.rs:2158`) /
`take_dialog_extra_ticks` (`:2193`) forgive wall time the guest spent **blocked inside a host
import**, and `host/live.rs` calls it around every `exec` / `proc` / `http-client` handler
(`:633`, `:707`, `:727`, `:757`, `:786`). It does **not** forgive guest CPU
(`take_dialog_extra_ticks_does_not_reward_a_fast_dialog_followed_by_an_unrelated_cpu_runaway`,
`services.rs:2417`).

**Therefore the design is viable only if the guest's `exec` body is "call a blocking host import,
come back"** — `proc.spawn` + `read-stdout` + `poll-exit` (`world.wit:817-838`), or `exec.run`
(`world.wit:799-802`). A guest that busy-loops traps. **This constraint must be written into the
export's WIT comment**, or the first ssh guest anyone writes will trap on a six-second build and the
failure will look like a host bug. Nothing in the tree says this today.

### Cost 2 — streamed output is BATCHED, not live (the one piece that may need David)

`host-tool.emit-update` chunks are queued on `GuestState` and replayed by `execute_tool` **after the
call settles** (`host/live.rs:1288-1296`; the replay is the second half of `execute_tool`). The
instance is held under `self.inner.lock().await` (`live.rs:1424`) for the whole call, so the host
cannot drain the queue while the guest is inside.

pi renders `!` output **live**: `interactive-mode.ts:6485-6492` calls
`bashComponent.appendOutput(chunk)` per delta, and cyrup's own local path does the same through
`spawn_event_pump` / `BashExecutionUpdate` (`session/bash.rs:107-118`).

So a straight copy of the `execute_tool` mechanism closes the marker but substitutes a smaller
observable divergence: with a guest backend, a long `!` command shows **nothing until it finishes**.
Making it live requires draining the chunk queue from the host while the guest call is in flight,
which the instance mutex forbids without restructuring `LiveExtension`. That restructure is out of
proportion to this item and would touch `execute_tool` too.

**This is a finding for David, not a descope.** Recommendation in §9.

## 7. The cost of NOT closing

State it precisely, because "a feature is missing" understates it:

An `ssh` / sandbox / VM guest is a class of extension whose *entire reason to exist* is that
commands must not run on the local machine. With the gap open, that extension's `user_bash` handler
returns `{ operations }`, cyrup reads the key, discards the meaning, and **runs the user's command
on the local host shell with no error, no warning, and no observable difference from success.** For
a sandbox extension that is a containment breach; for an ssh extension it is a command executed on
the wrong machine.

`crates/cyrup-ext/src/lib.rs:89-91` already says this in as many words. ADR-0002's rejected
alternative D names the class: *"Each fails **silently**, keyed on user configuration."*

Against that: §3 establishes the blast radius is the `!` / `!!` / JSON-RPC-`bash` path only — the
agent's own `bash` tool calls are already redirectable via tool-name override. That is the honest
scoping, and it is what makes an "accept" argument *arguable* rather than absurd. It does not make
it right: the failure is silent and safety-relevant, which is the specific property the parity rule
refuses.

## 8. Partial-closure ladder — what each rung buys

| rung | change | ABI | buys |
|---|---|---|---|
| **P1** | Doc-only: correct `ops/mod.rs:539` and `cyrup-ext/src/lib.rs:78-120` to say the agent-loop half IS expressible today via tool-name override (`registry.rs:1162`) + `tool_factory::bash_descriptor`; correct the stale `@v0.83.0` citations (§10). | none | stops the marker overclaiming. **A prerequisite for any honest accept decision**, and free. |
| **P2** | Land the three imports only (`register-bash-operations`, `emit-bash-output`, `is-bash-cancelled`), no export. | additive, **no bump** | **nothing.** Imports with no dispatch export are unreachable. `cyrup-ext-sdk/src/tests/world_import_coverage.rs` exists precisely because this state has happened twice before (EXT-M04, EXT-M05). **Do not ship P2 alone.** |
| **P3** | Full round-trip, §5, batched output. | `0.8` → `0.9` | closes the capability gap. Residual: no live streaming during a guest-backed `!`. |
| **P4** | P3 + drain-while-in-flight so output streams live. | `0.9` | full parity; requires restructuring `LiveExtension`'s instance lock, which also touches `execute_tool`. |

## 9. PRESCRIPTION

**CLOSE — at P1 + P3, in that order, as two changes.**

- **P1 now, unconditionally.** It is a doc correction, it costs nothing, and no accept/close
  decision should be made on the current overstated marker text.
- **P3 as the closure.** One `crates/cyrup-ext` + `crates/cyrup-ext-sdk` change carrying a
  `HOST_WORLD` minor bump to `cyrup:ext@0.9`, plus a one-field wiring change in
  `cyrup-session-svc/src/session/bash.rs` and two `operations: None` call sites. Effort **M**. The
  design is already written; §5's three corrections and Cost 1's WIT comment are the additions.
- **Record P4's residual (batched output) as an explicit, named open divergence at the moment P3
  lands** — with its own marker citing `host/live.rs:1288-1296` and `interactive-mode.ts:6485-6492`.
  Do not let P3 close the ledger silently over it; that is how this backlog was created.

**The accept case, argued for David so he has it:** the blast radius is the `!` path only (§3); the
agent-loop path is already redirectable; three example extensions upstream is a small population;
and P3 costs an ABI bump that invalidates every already-built guest at `0.8`. **Why I still
prescribe close over accept:** the failure is silent and is a containment failure in exactly the
extension class that exists for containment; ADR-0002 rule 7 already forbids the omission, so
accepting it means amending an accepted ADR, not merely annotating a marker; and the ABI bump is
cheap *now* and gets more expensive with every guest built against `0.8`, which is the same
"reversal is cheapest before the ABI is fixed" argument the ADR makes about itself.

**Explicitly NOT prescribed:** revisiting ADR-0002. §4 shows it is not the blocker and that its
rejected alternative A would not even address this item.

## 10. Additional divergences found (findings, not descopes)

1. **Citation staleness against the pinned tree.** cyrup's in-source pi citations for this seam are
   tagged `@v0.83.0`; the pinned reference tree `e8682309` is v0.84.3, and several now resolve to a
   *different symbol*: `bash.ts:82` (cited as `createLocalBashOperations`) is
   `createLocalShellOperations` at `e8682309`; `types.ts:1078-1083` (cited for
   `UserBashEventResult`) is `:1117-1122`; `agent-session.ts:2782` is `:2972`; `rpc-mode.ts:576` is
   `:581`; `bash.ts:52-73` is `:59-81`; `bash.ts:186-188` is `:198-200`; `bash.ts:451`
   (`tool_factory.rs:17`) is `:536`; `bash.ts:20-38` is `:26-40`; `bash.ts:100` is `:102`. Affected
   files: `cyrup-tools/src/ops/mod.rs`, `cyrup-ext/src/lib.rs`, `cyrup-ext/wit/world.wit`,
   `cyrup-session-svc/src/{bash.rs,session/bash.rs}`, `cyrup-modes/src/rpc/mod.rs`,
   `cyrup-ext-sdk/src/tool_factory.rs`. ADR-0002 rule 7 requires the tag *with* the citation and
   these do carry it, so they are compliant-but-stale rather than defective — but the world's own
   EXT-036 note (`world.wit:150-155`) says an unversioned/mismatched citation "reads as a
   fabrication to the next auditor", and this pass spent real time disproving three of them.
2. **This task file's own anchors drifted**: `world.wit:345-346` is `:351`; `cyrup-ext/src/lib.rs:100-120`
   is `:78-120`; `bash.ts:62-80` is `:63-81`. Corrected above.
3. **ADR-0002's own version framing is stale** (`0.4.0` / "the single `0.5.0` bump" / batch 19–20
   member lists) against a tree at `0.8.0`.
4. **`createLocalBashOperations` is not reachable from a guest.** pi exports it to extensions
   (`index.ts:290`) specifically so a `user_bash` interceptor can wrap-then-delegate; cyrup's
   `LocalBashOperations` (`ops/mod.rs:581`) is in-host Rust only. Even after P3, a cyrup guest can
   *replace* the backend but not *wrap the local one*. Small, separable, not covered by any existing
   item. → Open Question 3.
5. **cyrup has no guest-reachable analog of `BashToolOptions.commandPrefix`** (`bash.ts:201-202`),
   adjacent to `operations` in the same options bag. Not examined further here; flagged so it is not
   discovered later as part of "the bash options bag".

## 11. Open questions for David

1. **Batched vs. live output (Cost 2).** Does P3 land with batched output and a new, explicitly
   named marker for the streaming residual — or does closure mean P4, restructuring
   `LiveExtension`'s instance lock so chunks drain while a guest call is in flight?
2. **ABI scheduling.** ADR-0002's "one bump per batch" discipline was written at `0.4.0`. Does this
   take `cyrup:ext@0.9` on its own, or must it ride the next world-changing batch? Note the world's
   own gate comment (`world.wit:57-62`): `check_world` defends only old-guest-on-new-host, so
   batching bumps is a policy choice, not a safety one.
3. **Wrap-then-delegate (divergence 4).** Should the SDK expose a guest-callable local-shell backend
   (the analog of pi exporting `createLocalBashOperations`), so a guest can wrap rather than
   reimplement? That is a separate additive import (`exec`/`proc` already exist; this would be a
   convenience layer), and it changes the practical usefulness of P3 a lot for the sandbox/logging
   cases, which want to *observe* local execution rather than replace it.
4. **Parity baseline.** Is the baseline v0.83.0 (as the in-source tags say) or `e8682309` / v0.84.3
   (as the pinned reference tree says)? Divergence 1 is unfixable-in-principle until this is settled,
   and it affects far more than this item.

## 12. Guard tests

For the prescribed level (**P3**). All in `crates/cyrup-ext` unless noted. T1 is the DoD-2 test —
**it fails without the change**.

- **T1 — redirection actually happens (DoD-2).** A WASM guest fixture registers bash operations and
  returns `{ "operations": {} }` from `user_bash`. Drive a `!` command through
  `AgentSession::execute_bash_with_user_event` whose *local* execution would produce `local` and
  whose *guest* backend produces `remote`. Assert the recorded `BashResult.output == "remote"`.
  Today this yields `local`. It must be a **WASM guest** fixture, not a native: ADR-0002 rule 10
  makes the WIT world the contract, and a native-only proof would prove the wrong tier.
- **T2 — cancellation is a poll, and stays distinguishable (rule 6).** The guest's `exec` loops on
  `is-bash-cancelled(call-id)`; the host cancels mid-command. Assert `ExitStatus::Killed` (surfacing
  as `BashResult.cancelled == true`), **not** `TimedOut`, and no `EpochTimeout` trap. This is the
  test that pins Correction 2 — a `-> result<s32, string>` return cannot pass it.
- **T3 — raw bytes, host-side sanitization.** The guest emits a chunk containing invalid UTF-8 and
  an ANSI escape sequence. Assert the host delivers ANSI-stripped, lossily-decoded text through the
  same pipeline the local backend uses — i.e. sanitization runs host-side exactly once, whichever
  backend won (`ops/mod.rs:492-495`; pi `bash-executor.ts:78-102`). Pins Correction 3; a
  `chunk-json: string` seam cannot pass it.
- **T4 — no cross-call chunk leak (EXT-M06's lesson).** Two `!` commands in sequence, the first
  cancelled mid-stream. Assert the second receives none of the first's chunks. Mirrors
  `ToolCallBinding` (`host/live.rs:1298-1313`) and its `clear_tool_updates` on drop.
- **T5 — env additive + removal survives (rule 7).** Guest backend receives an `env` entry to set
  and an `env_remove` name to unset; assert both reach it, since cyrup's inherit model cannot
  express deletion by omission (`ops/mod.rs:503-506`). Pins Correction 1.
- **T6 — timeout survives (rule 7).** A `timeout` set on the call reaches the guest backend and a
  timing-out command yields `ExitStatus::TimedOut`, distinct from T2's `Killed`.
- **T7 — ABI.** `cyrup-ext/src/tests/wit_world_sync.rs` (both `world.wit` copies byte-identical)
  must still pass, and `manifest.rs`'s `check_world` coverage must show `HOST_WORLD ==
  "cyrup:ext@0.9"`.
- **T8 — SDK reachability, already automatic.**
  `cyrup-ext-sdk/src/tests/world_import_coverage.rs` is structural over the world text: every
  declared import must have a caller in the SDK. It will fail on `register-bash-operations`,
  `emit-bash-output` and `is-bash-cancelled` until §5d lands. No new test needed — but do not
  suppress it, it is exactly the P2-trap guard.
- **T9 — no regression (DoD-3).** Unchanged and still passing:
  `cyrup-session-svc/src/tests/round9_l5res.rs:610` / `:681` / `:716` (the three
  `..._operations_override_...` tests) and
  `cyrup-ext/src/tests/payload_and_seam_parity.rs:993`.

If only **P1** is taken, its guard is documentary and already exists:
`cyrup-ext/src/tests/native_dispatch.rs:473` (`a08_4_registered_tool_overrides_builtin_read`) is the
proof the corrected text asserts; cite it from the corrected doc comment so the claim is anchored.

---

## Decision required

One of:

1. **Close it** — bring cyrup to pi's behaviour. **← prescribed: P1 now, then P3.**
2. **Accept it** — David explicitly accepts the divergence; the marker stays and is
   annotated as authorized, with the reason. *(The case is argued in §9 so he has both sides;
   accepting also means amending ADR-0002 rule 7, not just annotating a marker.)*
3. **Reshape it** — the divergence is right but the current form is wrong.

Do not silently keep option 2 by leaving the marker as-is; that is how this became a
backlog in the first place.

## Definition of done

1. The gap is closed, or the marker records an explicit authorized acceptance.
2. If closed, a test fails without the change. → **T1**.
3. No behaviour regression in the owning crate. → **T9**.

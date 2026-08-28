---
title: "CYRUP-DELTA capability gap: `UserBashEventResult.operations` has no guest-supplied form (cyrup-tools/src/ops/mod.rs, `BashOperations`)"
priority: MEDIUM
crate: cyrup-tools
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: aug
status: done
updated: 2026-08-28 21:00
---

# Capability gap: the `[CYRUP-DELTA, mechanism]` note on `pub trait BashOperations`

Classified a **capability gap** — a caller can observe a difference — by the audit that reviewed all
87 `CYRUP-DELTA` markers against pi at `e8682309`.

**This was never authorized as an accepted divergence.** The marker was written by an agent. Nobody
decided it was acceptable.

---

# AUGMENTATION (stage: aug, 2026-08-28 21:00)

Reference tree [`./tmp/pi`](../../../tmp/pi) at **`e8682309`** = pi **v0.84.3**
([`package.json:3`](../../../tmp/pi/packages/coding-agent/package.json)). Every symbol below was
re-opened at that commit and at cyrup HEAD **in this pass**. **Anchor on SYMBOLS; the line numbers
are hints.** Several anchors in the previous augmentation of this file had drifted — see §0.

## 0. Anchor corrections — read this before trusting any line number in the prior text

### 0a. The file's own title anchor (`ops/mod.rs:539`) is STALE

`crates/cyrup-tools/src/ops/mod.rs` grew by ~43 lines since the anchors were taken. Re-derived by
`grep -n 'CYRUP-DELTA\|^pub trait \|^pub struct \|^pub enum ' crates/cyrup-tools/src/ops/mod.rs`:

| symbol | prior anchor | **actual** |
|---|---|---|
| `[CYRUP-DELTA, mechanism]` on the bash seam | `:539` | **`:582`** |
| `pub trait BashOperations` | `:554` | **`:597`** |
| `pub struct BashExecOptions` | `:507-513` | **`:550-556`** |
| `pub trait ProcOps` | `:464` | **`:507`** |
| `pub trait FsOps` | `:394` | **`:437`** |
| `pub struct LocalBashOperations` | `:581` | **`:624`** |
| `pub enum ExitStatus` | — | **`:428`** |
| `[CYRUP-DELTA, mechanism only]` on `detect_image_mime` | `:115` | `:115` (unchanged) |

Do not re-file a "line drift" finding; use the right-hand column.

### 0b. Other anchors corrected in this pass

| symbol | prior | **actual** |
|---|---|---|
| `WASM_EPOCH_BUDGET_TICKS` ([`facade.rs`](../../../crates/cyrup-ext/src/facade.rs)) | `:2135` | **`:2205`** |
| `ExtensionRegistry::active_tools` ([`registry.rs`](../../../crates/cyrup-ext/src/registry.rs)) | `:1162-1183` | **`:1174-1176`** |
| `user_bash_reduction_carries_the_operations_half…` ([`payload_and_seam_parity.rs`](../../../crates/cyrup-ext/src/tests/payload_and_seam_parity.rs)) | `:993` | **`:996`** |
| the three `…operations_override…` tests ([`round9_l5res.rs`](../../../crates/cyrup-session-svc/src/tests/round9_l5res.rs)) | `:610/:681/:716` | **`:613/:684/:719`** |
| `operations: None` ([`cyrup-modes/src/rpc/mod.rs`](../../../crates/cyrup-modes/src/rpc/mod.rs)) | `:1109` | **`:1111`** |
| `GuestState::note_dialog_wait` / `take_dialog_extra_ticks` ([`host/services.rs`](../../../crates/cyrup-ext/src/host/services.rs)) | `:2158`/`:2193` | **`:2173`/`:2208`** |
| `take_dialog_extra_ticks_does_not_reward_a_fast_dialog…` (same file) | `:2417` | **`:2432`** |
| `ToolCallBinding` ([`host/live.rs`](../../../crates/cyrup-ext/src/host/live.rs)) | `:1298-1313` | doc **`:1281-1296`**, struct **`:1298`**, `Drop` **`:1300-1312`** |

Unchanged and re-confirmed: `on-user-bash` at [`world.wit:351`](../../../crates/cyrup-ext/wit/world.wit);
`interface registration` `:502-546`; `register-markdown-transformer` `:517`; `interface events` `:191`;
`package cyrup:ext@0.8.0` `:63`; `HOST_WORLD = "cyrup:ext@0.8"`
([`manifest.rs:233`](../../../crates/cyrup-ext/src/manifest.rs));
`a08_4_registered_tool_overrides_builtin_read`
([`native_dispatch.rs:473`](../../../crates/cyrup-ext/src/tests/native_dispatch.rs));
`bash_descriptor` ([`tool_factory.rs:19`](../../../crates/cyrup-ext-sdk/src/tool_factory.rs));
`LiveExtension::execute_tool` (`host/live.rs:1416`); `DEFAULT_TICK`
([`host/epoch.rs:19`](../../../crates/cyrup-ext/src/host/epoch.rs)); both `world.wit` copies are
byte-identical (`md5 b89d5fc7f6408bac09e71a8f7af88bfe`).

## 0c. The `FsOps` premise in the intake brief is FALSE — and the correction matters

A prior audit note recorded that this task's marker text says `FsOps`. **It does not, and it never
did.** Verified directly:

- The `[CYRUP-DELTA, mechanism]` line at **`ops/mod.rs:582`** sits in the doc comment of
  **`pub trait BashOperations`** (`ops/mod.rs:597`). Its text is *"A WASM guest cannot RETURN an
  implementation of this trait"*.
- `pub trait FsOps` (`ops/mod.rs:437`) is a **different trait** in the same file, with its own
  unrelated `[CYRUP-DELTA, mechanism only]` at `ops/mod.rs:115` about `IMAGE_TYPE_SNIFF_BYTES`.
- `pub trait ProcOps` (`ops/mod.rs:507`) is the third. `ops/mod.rs:565-576` states the split
  deliberately: `ProcOps` is the **session-lifetime, construction-time** backend; `BashOperations`
  is the **per-call override an extension supplies**.

The distinction is load-bearing, not cosmetic: pi's fs-side seams are **not** extension-returnable at
all (§2.3), so an `FsOps` framing would point at a mechanism that is already closed and would
mis-scope the work. **Work the `BashOperations` premise; discard `FsOps`.**

## 1. What pi does, at `e8682309`

**The interface** — [`packages/coding-agent/src/core/tools/bash.ts`](../../../tmp/pi/packages/coding-agent/src/core/tools/bash.ts):

- JSDoc `:59-62`: *"Pluggable operations for the bash tool. Override these to delegate command
  execution to remote systems (for example SSH)."*
- `export interface BashOperations` `:63-88`. **Exactly one member**, `exec` `:71-80`:
  `exec: (command: string, cwd: string, options: { onData: (data: Buffer) => void; signal?:
  AbortSignal; timeout?: number; env?: NodeJS.ProcessEnv }) => Promise<{ exitCode: number | null }>`
- `createLocalShellOperations(shellName, resolveShellConfig)` `:84` — the local implementation
  (`env: env ?? getShellEnv()` at `:102`).
- `createLocalBashOperations(options?: { shellPath?: string })` `:158-160`, JSDoc `:153-157` — the
  wrapper pi **exports to extensions**
  ([`src/index.ts:290`](../../../tmp/pi/packages/coding-agent/src/index.ts); the type at `:280`)
  for the wrap-then-delegate case.
- `BashToolOptions.operations` `:198-200` (adjacent `commandPrefix` `:201-202`);
  `createBashTool(cwd, options?)` `:536`; the tool-path call `ops.exec(…, {onData, signal, timeout,
  env})` at `:451-456`; timeout validation `resolveTimeoutMs` `:26-40`.

**The extension-facing return** —
[`core/extensions/types.ts:1117-1122`](../../../tmp/pi/packages/coding-agent/src/core/extensions/types.ts):

```ts
export interface UserBashEventResult {
	/** Custom operations to use for execution */
	operations?: BashOperations;
	/** Full replacement: extension handled execution, use this result */
	result?: BashResult;
}
```

subscribed at `types.ts:1278`.

**Three consumption sites, all re-verified:**

- [`agent-session.ts:2955-2985`](../../../tmp/pi/packages/coding-agent/src/core/agent-session.ts)
  `executeBash(command, onChunk?, options?)`; the resolution is **`:2972`**
  `options?.operations ?? createLocalBashOperations({ shellPath })`, handed to
  `executeBashWithOperations`.
- [`modes/rpc/rpc-mode.ts:563-583`](../../../tmp/pi/packages/coding-agent/src/modes/rpc/rpc-mode.ts)
  `case "bash"`: short-circuits on `eventResult?.result` (`:571-576`), otherwise
  `operations: eventResult?.operations` at **`:581`**.
- [`modes/interactive/interactive-mode.ts:6471-6495`](../../../tmp/pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts)
  — the `!`/`!!` path: `{ excludeFromContext, operations: eventResult?.operations }` at **`:6494`**,
  with `bashComponent.appendOutput(chunk)` **per delta** at **`:6490`** (this is why §4.2 matters).

**What the backend actually receives on this seam.**
[`core/bash-executor.ts`](../../../tmp/pi/packages/coding-agent/src/core/bash-executor.ts):
`executeBashWithOperations` `:50-56` calls `operations.exec(command, cwd, { onData, signal })` at
**`:108-110`** — **no `timeout`, no `env`**. Sanitization lives in the *caller's* `onData` wrapper
`:78-105` (`sanitizeBinaryOutput(stripAnsi(decoder.decode(data, {stream:true}))).replace(/\r/g,"")`
at `:82`), **never** in the backend. Only the agent-loop tool path passes all four options
(`bash.ts:451-456`).

**What a real extension overrides.** All three shipped examples, in
[`examples/extensions/`](../../../tmp/pi/packages/coding-agent/examples/extensions):

| example | `BashOperations` factory | `user_bash` handler | also re-registers built-in tools |
|---|---|---|---|
| `ssh.ts` | `createRemoteBashOps` `:81` | `:203-206` → `{ operations }` | `registerTool` at `:128`, `:142`, `:156`, `:170` |
| `sandbox/index.ts` | `createSandboxedBashOps` `:132` | `:229-231` | `:214` (with `operations:` at `:223`) |
| `gondolin/index.ts` | `createGondolinBashOps` `:324` | `:517-519` | `:443`,`:454`,`:465`,`:476`,`:487`,`:498`,`:509` |

So the concrete override is **one method** that shells out to ssh / a sandbox / a VM instead of
spawning locally, streams combined stdout+stderr as raw `Buffer`s through `onData`, honours an
`AbortSignal`, and returns an exit code (`null` = killed).

## 2. Verified state of cyrup

### 2.1 Host + consumer halves are COMPLETE

- [`cyrup_tools::ops::BashOperations`](../../../crates/cyrup-tools/src/ops/mod.rs) `:597-611`
  (`exec(&self, command, cwd, opts) -> Result<ExitStatus, ToolError>`), `BashExecOptions` `:550-556`
  (`on_data: &mut dyn FnMut(&[u8])`, `cancel`, `timeout`, `env`, `env_remove`),
  `LocalBashOperations` `:624` with `::new` / `::with_proc`.
- [`BashOptions::operations: Option<Arc<dyn BashOperations>>`](../../../crates/cyrup-session-svc/src/bash.rs)
  `:99`; `run_bash(…, operations, …)` `:148-151` takes pi's `??` whole, with the override arm at
  `:195-216`.
- [`AgentSession::execute_bash`](../../../crates/cyrup-session-svc/src/session/bash.rs) `:62`
  resolves it at `:115-121`; `execute_bash_with_user_event` `:166-177` forwards it.
- Pinned by `round9_l5res.rs:613` / `:684` / `:719`.

### 2.2 The guest half is ABSENT, and the key is silently discarded

- [`world.wit:351`](../../../crates/cyrup-ext/wit/world.wit):
  `on-user-bash: func(command: string, exclude-from-context: bool, cwd: string) -> hook-outcome;`
  — a `hook-outcome` only. `interface registration` (`:502-546`) has **no** `register-bash-operations`;
  `interface events` (`:191`) has **no** bash dispatch export. The world's own comment at `:348-350`
  says the `operations`/`result` override "is returned via the `handled` outcome … NOT passed in".
- The reduction *can* carry it: `UserBashReduction::Handled(Value)`
  ([`facade.rs:57-61`](../../../crates/cyrup-ext/src/facade.rs), produced at `:968`), pinned by
  `payload_and_seam_parity.rs:996`.
- **Nothing reads it.** The production path does not even go through the facade helper:
  `emit_user_bash_event` ([`session/bash.rs:191-213`](../../../crates/cyrup-session-svc/src/session/bash.rs))
  calls `dispatcher().dispatch_block_mutate` directly and then reads **only** `handled.0.get("result")`
  (`:206-211`). `ExtensionHost::emit_user_bash` (`facade.rs:950`) has **no production caller** —
  `grep -rn 'emit_user_bash\b' crates/` finds it only in
  `crates/cyrup-it/tests/ext/wasm_dispatch.rs:121/:126` and `payload_and_seam_parity.rs:1004`.
  *(This corrects the prior augmentation's §5c, which wired the fix into the facade helper.)*
- Both remaining construction sites pass a literal `operations: None`:
  [`cyrup-modes/src/rpc/mod.rs:1111`](../../../crates/cyrup-modes/src/rpc/mod.rs) and
  [`cyrup-session-svc/src/command.rs:148`](../../../crates/cyrup-session-svc/src/command.rs).
- The design is already drafted in
  [`cyrup-ext/src/lib.rs:76-120`](../../../crates/cyrup-ext/src/lib.rs) (DRIFT-004 / SEAM-015).
  §3 corrects it in three places.

### 2.3 The gap is NARROWER than the marker text claims — two verified facts

**(a) A guest can already redirect the agent-loop `bash` tool.**
`ExtensionRegistry::active_tools` (`registry.rs:1174-1176`): *"Merge a base tool set (built-ins) with
extension tools; extension tools override by name (R-08-012/014)."* Pinned by
`native_dispatch.rs:473`. The SDK ships the descriptor
([`tool_factory.rs:19-34`](../../../crates/cyrup-ext-sdk/src/tool_factory.rs), whose doc says *"The
author supplies the executor (the guest runs `ctx.exec(...)` against its granted exec
capability)"*), and the guest has real execution capability: `interface exec` (`world.wit:799-802`)
and `interface proc` (`world.wit:817-841`: `spawn`/`write-stdin`/`read-stdout`/`read-stderr`/
`poll-exit`/`kill`), both behind `capabilities.exec`.

So the `registerTool({…})` half of all three pi examples — the *majority* of each file — **is
expressible in cyrup today**. What is **not** expressible is the `user_bash` half: the interactive
`!`/`!!` command and the JSON-RPC `bash` command.

**(b) pi's fs-side seams need no round-trip at all.** `ReadOperations`
([`read.ts:49`](../../../tmp/pi/packages/coding-agent/src/core/tools/read.ts)), `WriteOperations`
(`write.ts:31`), `EditOperations` (`edit.ts:96`), `LsOperations` (`ls.ts:37`), `GrepOperations`
(`grep.ts:56`), `FindOperations` (`find.ts:55`) are supplied **only** through tool-factory options,
never through an extension event result: `grep -n 'operations' core/extensions/types.ts` returns
exactly `:1119`, and it is `BashOperations`. Fact (a) already covers them.

**Consequence:** the blast radius is the `!` / `!!` / JSON-RPC-`bash` path, not "no ssh extensions".
That narrows the item; it does not excuse it, because the failure there is **silent**: the handler
returns `{ operations }`, cyrup reads the key, discards the meaning, and runs the user's command on
the **local host shell** with no error and no observable difference from success. For a sandbox
extension that is a containment breach; for an ssh extension it is a command run on the wrong
machine. `cyrup-ext/src/lib.rs:89-91` already says exactly this.

### 2.4 ADR-0002 is not the blocker — it *mandates* the fix

[`docs/adr/ADR-0002-extension-io-is-serde.md`](../../../docs/adr/ADR-0002-extension-io-is-serde.md)
(accepted 2026-08-13). **Decision** (`:127-129`): *"Every value that crosses the extension boundary
crosses as a value, not as a reference."*

- **Rule 4** (`:147-152`): *"Where pi passes or returns a function, port it as a WIT export plus …
  a matching import. Registration splits into `register-X(key)` (import) + a keyed dispatch export."*
  `BashOperations` is a one-method object, i.e. a function. This is rule 4 verbatim.
- **Rule 6** (`:158-162`): a live signal becomes a poll, never a blocking await.
- **Rule 7** (`:163-174`): *"The encoding is never a licence to drop a field."*
- **Rule 9** (`:184-186`): a new export bumps the minor; **added imports are additive**.
- **Rule 10** (`:188-193`): the WIT world is the contract for both tiers.

Rule 4 mandates the shape and rule 7 forbids the current omission: **this is compliance debt against
an accepted architecture, not an architecture question.** Rejected alternative **A** (`:305-313`,
WIT `resource` types) would not help — resources let a *guest hold a host object*, whereas
`BashOperations` is a **guest-owned object the host calls**. Rejected alternative **D** (`:331-336`)
is this exact category, and its stated cost applies verbatim: *"Each fails **silently**, keyed on
user configuration."*

**Do not reopen ADR-0002 for this item.**

---

## 3. REQUIRED IMPLEMENTATION PATH (single; no alternatives)

One change set, landing `crates/cyrup-ext` + `crates/cyrup-ext-sdk` + a one-field wiring change in
`crates/cyrup-session-svc`, carrying the `HOST_WORLD` minor bump. Effort **M**.

### 3.1 WIT — `crates/cyrup-ext/wit/world.wit` **and** `crates/cyrup-ext-sdk/wit/world.wit` (byte-identical)

**(i) `interface types`** — add, next to `record exec-result` (`world.wit:140-145`), which is the
existing fixed-shape precedent under rule 1:

```wit
// cyrup `ops::ExitStatus` (crates/cyrup-tools/src/ops/mod.rs:428). NOT an s32: pi collapses cancel
// and timeout into `exitCode: null` (bash.ts:79) and re-derives them from the signal afterwards;
// cyrup deliberately keeps them apart (ops/mod.rs:600-603) and the seam must not undo that.
variant exit-status { code(s32), signaled, killed, timed-out }
```

**Correction 1 vs the `lib.rs:96-105` draft: the return cannot be a bare `s32` / `result<s32,string>`.**
`BashOperations::exec` returns `Result<ExitStatus, ToolError>` and `ops/mod.rs:600-603` is explicit
that `Killed` (cancel) and `TimedOut` stay distinguishable. `Signaled` must be in the variant too —
`ops/mod.rs:423-426` records that a process killed by an external signal is treated as **success**
with output preserved (pi `bash.ts:405`), which is a third state, not a synonym for `Killed`.

**(ii) `interface registration`** — add next to `register-markdown-transformer` (`world.wit:517`):

```wit
// pi `UserBashEventResult.operations` (core/extensions/types.ts:1117-1122 @v0.84.3; the interface
// itself at core/tools/bash.ts:63-88). Argument-less for the same reason
// `register-markdown-transformer` is: upstream keeps at most one per handler result, the closure
// stays guest-side, and the host reaches it through the `bash-operations-exec` EXPORT — this
// import only declares that this guest HAS one.
register-bash-operations: func();
```

**(iii) `interface events`** — add the keyed dispatch export:

```wit
// Called only on a guest that declared `registration.register-bash-operations`.
// `env-json` is {"env": [[k,v],…], "envRemove": [k,…]} — see the env note below.
// HARD CONSTRAINT, and it belongs here rather than in prose: the guest's body MUST be
// "call a blocking host import, come back" (`proc.spawn`/`read-stdout`/`poll-exit`, or
// `exec.run`). Guest CPU is charged against the 5s epoch budget and is NOT forgiven; only time
// blocked inside a host import is (`GuestState::take_dialog_extra_ticks`, host/services.rs:2208).
// A guest that busy-loops here traps with EpochTimeout on any command longer than ~5s.
bash-operations-exec: func(call-id: string, command: string, cwd: string,
                           env-json: string, timeout-ms: option<u64>) -> result<exit-status, string>;
```

**(iv) `interface host-tool`** (or a sibling; it is the block that already owns per-call-id
streaming, `world.wit:912-918`) — the two imports the streaming and cancel halves need:

```wit
// pi `onData: (data: Buffer) => void` (bash.ts:73). RAW combined stdout+stderr.
emit-bash-output:  func(call-id: string, chunk: list<u8>);
// pi `signal?: AbortSignal` (bash.ts:74) as the rule-6 poll — the same substitution
// `ctx-state.is-run-cancelled` (world.wit:1039) and `host-tool.is-cancelled` (:915) already make.
is-bash-cancelled: func(call-id: string) -> bool;
```

**Correction 2 vs the draft: `chunk` must be `list<u8>`, NOT the `chunk-json: string` of
`host-tool.emit-update` (`world.wit:913`).** pi's `onData` takes a `Buffer`, and sanitization happens
in the **caller's** wrapper (`bash-executor.ts:78-105`, the `sanitizeBinaryOutput(stripAnsi(…))` line
at `:82`) — never in the backend. cyrup ports that contract deliberately: `on_data: &mut dyn
FnMut(&[u8])` (`ops/mod.rs:551`) with the sanitize/rolling-buffer/spill pipeline owned by
`run_bash`'s single shared sink (`cyrup-session-svc/src/bash.rs:189-194`). A JSON string forces a
lossy UTF-8 coercion at the seam and moves sanitization to the wrong side.

**Correction 3 vs the draft: the draft's `bash-operations-exec(call-id, command, cwd, env-json)`
omits `timeout`.** Rule 7 requires it, and `BashExecOptions` (`ops/mod.rs:550-556`) carries
`timeout` and `env_remove`. Note precisely what is and is not live on this seam today:
`run_bash`'s override arm hardcodes `timeout: None, env_remove: Vec::new()`
(`cyrup-session-svc/src/bash.rs:196-211`), matching `executeBashWithOperations`, which passes only
`{onData, signal}` (`bash-executor.ts:108-110`) — so those two are **type-level completeness** for
the shared trait, carried but structurally empty here. **`env` is NOT empty and is load-bearing**:
`run_bash` builds `shell_env(bin_dir)` and pushes `PI_CODING_AGENT=true` and `AI_AGENT=cyrup`
(`bash.rs:161-183`). A guest backend that does not receive those makes a guest-backed `!` observably
different from a local `!` — a regression *created by* the fix if `env-json` is skipped.

**ABI:** the export is new, so `HOST_WORLD` goes `cyrup:ext@0.8` → **`cyrup:ext@0.9`**
(`manifest.rs:233`), rule 9. Read `world.wit:56-62` before scheduling: `check_world` accepts a guest
whose minor is **≥** the host's, so the bump moves the host's floor and **invalidates every
already-built guest at `0.8`**. That cost only grows; it is cheapest now. The three imports alone
would be additive and free — **and worthless**: see §5.

### 3.2 Host — `crates/cyrup-ext`

Add `GuestBashOperations { ext: Arc<LiveExtension>, … }` implementing
`cyrup_tools::ops::BashOperations`, dispatching exactly the way `LiveExtension::execute_tool`
(`host/live.rs:1416-1445`) does:

1. `let mut guard = self.inner.lock().await;`
2. `inner.store.set_epoch_deadline(self.epoch_ticks); self.guest.arm_epoch_deadline_estimate(self.epoch_ticks);`
3. bind the call's `CancelToken` so `is-bash-cancelled` reads live state;
4. **copy `ToolCallBinding` (`host/live.rs:1281-1312`) as a `BashCallBinding`** and drop the binding
   under it, declared *after* the `inner` guard so it unwinds first. Its doc records **EXT-M06**: a
   dropped or cancelled call left its queued chunks for the *next* call to drain. The same bug is
   reachable here by construction — the chunk queue is instance-scoped and the sink is call-scoped.
   Filter the replay by `call-id` the way `take_tool_updates_for` does (`live.rs:1451-1453`).

Wrap the `emit-bash-output` / `is-bash-cancelled` / `proc` / `exec` host handlers in the
`Instant::now()` + `guest.note_dialog_wait(started)` pair (`host/services.rs:2173`), exactly as
`host/live.rs` already does at `:633`, `:707`, `:727`, `:757`, `:786`. Without it the 5-second epoch
budget (`WASM_EPOCH_BUDGET_TICKS = 1000` at `facade.rs:2205` × `DEFAULT_TICK = 5ms` at
`host/epoch.rs:19`) kills any command longer than five seconds. `take_dialog_extra_ticks`
(`services.rs:2208`) forgives only time blocked in a host import, never guest CPU
(`services.rs:2432`).

### 3.3 Wiring — `crates/cyrup-session-svc`

`emit_user_bash_event` (`session/bash.rs:191-213`) currently returns `Option<BashResult>` from
`handled.0.get("result")`. Widen its return to carry **both** halves (e.g.
`Option<UserBashOverride>` with a `result` and an `operations` arm), reading `handled.0.get(
"operations")` and, when the extension that produced the reduction registered one, constructing the
`GuestBashOperations` proxy for it. `execute_bash_with_user_event` (`session/bash.rs:166-177`) then
sets `options.operations` before delegating to `execute_bash` — **exactly as its own doc predicts at
`session/bash.rs:165`: *"Once it exists this wrapper sets one field."*** Nothing in
`execute_bash`/`run_bash` changes.

The dispatcher must surface **which** extension produced the `Handled` value, since the proxy is
per-extension; `Reduced::Handled` carries `HandledValue` only (`facade.rs:968`), so thread the
`ExtensionId` the way `Reduced::Blocked { by, .. }` already does.

Replace the two literals: `cyrup-modes/src/rpc/mod.rs:1111` and
`cyrup-session-svc/src/command.rs:148`.

### 3.4 Guest SDK — `crates/cyrup-ext-sdk` (MUST land in the same change)

The template is `register_markdown_transformer`, the most recent instance of this exact pattern:

- [`src/api.rs`](../../../crates/cyrup-ext-sdk/src/api.rs) — an author-facing `BashOperations` trait
  + `register_bash_operations(&mut self, …)` (mirror `api.rs:732`) + the dispatch entry point
  (mirror `transform_markdown`, `api.rs:738`), and a `has_bash_operations()` predicate.
- [`src/guest.rs`](../../../crates/cyrup-ext-sdk/src/guest.rs) — call
  `registration::register_bash_operations()` in the init sweep behind that predicate (mirror
  `guest.rs:159-161`) and add the export body (mirror `transform_markdown`, `guest.rs:323-329`).
- [`src/macros.rs`](../../../crates/cyrup-ext-sdk/src/macros.rs) — the wit-bindgen shim (mirror
  `macros.rs:95-99`).

Keep both `wit/world.wit` copies byte-identical (`crates/cyrup-ext/src/tests/wit_world_sync.rs`
already enforces this).

### 3.5 Correct the marker text as part of this change

`ops/mod.rs:582-590` and `cyrup-ext/src/lib.rs:76-120` currently imply an ssh-style extension is not
expressible **at all**. §2.3 disproves that. Both must be narrowed to "the `!` / `!!` / JSON-RPC-`bash`
path", citing `registry.rs:1174-1176` and `native_dispatch.rs:473`. The stale `@v0.83.0` citations in
those two comments must move to `@v0.84.3` (§6.1) at the same time.

## 4. What this does NOT close — record it, do not let it vanish

### 4.1 (nothing on the agent-loop path) — already expressible, §2.3.

### 4.2 Streamed output will be BATCHED, not live

`host-tool.emit-update` chunks are queued on `GuestState` and replayed by `execute_tool` **after the
call settles** (`host/live.rs:1281-1296` doc; replay at `:1451-1470`). The instance is held under
`self.inner.lock().await` (`live.rs:1424`) for the whole call, so the host cannot drain while the
guest is inside. pi renders `!` output **live** (`interactive-mode.ts:6490`
`bashComponent.appendOutput(chunk)` per delta), and cyrup's local path does too via
`spawn_event_pump` / `BashExecutionUpdate` (`session/bash.rs:106-118`).

So a guest-backed `!` will show nothing until the command finishes. Draining while a guest call is
in flight requires restructuring `LiveExtension`'s instance mutex, which also touches `execute_tool`
— out of proportion to this item.

**This is not a descope to be argued: it is a new, smaller divergence created by the fix.** When
§3 lands, file a `[CYRUP-DELTA]` marker for it in `crates/cyrup-ext/src/lib.rs`'s register naming
`host/live.rs:1281-1296` and `interactive-mode.ts:6490`, per ADR-0002 rule 7's mandatory-note clause
(`:169-174`). Closing this item silently over it is exactly how this backlog was created.

### 4.3 Wrap-then-delegate stays unavailable

pi exports `createLocalBashOperations` to extensions (`index.ts:290`) *specifically* so a `user_bash`
interceptor can wrap-then-delegate — its JSDoc `:153-157` names the case. cyrup's
`LocalBashOperations` (`ops/mod.rs:624`) is in-host Rust only. Even after §3, a cyrup guest can
*replace* the backend but not *wrap the local one*; a sandbox/logging extension that wants to observe
local execution must reimplement the shell path. Separable, additive (`exec`/`proc` already exist),
**not covered by any existing item** — file it.

### 4.4 `BashToolOptions.commandPrefix`

`bash.ts:201-202`, applied at `:344`/`:362`, sits in the same options bag as `operations` and has no
guest-reachable analog. Flagged so it is not later discovered as part of "the bash options bag".

## 5. Explicitly forbidden partial landing

**Do not ship the three imports without the export.** They are additive and cost no bump, and they
buy **nothing** — an import with no dispatch export is unreachable.
[`crates/cyrup-ext-sdk/src/tests/world_import_coverage.rs`](../../../crates/cyrup-ext-sdk/src/tests/world_import_coverage.rs)
exists precisely because that state has already happened twice (EXT-M04 `ui.unsubscribe-terminal-input`,
EXT-M05 `provider-stream.on-payload`/`on-response`); it is structural over the world text and will
fail on `register-bash-operations` / `emit-bash-output` / `is-bash-cancelled` until §3.4 lands. **Do
not suppress it.**

## 6. Findings recorded in passing (not descopes)

1. **Citation staleness against the pinned tree.** The in-source pi citations for this seam are
   tagged `@v0.83.0`; the pinned tree is v0.84.3, and several now resolve to a **different symbol**:
   `bash.ts:82` (cited as `createLocalBashOperations`, `ops/mod.rs:615`) is `createLocalShellOperations`
   at `:84`; `types.ts:1078-1083` is `:1117-1122`; `agent-session.ts:2782` is `:2972`;
   `rpc-mode.ts:576` is `:581`; `bash.ts:52-73` is `:63-88`; `bash.ts:186-188` is `:198-200`;
   `bash.ts:451` (`tool_factory.rs:17`) is `:536`; `bash.ts:20-38` is `:26-40`; `bash.ts:100` is
   `:102`; `bash.ts:64-71` is `:71-80`; `index.ts:281` is `:290`; `bash.ts:158-184`
   (`resolveSpawnContext`) needs re-derivation at `e8682309`. Affected: `cyrup-tools/src/ops/mod.rs`,
   `cyrup-ext/src/lib.rs`, `cyrup-ext/wit/world.wit`, `cyrup-session-svc/src/{bash.rs,session/bash.rs}`,
   `cyrup-modes/src/rpc/mod.rs`, `cyrup-ext-sdk/src/tool_factory.rs`. The world's own EXT-036 note
   (`world.wit:524-531`) says an unversioned/mismatched citation *"reads as a fabrication to the next
   auditor"*; this pass spent real time disproving three of them.
2. **ADR-0002's version framing is stale**: it is written at `0.4.0` and speaks of "batch 19, the
   single `0.5.0` bump" (`:257-302`, `:347-359`) against a tree at `cyrup:ext@0.8.0`. Do not schedule
   off its batch numbers. Its `world.wit` line citations (`:392-413`, `:423-460`, `:492-500`,
   `:523-540`) are also stale against the current 78 KB world.
3. **`ExtensionHost::emit_user_bash` (`facade.rs:950`) is dead on the production path** — only tests
   call it, while `session/bash.rs:191` reimplements the dispatch. Either route the session through
   the facade or delete the facade helper; two divergent copies of the `user_bash` reduction are how
   the `operations` key came to be dropped in one of them.

## 7. Guard

One guard, and it fails today:

**`bash_operations_registered_by_a_wasm_guest_redirects_the_user_bash_command`**, in
`crates/cyrup-it/tests/ext/` (alongside `wasm_dispatch.rs`, which already drives a live `.wasm`
guest).

A WASM guest fixture calls `register_bash_operations`, returns `{"operations": {}}` from its
`user_bash` handler, and implements `bash-operations-exec` by emitting the literal `remote` through
`emit-bash-output` and returning `exit-status::code(0)`. Drive a command through
`AgentSession::execute_bash_with_user_event` whose **local** execution would produce `local`. Assert
`BashResult.output == "remote"`.

Today this yields `local`. It must be a **WASM guest** fixture, not a native: ADR-0002 rule 10
(`:188-193`) makes the WIT world the contract, and a native-only proof would prove the wrong tier.

Existing coverage that must keep passing unchanged: `round9_l5res.rs:613`/`:684`/`:719`,
`payload_and_seam_parity.rs:996`, `native_dispatch.rs:473`, `wit_world_sync.rs`.

---

## Definition of done

1. **`bash-operations-exec` exists as an export in both byte-identical `world.wit` copies**, with
   `register-bash-operations`, `emit-bash-output(call-id, chunk: list<u8>)` and
   `is-bash-cancelled(call-id)` as imports, an `exit-status` variant carrying
   `code`/`signaled`/`killed`/`timed-out`, and `HOST_WORLD == "cyrup:ext@0.9"` in
   `crates/cyrup-ext/src/manifest.rs`.
2. **A WASM guest's `{"operations": …}` reaches execution.** `emit_user_bash_event`
   (`crates/cyrup-session-svc/src/session/bash.rs:191`) reads the `"operations"` key, builds the
   `GuestBashOperations` proxy for the producing extension, and `execute_bash_with_user_event` sets
   `BashOptions::operations`. `cyrup-modes/src/rpc/mod.rs:1111` and
   `cyrup-session-svc/src/command.rs:148` no longer pass a literal `None`.
3. **The guard in §7 passes**, and fails on the parent commit.
4. **`crates/cyrup-ext-sdk/src/tests/world_import_coverage.rs` passes unsuppressed** — i.e. all three
   new imports have SDK callers (§3.4).
5. **The proxy cannot leak chunks across calls**: a `BashCallBinding` modelled on
   `ToolCallBinding` (`crates/cyrup-ext/src/host/live.rs:1281-1312`) clears the queue and unbinds the
   cancel token on every exit path, including the dropped-future path.
6. **`env` survives the seam**: `PI_CODING_AGENT` and `AI_AGENT`
   (`crates/cyrup-session-svc/src/bash.rs:161-183`) reach the guest backend, so a guest-backed `!` is
   not observably different from a local `!` in its child environment.
7. **The overclaiming marker text is corrected** at `crates/cyrup-tools/src/ops/mod.rs:582` and
   `crates/cyrup-ext/src/lib.rs:76-120` (§3.5), and **the batched-output residual (§4.2) is filed as
   its own `[CYRUP-DELTA]` register entry** in the same change.
8. **No regression** in `round9_l5res.rs:613`/`:684`/`:719` or `payload_and_seam_parity.rs:996`.

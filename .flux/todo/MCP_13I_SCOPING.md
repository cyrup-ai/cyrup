---
stage: exec
status: done
updated: 2026-08-22 20:05
---

# Scope 13i — Protocol Tracer, Conformance And Verification

## Objective

`13i` is the weakest surface in the port. The ledger records **50 units, 31 missing, 11 partial**
([13-cyrup-mcp-STATUS.md:933](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)), and 16 of its 42 open
units are critical-or-high ([13-cyrup-mcp-STATUS.md:381](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)).

This is a **scoping task**. Every other open MCP section realigns something that already exists; 13i
means building absent surfaces, which is a different job and must not be started by picking units off
a list. **The deliverable is a plan. No code.**

The triage below is the starting table, not the answer — it was produced by reading the ledger and
spot-checking each row against the tree at the timestamp in the frontmatter. Files under
`crates/cyrup-mcp/src/` are being changed by concurrent work; re-confirm any row before scheduling it.

## Sources

| what | where |
|---|---|
| the section spec, 50 units | [13i-mcp-protocol-and-verification.md](../../docs/gap-analysis/13i-mcp-protocol-and-verification.md) |
| the per-unit status ledger (13i at :931-:986) | [13-cyrup-mcp-STATUS.md](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |
| ADRs 0021 / 0022 / 0027, guardrail rationale | [MCP-PORT-METHODOLOGY.md](../../docs/gap-analysis/MCP-PORT-METHODOLOGY.md) |
| the G1-G5 gates, the test-target cap | [TEST-ARCHITECTURE.md](../../docs/TEST-ARCHITECTURE.md) |
| the crate under triage | [crates/cyrup-mcp/src/](../../crates/cyrup-mcp/src/) |

---

## 1. The 50 units, recorded status and spot-check verdict

Ledger rows run [13-cyrup-mcp-STATUS.md:937](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) (`MCP-450`)
through `:986` (`MCP-499`), one line per unit in id order. Spec unit headings are cited per row.

**Spot-check protocol used.** For every `missing` row: grep the crate for the upstream TypeScript
identifier *and* for at least one plausible Rust renaming (`camelCase` → `snake_case`, the literal
message strings, and the rmcp type the unit would have to touch). A row is recorded `confirmed
missing` only when both searches came back empty.

### 1.1 Sampling — `MCP-450` … `MCP-459`

| id | sev | recorded | spot-check | evidence |
|---|---|---|---|---|
| `MCP-450` | high | missing | **confirmed missing** | no `sampling.rs`; no `handle_sampling_request`, no `SamplingOptions`. The hook *type* exists unproduced at [runtime.rs:1388](../../crates/cyrup-mcp/src/runtime.rs) (`pub type SamplingHook`). Spec [:861](../../docs/gap-analysis/13i-mcp-protocol-and-verification.md) |
| `MCP-451` | medium | missing | **confirmed missing** | zero occurrences of `not supported` anywhere in `crates/cyrup-mcp/src/`. Spec [:874](../../docs/gap-analysis/13i-mcp-protocol-and-verification.md) |
| `MCP-452` | high | missing | **confirmed missing** | no `sampling_candidates` / `resolve_sampling_model` / any candidate-ordering function. Spec [:890](../../docs/gap-analysis/13i-mcp-protocol-and-verification.md) |
| `MCP-453` | high | missing | **confirmed missing** | no `cyrup_provider` completion call anywhere in the crate. Spec [:903](../../docs/gap-analysis/13i-mcp-protocol-and-verification.md) |
| `MCP-454` | medium | missing | **confirmed missing** | `current_model` exists only as a fenced delegation with no consumer, [owner.rs:425](../../crates/cyrup-mcp/src/owner.rs); no `builtin_catalog` / `load_catalog` reference. Spec [:921](../../docs/gap-analysis/13i-mcp-protocol-and-verification.md) |
| `MCP-455` | **critical** | missing | **OVERTURNED → implemented, unwired** | see §2.1 |
| `MCP-456` | medium | missing | **confirmed missing** | no `convert_sampling_message` / `convert_assistant_result` / `map_stop_reason`. [owner.rs:793](../../crates/cyrup-mcp/src/owner.rs) `message_text` is a *dialog* formatter, not the converter. Spec [:952](../../docs/gap-analysis/13i-mcp-protocol-and-verification.md) |
| `MCP-457` | low | implemented | **holds, but inert** | `build_client_capabilities` at [runtime.rs:1220](../../crates/cyrup-mcp/src/runtime.rs) is correct and is reached only from `bare_handler_factory` ([runtime.rs:1933](../../crates/cyrup-mcp/src/runtime.rs)), which passes `sampling: None`. See §3.1 |
| `MCP-458` | high | missing | **confirmed missing** | no options bag, no live `current_model` closure, no composed child `CancellationToken` for sampling. Spec [:983](../../docs/gap-analysis/13i-mcp-protocol-and-verification.md) |
| `MCP-459` | low | implemented | **holds** | [registration.rs:571](../../crates/cyrup-mcp/src/registration.rs) `pub fn truncate_at_word` |

### 1.2 Elicitation — `MCP-460` … `MCP-472`

| id | sev | recorded | spot-check | evidence |
|---|---|---|---|---|
| `MCP-460` | low | partial | **holds** | `create_elicitation` exists at [runtime.rs:1563](../../crates/cyrup-mcp/src/runtime.rs) and delegates to an `ElicitationHook` ([runtime.rs:1395](../../crates/cyrup-mcp/src/runtime.rs)) that has no producer; no form/url split exists to exercise |
| `MCP-461` | high | missing | **confirmed missing** | no gate dialog, no review loop, no edit picker anywhere in the crate. Spec [:1023](../../docs/gap-analysis/13i-mcp-protocol-and-verification.md) |
| `MCP-462` | low | missing | **confirmed missing** | zero references to `property_order` in `crates/cyrup-mcp/src/` — there is no iteration site yet. Spec [:1038](../../docs/gap-analysis/13i-mcp-protocol-and-verification.md) |
| `MCP-463` | medium | missing | **confirmed missing** | no per-field re-prompt loop; note `McpDialog` has no `input` arm to build it on (§2.3) |
| `MCP-464` | high | missing | **confirmed missing** | no coercion core, none of the 13 message templates present |
| `MCP-465` | high | missing | **confirmed missing, cost reduced** | `jsonschema` is *already* a declared dependency with the exact intent written into the manifest — [cyrup-mcp/Cargo.toml:112-118](../../crates/cyrup-mcp/Cargo.toml) names `should_validate_formats(true)` and `$schema` dispatch — and has **zero** uses in the crate. Shared obligation with `MCP-092`; see §3.3 |
| `MCP-466` | medium | missing | **confirmed missing** | no `format_choice` / `unique_labels` / `extract_multi_select_options` |
| `MCP-467` | high | missing | **confirmed missing, cost reduced** | the handler is absent; the injectable browser-open seam it needs already exists — [oauth.rs:2382](../../crates/cyrup-mcp/src/oauth.rs) `trait BrowserLauncher`, `:2394` `OpenerLauncher`, `:2402` `NoopLauncher`. See §2.4 |
| `MCP-468` | medium | partial | **holds, root cause pinned** | `with_handler_factory` at [runtime.rs:2289](../../crates/cyrup-mcp/src/runtime.rs) has **zero callers**; the installed default is `bare_handler_factory` at [runtime.rs:2282](../../crates/cyrup-mcp/src/runtime.rs). See §3.1 |
| `MCP-469` | medium | partial | **evidence OVERTURNED** | see §2.2 — the registry the ledger reports absent now exists |
| `MCP-470` | medium | partial | **holds; caller half already ported** | [proxy.rs:1302](../../crates/cyrup-mcp/src/proxy.rs) `UrlElicitationAction`, `:1486` the `ProxyEnv` seam, `:3731-3737` the three action-specific messages. Missing: the `-32042` decode + sequential loop on the manager side |
| `MCP-471` | high | missing | **OVERTURNED → partial** | see §2.3 |
| `MCP-472` | low | missing | **confirmed missing** | zero occurrences of `invalid_params` in `crates/cyrup-mcp/src/` |

### 1.3 Tracer — `MCP-473` … `MCP-481`

| id | sev | recorded | spot-check | evidence |
|---|---|---|---|---|
| `MCP-473` | medium | missing | **confirmed missing** | no `McpTraceEvent` type; no serialised trace record of any spelling |
| `MCP-474` | high | missing | **confirmed missing, cost reduced** | no `redact_trace_text`; `regex` is already a dependency, [cyrup-mcp/Cargo.toml:107](../../crates/cyrup-mcp/Cargo.toml), with `LazyLock<Regex>` precedent at [agent_plugin.rs:149](../../crates/cyrup-mcp/src/agent_plugin.rs) |
| `MCP-475` | low | missing | **confirmed missing** | no `trace_id` / `message_kind` / `message_bytes` |
| `MCP-476` | medium | missing | **confirmed missing** | no `TraceWriter` of any spelling |
| `MCP-477` | low | partial | **holds; the open decision is already settled and landed** | `.cyrup` side chosen and in the tree: [dirs.rs:116](../../crates/cyrup-mcp/src/dirs.rs) `TRACE_DIR`, `:203` `trace_dir()`. Missing: the filename derivation (`settings.file` absolute/relative resolution, the ISO-timestamp + base36 fallback) |
| `MCP-478` | low | partial | **holds** | [config.rs:1267](../../crates/cyrup-mcp/src/config.rs) `trace_enabled()` and `:876` `ServerEntry::trace` exist; no combining function, and `trace_enabled()` has no caller |
| `MCP-479` | medium | missing | **confirmed missing** | no `TracingTransport` |
| `MCP-480` | medium | missing | **confirmed missing** | the two absent wiring points are named in place: [server_manager.rs:1431-1434](../../crates/cyrup-mcp/src/server_manager.rs) (no `setTraceConfig` counterpart) and `:2479` (no writer flush on disposal) |
| `MCP-481` | low | partial | **holds** | [config.rs:1620](../../crates/cyrup-mcp/src/config.rs) `TraceSettings`, `:1273` / `:1279` the caps — no consumer for any of them |

### 1.4 Conformance and verification — `MCP-482` … `MCP-499`

| id | sev | recorded | spot-check | evidence |
|---|---|---|---|---|
| `MCP-482` | n/a | implemented | **holds** | index unit, no code obligation |
| `MCP-483` | high | missing | **confirmed missing** | there is no `.github/` directory in the repository at all |
| `MCP-484` | high | missing | **confirmed missing, cost reduced** | no driver; the hidden-subcommand pre-dispatch pattern it should copy exists twice — [subagent_runner_cmd.rs:61](../../crates/cyrup/src/subagent_runner_cmd.rs) + [main.rs:115](../../crates/cyrup/src/main.rs), and [intercom_broker_cmd.rs:36](../../crates/cyrup/src/intercom_broker_cmd.rs) + [main.rs:131](../../crates/cyrup/src/main.rs) |
| `MCP-485` | medium | missing | **confirmed missing** | no runner, no results dir, no `expected-failures` file |
| `MCP-486` | medium | missing | **confirmed missing** | no baseline file exists to have been copied — the safe starting state |
| `MCP-487` | low | missing | **confirmed missing; may dissolve** | ADR-0022 ([MCP-PORT-METHODOLOGY.md:1199](../../docs/gap-analysis/MCP-PORT-METHODOLOGY.md)) recommends the runner shape that removes the ephemeral-port probe entirely |
| `MCP-488` | n/a | implemented | **holds** | record-only unit |
| `MCP-489` | medium | not-applicable | **not work until ruled** | needs the test-only `node` ruling and the `rmcp/server` dev-dependency ruling |
| `MCP-490` | high | partial | **holds** | 613 `#[test]` / `#[tokio::test]` items exist across `crates/cyrup-mcp/src/*.rs`; **zero** touch sampling, elicitation or tracing, because none of that code exists |
| `MCP-491` | medium | partial | **premise OVERTURNED** | see §2.5 |
| `MCP-492` | high | partial | **holds; citation drifted** | the ledger cites `oauth.rs:4797`; the test is now [oauth.rs:4655](../../crates/cyrup-mcp/src/oauth.rs) `the_callback_listener_end_to_end`. Re-derive line numbers before scheduling |
| `MCP-493` | low | missing | **confirmed missing; cheapest unit in the section** | no manifest-parsing test; `toml` is a normal dependency at [cyrup-mcp/Cargo.toml:111](../../crates/cyrup-mcp/Cargo.toml) and the rmcp block it must pin is `:70-76` (`default-features = false`, five features, `server` and `elicitation` absent) |
| `MCP-494` | medium | not-applicable | **not work until ruled; nothing to build on** | no `.github/` exists |
| `MCP-495` | medium | partial | **holds, and harder than recorded** | `cyrup-test-support` has **no `env` module at all** — `crates/cyrup-test-support/src/` contains `auth, differential, golden, harness, interop, lib, messages, response, scripted, tempdir, tool_ext, tree, tui`. Both doc references ([TEST-ARCHITECTURE.md:612-613](../../docs/TEST-ARCHITECTURE.md), `:650-657`) are dangling |
| `MCP-496` | high | missing | **confirmed missing** | no pty crate in any workspace manifest. [cyrup-test-support/src/tui.rs:14](../../crates/cyrup-test-support/src/tui.rs) `TestTerminal` is a ratatui `TestBackend`, which [MCP-PORT-METHODOLOGY.md:1229-1230](../../docs/gap-analysis/MCP-PORT-METHODOLOGY.md) declares inadmissible |
| `MCP-497` | n/a | not-applicable | **holds** | `cut` |
| `MCP-498` | medium | missing | **confirmed missing, and now UNBLOCKED** | see §4 |
| `MCP-499` | medium | not-applicable | **not work until ruled; one spec detail is wrong** | the spec at [:1658](../../docs/gap-analysis/13i-mcp-protocol-and-verification.md) names `run_differential` as existing. It does not: [differential.rs](../../crates/cyrup-test-support/src/differential.rs) provides `diff_sequences` (`:36`), `normalized_jsonl` (`:56`), `diff_normalized` (`:71`), `canonicalize_cross_impl` (`:96`) and nothing named `run_differential` |

### 1.5 Triage totals

| | ledger | after spot-check |
|---|---:|---:|
| missing | 31 | **29** |
| partial | 11 | **13** |
| implemented | 4 | **5** |
| not-applicable | 4 | 4 |
| **open work** | 42 | **42** |

Two `missing` rows were overturned (`MCP-455` → implemented, `MCP-471` → partial). Two `partial`
rows had their supporting evidence overturned without changing the verdict (`MCP-469`, `MCP-491`).
One row carries a stale citation (`MCP-492`) and one spec sentence is wrong (`MCP-499`).

---

## 2. The overturned rows, in full

### 2.1 `MCP-455` is implemented — it is only unwired

Ledger row [:942](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) records `missing` and `critical`.
Every obligation the spec lists at [:935-:950](../../docs/gap-analysis/13i-mcp-protocol-and-verification.md)
is in the tree, in [owner.rs](../../crates/cyrup-mcp/src/owner.rs):

* the four literals — `:604` request title, `:607` response title, `:614` the no-UI message, `:618` the decline message;
* the explicit `has_ui: bool` the spec insists must not be inferred from a `false` — `:629` `SamplingApproval`, field documented at `:637-644`;
* the three-branch gate in upstream's order — `:663` `confirm_sampling`, auto-approve short-circuit, no-UI throw, decline throw;
* both formatters — `:696` `format_request_approval` (pluralisation on 1, optional `System:` line, 1-indexed rows, `"\n\n"` join, 400-char `truncate_at_word`), `:740` `format_response_approval` (1000-char budget);
* `messageText` — `:793`;
* both dialogs taken under the interaction lock and the human-wait guard, via `McpDialog` (§2.3);
* unit coverage of the titles and body substrings — `:1101` onward.

What is absent is the **call site**, which belongs to `MCP-450`. Re-file `MCP-455` as implemented and
move its unmet `verify` half (the live-pty render check) onto `MCP-496`, where the spec already puts it.

### 2.2 `MCP-469`'s registry exists — the ledger looked in the wrong file

The ledger row [:956](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) reports no accepted-elicitation
registry in `state.rs`. The registry is on the **manager**:
[server_manager.rs:1224](../../crates/cyrup-mcp/src/server_manager.rs) `accepted_url_elicitations:
HashMap<String, HashSet<String>>`, with `:2582` `remember_url_elicitation` (including the aborted-runtime
no-op), `:2601` `forget_url_elicitation` returning the `bool` that gates the notice, and `:2610`
`has_accepted_url_elicitation`. It is cleared per-server at `:2265` and wholesale at `:2470`.

The same correction applies to `MCP-122`'s 13c row at
[13-cyrup-mcp-STATUS.md:633](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md), which reports the same
registry absent. `server_manager.rs` changed after the audit; treat every `server_manager.rs`-based
`missing` claim in the ledger as needing re-confirmation.

`MCP-469` remains partial. What is genuinely absent: the notice text, and a producer for the
`ElicitationCompleteHook` ([runtime.rs:1383](../../crates/cyrup-mcp/src/runtime.rs); the decode side is
done at `:1617-1645`).

### 2.3 `MCP-471`'s mechanism exists — its coverage is incomplete

Recorded `missing` at [:958](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md). In the tree:
[owner.rs:522](../../crates/cyrup-mcp/src/owner.rs) `McpDialog`, whose `enter()` (`:560-572`) acquires
`HostServices::human_interaction_lock` and `HostCtx::begin_human_wait` together and returns them as a
named guard tuple; `:534` the constructors, including `fenced()` for the owner-fenced handle;
[state.rs:227](../../crates/cyrup-mcp/src/state.rs) `dialog()`, the production constructor.

**The invariant currently holds with no bypass**: the only `HostServices::{confirm, select}` calls in
the whole crate are inside `McpDialog` — `:577` and `:587`.

Two things keep it partial: there is **no `input` arm** (the spec's verb list is `select`/`input`/`confirm`),
and the elicitation call sites that would exercise it do not exist. Adding the `input` arm is small; the
scheduling point is that its shape must be fixed **before** `MCP-463`'s field loop is written, or the
loop will grow its own dialog path.

### 2.4 `MCP-467` inherits a browser-open seam, and inherits a question with it

[oauth.rs:2382](../../crates/cyrup-mcp/src/oauth.rs) already defines an injectable `BrowserLauncher`
with a production `OpenerLauncher` (`:2394`, `opener::open`) and a `NoopLauncher` (`:2402`). The comment
at `:2387-2392` states deliberately that this is **not** the adapter's `openUrl` helper — the adapter's
version dispatches per-platform, honours a `browser` override and `$BROWSER`, and the OAuth site chose
the simpler one on purpose.

So `MCP-467` cannot silently reuse it. Its first scoping decision is: reuse `BrowserLauncher` and record
the divergence, or port `openUrl` properly and migrate the OAuth site onto it. Either answer is fine;
discovering the question during implementation is not.

### 2.5 `MCP-491`'s premise is stale — the MCP seam-test home already exists, and the caps are already breached

* The `mcp` target exists: [cyrup-it/Cargo.toml:198-204](../../crates/cyrup-it/Cargo.toml), pointing at
  [tests/mcp/main.rs](../../crates/cyrup-it/tests/mcp/main.rs), with `tests/mcp/activation.rs` already in it.
* `cyrup-it` declares **eight** `[[test]]` targets (`subagents`, `intercom`, `ext`, `permission`, `mcp`,
  `session_svc`, `bin`, `misc`), not seven.
* **G1 is already violated.** [TEST-ARCHITECTURE.md:1113-1116](../../docs/TEST-ARCHITECTURE.md) requires
  `crates/*/tests/*` to match nothing outside `cyrup-it`. It matches twelve files today, across five
  crates: `cyrup` (3), `cyrup-permission-system` (3), `cyrup-provider` (1), `cyrup-tools` (2),
  `cyrup-tui` (3). None of those crates sets `autotests = false`, so each file is its own target.
* **G2 is already violated.** [TEST-ARCHITECTURE.md:1118-1121](../../docs/TEST-ARCHITECTURE.md) caps the
  workspace at seven; the true count is **20** (8 + 12).
* [MCP-PORT-METHODOLOGY.md:1121-1126](../../docs/gap-analysis/MCP-PORT-METHODOLOGY.md) states "the seven
  are already taken … MCP seam tests meet neither justification". That reasoning is superseded by the
  target that now exists.

`MCP-491` therefore is not "find a home"; it is "reconcile two guardrails with a tree that already
breaks both, and decide whether `MCP-498` lands in `bin` or in the existing `mcp` target". That is an
ADR-0021 ruling ([MCP-PORT-METHODOLOGY.md:1131](../../docs/gap-analysis/MCP-PORT-METHODOLOGY.md)), and it
is smaller than recorded.

---

## 3. Dependency order

### 3.1 The one structural blocker — and it is not a 13i unit

**No handler hook reaches production.** `ConnectionBuilder` installs `bare_handler_factory`
([runtime.rs:2282](../../crates/cyrup-mcp/src/runtime.rs)), which constructs `McpClientHandler` with
`sampling: None, elicitation: None, elicitation_complete: None` (`:1933-1944`). The override,
`with_handler_factory` (`:2289`), has **zero callers**. The manager's setters
`set_sampling_config` ([server_manager.rs:1338](../../crates/cyrup-mcp/src/server_manager.rs)) and
`set_elicitation_config` (`:1343`) also have **zero callers**.

Consequences that must be stated before any wave is scheduled:

* the sampling and elicitation capabilities are never advertised in production, so `MCP-457`
  (implemented) and `MCP-468` (partial) are *inert*, not done;
* every unit in `MCP-450` … `MCP-472` is downstream of one non-13i obligation: the manager supplying a
  production handler factory. That is `MCP-118` / `MCP-120` / `MCP-122` in 13c
  ([13-cyrup-mcp-STATUS.md:629](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md), `:631`, `:633`);
* the tracer (`MCP-473` … `MCP-481`) is **not** downstream of it — it decorates the transport, not the
  handler — and can proceed in parallel from t=0.

### 3.2 13i units that are verification harnesses for other 13i units

| harness | asserts | worthless before |
|---|---|---|
| `MCP-496` (live-pty) | the elicitation dialog sequence and both sampling gates render | `MCP-450`/`MCP-455` wired (§3.1) and `MCP-461`…`MCP-467` |
| `MCP-490` (unit-test share) | 13i's own case-count parity | the code under test exists at all |
| `MCP-499` (trace differential) | the JSONL oracle | the tracer (`MCP-473`…`MCP-480`) **and** an ADR-0027 ruling ([MCP-PORT-METHODOLOGY.md:1222](../../docs/gap-analysis/MCP-PORT-METHODOLOGY.md)) |
| `MCP-485`/`MCP-486` | conformance outcomes | `MCP-484` (the driver) |
| `MCP-481`'s verify line | per-server `false` beats global `true` | `MCP-478`'s combining function |

Three harnesses have **no** 13i dependency and can be scheduled immediately: `MCP-493` (manifest
policy), `MCP-495` (the env-contract reconciliation), `MCP-498` (see §4).

### 3.3 Cross-section shared obligations — the ones file-based grouping would split

* **`MCP-465` ↔ `MCP-092`.** Both need one `$schema`-dispatching JSON-Schema validator with
  `should_validate_formats(true)`. `MCP-092` (13b, high, missing,
  [13-cyrup-mcp-STATUS.md:595](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) owns the dual-dialect
  gate; `MCP-465` needs exactly the same compiled-and-cached validator for the final elicitation
  assertion. The dependency is already declared and unused
  ([cyrup-mcp/Cargo.toml:112-118](../../crates/cyrup-mcp/Cargo.toml)). Whoever takes one takes both, or
  the crate grows two validators that disagree on formats.
* **`MCP-467` ↔ the OAuth browser launcher** (§2.4) — same seam, one ruling.
* **`MCP-471` ↔ `MCP-232`.** [owner.rs:485-489](../../crates/cyrup-mcp/src/owner.rs) records that
  `McpDialog` exists so both consent gates go through one type. `MCP-232` (13e, critical, partial) is the
  other consumer. Adding the `input` arm belongs with whichever lands first.
* **`MCP-469`/`MCP-470` ↔ `MCP-122`** (13c) — same registry, now already built (§2.2).

### 3.4 Open decisions that gate a wave

| decision | ADR | blocks |
|---|---|---|
| seam-test home + merge-gate shape | ADR-0021, [MCP-PORT-METHODOLOGY.md:1131](../../docs/gap-analysis/MCP-PORT-METHODOLOGY.md) | `MCP-491`, `MCP-494`, and where `MCP-498` lands |
| conformance runner shape (sequential vs `--suite all`, `:0` ports) | ADR-0022, [MCP-PORT-METHODOLOGY.md:1199](../../docs/gap-analysis/MCP-PORT-METHODOLOGY.md) | `MCP-485`, and whether `MCP-487` dissolves |
| keep `pi-mcp-adapter` installable in CI | ADR-0027, [MCP-PORT-METHODOLOGY.md:1222](../../docs/gap-analysis/MCP-PORT-METHODOLOGY.md) | `MCP-499` |
| fixture strategy (`node` in tests, `rmcp/server` dev-dep) | `MCP-489`, spec [:1487](../../docs/gap-analysis/13i-mcp-protocol-and-verification.md) | what `MCP-483`…`MCP-486` point at |
| `metadata` pass-through for sampling | spec [:1723-1727](../../docs/gap-analysis/13i-mcp-protocol-and-verification.md) | one line inside `MCP-453`; recommendation (b), drop and record |

---

## 4. Host additions — HA-1 has landed; the 13i gate is lifted

**The spec's own position.** [13i:1712](../../docs/gap-analysis/13i-mcp-protocol-and-verification.md):
"No host addition survives in this section." And `:1718-1719`: the three `host-addition` neighbours —
`HA-1` late tool registration, `HA-2` argument completions, `HA-3` overlay geometry — "are owned
elsewhere and **none of them gates sampling, elicitation or tracing**."

That is confirmed by the triage: not one of `MCP-450` … `MCP-481` touches a host verb that does not
already exist. **`HA-2` and `HA-3` gate nothing in 13i.** Do not carry them as blockers.

**`HA-1` is now implemented, end to end.** The audit's assumption that it is absent is stale:

* the trait — [cyrup-ext/src/native.rs:768](../../crates/cyrup-ext/src/native.rs) `pub trait LateRegistrar`
  (`register_tool`, `register_command`, `register_tool_renderer`, `owner`), deliberately feature-independent
  (`:759-767`), with the injection point at `:697`;
* the host implementation — [cyrup-ext/src/facade.rs:131](../../crates/cyrup-ext/src/facade.rs)
  `HostLateRegistrar`, impl at `:139`, minted per extension at `:736` `late_registrar_for`;
* the MCP consumer — [extension.rs:118](../../crates/cyrup-mcp/src/extension.rs) stores it, `:783`
  `set_late_registrar` binds it, `:166` `sync_tool_surface` runs a full re-resolution through it;
* the sink — [registration.rs:2021](../../crates/cyrup-mcp/src/registration.rs) `LateSink`, the
  write-through half of the `SurfaceSink` trait (`:1946`) that `register_surface` (`:2072`) is generic over,
  with the fingerprint diff so a no-op resync does not invalidate the prompt cache;
* the trigger — [extension.rs:425](../../crates/cyrup-mcp/src/extension.rs) `install_surface_sync` installs
  the metadata listener; [proxy.rs:4510](../../crates/cyrup-mcp/src/proxy.rs) fires it from `execute_connect`.

**Consequence for `MCP-498`.** The spec's note at
[13i:1648-1651](../../docs/gap-analysis/13i-mcp-protocol-and-verification.md) says the child-process
harness cannot assert the cold-cache case until `HA-1` exists, and that the test must state whether it
is testing the warm path. That constraint is **discharged**. `MCP-498` is now schedulable immediately and
should assert both paths:

1. **warm cache** — the direct tool is registered before `agent_start`, the original upstream assertion;
2. **cold cache** — only the `mcp` proxy tool at `init`, then the direct tool appearing *without a
   restart* after the first connect, via `install_surface_sync` → `sync_tool_surface` → `LateSink`.

The second case is the one HA-1 made assertable, and it is a stronger test than the original. The
harness's binary-resolution convention already exists too: `CYRUP_IT_BIN_*` from
[cyrup-it/build.rs:190-197](../../crates/cyrup-it/build.rs) — and the spec's prohibition on
`env!("CARGO_BIN_EXE_…")` is a live guardrail, G4 at
[TEST-ARCHITECTURE.md:1127-1130](../../docs/TEST-ARCHITECTURE.md).

---

## 5. Proposed waves — grouped by shared obligation

Each wave is named by the **one obligation** it exists to satisfy. Grouping by file is what previously
put a needed file in a different agent's set than the unit whose obligation required it; the file lists
below are consequences, never the grouping key. A wave may touch a file another wave also touches — that
is expected and is the reason the obligation, not the file, is the boundary.

### Wave 0 — *a production handler factory exists* — PREREQUISITE, NOT 13i

**Obligation.** One place where the manager supplies the connection builder with a real hook bag.
**Units.** `MCP-118` / `MCP-120` / `MCP-122` (13c). No 13i unit.
**Entry condition.** None.
**Unblocks.** Waves 1-4 in their entirety. Makes `MCP-457` and `MCP-468` observable rather than inert.
**Why it is here.** §3.1. Scheduling any sampling or elicitation unit before this produces code that
cannot be reached at runtime and cannot be verified end to end.

### Wave 1 — *one consent surface, and one caller that uses it*

**Obligation.** Every human question this crate asks goes through one dialog type holding both guards,
and the sampling handler is the first caller that proves it.
**Units.** `MCP-471` (add the `input` arm; the rest is done), `MCP-455` (acquires its call site),
`MCP-450`, `MCP-451`, `MCP-456`, `MCP-458`.
**Entry condition.** Wave 0.
**Notes.** `MCP-455` is *already written* (§2.1) — this wave wires it, it does not rewrite it. The
`input` arm's shape is fixed here because wave 3 consumes it. Coordinate with `MCP-232` (§3.3).

### Wave 2 — *one candidate-set builder and one provider call*

**Obligation.** Sampling resolves a model and runs a completion, once, in one place.
**Units.** `MCP-452`, `MCP-453`, `MCP-454`.
**Entry condition.** Wave 1 (`MCP-450`'s handler body is the caller).
**Notes.** The only wave that reaches outside `cyrup-mcp` for a dependency (`cyrup-provider`'s catalogue
and auth probe). Carries the `metadata` pass-through decision (§3.4).

### Wave 3 — *one traversal of `requestedSchema` drives four user-visible orderings*

**Obligation.** Question order, review-row order, edit-picker order and coercion all read the same
ordered property list once.
**Units.** `MCP-460`, `MCP-462`, `MCP-461`, `MCP-463`, `MCP-464`, `MCP-465`, `MCP-466`.
**Entry condition.** Wave 0, plus wave 1's `input` arm.
**Notes.** Splitting `MCP-464`'s coercion from `MCP-463`'s re-prompt loop duplicates the 13 error
templates. `MCP-465` carries the `MCP-092` obligation with it (§3.3) — this wave's owner takes both or
the wave is mis-scoped. May run in parallel with wave 1 only if the `input` arm lands first.

### Wave 4 — *one browser-open decision and one accepted-elicitation registry*

**Obligation.** The URL half of elicitation, end to end, over machinery that already exists.
**Units.** `MCP-467`, `MCP-472`, `MCP-469` (remaining half), `MCP-470` (remaining half).
**Entry condition.** Wave 3 (shares the gate dialog and the `ElicitationHook` producer).
**Notes.** Consumes rather than builds: the registry is done (§2.2), the launcher exists (§2.4), the
caller-side `-32042` arm and its three messages are ported ([proxy.rs:3731-3737](../../crates/cyrup-mcp/src/proxy.rs)).
The wave's first act is the reuse-vs-port-`openUrl` ruling.

### Wave 5 — *one event, one writer, one decorator* — PARALLEL FROM t=0

**Obligation.** A metadata-only record of every message in both directions, bounded and never throwing.
**Units.** `MCP-473`, `MCP-474`, `MCP-475`, `MCP-476`, `MCP-477`, `MCP-478`, `MCP-479`, `MCP-480`, `MCP-481`.
**Entry condition.** None. Independent of waves 0-4 — it decorates the transport, not the handler.
**Notes.** The settings half already exists and is unconsumed (`config.rs:1620`/`:876`/`:1267-1280`,
`dirs.rs:116`/`:203`), so the wave supplies the producer. `MCP-478`'s combining function is what makes
`MCP-481`'s own verify line writable, so they belong in the same set. `regex` is already a dependency.

### Wave 6 — *one driver process and one runner contract* — PARALLEL FROM t=0

**Obligation.** The protocol gate: a binary the conformance CLI can drive, and a runner that reports it.
**Units.** `MCP-483`, `MCP-484`, `MCP-485`, `MCP-486`, `MCP-487` (may dissolve).
**Entry condition.** ADR-0022 for the runner shape; `MCP-489` for what it points at.
**Notes.** Independent of every other wave. Copies the existing hidden-subcommand pre-dispatch pattern
(§1.4, `MCP-484`). The baseline starts **empty** and is written from an observed run — there is no file
to copy, which is the safe state (`MCP-486`).

### Wave 7 — *proof at the seam, not at the function*

**Obligation.** Assertions that only an assembled session, a real child process or a real terminal can make.
**Units.** `MCP-496` (needs waves 1, 3, 4), `MCP-490` (needs waves 1-5), `MCP-491` + `MCP-492` (need the
ADR-0021 ruling), and — schedulable **immediately, in parallel with wave 0** — `MCP-493`, `MCP-495`, `MCP-498`.
**Notes.** `MCP-498` is unblocked by HA-1 (§4). `MCP-493` and `MCP-495` have no 13i dependency at all and
are the cheapest open units in the section.

### Not scheduled — a ruling first

`MCP-489`, `MCP-494`, `MCP-499`. Each is `not-applicable` in the ledger by verdict class, not by being
finished; each becomes work the moment its decision lands (§3.4).

### Critical path

```
Wave 0 ──┬─> Wave 1 ──> Wave 2
         └─> Wave 3 ──> Wave 4 ──┐
                                  ├─> Wave 7 (MCP-496, MCP-490)
Wave 5 ───────────────────────────┘
Wave 6 (independent)
Wave 7 subset (MCP-493, MCP-495, MCP-498) — parallel from t=0
```

---

## 6. Definition of done — for the SCOPING work

This task is done when all of the following are true of **this file**:

1. All 50 13i units carry a triage verdict, and each of the 42 open ones is classified
   confirmed-missing, present-but-unrecognised, or blocked-on-a-named-unit-or-ADR.
2. Every row recorded `missing` was searched under both the upstream TypeScript identifier and at least
   one plausible Rust renaming before the verdict was accepted, and every overturned row carries the
   `file:line` that overturns it.
3. The dependency order names every **non-13i** unit that gates 13i work, by id.
4. The HA-1 / HA-2 / HA-3 position is stated against the current tree, with citations, not against the
   audit snapshot.
5. Every wave names the single obligation it exists to satisfy, the units it contains, and its entry
   condition — and no wave is defined by the files it touches.
6. Every open decision that blocks a wave is named with its ADR and the wave it blocks.
7. Every markdown link resolves from `.flux/todo/` and every factual claim carries an exact `file:line`.

It is **not** done — regardless of the above — if any implementation has begun, if any file outside this
one was modified, or if a wave is described by its file set rather than its obligation.

**Hand-off note.** The triage lives here, in this file, so a downstream task consumes it directly. Do not
treat updating any file under `docs/` as part of this task; the ledger corrections in §2 are findings to
be relayed, not edits to be made.

## Acceptance Criteria

- [ ] All 50 13i units carry a triage verdict; the 42 open ones are each confirmed-missing,
      present-but-unrecognised, or blocked-on-`<named unit or ADR>`
- [ ] Every `missing` row was checked against the actual Rust under both the upstream spelling and a
      plausible Rust renaming before being accepted as missing
- [ ] A dependency order exists, naming the non-13i units (`MCP-092`, `MCP-118`/`120`/`122`) and the
      ADRs (0021, 0022, 0027) that gate 13i work
- [ ] The HA-1 / HA-2 / HA-3 position reflects the current tree — HA-1 landed, HA-2 and HA-3 gate nothing
      in 13i — and `MCP-498` is re-scoped accordingly
- [ ] Waves are proposed and sized, each named by one shared obligation, each with an entry condition
- [ ] No production code changes, no edits under `docs/`, no files created outside `tmp/`

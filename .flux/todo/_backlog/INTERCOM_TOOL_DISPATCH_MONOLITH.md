---
stage: exec
status: done
updated: 2026-08-23 00:15
---

# IntercomTool::dispatch Is a 592-Line Function — 3.2x the Next-Largest Function in the Crate — and Two of Its E

> Source: `intercom-hygiene-audit` workflow. Severity **medium**, effort **large**.
> Every claim below was produced by a finder agent and then reproduced by an independent
> adversarial verifier; findings that did not reproduce were dropped.

## Scope

- `crates/cyrup-intercom/src/tools/intercom.rs`

## Description

src/tools/intercom.rs:137-728 is a single `async fn dispatch` of 592 lines (428 non-blank, non-
comment code lines) holding an eight-arm `match params.action.as_str()`. `impl IntercomTool`
(130-838) contains exactly two methods: `new` (133-136) and `dispatch`. A brace-matched
enumeration of every non-test `fn` under crates/cyrup-intercom/src/ ranks `dispatch` first at ~590
lines, with the next-largest being broker/send.rs:22-205 `handle_send` at 184 lines — a 3.2x gap,
so this is not merely the largest of the six >1000-line files, it is an outlier in the whole
crate. The concrete consequence is test reachability: `dispatch` is crate-private (stated verbatim
at crates/cyrup-it/tests/intercom/tool_actions.rs:13-14, "the crate-private
`IntercomTool::dispatch(IntercomParams { … })`, which an external crate cannot name"), the in-
crate test module contains zero references to it, and the only external entry point is
`Tool::execute` driven by a JSON `action` string. Enumerating those strings shows two arms —
`list-cwd` (198-269, 72 lines) and `pending` (667-709, 43 lines), 115 lines together — are
executed by no test anywhere in the repo. The arms are structurally incapable of sharing state
(match arms cannot see each other's bindings), so the 592 lines are eight independent handlers
sharing one scope.

## Why it matters

Two action arms totalling 115 lines (list-cwd, pending) have no executing test anywhere in the
repo, and the reason is structural rather than accidental: the only way in is a private 592-line
function reached through a JSON action string, so writing a focused test for one arm means driving
a real broker subprocess through `Tool::execute`. Every future edit to those two arms lands
unverified. The same shape makes the other six arms expensive to test in isolation and makes the
file the crate's single largest maintenance surface by a 3.2x margin over anything else.

## Evidence

- src/tools/intercom.rs:137-728 — `async fn dispatch(&self, params: IntercomParams, cancel: &CancelToken)` spans 592 lines; `awk 'NR>=137 && NR<=728' | grep -vE '^\s*$' | grep -vE '^\s*//' | wc -l` = 428 code lines. `grep -n '^impl \|^    (pub )?(async )?fn '` over the file shows `impl IntercomTool` at 130 with exactly two methods: `new` [133-136] and `dispatch` [137-728]
- src/tools/intercom.rs:151,198,270,305,474,585,667,710 — `grep -n '^            "'` returns the eight action arms in order: "list" 151-197 (47L), "list-cwd" 198-269 (72L), "cancel" 270-304 (35L), "send" 305-473 (169L), "ask" 474-584 (111L), "reply" 585-666 (82L), "pending" 667-709 (43L), "status" 710-727 (18L), plus the `other =>` catch-all at 728
- Brace-matched scan of every non-test `fn` in crates/cyrup-intercom/src/**.rs, sorted by length: dispatch (src/tools/intercom.rs:137) ~590L; handle_send (src/broker/send.rs:22-205) 184L; read_task (src/transport/client.rs:775-941) 167L; spawn_inbound_loop (src/inbound.rs:363-511) 149L; on_event (src/extension.rs:542-687) 146L. dispatch is 3.2x the runner-up
- src/tools/intercom.rs:141-149 — the entire shared prelude is three statements: `crate::connect::ensure_connected(&self.state, ConnectReason::Tool)` binding `client` (141-143) and `self.state.sync_presence_identity()` (149). Nothing else is computed before the `match` at 150
- `grep -n 'cancel' src/tools/intercom.rs` restricted to 137-728 yields exactly one use of the `cancel` parameter inside the body — line 533, in the "ask" arm. (Lines 264/269/270/279-282/397 are the "cancel" ACTION and the string "Message cancelled by user", not the token.) So seven of the eight arms ignore the parameter entirely
- `grep -rn '"list-cwd"' --include=*.rs /home/user/cyrup` returns exactly four hits: src/tools/intercom.rs:198 (the arm), :805 (the schema enum), :958 (a schema-shape unit test), and src/resources.rs:174 (an assertion that the string is *advertised* in the schema). No hit executes the arm
- `grep -rhno '"action": *"[a-z-]*"' crates/cyrup-it/tests/intercom/*.rs | sort | uniq -c` yields six distinct actions and no others: ask(1), cancel(4), list(4), reply(3), send(4), status(1). Neither "list-cwd" nor "pending" appears, so src/tools/intercom.rs:198-269 and :667-709 — 115 lines — are executed by no test in the repo. crates/cyrup-it/tests/intercom/tool_actions.rs:1 describes itself as "The `intercom` TOOL's six actions"
- src/tools/intercom.rs:896 — `mod tests` begins here; `sed -n '897,1124p' | grep -c dispatch` = 0. All eight in-crate test fns (913, 946, 983, 1014, 1036, 1056, 1066, 1078) target free helpers or `parameters_schema` (:799). Test share is 229/1124 = 20%, the lowest of the six >1000-line files (client.rs 762/1705, inbound.rs 521/1148, session_state.rs 426/1297, protocol.rs 365/1333, extension.rs 335/1172)
- src/broker/mod.rs:24-35 — the in-repo precedent and its stated rationale: "`broker.ts` is one file upstream and so was this one, until it reached 3,292 lines; the modules below are the seams it was carrying… `dispatch` is the frame switch; `session`/`send`/`receipts`/`presence`/`extensions` are the handlers, one per protocol concern, each an `impl BrokerState` block". src/broker/state.rs:26 shows the mechanism: `pub(super) struct ConnectedSession` with `pub(super)` fields; src/broker/mod.rs:57 `pub use lifecycle::run` keeps the public surface unchanged

## Required fix

Apply the broker/mod.rs template. Turn src/tools/intercom.rs into src/tools/intercom/mod.rs
holding `IntercomTool`, `IntercomParams`, `DeliveryTarget`, `parameters_schema` (:799), the
existing free helpers (`resolve_target_cwd` :86, `resolve_cwd_delivery_target` :95, `require`
:729, `to_tool_err` :736, `display_name` :742, `format_session_list_row` :761) raised to
`pub(super)`, the `Tool` impl (840-895), and a `dispatch` reduced to its three-statement prelude
plus an eight-line match whose arms read `"send" => self.action_send(&params, &client).await`. One
submodule per action — list.rs, list_cwd.rs, cancel.rs, send.rs, ask.rs, reply.rs, pending.rs,
status.rs — each a `pub(super) async fn action_*` in its own `impl IntercomTool` block taking
`(&self, params: &IntercomParams, client: &Arc<IntercomClient>)`, with `cancel: &CancelToken`
added only to `ask` (the sole arm that uses it, line 533). Move each arm's `index.ts` citation
block with the code it annotates (152-157, 195-197, 264-269, 711-716, …) — no comment dropped, no
branch collapsed. Add a `## Layout` doc section to mod.rs the way broker/mod.rs:24-35 does. Then
add the two missing tests — `list-cwd` and `pending` — which become writable as direct calls to
named `pub(super)` functions rather than requiring a broker subprocess.

## Acceptance Criteria

- [ ] The fix above applied as written; no scope beyond it.
- [ ] Port fidelity preserved — no `broker.ts`/`client.ts`/`paths.ts` citation dropped, no
      `[CYRUP-DELTA]` note removed, no ported branch collapsed.
- [ ] Baseline recorded before the change and matched after:
      `cargo clippy -p cyrup-intercom --all-targets`, `cargo test -p cyrup-intercom --lib`.
- [ ] `cargo build -p cyrup` still succeeds.

# 11 — cyrup-intercom

This area covers `cyrup/crates/cyrup-intercom` — the Unix-socket supervisor↔subagent broker, its client transport, inbound-message delivery, presence/lifecycle reporting, the `intercom` tool surface, and the broker binary — measured against `pi-intercom` at the ported baseline **v0.7.0** (the crate's own `lib.rs` banner still says v0.6.0; it is wrong, see ICOM-012). Headline finding: the reconnect ladder (ICOM-003), reply-target precedence (ICOM-001) and the idle-gated inbound branch (ICOM-002) are now genuinely ported and well tested, but the model-facing half of inbound delivery is still wrong — an incoming message reaches the LLM as bare body text with pi's `**📨 From …**` attribution header and reply instruction stripped — and a large block of post-baseline upstream work (mailbox, extension bus, cwd scoping, context presence, runtime claim) remains unported. Re-baselined against HEAD `1806375` on 2026-08-03; every open item below was re-read at that commit and every closure was checked against code, not commit messages.

## Status since the c8bd2ab baseline

| ID | Status | Note |
|---|---|---|
| ICOM-001 | **Closed** (6f667c5) | `reply_tracker.rs:98-145` matches upstream `resolveReplyTarget` statement-for-statement: `reply_to` branch incl. the `to`-mismatch cross-check → explicit `to` filter (terminal) → `current_turn_context` → single pending → the two error strings. Arm order inside the `to` branch differs textually; semantics and all message strings are identical. |
| ICOM-002 | **Closed** (7c3862b) | `decide_inbound_policy` (`inbound.rs:99-116`) reproduces pi's `handleIncomingMessage` tree incl. the `!message.replyTo` sub-gate and markReplied-only-on-delivered. Dispatch reads `state.is_idle()` (`inbound.rs:336`), resolving through `HostServices::is_idle` to `AgentSession::is_idle` (`session.rs:553`). Two adjacent defects filed new (ICOM-022, ICOM-023). |
| ICOM-003 | **Partially closed** (ace01cb) — downgraded from closed | Ladder itself faithful and covered by real-broker integration tests. But `seams.rs:204` `IntercomClarifyChannel::ask` still reads `self.state.client()` directly while its two siblings (`seams.rs:96,138`) use `ensure_connected`. Remains open at low/S. |
| ICOM-004 | Open | `skills/pi-intercom/SKILL.md` still unported; crate has no `.md` at all. |
| ICOM-005 | Open | Mechanism corrected: the failure is an idle-broker leak, not a premature kill. |
| ICOM-006 | Open | Name poll absent; `ENV_INTERCOM_NAME_POLL_MS` declared and never read. |
| ICOM-007 | Open | `pending` output still lacks message id / elapsed / preview cap. |
| ICOM-008 | Open | `ask` still drops `replyTo`. |
| ICOM-009 | Open | Sharpened: `EventKind::ModelSelect` exists; only the subscription is missing. |
| ICOM-010 | Open | Broker mailbox absent (post-baseline upstream, v0.9.x). |
| ICOM-011 | Open | `stableId` / `/intercom-id` absent; wire already supports it. |
| ICOM-012 | Open | `lib.rs:1-2` still says v0.6.0. |
| ICOM-013 | Open | String divergences confirmed; crate is internally inconsistent too. |
| ICOM-014 | Open | Presence validation still precedes the ownership check. |
| ICOM-015 | Open | Windows named-pipe / TCP-loopback absent; upstream evidence IS at v0.7.0. |
| ICOM-016 | Open | Silent extension bus absent (post-baseline, v0.8.0 `db22c07`). |
| ICOM-017 | Open | Receipts / ordering metadata / cancel absent (post-baseline). |
| ICOM-018 | Open | `list-cwd` + cwd normalization absent (post-baseline, v0.9.0 `57d0da4`). |
| ICOM-019 | Open | Context-window usage in presence/list absent (same commit as ICOM-018). |
| ICOM-020 | Open | Evidence corrected: `runtime-claim.ts` is v0.8.0, so this is not-ported/post-baseline, not a v0.7.0 parity bug. |
| ICOM-021 | Open | Registration `status` still the raw config suffix; now hit on every reconnect rung. |

Closed this window: **2** (ICOM-001, ICOM-002).

## Open items

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| ~~ICOM-022~~ **CLOSED** `513e45a` | high | parity-bug | S | Inbound message injected without pi's attribution header and reply instruction |
| ICOM-004 | medium | not-ported | S | `skills/pi-intercom/SKILL.md` not ported |
| ICOM-006 | medium | not-ported | M | No name poll; presence name fixed at registration |
| ICOM-007 | medium | parity-bug | S | `pending` omits message id, elapsed time, preview truncation |
| ICOM-008 | medium | parity-bug | S | `ask` silently drops `replyTo`; counter-ask refused by broker |
| ICOM-009 | medium | parity-bug | M | Lifecycle status has no active-tool map and no `model_select` hook |
| ICOM-010 | medium | not-ported | L | Broker mailbox for briefly-disconnected sessions absent |
| ICOM-011 | medium | not-ported | M | Restart-stable session IDs absent; `stableId` silently ignored |
| ICOM-013 | medium | parity-bug | M | User-visible message strings diverge across the tool surface |
| ICOM-020 | medium | not-ported | S | No live-broker claim guard; a second broker orphans a running one |
| ICOM-023 | medium | parity-bug | S | `schedule_inbound_flush(state, 0)` races its own task and aborts the retry |
| ICOM-024 | medium | not-ported | M | No `intercom_message` renderer; card frozen at width 80, expand/collapse dead |
| ICOM-003 | low | not-ported | S | `IntercomClarifyChannel::ask` bypasses `ensure_connected` |
| ICOM-005 | low | parity-bug | S | `register` inside the pending window leaves an idle broker alive forever |
| ICOM-012 | low | stale-port | S | `lib.rs` claims v0.6.0 when the code is at v0.7.0 |
| ICOM-014 | low | parity-bug | S | Broker `presence` validation runs before the socket-ownership check |
| ICOM-015 | low | not-ported | L | Windows named-pipe / TCP-loopback transports absent |
| ICOM-016 | low | not-ported | L | Silent namespaced extension bus absent |
| ICOM-017 | low | not-ported | L | Delivery diagnostics (receipts, ordering, cancel/supersede) absent |
| ICOM-018 | low | not-ported | M | `list-cwd` and cwd normalization absent |
| ICOM-019 | low | not-ported | M | Live context-window usage not reported in presence or list |
| ICOM-025 | low | test-defect | S | Two tests assert fixed wall-clock sleeps instead of polled conditions |
| ICOM-026 | low | test-defect | S | Send-confirmation test pins ICOM-013's divergent string |

## ICOM-022 — Inbound message injected without pi's attribution header and reply instruction

**Kind** parity-bug · **Severity** high · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-intercom/src/inbound.rs:217` does `let body = build_inline_message(state, from, message).body().to_string();` then `:218` `services.inject_message(&body, Some(INBOUND_MESSAGE_CUSTOM_TYPE), false, trigger)`; the follow-up path repeats it at `:247-249`. `InlineMessage::body()` (`ui/inline_message.rs:49-51`) returns message text + attachments only. The correctly-ported string exists and is unused on this path: `content_markdown()` (`ui/inline_message.rs:67-80`) builds `**📨 From {sender}** ({cwd}){reply_instruction}\n\n{body}` with `sender_display()` (:55-62) reproducing pi's `name || id.slice(0,8)` fallback — called only at `inbound.rs:428` inside the `append_entry("intercom_message", …)` payload, which is verified non-LLM (`cyrup-session-svc/src/host_services.rs:943-954`, "Persist the custom (non-LLM) entry"). `inject_message` (`host_services.rs:711-734`) is the only path into the conversation.

**upstream** — `pi-intercom` `v0.7.0:index.ts`, `sendIncomingMessage`: one delivery, ``content: `**📨 From ${senderDisplay}** (${entry.from.cwd})${replyInstruction}\n\n${entry.bodyText}` ``, with `replyInstruction = entry.replyCommand ? "\n\nTo reply, use the intercom tool: …" : ""`. The string the human sees and the string the model sees are the same string.

**Impact** — Every inbound intercom message reaches the model with no sender attribution and no reply guidance. The model cannot distinguish an intercom message from a user turn, cannot tell which peer asked when several are active, and is never told the `intercom({action:"reply"})` command even though `config.reply_hint` defaults on and the hint is already computed (`inbound.rs:404-405`). Worse on flush: a busy session's backlog drains as N consecutive header-less injections (`inbound.rs:179-183`), concatenating several peers' messages indistinguishably. `pending` is the only remaining way to recover attribution and is itself broken (ICOM-007).

**Fix** — Replace `.body().to_string()` with `.content_markdown()` at `inbound.rs:217` and `:247`. `content_markdown()` is already unit-tested against pi's shape (`ui/inline_message.rs:281-288`). Leave the `append_entry` surface as is — that split is the documented port-doc §4.2 divergence; only the content of the injected half is wrong.

**Verify** — Extend `idle_headless_session_delivers_instead_of_auto_replying` (`inbound.rs:712-733`) to assert `injected[0].0.starts_with("**📨 From subagent-chat-1** (/w)")` and, with `reply_hint` on and `expects_reply: Some(true)`, that it contains "To reply, use the intercom tool:". Add the same to the flush test (`:699-705`). Both currently assert only `.contains("ping")`/`.contains("first")`, which pass either way.

## ICOM-004 — `skills/pi-intercom/SKILL.md` is not ported

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-intercom` contains no `.md` file at all; the crate is `src/`, `tests/`, `Cargo.toml`.

**upstream** — `pi-intercom/skills/pi-intercom/SKILL.md` ships the operator-facing usage guide the extension surfaces as a skill.

**Impact** — The agent gets no skill describing how to use the intercom tool — no worked examples of list/send/ask/reply, no guidance on addressing peers. This is the primary documentation of a subsystem whose tool errors (ICOM-007, ICOM-008, ICOM-013) are already opaque.

**Fix** — Add `crates/cyrup-intercom/resources/skills/cyrup-intercom/SKILL.md` and register it the way `crates/cyrup-ext-subagents/resources/skills/pi-subagents/SKILL.md` is registered; rewrite `pi`→`cyrup` naming and the tool signature to match cyrup's schema (`tools/intercom.rs:329-357`).

**Verify** — Assert the skill is discoverable from the extension's registration and that its documented actions match the tool's action enum.

## ICOM-006 — No name poll; presence name is fixed at registration

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-intercom/src/extension.rs:159-163` `sync_presence` calls `client.update_presence(None, Some(self.presence_status(base)), None)` — name hard-coded `None`. `ENV_INTERCOM_NAME_POLL_MS` has exactly one occurrence workspace-wide, its declaration at `src/identity.rs:17`; no poll task exists. The name is computed once in `connect::build_registration` (`connect.rs:408-422`).

**upstream** — `pi-intercom` `v0.7.0:index.ts`: `syncPresenceIdentity(sessionId)` sends `{ ...identity, status: currentStatus() }`, and `startNamePoll()` is a `setInterval(getNamePollMs())` that re-derives `buildPresenceIdentity` and re-syncs when `identity.name !== lastPresenceName`.

**Impact** — A session that renames itself mid-run (branch switch, title change) keeps advertising its startup name to every peer's `intercom{list}`, so operators address the wrong worker.

**Fix** — Recompute via `connect::build_registration`'s name helper and push it from `extension.rs:159-163`; start a poll task alongside `connect::begin_runtime` (`extension.rs:295`) and cancel it in the `SessionShutdown` arm (`extension.rs:312-327`).

**Verify** — Unit test: change the identity source, advance the poll interval, assert a `Presence` frame carrying the new name is sent exactly once (not once per tick).

## ICOM-007 — `pending` omits message id, elapsed time, and preview truncation

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-intercom/src/tools/intercom.rs:249-268`: empty case `"No pending intercom asks."` (:258); rows `format!("• {} ({}): {}", who, short_session_id(&c.from.id), c.message.content.text)` (:264). No header, no elapsed, no length cap, and the id printed is the SENDER's, not the message's.

**upstream** — `pi-intercom` `v0.7.0:index.ts`, `case "pending"`: `` `- ${from.name || from.id} · ${message.id} · ${elapsedSeconds}s ago · ${preview}` `` with `preview = text.replace(/\s+/g," ").slice(0,80)`, wrapped as `**Pending asks:**\n…`; empty case `"No unresolved inbound asks."`.

**Impact** — The message id is exactly what `intercom({action:"reply", replyTo})` needs, so the agent cannot construct a targeted reply from `pending` output; this compounds ICOM-008 and ICOM-022. A long message floods the tool result untruncated, and there is no way to see which ask is oldest.

**Fix** — Rewrite the formatter at `tools/intercom.rs:249-268` to pi's row shape and empty string; add an elapsed computation from `received_at`. Ordering already matches (`list_pending` sorts by `received_at`, as upstream sorts by `receivedAt`).

**Verify** — Unit test the formatter against a two-entry fixture with a >80-char body: assert the header line, both message ids, monotonic `Ns ago`, and an 80-char preview.

## ICOM-008 — `ask` silently drops `replyTo`, so counter-ask is refused by the broker

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-intercom/src/tools/intercom.rs:154-194` (the `"ask"` arm) never reads `params.reply_to` when calling `ask_and_wait` (:162-166); it only echoes it into the audit entry (:178). `session_state.rs:208-231` `ask_and_wait` hard-codes `reply_to: None` in its `SendOptions` (:227). The field is declared (`tools/intercom.rs:34`) and schema-advertised as `replyTo` (:353).

**upstream** — `pi-intercom` `v0.7.0:index.ts`, `case "ask"`: `connectedClient.send(sendTo, { messageId: questionId, text: message, attachments, replyTo, expectsReply: true })`.

**Impact** — Asking a clarifying question back at a peer's pending ask is rejected by cyrup's own broker: `broker/mod.rs` `handle_send` returns `"Reply target does not match a pending ask"` when `message.reply_to.is_some() && reply_edge.is_none()`. The tool advertises a parameter it discards, so the failure looks like a broker bug.

**Fix** — Thread `params.reply_to` through `ask_and_wait`'s signature into `SendOptions.reply_to` (`session_state.rs:227`).

**Verify** — Integration test against the real broker: peer A asks, B counter-asks with `replyTo` set to A's message id, assert delivery instead of `Reply target does not match a pending ask`.

## ICOM-009 — Lifecycle status has no active-tool map and no `model_select` hook

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-intercom/src/extension.rs:328-347` maps events 1:1 with no per-tool-call-id map: `AgentStart→"thinking"` (:329), `AgentEnd→"idle"` (:333), `ToolExecStart{name}→"tool:{name}"` (:341), `ToolExecEnd→"thinking"` (:345). `SharedIntercomState` (`session_state.rs:20-60`) has no `active_tools`. `EventKind::ModelSelect` exists (`cyrup-ext/src/event.rs:39`, `= 22`, dispatched at `cyrup-ext/src/host/live.rs:1550`) but is absent from the subscription list (`extension.rs:239-251`), and `sync_presence` passes `None` for model (`extension.rs:161`).

**upstream** — `pi-intercom` `v0.7.0:index.ts` `currentStatus()`: `const activeToolName = activeTools.values().next().value; const lifecycleStatus = activeToolName ? \`tool:${activeToolName}\` : agentRunning ? "thinking" : "idle"; return config.status ? \`${lifecycleStatus} · ${config.status}\` : lifecycleStatus;` — a map keyed by tool-call id, not a scalar.

**Impact** — With two overlapping tool calls, the first `ToolExecEnd` resets presence to `thinking` while a tool is still running, so peers see the wrong activity. The advertised model is always empty in `intercom{list}`, so a supervisor cannot tell which worker is on which model.

**Fix** — Add `active_tools: Mutex<IndexMap<ToolCallId, String>>` to `SharedIntercomState`, derive status in a `current_status()` helper (also required by ICOM-021 — port them together), and add `EventKind::ModelSelect` to `extension.rs:239-251` with a `sync_presence` overload carrying the model. No new seam is needed.

**Verify** — Unit test: start tool A, start tool B, end A → status still `tool:…`; end B → `thinking`. Separately assert a `ModelSelect` event produces a `Presence` frame with `model` set.

## ICOM-010 — Broker mailbox for briefly-disconnected sessions is absent

**Kind** not-ported · **Severity** medium · **Effort** L · **Confidence** medium (cyrup side verified; upstream lines carried forward)

**cyrup** — `cyrup/crates/cyrup-intercom/src/broker/mod.rs` `handle_send` answers an unregistered target with `DeliveryFailed { reason: "Session not found" }` and drops the message (arm around `:433-440`). `BrokerState` (`:70-140`) has no disconnected-session or mailbox map.

**upstream** — `pi-intercom` added the mailbox post-baseline (v0.9.0 `7b4b760`, v0.9.1 `d7691b6`): the broker parks messages for a session that disconnected within the grace window and redelivers on re-register.

**Impact** — Any message sent during a peer's reconnect gap is lost with a misleading "Session not found", now more likely because ICOM-003's ladder makes reconnect gaps a routine state rather than a terminal one.

**Fix** — Port upstream's disconnected-session map and redelivery-on-register into `broker/mod.rs`. `connect.rs:44-51` documents "there is no mailbox, no queue, no redelivery" as an invariant the reconnect design relies on; a mailbox port must revisit that reasoning and the `fail_pending`-on-disconnect decision at `connect.rs:386`.

**Verify** — Integration test: register A and B, kill B's socket, send from A, restart B with the same session id, assert delivery.

## ICOM-011 — Restart-stable session IDs (`stableId` / `/intercom-id`) absent; `stableId` silently ignored

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** medium (cyrup side verified; upstream lines carried forward)

**cyrup** — No `stable_id`/`stableId` anywhere in `cyrup/crates/cyrup-intercom/src`. `config.rs:95-143` `parse_config` reads only brokerCommand/brokerArgs/confirmSend/enabled/inboundTrigger/replyHint/status and never rejects unknown keys, so a configured `stableId` is silently dropped. Only one command is registered (`extension.rs:234-237`). The register id comes from `ENV_INTERCOM_SESSION_ID` with a `state.connect.last_session_id()` fallback (`connect.rs:354-357`) — stable across a reconnect, not across a process restart.

**upstream** — `pi-intercom` v0.8.0 (`18d9027`) adds a configured `stableId` plus an `/intercom-id` command so a session keeps its address across restarts.

**Impact** — A restarted worker gets a fresh id, so every peer's stored target breaks and long-lived supervisor scripts must re-list after each restart. The config key is accepted and ignored, which reads as a working feature.

**Fix** — Add `stable_id` to `config.rs`'s parse + struct, prefer it in `connect.rs:354-357`, and register an `/intercom-id` command at `extension.rs:234-237`. The wire already carries it: `ClientMessage::Register` has `session_id: Option<String>` (`transport/protocol.rs:130-141`), so this is a config+command gap, not a protocol gap.

**Verify** — Set `stableId`, connect, drop the process, reconnect, assert the broker sees the same session id and prior peers' targets still resolve.

## ICOM-013 — User-visible message strings diverge across the tool surface

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-intercom/src/tools/intercom.rs`: `:93` `"Cannot send an intercom message to yourself."` and `:159` `"Cannot ask yourself."` (while `:208`, the `reply` arm, already uses pi's exact string — the crate is inconsistent with itself); `:152` `Message sent to {target}.` using the RESOLVED id plus a period; `:244` `Reply sent to {target.from.id}.`; `:258` `"No pending intercom asks."`; `:273-276` a one-line `intercom: connected | session id: … | active sessions: N`; non-delivery raised as a `ToolError` at `:126`/`:246`. Also `session_state.rs:177-179` `"Multiple sessions match \"{name_or_id}\". Use the session ID instead."` and `inbound.rs:39-40` `NON_INTERACTIVE_BUSY_NOTICE`.

**upstream** — `pi-intercom` `v0.7.0:index.ts`: a single `"Cannot message the current session"`; `` `Message sent to ${to}` `` (the caller's original string, no period); `` `Reply sent to ${target.from.name || target.from.id}` ``; `"No unresolved inbound asks."`; a multi-line `**Intercom Status:**\nConnected: Yes\nSession ID: …\nActive sessions: N`; non-delivery returned as a text result with `details: { error: true }`, not a thrown tool error. `resolveSessionTarget` names the candidate short ids: `Multiple sessions named "…" are connected. Address one by the id shown in parentheses by "list" (${ids})`. The non-interactive busy notice reads "This agent is running in non-interactive mode and cannot respond to intercom messages while it is working. It will continue its current task and exit when done."

**Impact** — Prompt-visible drift: agents (and skill docs written against pi) pattern-match on these strings. The ambiguity error is materially worse — pi tells the caller which ids to choose between, cyrup does not, leaving no path forward. The `ToolError` vs text-result difference is behavioral, not cosmetic: they reach the model differently and a `ToolError` can abort a flow pi would let continue.

**Fix** — Normalize each string at the cited lines; echo the caller's original target at `:152`; prefer `name || id` at `:244`; return a text result with an error detail instead of `ToolError` at `:126`/`:246`; include candidate ids in `session_state.rs:177-179`.

**Verify** — String-equality tests per site against the v0.7.0 text. Note the obstacle at `tools/intercom.rs:709`, filed as ICOM-026.

## ICOM-020 — No live-broker claim guard; a second broker orphans a running one

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-intercom/src/broker/mod.rs:800-802` does `let _ = std::fs::remove_file(&socket_path); let listener = UnixListener::bind(&socket_path)?;` with no read of `broker.pid`; the pid path is resolved at `:798` and used only for the write at `:804`.

**upstream** — `pi-intercom` `broker/runtime-claim.ts` `assertNoLiveBroker`, called from the broker constructor — introduced by `db22c07`, first tagged **v0.8.0**, i.e. POST-baseline (`git show v0.7.0:broker/runtime-claim.ts` errors). At `v0.7.0` upstream's constructor does exactly what cyrup does: a bare `unlinkSync(LISTEN_TARGET)` in try/catch.

**Impact** — A second broker start unlinks the live socket and binds a new inode. The first broker keeps running with all existing clients attached to a path nobody can reach; new clients land on the new broker. Sessions silently split into two disjoint address spaces. Severity stays medium despite the post-baseline classification because the consequence is real regardless, and ICOM-003 amplifies it: an orphaned broker's clients now retry forever against the wrong inode instead of failing once.

**Fix** — Before `remove_file` at `broker/mod.rs:800`, read `broker.pid`, probe liveness, and exit non-zero if a live broker owns the socket.

**Verify** — Start a broker, start a second one, assert the second exits non-zero and the first's clients stay connected.

## ICOM-023 — `schedule_inbound_flush(state, 0)` races its own task and aborts the retry

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-intercom/src/inbound.rs:153-162` spawns FIRST and installs the handle AFTER: `let handle = tokio::spawn(async move { if delay_ms > 0 { sleep(..).await; } flush_idle_messages(&flush_state); }); state.set_flush_timer(Some(handle));`. With `delay_ms == 0` the task never awaits, so on a multi-thread runtime it can complete before the caller reaches `:161`. Inside it, `flush_idle_messages` (`:169-184`) calls `release_flush_timer()` (:171) then, when `!state.is_idle()` (:175), `schedule_inbound_flush(state, INBOUND_IDLE_RETRY_MS)` (:176), installing the RETRY handle — which the caller's `:161` then replaces and `.abort()`s (`session_state.rs:146-154`). Reachability is proven, not assumed: the two `delay_ms == 0` call sites are `extension.rs:337` (`AgentEnd`) and `:359` (`TurnEnd`); `HostEvent::TurnEnd` fires per agent turn inside a run (`cyrup-ext/src/event.rs:458`, kind at `:389`); `is_idle()` bottoms out at `AgentSession::is_idle` = `!driver_tx && !agent.is_running()` (`cyrup-session-svc/src/session.rs:553`), necessarily false mid-run.

**upstream** — `pi-intercom` `v0.7.0:index.ts` `scheduleInboundFlush(delayMs)`: `clearInboundFlushTimer(); inboundFlushTimer = setTimeout(() => { inboundFlushTimer = null; flushIdleMessages(scheduledGeneration); }, delayMs);` — the assignment completes before any callback can run (single-threaded, `setTimeout(…,0)` is a macrotask), so `flushIdleMessages`'s own retry scheduling is always the last writer.

**Impact** — A message parked by `InboundPolicy::Queue` can lose its retry and sit in `pending_idle` until an unrelated later event re-arms the flush. Mid-run `TurnEnd`s are self-correcting (the next one re-arms); the real loss window is the LAST event of a run — if the final `AgentEnd` is dispatched while the session still reads busy and its retry is aborted, the run ends, no further events fire, and the queued peer message is never delivered while the session sits idle. That is exactly the failure ICOM-002's fix removed, reintroduced as a scheduling race. Escalate to high if the terminal interleaving proves common.

**Fix** — Install the slot before the work can run: (a) hold the flush-timer lock across spawn+store so the task's `release_flush_timer` blocks until installed; or (b) have the task await a `Notify`/oneshot signalled by the caller right after `set_flush_timer`; or (c) give `set_flush_timer` a monotonic schedule id and refuse to install a handle older than the one in the slot.

**Verify** — `#[tokio::test(flavor = "multi_thread", worker_threads = 4)]` in `inbound.rs`: bind an `IdleControlledHost` reporting busy, `queue_idle_message` once, call `schedule_inbound_flush(&s, 0)` in a 50-iteration loop to widen the window, then `set_idle(true)` and poll (not sleep) for delivery within ~1 s. Against current code the retry is aborted and nothing is injected.

## ICOM-024 — No `intercom_message` renderer; card frozen at width 80, expand/collapse dead

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-intercom/src/extension.rs:224-253` `init` registers tools (:227-230), one command (:234-237) and event subscriptions (:239-251) — no `api.register_message_renderer(..)`. Instead `inbound.rs:429` pre-renders the card ONCE at a hard-coded `SURFACE_CARD_WIDTH: usize = 80` (`inbound.rs:30`) with `collapsed: true` frozen at `inbound.rs:411`. The seam landed in this window: `cyrup-ext/src/native.rs:268` `InitApi::register_message_renderer`, dispatched via `render_call` (`native.rs:355`) / `render_result` (`:359`), plumbed through `cyrup-ext/src/facade.rs:272,726,756` with `has_message_renderer` at `facade.rs:794`, consumed by `cyrup-tui/src/app.rs:3034-3040`. The crate concedes the debt at `ui/inline_message.rs:10-14` ("NOTE (EXT-006): the reason for that degradation is gone… deliberate follow-up work").

**upstream** — `pi-intercom` `v0.7.0:index.ts`: `pi.registerMessageRenderer("intercom_message", (message, options, theme) => { const details = message.details as {...}; if (!details) return undefined; return new InlineMessageComponent(details.from, details.message, theme, details.replyCommand, details.bodyText, !options.expanded); })` — re-invoked per frame with the live theme and `options.expanded`.

**Impact** — A resized terminal shows an 80-column card in a 120- or 60-column pane with misaligned borders and truncated text (`ui/inline_message.rs:85-108` pads every line to exactly `width`). Collapse/expand is unreachable — `collapsed` is baked in at receive time, so a long message can never be expanded, while the card still renders the literal hint "Ctrl+O expands" (`ui/inline_message.rs:95`), which does nothing. The card also ignores the active theme, having been rendered with `PlainTheme` (`inbound.rs:429`).

**Fix** — Add `api.register_message_renderer("intercom_message")` in `IntercomExtension::init` and implement `NativeExtension::render_call` for that key: deserialize `from`/`message`/`replyCommand`/`bodyText` from the payload `surface_incoming_message` already writes (`inbound.rs:427-435`, matching pi's `details`), rebuild an `InlineMessage`, return the serialized widget tree. Keep the pre-rendered `card` field as the degrade for hosts without a renderer. Pairs with ICOM-022 — same custom type (`inbound.rs:36`).

**Verify** — Unit-test `render_call("intercom_message", &payload)` returning `Some(..)` for the exact payload `surface_incoming_message` produces and `None` for one missing `from`/`message` (pi's `if (!details) return undefined`); then assert `ExtensionHost::has_message_renderer("intercom_message")` is true after loading the extension.

## ICOM-003 — `IntercomClarifyChannel::ask` bypasses `ensure_connected`

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-intercom/src/seams.rs:204` `IntercomClarifyChannel::ask` still does `self.state.client().ok_or_else(|| "intercom not connected: cannot route the human answer to the child")` instead of `connect::ensure_connected`. `grep -n '\.client()' src/seams.rs` at HEAD returns only that line; the other three seams in the same file use `ensure_connected(ConnectReason::Background)` (`seams.rs:96`, `:138`). The rest of the ladder is a faithful port (`connect.rs:70,74-78,235-265,274-340,344-371,378-394`) with real-broker integration tests (`tests/reconnect.rs`).

**upstream** — `pi-intercom` `v0.7.0:index.ts` `ensureConnected` is the single client-acquisition contract for every send path; this particular seam is cyrup-original (pi-intercom has no ClarifyChannel — it bridges pi-subagents), so there is no line-for-line counterpart beyond that contract.

**Impact** — Bounded: `handle_disconnect` has already armed the ladder, so intercom is not dead. But a clarify whose human answer becomes ready inside a backoff gap fails once with a misleading "not connected" instead of waiting out the reconnect, and the human's answer is lost from the child's perspective.

**Fix** — Route the client acquisition at `seams.rs:204` through `connect::ensure_connected(&self.state, ConnectReason::Background)` like its siblings at `seams.rs:96,138`. ~3 lines.

**Verify** — Kill the broker mid-clarify, assert `ask` succeeds after the ladder reconnects rather than returning "intercom not connected".

## ICOM-005 — `register` inside the pending window leaves an idle broker alive forever

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-intercom/src/broker/mod.rs:579-599` `schedule_shutdown_check` early-returns on `g.shutdown_scheduled` (:582-583); the flag is cleared only inside the spawned task after the 5 s sleep (:593). `handle_register` bumps `self.shutdown_gen` (:335) without clearing `shutdown_scheduled` and without aborting the pending task; the comment at :334 ("A register cancels any pending auto-shutdown") overstates the code. Call sites: `:659` (unregister) and `:740` (connection close).

**upstream** — `pi-intercom` `v0.7.0:broker/broker.ts:378-381` does `clearTimeout(this.shutdownTimer); this.shutdownTimer = null;` — nulling the handle is what lets a later unregister/close re-arm. Upstream's `scheduleShutdownCheck` has the same `if (this.shutdownTimer) return;` guard (`:287`), so the guard is not the divergence; the missing null is. Call sites match at `:397` and `:246`.

**Impact** — Not a premature kill — the generation stamp correctly prevents that. What is lost is the re-arm: t=0 last session leaves (scheduled=true, gen=G); t=1 register (gen=G+1, still scheduled); t=2 that session disconnects → `schedule_shutdown_check` early-returns; t=5 the pending task sees `G+1 != G` → no shutdown, scheduled=false. The broker then idles indefinitely with zero sessions until an unrelated connect/disconnect cycle re-arms it. An idle-broker leak.

**Fix** — Have `handle_register` set `shutdown_scheduled = false` and abort the pending task (keep the generation bump as belt-and-braces), or store the `JoinHandle` and abort it the way `set_flush_timer` / `ConnectSupervisor::set_timer` already do in this crate.

**Verify** — Register-then-disconnect inside one 5 s window with no other sessions; assert the broker process exits.

## ICOM-012 — `lib.rs` claims v0.6.0 when the code is at v0.7.0

**Kind** stale-port · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-intercom/src/lib.rs:1-2`: "cyrup-intercom — out-of-band supervisor coordination companion (a 1:1 source port of `pi-intercom` v0.6.0)."

**upstream** — The health probe / `INTERCOM_PROTOCOL_NAME`, rate limiting, ask edges, trust controls and the ask-timeout env var are all present in cyrup and all absent at `pi-intercom` v0.6.0; they arrive at v0.7.0.

**Impact** — Every agent that diffs from v0.6.0 "finds" a pile of already-done work. Two commits in this window noticed and did not fix it (6f667c5, 7c3862b); 02d8680 documented it in the README instead. It has cost at least three agents a correction.

**Fix** — Edit the banner at `lib.rs:2` to v0.7.0.

**Verify** — `grep -n 'v0\.[0-9]' crates/cyrup-intercom/src/lib.rs` reads v0.7.0.

## ICOM-014 — Broker `presence` validation runs before the socket-ownership check

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-intercom/src/broker/mod.rs:534-541`: the `for key in ["name","status","model"]` type-check loop runs first and returns `FrameResult::protocol_error()`; ownership is only checked at `:543` via `self.sessions.get_mut(&current_id).filter(|s| s.conn_id == conn_id)`, whose miss is a benign `FrameResult::cont()`.

**upstream** — `pi-intercom` `v0.7.0:broker/broker.ts`, `case "presence"`: every `throw new Error("Invalid presence …")` is nested INSIDE `if (session?.socket === socket) { … }`, so a non-owning socket's malformed presence is ignored, not fatal.

**Impact** — A superseded socket sending a late malformed presence frame gets its connection killed as a protocol error rather than ignored. ICOM-003's ladder makes this a live path, not a theoretical one: `connect.rs:352-357` deliberately re-offers the previous session id, so takeover races are real.

**Fix** — Move the type-check loop inside the ownership `let Some(session) = …` block at `broker/mod.rs:543`.

**Verify** — Connect two sockets claiming the same session id; from the losing one send a presence frame with `name: 5`; assert the connection survives.

## ICOM-015 — Windows named-pipe / TCP-loopback transports absent

**Kind** not-ported · **Severity** low · **Effort** L · **Confidence** high

**cyrup** — Unix socket types are ungated: `cyrup/crates/cyrup-intercom/src/broker/mod.rs:23` `use tokio::net::UnixListener;`, `:748` `stream: tokio::net::UnixStream`, `:802` `UnixListener::bind`, `:814` `tokio::signal::unix::signal`; `transport/client.rs:19` and `transport/spawn.rs:20` import `UnixStream` unconditionally. The only `cfg(unix)` guards are file-mode/signal helpers (`paths.rs:106,125,247`; `transport/spawn.rs:142,159,296`), never the socket types. No `broker.port.json`, no `should_use_tcp_transport`, no `BrokerConnectTarget`.

**upstream** — At the ported baseline: `pi-intercom` `v0.7.0:broker/paths.ts:7,11-12` defines `INTERCOM_TCP_HOST = "127.0.0.1"` and `interface BrokerTcpEndpoint { transport: "tcp"; … }`, with named-pipe support alongside.

**Impact** — The crate does not compile on Windows, and there is no opt-in TCP loopback fallback for environments where Unix sockets are unavailable (some container/WSL and network-filesystem setups).

**Fix** — Introduce a `BrokerConnectTarget` enum in `transport/`, cfg-gate the Unix path, add a named-pipe impl and the `broker.port.json` TCP discovery file per upstream `paths.ts`.

**Verify** — A Windows-target `cargo check -p cyrup-intercom` compiles (a future gate; not runnable in this workspace).

## ICOM-016 — Silent namespaced extension bus absent

**Kind** not-ported · **Severity** low · **Effort** L · **Confidence** medium (cyrup side verified; upstream lines carried forward)

**cyrup** — `cyrup/crates/cyrup-intercom/src/transport/protocol.rs`: `ClientMessage` (`:129-172`) has exactly six variants — Register/Unregister/List/Send/CancelAsk/Presence; `BrokerMessage` (`:177+`) starts with `Registered { session_id }` and carries no `features` field. No `extension_bus`/`features` anywhere in the file.

**upstream** — `pi-intercom` v0.8.0 `db22c07` ("feat: add silent extension bus", the same commit that added `broker/runtime-claim.ts`) adds a namespaced side-channel so extensions can exchange messages without surfacing them to the model.

**Impact** — Extensions built on the upstream bus have no cyrup equivalent, and there is no way to carry coordination metadata between sessions without it appearing as a user-visible intercom message.

**Fix** — Add the bus variants to `transport/protocol.rs`, route them in `broker/mod.rs` without touching the inbound-delivery path, and expose a registration API on the extension seam.

**Verify** — Two sessions exchange a namespaced bus message; assert neither session's conversation receives an injected message.

## ICOM-017 — Delivery diagnostics (receipts, ordering metadata, cancel/supersede) absent

**Kind** not-ported · **Severity** low · **Effort** L · **Confidence** medium (cyrup side verified; upstream lines carried forward)

**cyrup** — `cyrup/crates/cyrup-intercom/src/transport/protocol.rs` `Message` (`:48-70`) carries only id/timestamp/reply_to/expects_reply/content; no `MessageReceipt`, no `sender_sequence`. `tools/intercom.rs:335` action enum is `["list","send","ask","reply","pending","status"]` — no `cancel`.

**upstream** — `pi-intercom` post-baseline adds per-message receipts, a sender sequence for ordering, and cancel/supersede semantics for outstanding asks.

**Impact** — A sender cannot tell whether a message was delivered, read or superseded, and cannot withdraw an ask that is no longer relevant, so a stale ask blocks the peer's `pending` list until it times out.

**Fix** — Extend `Message` and `BrokerMessage` in `transport/protocol.rs`, add a `cancel` action in `tools/intercom.rs`, and honour supersede in `broker/mod.rs` `handle_send`. Keep consistent with the at-most-once no-mailbox invariant documented at `connect.rs:44-51` (or port ICOM-010 first).

**Verify** — Send an ask, cancel it, assert it disappears from the peer's `pending` and the sender receives a receipt.

## ICOM-018 — `list-cwd` and cwd normalization absent

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-intercom/src/tools/intercom.rs:335` action enum has no `list-cwd`, and the schema (`:329-357`) has no `cwd` parameter. `format_session_list_row` does the raw byte comparison `session.cwd == current_cwd` at `:312`. No `normalize_cwd` anywhere in the crate.

**upstream** — `pi-intercom` v0.9.0 `57d0da4` ("feat: add cwd-scoped lists and context presence") adds `cwd.ts` with normalization plus a cwd-scoped list action.

**Impact** — `/w` and `/w/`, or a symlinked vs realpath'd cwd, read as different directories, so the "same project" marker in `list` output is wrong for any session started through a symlink. There is no way to list only the peers working in this repo, which is the common supervisor query.

**Fix** — Port `cwd.ts` as `src/cwd.rs`, use it at `tools/intercom.rs:312`, and add the `list-cwd` action plus its `cwd` parameter to the schema. Port together with ICOM-019 (same upstream commit).

**Verify** — Register two sessions whose cwds differ only by a trailing slash and a symlink; assert both are marked same-project and both appear under `list-cwd`.

## ICOM-019 — Live context-window usage not reported in presence or list

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-intercom/src/transport/protocol.rs` `SessionInfo` (`:20-45`) has no `context_pct`/`context_tokens`/`context_window`; `ClientMessage::Presence` (`:160-171`) carries only name/status/model. `format_session_list_row` (`tools/intercom.rs:303-327`) renders no context segment.

**upstream** — `pi-intercom` v0.9.0 `57d0da4` (same commit as ICOM-018) adds context-window usage to presence and to `list` output.

**Impact** — A supervisor cannot see that a worker is about to compact or is near its context limit, so it keeps dispatching work to a session that is about to lose state.

**Fix** — Add the fields to `SessionInfo` and `Presence` in `transport/protocol.rs`, populate from `HostServices::context_usage()` (`cyrup-ext/src/host/services.rs:321`, live impl in `cyrup-session-svc/src/host_services.rs`) inside `extension.rs`'s `sync_presence`, and render a segment in `format_session_list_row`.

**Verify** — Drive a session's context usage up, assert a `Presence` frame carries the percentage and that a peer's `list` shows it.

## ICOM-025 — Two tests assert fixed wall-clock sleeps instead of polled conditions

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-intercom/src/connect.rs:530-548` `a_failing_rung_waits_its_backoff_instead_of_busy_looping`: `sleep(300ms)` → `assert_eq!(attempt(), 0)`; `sleep(1000ms)` → `assert_eq!(attempt(), 1)` + `assert!(reconnect_armed())`; `sleep(700ms)` → `assert_eq!(attempt(), 1)` — 300 ms of slack over a 1000 ms timer plus a spawn hop plus a filesystem-failing `ensure_broker`. `src/inbound.rs:664-706`: `sleep(INBOUND_FLUSH_DELAY_MS + 100)` then `sleep(INBOUND_IDLE_RETRY_MS + 200)` then `assert_eq!(injected.len(), 2)` — 200 ms of slack over a 500 ms retry, across threads. Neither uses `start_paused = true`; both run on the real clock while the rest of the suite runs in parallel. Scope narrowed: `tests/reconnect.rs` does it correctly with a polling `within(budget, predicate)` helper (`:85-96`, used at `:147,:163,:287`), and the two other fixed sleeps in the crate — `tests/reconnect.rs:299` and `tests/shared_human_lock.rs:265` — are NEGATIVE assertions after a fixed wait, which is sound and must not be "fixed".

**upstream** — Not an upstream-behavior question: the behaviors under test are correct ports of `pi-intercom` `v0.7.0:index.ts` `getReconnectDelayMs` and `flushIdleMessages`. Only the assertion technique is wrong. This is the defect shape 1806375's own message records the repo hitting three times (`providers/anthropic.rs`, `round9_l5res.rs`, `caps/proc.rs`).

**Impact** — Intermittent red on a clean tree. The standard reaction is to widen the sleep, which slows the suite and hides genuine regressions in the backoff/retry timing these two tests are the only guard for. ace01cb's own message flags the risk ("run twice — 251 passed both times") — passing twice on one machine is not determinism.

**Fix** — For `connect.rs:530-548` switch to `#[tokio::test(start_paused = true)]` + `tokio::time::advance(..)`; assertions become exact and instant. For `inbound.rs:664-706` keep the real clock (it drives `IdleControlledHost` across threads) but replace the FINAL fixed sleep with a `within`-style poll on `host.injected().len() == 2`, keeping the mid-test negative assertion as a fixed sleep.

**Verify** — Run both under `--test-threads=1` alongside a CPU load generator and confirm they still pass; confirm the paused-clock connect test completes in milliseconds rather than ~2 s.

## ICOM-026 — Send-confirmation test pins ICOM-013's divergent string

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-intercom/src/tools/intercom.rs:709` `assert_eq!(result_text(&result), "Message sent to target-session.");` — exact equality on the string produced at `:152` `Ok(text_result(format!("Message sent to {target}.")))`, where `target` is the RESOLVED session id (from `resolve_or_err`, `:91`) and the trailing period is cyrup's own addition. The test is a real integration test (spawns the broker binary, registers a peer), but the fixture's registered name and broker id are both `target-session`, so the id-vs-original-string half of the divergence is invisible; the period is asserted outright.

**upstream** — `pi-intercom` `v0.7.0:index.ts`, `case "send"`: `` content: [{ type: "text", text: `Message sent to ${to}` }] `` — the caller's original target string, no trailing period (`sendTo` is the resolved id and is not what is echoed).

**Impact** — Anyone fixing ICOM-013 must edit this assertion, and its phrasing — a bare `assert_eq!` with no parity citation — reads as an intentional contract rather than an accident. Same shape as the three defects this repo has already found. It additionally masks the id-vs-original-target half.

**Fix** — Land with ICOM-013's normalization: expect pi's `"Message sent to target-session"` once `tools/intercom.rs:152` echoes the caller's original string. In the interim, make the fixture's registered NAME and broker session id differ (register as `target`, id `target-session`) and assert on the name — that alone converts the test from pinning the bug to exposing it.

**Verify** — With name and id made distinct, current code returns `Message sent to target-session.` while the assertion expects `Message sent to target` — red before the ICOM-013 fix, green after.

## Coverage

Read at HEAD `1806375` (tree clean): `cyrup/crates/cyrup-intercom/src/{reply_tracker.rs:80-159, inbound.rs (full), connect.rs:1-440 + tests 500-564, session_state.rs (full), extension.rs:130-380, seams.rs:85-230, tools/intercom.rs:80-380 + 690-724, transport/protocol.rs:15-200, ui/inline_message.rs:1-110, identity.rs:10-22, lib.rs:1-10}`, targeted ranges of `broker/mod.rs` (:320-345 register, :425-445 send, :520-600 presence + shutdown, :790-815 bind), and `tests/reconnect.rs` (:1-130, :262-310). Cross-crate: `cyrup-session-svc/src/host_services.rs:700-770,930-960`, `cyrup-session-svc/src/session.rs:545-565`, `cyrup-ext/src/{native.rs,facade.rs,event.rs}` renderer + `EventKind`, `cyrup-tui/src/app.rs:3028-3045`. Upstream always via `git -C pi-intercom show v0.7.0:<file>` (never the working tree, never v0.6.0): `reply-tracker.ts` in full; `index.ts` `sendIncomingMessage` / `handleIncomingMessage` / `scheduleInboundFlush` / `ensureConnected` / `currentStatus` / `case "send"|"ask"|"pending"|"status"` / `registerMessageRenderer`; `broker/broker.ts` presence, send, register, shutdown. No builds, tests, clippy or npm were run.

Closure verdicts: ICOM-001 and ICOM-002 survived refutation — compared statement-for-statement against upstream, with their regression tests read line by line (both use the real `IdleControlledHost` / real broker, not enum-level stubs). No still-open item was found to be secretly fixed. ICOM-003 was downgraded from closed because `seams.rs:204` is a named, unported production call site that the prior pass folded into prose.

Evidence corrections propagated: ICOM-020's upstream citation (`broker/runtime-claim.ts` `assertNoLiveBroker`) is post-baseline — `git show v0.7.0:broker/runtime-claim.ts` errors, `git log --diff-filter=A` dates it to `db22c07`/v0.8.0 — so it is reclassified not-ported, severity unchanged. ICOM-018 and ICOM-019 both originate in v0.9.0 `57d0da4` and should be ported together. ICOM-015's upstream evidence IS at v0.7.0 and was re-verified. ICOM-005's mechanism was corrected from "premature kill" to "idle-broker leak". ICOM-009 was sharpened: `EventKind::ModelSelect` already exists, so no new seam is needed. ICOM-013 gained the finding that `tools/intercom.rs:208` already uses pi's exact string, making the crate inconsistent with itself.

Test-defect hunt, negative result beyond ICOM-025/026: grepped all six `tests/*.rs` plus `relay.rs`, `tools/contact_supervisor.rs`, `ui/*.rs` and `broker/*.rs` for exact-equality assertions on the strings ICOM-013 lists and for timing/scheduling assertions. Nothing further. `contact_supervisor.rs`'s assertions (`:586-664`) are all on validation errors and structured-reply parsing, with no upstream-divergent string pinned.

Blind spots, inherited and not closed: `src/ui/{compose.rs, session_list.rs, mod.rs}` and `src/relay.rs` were not read line-by-line against upstream `ui/session-list.ts` / `ui/compose.ts`, so an overlay-rendering divergence would still be missed. `src/broker/{ratelimit.rs, routing.rs}`, `transport/framing.rs` and `transport/spawn.rs` were only grepped. `broker/mod.rs` was read in ranges, not in full, so divergences outside register/send/presence/shutdown/bind are unverified. Upstream v0.8.0/v0.9.x line numbers for ICOM-010/011/016/017 were not re-read this pass — only their introducing commits and tags were pinned; their cyrup sides are verified absent at HEAD, so the items stand regardless.

Not filed (cosmetic): `tests/reconnect.rs:9` contains a dangling intra-doc link to `a_tool_connect_that_is_refused_succeeds_on_the_next_call`; the function is `a_tool_call_after_a_refused_broker_retries_and_succeeds` (`:205`), in a `//!` doc on a test binary where rustdoc does not run by default.

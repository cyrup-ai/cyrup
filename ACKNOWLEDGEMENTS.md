# Acknowledgements

cyrup exists because six TypeScript projects did the hard part first. Every architectural decision
worth defending in this codebase was made by someone else, in another language, and proved out in
use before a line of Rust was written.

The source carries **20,561 citations** naming the exact upstream file and line a given Rust item
mirrors. That index is not decoration — it is how equivalence gets audited, and it is a standing
record of authorship. When cyrup is right about something subtle, it is because one of the projects
below was right about it first.

---

## pi — the design

**[earendil-works/pi](https://github.com/earendil-works/pi)** · [pi.dev](https://pi.dev) ·
MIT © 2025 Mario Zechner

The agent harness cyrup is modelled on. The shape of the whole system comes from here: a minimal
core, everything-is-an-extension, and an agent that can extend itself.

What made it worth following closely, rather than merely referencing:

- **The turn loop is small enough to be correct.** Steering and follow-up queues, abort propagation,
  and hook ordering are separable and legible. Porting it surfaced how much care went into the parts
  that look simple — the guard that runs *before* both queue drains, for instance, is load-bearing
  in a way that only becomes obvious when you get it wrong.
- **A vendor-neutral provider layer that is honest about vendors.** Per-provider compat flags, wire
  APIs kept distinct from providers, and catalogs as data. cyrup registers 35 of pi's 38 built-in
  providers over the same 10 chat wire APIs, and the seams held. The three that are not registered —
  `qwen-token-plan`, `qwen-token-plan-cn` and `radius` — have a guard test asserting their absence,
  so a half-finished provider cannot quietly answer requests it cannot serve.
- **Sessions as an append-only JSONL tree.** Simple enough to inspect with `tail`, structured enough
  to fork and resume.
- **Restraint about scope.** pi ships no permission system of its own, which is what makes the
  extension model real rather than nominal.

Reading it end to end is the best documentation the design has.

## pi-subagents — delegation

**[nicobailon/pi-subagents](https://github.com/nicobailon/pi-subagents)** · MIT © 2026 Nico Bailon

*Pi extension for single-agent delegation and scripted multi-agent workflows.*

The largest and most intricate subsystem cyrup follows, and the one with the most behaviour per line.
Subagents run as real OS subprocesses with their own lifecycle, budget, artifact directory and
structured-output contract — a design that is far harder than it looks, because every one of those
concerns has to survive a child that dies, hangs, or returns nonsense.

Details worth naming: the tool-permission model that decides what a child may do; run state that
survives a supervisor restart; and structured output as a declared schema rather than a hopeful
parse. cyrup had a genuine bug here — a declared `outputSchema` that never reached the child, with a
fallback that scraped a JSON fence out of the last message — and the reason it was diagnosable at all
is that upstream's contract was explicit enough to compare against.

## pi-intercom — coordination

**[nicobailon/pi-intercom](https://github.com/nicobailon/pi-intercom)** · MIT © 2026 Nico Bailon

A Unix-socket broker letting a supervisor and its subagents talk: presence, mailboxes, receipts,
namespaces, and clean shutdown.

The protocol is unusually well specified for its size. Registration timeouts, an unregistered-
connection cap with oldest-eviction, a delayed shutdown check, message retention windows, and
per-message attribution are all pinned down rather than left to chance — which is exactly why a
Rust port of it could be checked frame by frame. cyrup's broker carries the same constants
(`REGISTRATION_TIMEOUT_MS`, `MAX_UNREGISTERED_CONNECTIONS`, the 5s shutdown check) because they were
chosen deliberately upstream.

## pi-permission-system — the gate

**[MasuRii/pi-permission-system](https://github.com/MasuRii/pi-permission-system)** ·
MIT © 2026 MasuRii

*Permission enforcement extension for the Pi coding agent.*

Runtime allow / ask / deny policy over every tool call, with per-agent scoping, wildcard matching,
prompt deduplication and an audit trail.

Its exposure rule is precise in a way that matters: `shouldExposeTool` has a narrow, enumerated set
of bypasses and nothing else. cyrup had added a bash bypass that upstream does not have — the effect
was that a configured `tools.bash: deny` could be defeated by a narrower command rule — and the fix
was simply to match upstream. A specification tight enough to be *compared* against is worth more
than one that is merely documented.

## pi-mcp-adapter — the tool bridge

**[nicobailon/pi-mcp-adapter](https://github.com/nicobailon/pi-mcp-adapter)** ·
MIT © 2026 Nico Bailon

*MCP (Model Context Protocol) adapter extension for the Pi coding agent.*

Connects an agent to MCP servers: transports, server lifecycle, tool and prompt registration,
credentials, OAuth, and a `/mcp` surface for managing it all. 172 files of TypeScript, and the
subsystem cyrup is furthest from finishing — `cyrup-mcp` is a port in flight
(`docs/gap-analysis/13*` enumerates it as 425 units).

Two decisions in it are worth naming because they are easy to get wrong and expensive to fix later:

- **The entire tool surface registers synchronously, from disk caches, before anything connects.**
  `installMcpAdapter` reads the config and the metadata cache and registers every direct tool, the
  `mcp` gateway, the slash commands and one command per cached prompt — with no subprocess spawned
  and nothing awaited. The effect is that a session opens instantly with the same surface it had
  last time, and the system prompt does not change shape between a cold start and a warm one. An
  adapter that registered after connecting would make the model's tool list depend on server
  latency.
- **That registration path cannot fail, and is written so it cannot.** Every disk read in it is
  defensive. Porting it made the reason concrete: in cyrup a native extension whose `init()` returns
  `Err` is a fatal startup diagnostic, so a stray `{{{` in a user's `mcp.json` would take the whole
  agent down on a normal path. Upstream had already decided that degrading to an empty surface is
  the only acceptable behaviour.

The port also caught something upstream gets for free and Rust does not: server key order in
`mcp.json` is significant — it is connect order, `/mcp` listing order, and the collision tie-break —
and JavaScript object insertion order preserves it without anyone having to say so. The natural Rust
path through `serde_json::Value` sorts those keys and silently destroys it. That is not an upstream
subtlety so much as an upstream *affordance*, but it is only visible if you read the original
closely enough to notice what it never had to state.

## pi-acp — the editor seam

**[svkozak/pi-acp](https://github.com/svkozak/pi-acp)** · MIT © 2026 Sergii Kozak

*ACP ([Agent Client Protocol](https://agentclientprotocol.com)) adapter for the pi coding agent.*

The odd one out, and the reason it is worth reading closely. Every other project here is a pi
extension — it loads into pi and is handed typed objects. pi-acp is the inverse: it sits entirely
outside, spawns `pi --mode rpc`, and reconstructs the agent's state from untyped NDJSON on the
child's stdout while speaking JSON-RPC to an editor on its own. It knows nothing that did not
survive a serialize/parse round trip, and roughly 40% of its code exists to cope with that —
`translate/bash.ts` probes twelve key paths to find one command string.

Which is exactly what makes it a good specification. An adapter that can only observe has to state
what it observes, and three of its rules are the kind that are invisible until you get them wrong:

- **A turn ends on `agent_settled`, and on nothing else.** pi emits several `turn_end` and
  `agent_end` events for a single user prompt whenever retry, compaction, or a queued continuation
  runs. Resolving the ACP `session/prompt` on either of them closes the editor's turn while the
  agent is still working — the user sees a finished response and then more text arriving into a
  session the client believes is idle. `session.ts` names this in a comment and structures the whole
  turn state machine around it. It is the correctness core of the adapter and it is one boolean
  (`inAgentLoop`) away from being wrong.
- **Tool-call status is monotonic.** Late `toolcall_*` deltas can arrive after execution has already
  started, and a client that sees `in_progress` fall back to `pending` hides its progress UI. So
  `currentToolCalls` exists solely to refuse the downgrade. That is a rendering invariant of real
  clients, discovered by using them, and it is not written down in the protocol.
- **A structured diff has to be manufactured.** pi's tool events do not carry the old and new text
  of an edit, so `tool_execution_start` snapshots the file, `tool_execution_end` re-reads it, and the
  ACP `diff` content block is synthesised from the pair — with a 1-based line number inferred from a
  *unique* `oldText` match, and no location emitted at all when the match is ambiguous. The
  restraint in that last clause is the part worth copying.

The port is planned rather than done (`docs/gap-analysis/15-cyrup-acp.md`), and planning it was
already useful, because most of pi-acp turns out to be scaffolding against a limitation cyrup does
not have. `cyrup-acp` is a workspace crate: it binds to `AgentSession` directly, so the subprocess,
its ENOENT diagnostics, its ANSI prelude scraping and its twelve-key probes have no counterpart at
all, and `AgentSessionEvent` supplies typed variants — `QueueUpdate`, `BashExecutionUpdate`,
`SessionInfoChanged` — that pi-acp had to infer or fake. What survives the deletion is the part
that was never about the transport: the three rules above, the exact user-visible strings, and the
ordering constraint that `available_commands_update` must follow the `session/new` response because
clients drop notifications for a session id they have not yet seen.

---

## On the relationship

cyrup is a reimplementation, not a fork or a repackaging. It is written in Rust, uses WebAssembly
components where pi uses runtime TypeScript, and diverges wherever the two languages genuinely
differ — those divergences are marked `CYRUP-DELTA` in the source, each naming the upstream line and
the reason.

None of that lessens the debt. Design is the expensive part, and it was already paid. Where cyrup
differs, it is usually because Rust forced the issue; where it agrees — which is nearly everywhere —
it is because these projects had already worked out the right answer.

Each remains the reference implementation of its own behaviour, and cyrup treats that as binding:
where the two disagree, the upstream is right by definition and cyrup is what changes. That rule is
what makes the citations above auditable rather than decorative — a divergence cannot be quietly
preferred, it has to be recorded as a `CYRUP-DELTA` naming the upstream symbol and the reason it was
forced.

Thank you to Mario Zechner, Nico Bailon, MasuRii and Sergii Kozak.

---

Full license texts and copyright notices: [`LICENSE`](LICENSE).

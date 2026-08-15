# Acknowledgements

cyrup exists because four TypeScript projects did the hard part first. Every architectural decision
worth defending in this codebase was made by someone else, in another language, and proved out in
use before a line of Rust was written.

The source carries **19,260 citations** naming the exact upstream file and line a given Rust item
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

Thank you to Mario Zechner, Nico Bailon and MasuRii.

---

Full license texts and copyright notices: [`LICENSE`](LICENSE).

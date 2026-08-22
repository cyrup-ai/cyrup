---
stage: new
status: done
updated: 2026-08-22 18:28
---

# Decide The Config Type Model — The lenient / Typed-Reader Divergence Family

## Description

Four recorded divergences share one root cause, and they want **one deliberate decision about the
config type model, not four patches**. Recorded at
`docs/gap-analysis/13-cyrup-mcp-STATUS.md:276`, each measured against upstream on node 22 @
`v2.26.1`, none fixed because each lives outside the files the wave owned.

1. **Non-object `env` hashes wrong.** `computeServerHash({command:"x",env:"abc"})` = `01ed7340…`
   upstream; the writer produces `f0211144…` — upstream's digest for the same definition with
   `env` **absent**. Same family: `env: []`, `env: 5`, `env: true` all hash as `{}` upstream
   (`1d224401…`) and as absent here.

2. **The reader is worse than the writer, and the note says otherwise.** The writer degrades to
   `None`; the **reader drops the whole server** from the direct-tool surface —
   `mcp_direct_tools.rs::extract_server_map` skips any entry `from_value::<ServerEntry>` rejects,
   and `env: Option<BTreeMap<String, Value>>` rejects a string, array, number or bool. Measured
   over six definitions: **the reader keeps three where upstream keeps six.** `args: [1,"b"]` and
   `command: 5` behave identically — one root cause, not three items.

3. **`secrets.rs:386` spawns children with a partial env.** `entry.env.as_deref()` `Deref`s to
   the string members only, so `env: {"GOOD":"1","BAD":5}` spawns with `GOOD=1`; before the
   `StringRecord` retype, `lenient` dropped the whole block and it spawned with none. Upstream
   does neither.

4. **`MCP-174`** — `search_keywords` behind `lenient` (`config.rs:715`) drops the whole key where
   upstream's `resolveSearchKeywords` skips only the offending entry
   (`13-cyrup-mcp-STATUS.md:690`).

The decision to make: does cyrup keep typed reader fields with `lenient` dropping what does not
fit, or does it adopt upstream's per-key skip semantics? Patching these one at a time will produce
four different answers to the same question.

Also dangling and cheap to fix once decided: `config.rs:618-621` defers to "`13c`'s MCP-144
notes", which say nothing about the non-object-`env` case; and the docs disagree on whether this
is the fifth or sixth divergence.

## DECISION (recorded 2026-08-22) — adopt upstream's per-key skip semantics

The maintainer delegated this call. It goes to **per-key skip**, matching upstream, not to keeping
typed fields with `lenient` dropping what does not fit. Three things decide it:

1. **The current behaviour is silent data loss, not a different error.** Measured over six
   definitions, the reader keeps three where upstream keeps six. A Pi-written config that loads
   upstream loses half its servers here, with no diagnostic. That is the worst failure shape a port
   can have — it is invisible until someone notices a server missing.
2. **It corrupts identity, not just parsing.** `computeServerHash({command:"x",env:"abc"})` is
   `01ed7340…` upstream and `f0211144…` here — the digest upstream uses for the same definition with
   `env` **absent**. Two definitions that upstream considers distinct collide here, so the divergence
   propagates into caching and dedup rather than staying at the edge.
3. **Parity is this repo's stated hard requirement** (R-00-013 and the arch-00 port rules). "Typed
   fields are more idiomatic Rust" is a real argument, but it is an argument for changing upstream,
   not for diverging from it silently. Where cyrup deliberately deviates it says so in a
   `[CYRUP-DELTA]` note; nobody wrote one here, which is itself evidence this was drift, not a choice.

**What that means concretely** — one root cause, so all four resolve together:
- `mcp_direct_tools.rs::extract_server_map` stops discarding an entry `from_value::<ServerEntry>`
  rejects; it keeps the entry and skips only the key that failed.
- Non-object `env` (`"abc"`, `[]`, `5`, `true`) normalizes to `{}` so it hashes as `1d224401…`,
  matching upstream — NOT to absent. Same for `args: [1,"b"]` and `command: 5`.
- `secrets.rs:386` stops spawning children with a partial env; it applies the same normalization the
  reader does, so the spawned environment matches what upstream would spawn.
- MCP-174: `search_keywords` skips the offending entry, keeping the rest of the key.

**Do not reason about upstream — measure it.** Every claim above was recorded against node 22 @
`v2.26.1`, and the acceptance criteria require reader/writer agreement to be measured the same way.
If node 22 is unavailable in the execution environment, that is a BLOCKER to report, not something to
substitute reasoning for.

## Acceptance Criteria

- [ ] A single stated decision on the config type model, written into `13b-mcp-config.md`
- [ ] All four divergences resolved consistently with that decision
- [ ] Reader and writer agree — measured against upstream on node 22, not reasoned about
- [ ] `config.rs:621`'s cross-reference points somewhere real
- [ ] The fifth/sixth divergence numbering is made consistent across the docs
- [ ] `cargo nextest run --workspace` and `cargo clippy --workspace --all-targets` are clean

---
stage: new
status: done
updated: 2026-08-22 18:28
---

# Fix The 961 rustdoc Warnings

## Description

`cargo doc --workspace --no-deps` **exits 0 but emits 961 warnings** (measured 2026-08-22, rustc
1.98.0). Nothing is broken enough to fail the build, which is exactly why it has accumulated.

Two classes account for 936 of them:

| count | warning | what it means |
|---:|---|---|
| 494 | `unresolved link to X` | the doc names an item that does not resolve |
| 442 | `public documentation for X links to private item Y` | public docs point at something a reader cannot reach |
| 5 | `redundant explicit link target` | |
| 1 | `unclosed HTML tag` | |
| 1 | `X is both a function and a module` | ambiguous link, needs a disambiguator |

By crate — **one crate is 59% of the total**:

| crate | warnings |
|---|---:|
| `cyrup-ext-subagents` | 564 |
| `cyrup-tui` | 97 |
| `cyrup-provider` | 75 |
| `cyrup-ext` | 38 |
| `cyrup-mcp` | 32 |
| `cyrup-permission-system` | 28 |
| `cyrup-intercom` | 20 |
| `cyrup-session-svc` | 19 |
| `cyrup` | 16 |
| `cyrup-tools` | 10 |
| `cyrup-modes` | 9 |
| `cyrup-ext-sdk` | 8 |
| `cyrup-config` | 6 |
| `cyrup-agent` | 6 |

## Do the unresolved links first

They are not all cosmetic. Some name APIs that **do not exist**, which makes the doc actively
wrong rather than merely unlinked:

```
warning: unresolved link to `ToolCall::is_cancelled`
  --> crates/cyrup-ext-sdk/src/ctx.rs:213:20
     the struct `ToolCall` has no field or associated item named `is_cancelled`
```

A doc that describes an API the reader cannot call is the same failure class as the false comments
found during the cyrup-mcp audit — the doc outlives the reader's memory of whether to trust it.
Triage each into: the item was renamed (fix the link), the item never existed (fix or delete the
claim), or the path is right but needs disambiguation.

The 442 private-item links are a **policy decision, not 442 edits**: either those items become
`pub`, or the links become plain code spans. Decide once, apply uniformly. Many are likely
deliberate cross-references in explanatory docs.

Suggest working crate-by-crate, largest first, so progress is measurable. Consider
`#![warn(rustdoc::broken_intra_doc_links)]` and a CI gate once the count is near zero — otherwise
it grows straight back.

## DECISION (recorded 2026-08-22) — plain code spans, with one deterministic carve-out

The maintainer delegated the policy call for the 442 `public documentation for X links to private
item Y` warnings. It goes to **turning those links into plain code spans**, applied uniformly.

Why not promote the items to `pub`: it would grow the public API of 14 crates for a formatting
reason, and every promoted item becomes something downstream can depend on and that the next
refactor has to keep. cyrup-core is the worked example — its five warnings are all links to the
`de_*` content deserializers, which are private **on purpose**; the crate's own docs explain that
they exist to be named by serde attributes, not called. Promoting them to satisfy rustdoc would
invert a deliberate design decision to silence a lint.

Why not case-by-case: the task itself warns against it, and 442 individual judgment calls across 14
crates will not come out consistent.

**The one carve-out, stated as a rule rather than a judgment** so it stays uniform: if the linked
item is already publicly reachable by another path — re-exported at the crate root, or `pub` in a
private module that is itself re-exported — then the link is not wrong, only mis-addressed. Fix the
path to the public one instead of downgrading it to a code span. This is deterministic (either a
public path exists or it does not), so it does not reopen the case-by-case debate.

**Order of work.** Do the 494 `unresolved link` warnings FIRST, as the task says. They are the ones
that make docs actively wrong — a doc naming an API that does not exist, like the recorded
`ToolCall::is_cancelled`. The 442 private-item links are cosmetic by comparison: they point at
something real that the reader merely cannot click.

**Regrowth gate.** After the sweep, add `#![warn(rustdoc::private_intra_doc_links)]` plus
`#![warn(rustdoc::broken_intra_doc_links)]` at each crate root and a `cargo doc --workspace
--no-deps` CI step, so the count cannot climb back.

## Acceptance Criteria

- [ ] `cargo doc --workspace --no-deps` emits zero warnings, or every remaining one is justified in writing
- [ ] Every `unresolved link` naming a nonexistent API is fixed or deleted — no doc describes an API that is not there
- [ ] A single stated policy for public-docs-linking-private-items, applied uniformly
- [ ] A CI gate or crate-level lint prevents regrowth

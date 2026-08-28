---
title: "A CYRUP-DELTA marker still advertises a divergence that item 2 closed"
priority: MEDIUM
crate: cyrup-tools
source: agents' surfaced questions from workflow wf_12c49023-adf
stage: qa
status: completed
updated: 2026-08-28 20:55
---

# One stale `CYRUP-DELTA` marker

QA verdict: **9/10**. All seven residuals are implemented, and the three comment corrections from
the previous round are correct and complete — verified, not accepted: `read_line_range`'s doc names
all four operators accurately, the `9007199254740993` → `9007199254740992` re-attribution to
`js_number` holds (serde parses it as `PosInt`, `as_f64` narrows it, `js_arg` delegates), and
`find`/`ls` point at `render_grep`, which does carry the reasoning. Doc links resolve. 338 + 1302
tests green, counts unchanged, clippy clean.

One item remains, and it is a worse class than the three that were fixed.

---

## The marker on `cwd_relative_path` says the Windows divergence is still open

[`path.rs:452-457`](../../crates/cyrup-tools/src/path.rs):

> **[CYRUP-DELTA — the inside-cwd compare is case-SENSITIVE on Windows]** Node's
> `path.win32.relative` compares path segments CASE-INSENSITIVELY, whereas `Path::strip_prefix` is
> byte-exact. On Windows only, `C:\Foo\AGENTS.md` under a `cwd` spelled `c:\foo` therefore returns
> `None` here (and renders as the absolute path) where Pi returns `AGENTS.md`. Unix is unaffected —
> `path.posix.relative` is case-sensitive too. **Closing it needs a `cfg(windows)` case-folded
> comparison in place of `strip_prefix`; awaiting a decision.**

Item 2 closed it. Three lines below the marker, the function calls `strip_cwd_prefix`, whose
`cfg(windows)` arm is precisely the case-folded comparison the marker asks for, and whose
`cfg(not(windows))` arm keeps Unix byte-exact.

### Why this outranks the three comments already fixed

1. **It is a `CYRUP-DELTA` marker** — the artifact the parity audits inventory. This whole
   workstream started from a sweep of all 87 of them. A marker advertising a CLOSED divergence gets
   re-filed as an open gap. That is exactly what happened to `bash:72`: the code was fixed, the
   record was not, and it cost two full re-derivations before anyone noticed the gap did not exist.
   This is the same failure with the sign reversed.
2. **"awaiting a decision"** is the only occurrence of that phrase in the codebase, and it is the
   fabricated-escalation pattern that has already been corrected twice in this session. Left in
   place, the next sweep escalates a decision that nobody needs to make.
3. **It names the wrong function.** The code calls `strip_cwd_prefix`, not `strip_prefix`.

### Fix

Rewrite the marker as a CLOSED note, or remove it. If it stays, it must say the divergence is
closed, name `strip_cwd_prefix`, and drop the decision language. What is worth keeping either way:

- Why the Unix arm is deliberately byte-exact (`path.posix.relative` is case-sensitive, so folding
  there would be a divergence rather than a fix).
- That `eq_ignore_ascii_case` is the chosen width, and that a non-ASCII segment compares exactly —
  the conservative direction, since it can only fail to strip, never strip the wrong thing.

Check first whether any parity tooling greps for marker COUNT in this file. The one marker-scanning
test in `path.rs` (`cfg072_the_widening_carries_a_delta_naming_what_it_extends`) anchors on
`fn windows_home_from` and does not touch this marker, so it is unaffected — but confirm rather
than assume.

### Also: the inner comment names the old function

[`path.rs:462-464`](../../crates/cyrup-tools/src/path.rs) still reads *"because `strip_prefix("")`
succeeds and hands the whole absolute path back"*. The behavioural claim is still true — the Unix
arm delegates to `strip_prefix`, and the Windows arm returns the whole path for an empty base by an
explicit early return — but it names a function this code no longer calls directly.

---

## Note on how this was missed

The previous QA pass swept `cyrup-tui` only, because that is where the three comments were. This
staleness was introduced by the same `/exec` that fixed item 2, in `cyrup-tools`, and survived a
round because the sweep was scoped to where the last defect happened rather than to everything the
task touched.

## Definition of done

1. The `path.rs` marker no longer advertises an open divergence, names `strip_cwd_prefix`, and
   carries no decision language.
2. The inner comment at `:462-464` names the function the code actually calls.
3. Confirmed that no marker-scanning test depends on this marker.
4. No behaviour change; suite green; every existing guard still passes.

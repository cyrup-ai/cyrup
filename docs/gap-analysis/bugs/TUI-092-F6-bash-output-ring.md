---
stage: aug
status: done
updated: 2026-08-18 02:37
---

# TUI-092-F6 — Bound `BashExecution::output_lines` to a rolling window with an omission counter

> **Part of** [`TUI-092-progressive-lockup.md`](TUI-092-progressive-lockup.md) (the umbrella audit).
> Self-contained change in `crates/cyrup-tui/src/bash.rs` plus one render row.
>
> **Kind** `cyrup-original` · **Severity** high · **Effort** S · **Phase driven** 2→3 during chatty
> runs (`! npm install` with 200k lines)

## Coordinates with

Nothing. Independent of F1–F5, F7, F8. Matches the executor's own truncation ethos
([`cyrup_tools::truncate::DEFAULT_MAX_LINES`](../../../crates/cyrup-tools/src/truncate.rs#L11) =
2000) — the live block makes the same trade the recorded result already makes.

---

## Evidence

[`bash.rs:45`](../../../crates/cyrup-tui/src/bash.rs#L45) (the `output_lines: Vec<String>` field) +
[`append_output`](../../../crates/cyrup-tui/src/bash.rs#L121): every chunk the executor streams is
split and pushed, uncapped, for the block's whole lifetime. The session-side sink forwards **every**
sanitised chunk ([`cyrup-session-svc/src/bash.rs:179-184`](../../../crates/cyrup-session-svc/src/bash.rs#L179));
the rolling 100 KB cap (`ROLLING_MAX_BYTES`) applies to the *result preview*, not the live stream.
A `! npm install` with 200k lines accumulates 200k `String`s; the collapsed render shows the last
20 of them. On commit the whole vec renders into scrollback (`Entry::Bash` arm in
[`entry_lines`](../../../crates/cyrup-tui/src/transcript.rs#L2962), which clones the live
`BashExecution` and calls [`render_lines`](../../../crates/cyrup-tui/src/bash.rs#L211) at the
current expansion flag — no separate commit-time truncation exists).

**Cost shape.** memory ∝ run output, for the whole block lifetime, independent of what is ever
displayed (preview caps at [`PREVIEW_LINES`](../../../crates/cyrup-tui/src/bash.rs#L20) = 20; even
`Ctrl+O`-expanded, nobody scrolls through 200k rows).

---

## Current shape (read before editing)

`crates/cyrup-tui/src/bash.rs` today:

```rust
pub struct BashExecution {
    command: String,
    excluded: bool,
    output_lines: Vec<String>,          // <- L45, unbounded
    status: BashStatus,
    exit_code: Option<i32>,
    expanded: bool,
    started: Instant,
    truncated: bool,
    full_output_path: Option<String>,
}
```

Six call sites touch `output_lines` today and every one needs to change for the type swap
`Vec<String>` → `std::collections::VecDeque<String>`:

| Line | Site | Current | Why it breaks with `VecDeque` |
|---|---|---|---|
| [`:45`](../../../crates/cyrup-tui/src/bash.rs#L45) | field decl | `Vec<String>` | type swap itself |
| [`:73`](../../../crates/cyrup-tui/src/bash.rs#L73) | `new()` init | `Vec::new()` | → `VecDeque::new()` |
| [`:104-105`](../../../crates/cyrup-tui/src/bash.rs#L104) | `output()` | `self.output_lines.join("\n")` | `VecDeque` has no `.join`; needs `.iter().map(String::as_str).collect::<Vec<_>>().join("\n")` (or an `itertools`-free fold) |
| [`:124-128`](../../../crates/cyrup-tui/src/bash.rs#L121) | `append_output` | `.last_mut()` / `.extend(...)` | `VecDeque::back_mut()` replaces `.last_mut()`; `.extend()` is unchanged (both implement `Extend<String>`) — **this is also where eviction is added** |
| [`:178-189`](../../../crates/cyrup-tui/src/bash.rs#L178) | `context_truncated` | `.len()`, `.iter().map(String::len).sum()` | both work unchanged on `VecDeque` — but the semantics of the line-count leg must change (see below) |
| [`:257`](../../../crates/cyrup-tui/src/bash.rs#L257), [`:260-261`](../../../crates/cyrup-tui/src/bash.rs#L260) | `render_lines_at` | `.join("\n")`, `.clone()` into `Vec<String>` | same `.join` gap; `.clone()` on a `VecDeque<String>` yields a `VecDeque<String>`, but `visible` is typed `Vec<String>` (fed to `for line in &visible` and mixed with `vt.lines: Vec<String>` from the other branch) — needs `.iter().cloned().collect::<Vec<_>>()` |

**A subtlety that must be handled correctly, not glossed over:** `context_truncated()`
([`:178-192`](../../../crates/cyrup-tui/src/bash.rs#L178)) currently gates on
`self.output_lines.len() > MAX_LINES` where `MAX_LINES = 2000` — the *same* 2000 this fix uses for
`MAX_OUTPUT_LINES`. Once `output_lines` is ring-bounded to ≤ 2000, `.len() > 2000` can **never be
true again** — that leg of `context_truncated` becomes permanently dead code, silently changing
behavior for the one existing test that depends on it
([`x13_truncated_output_names_the_spool_file`](../../../crates/cyrup-tui/src/bash.rs#L780),
"MIRROR 3": `ctx.append_output(&"x\n".repeat(2001))` then asserts the truncation warning still
renders because `contextTruncation.truncated` trips on line count alone, with no executor-reported
truncation and no `context_truncated` byte overflow — 2001 one-char lines is nowhere near 50 KB).
**The line-count leg of `context_truncated` must be replaced with `self.omitted_lines > 0`** — ring
eviction already means "more logical lines existed than fit," which is exactly what that leg was
testing before the ring existed. Do not just delete the leg; replace it, or that existing test goes
red.

---

## FIX — a bounded ring with an omission counter

### 1. Field + constant ([`bash.rs:20-21`](../../../crates/cyrup-tui/src/bash.rs#L20) area, and the struct at `:38-65`)

```rust
use std::collections::VecDeque;

/// The live block retains at most the LAST `MAX_OUTPUT_LINES` output lines — the same bound Pi's
/// executor applies to the recorded result (`truncate.ts`'s `DEFAULT_MAX_LINES`); earlier lines are
/// counted, not kept (TUI-092 F6). Deliberately equal to `context_truncated`'s own `MAX_LINES` so
/// hitting the ring cap and hitting the context-truncation threshold are the same event.
const MAX_OUTPUT_LINES: usize = 2000;

pub struct BashExecution {
    command: String,
    excluded: bool,
    /// Accumulated output, one logical line per element (`outputLines`), bounded to the last
    /// [`MAX_OUTPUT_LINES`] — earlier lines are evicted from the front in [`Self::append_output`]
    /// and counted in `omitted_lines`, never rendered again (TUI-092 F6).
    output_lines: VecDeque<String>,
    /// Count of lines evicted from the front of `output_lines` so far. Rendered as a one-line dim
    /// omission notice ahead of the retained window whenever non-zero (TUI-092 F6).
    omitted_lines: usize,
    status: BashStatus,
    exit_code: Option<i32>,
    expanded: bool,
    started: Instant,
    truncated: bool,
    full_output_path: Option<String>,
}
```

### 2. Constructor ([`bash.rs:68-79`](../../../crates/cyrup-tui/src/bash.rs#L68))

```rust
pub fn new(command: impl Into<String>, excluded: bool) -> Self {
    BashExecution {
        command: command.into(),
        excluded,
        output_lines: VecDeque::new(),
        omitted_lines: 0,
        status: BashStatus::Running,
        exit_code: None,
        expanded: false,
        started: Instant::now(),
        truncated: false,
        full_output_path: None,
    }
}
```

### 3. `output()` ([`bash.rs:104-106`](../../../crates/cyrup-tui/src/bash.rs#L104)) — `VecDeque` has no `.join`

```rust
pub fn output(&self) -> String {
    self.output_lines.iter().cloned().collect::<Vec<_>>().join("\n")
}
```

### 4. `append_output` ([`bash.rs:121-129`](../../../crates/cyrup-tui/src/bash.rs#L121)) — swap `.last_mut()` → `.back_mut()`, add eviction

```rust
pub fn append_output(&mut self, chunk: &str) {
    let clean = crate::ansi::strip_ansi(chunk).replace("\r\n", "\n").replace('\r', "\n");
    let new_lines: Vec<&str> = clean.split('\n').collect();
    if let (Some(last), Some(first)) = (self.output_lines.back_mut(), new_lines.first()) {
        last.push_str(first);
        self.output_lines.extend(new_lines.iter().skip(1).map(|s| (*s).to_string()));
    } else {
        self.output_lines.extend(new_lines.iter().map(|s| (*s).to_string()));
    }
    while self.output_lines.len() > MAX_OUTPUT_LINES {
        self.output_lines.pop_front();
        self.omitted_lines += 1;
    }
}
```

Eviction runs after every chunk (not just at the end), so memory is bounded continuously through a
long-running command, not just at completion.

### 5. `context_truncated` ([`bash.rs:178-192`](../../../crates/cyrup-tui/src/bash.rs#L178)) — replace the line-count leg with the omission counter

```rust
fn context_truncated(&self) -> bool {
    /// `truncate.ts:12` `DEFAULT_MAX_BYTES` (50 KB).
    const MAX_BYTES: usize = 50 * 1024;
    // Ring eviction (TUI-092 F6) already means more logical lines existed than were kept — the
    // direct replacement for the old `self.output_lines.len() > MAX_LINES` check, which became
    // unreachable once `output_lines` itself is bounded to `MAX_OUTPUT_LINES` (= the old `MAX_LINES`).
    if self.omitted_lines > 0 {
        return true;
    }
    // `\n`-joined, matching `getOutput`/`updateDisplay`'s `this.outputLines.join("\n")`.
    let bytes: usize = self.output_lines.iter().map(String::len).sum::<usize>()
        + self.output_lines.len().saturating_sub(1);
    bytes > MAX_BYTES
}
```

### 6. `render_lines_at` — `.join`/`.clone()` fixups plus the new omission row ([`bash.rs:255-280`](../../../crates/cyrup-tui/src/bash.rs#L255) area)

```rust
let body_width = width.saturating_sub(2).max(1);
let joined = self.output_lines.iter().cloned().collect::<Vec<_>>().join("\n");
let vt = crate::chrome::truncate_to_visual_lines(&joined, PREVIEW_LINES, body_width);
let hidden = vt.skipped;
let visible: Vec<String> = if self.expanded {
    self.output_lines.iter().cloned().collect()
} else {
    vt.lines
};
if !visible.is_empty() || self.omitted_lines > 0 {
    // One shared leading blank for the whole output section (the omission notice, when present,
    // and the retained window are one logical `Text` group — X3's `\n${displayText}` behavior).
    out.push(Line::default());
    if self.omitted_lines > 0 {
        // TUI-092 F6 — dim, one row, ahead of the retained window, in BOTH the collapsed and
        // expanded views (this branch runs regardless of `self.expanded`).
        out.extend(crate::transcript::text_lines_of(
            &Line::styled(
                format!("… ({} earlier lines omitted) …", self.omitted_lines),
                theme.dim_style(),
            ),
            width,
            1,
        ));
    }
    for line in &visible {
        out.extend(crate::transcript::text_lines_of(
            &Line::from(Span::styled(line.clone(), theme.muted_style())),
            width,
            1,
        ));
    }
}
```

This replaces the existing `if !visible.is_empty() { … }` block
([`bash.rs:262-279`](../../../crates/cyrup-tui/src/bash.rs#L262)) in place — same spacer/margin
behavior when `omitted_lines == 0` (today's only case), new omission row prepended once the ring
has evicted anything.

No separate change is needed for the committed `Entry::Bash` arm in
[`transcript.rs:2962-2967`](../../../crates/cyrup-tui/src/transcript.rs#L2962): it clones the live
`BashExecution`, sets `expanded` from the broadcast flag, and calls `render_lines`, so the omission
row falls out of the same code path for free.

---

## Existing tests this interacts with

`cargo test -p cyrup-tui bash::` will exercise these; nothing outside `bash.rs` needs a change.

- [`append_merges_incomplete_lines`](../../../crates/cyrup-tui/src/bash.rs#L417) — well under 2000
  lines, unaffected by the ring; `output()`'s new `VecDeque` join must still produce the same
  string.
- [`x13_truncated_output_names_the_spool_file`](../../../crates/cyrup-tui/src/bash.rs#L780),
  "MIRROR 3" (`ctx.append_output(&"x\n".repeat(2001))`) — this is the test that silently breaks if
  the line-count leg of `context_truncated` is deleted instead of replaced with
  `self.omitted_lines > 0`; see the subtlety called out above. With the replacement, 2001 pushed
  lines → 1 eviction (`omitted_lines == 1`) → `context_truncated()` still returns `true` → the test
  still passes.
- [`collapsed_preview_truncates_and_counts_hidden`](../../../crates/cyrup-tui/src/bash.rs#L578) and
  [`expand_and_collapse_hints_match_upstream_wording`](../../../crates/cyrup-tui/src/bash.rs#L546)
  push 30 lines — well under the ring bound, `omitted_lines` stays `0`, no new omission row appears,
  existing assertions on the `PREVIEW_LINES`/hidden-count hint are untouched.

---

## Definition of done

* **Every live collection has a stated bound.** `BashExecution::output_lines` is a
  `VecDeque<String>` capped at `MAX_OUTPUT_LINES` (2000) with an `omitted_lines: usize` counter;
  eviction happens in `append_output` after every chunk, not just at completion.
* `context_truncated`'s line-count leg is replaced by `self.omitted_lines > 0` (not deleted), so
  hitting the ring cap is still treated as context truncation exactly as hitting the old
  unbounded-`len()` check was.
* The omission row — `… ({omitted_lines} earlier lines omitted) …`, dim-styled via
  `theme.dim_style()` — renders exactly once, when `omitted_lines > 0`, ahead of the retained
  window, in both the collapsed and expanded views (both flow through the same
  `render_lines_at` branch).
* All six `output_lines` call sites (field, constructor, `output()`, `append_output`,
  `context_truncated`, `render_lines_at`) compile against the new `VecDeque<String>` type — no
  leftover `Vec`-only method calls (`.join` used directly on the deque, bare `.last_mut()`).

## Do not touch

The session-side sink's per-chunk forwarding (that is the live stream the TUI consumes; bounding
the *consumer* here is the fix, not the producer), and the rolling 100 KB cap on the *result
preview* in `cyrup-session-svc/src/bash.rs` (a separate, already-correct bound). Do not touch
`transcript.rs`'s `Entry::Bash` commit arm — it needs no edit, per the note above.

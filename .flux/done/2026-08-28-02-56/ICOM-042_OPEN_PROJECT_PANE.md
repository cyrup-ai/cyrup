---
stage: qa
status: completed
updated: 2026-08-30 15:14
---

# ICOM-042 rework 3 — two off-by-one citations, introduced while fixing off-by-one citations

> **QA verdict: 9/10.** All three specified items are implemented and independently verified. The
> headline outcome holds: **`cargo doc -p cyrup-intercom --no-deps` exits 0**, for the first time
> since the launcher landed. `cargo clippy -p cyrup-intercom --all-targets` clean,
> `cargo test -p cyrup-intercom` **293 passed / 0 failed**.
>
> One defect, appearing twice, and it is the task's own signature defect class: a wrong `index.ts`
> line number in a comment. ICOM-042 has now corrected `:1221` (§0.3 of the original brief) and
> `:1532` → `:1533` (rework 1) — and this pass shipped two fresh instances while doing so.

---

## Verified complete — do NOT re-litigate

* **`ProjectPaneLaunch::launcher_name`** — `&'static str`, set from `self.name()` at the single
  construction site. Mirrors `PaneLaunchError::backend` exactly, as claimed. Upstream's
  `ProjectPaneLaunch` ([`project-agent.ts:28-33`](../../tmp/pi-intercom/project-agent.ts)) really
  does carry only `paneId` / `projectRoot` / `command` / `herdrVersion`, so "no upstream counterpart"
  is accurate.
* **`send.rs:247-254`** reads `pane.launcher_name`. The `project_pane_launcher()` re-read and the
  `map_or("project", …)` fallback are gone — `grep 'map_or("project"'` returns nothing crate-wide,
  and `project_pane_launcher()` now has exactly one reader, `mod.rs:179`.
* **`resolve_agent_command` is `pub(crate)`**, `identity.rs:47`'s link is intact, and rustdoc emits
  neither error nor warning.
* **`name()`'s four-frame table is correct.** Each frame checked against the code that renders it:
  `project_pane.rs:87` (Display), `send.rs:252` (`Opened …`), `mod.rs:186-192` (the missing-peer
  sentence, which composes `{name} project pane` then embeds it), `project_target.rs:245` (the
  timeout sentence — genuinely the only frame with no trailing "project pane").
* No behaviour change: with a launcher bound, old and new both render `Herdr`.

---

## 1. The only outstanding item — `index.ts:2393` should be `:2394`

Two comments added this pass cite the upstream result line one line too high:

| File | Line | Text |
| --- | --- | --- |
| [`project_pane.rs`](../../crates/cyrup-intercom/src/project_pane.rs) | `:120` | `` …so `index.ts:2393` hard-codes `Herdr` in the result line.`` |
| [`send.rs`](../../crates/cyrup-intercom/src/tools/intercom/send.rs) | `:248` | `` // `index.ts:2393` hard-codes `Herdr`; here the name rides on the launch… `` |

What is actually there in the v0.12.0 checkout:

```
2393                text: target.projectPane
2394                  ? `Opened Herdr project pane ${target.projectPane.paneId} for ${…projectRoot} and sent message to ${targetDisplay}`
2395                  : inferredAsk ? `Reply sent to ${targetDisplay} (inferred from pending ask)` : `Message sent to ${targetDisplay}`,
```

`:2393` is the ternary's **condition** (`text: target.projectPane`). The hard-coded `Herdr` — the
thing both comments are pointing at — is on **`:2394`**.

**Fix:** change both to `index.ts:2394`. Nothing else in either comment changes; both are otherwise
accurate.

### Do not touch this one

`extension.rs:329` also matches a `:2393` grep, but it cites **`v0.10.1 index.ts:2393`** for
`duplicateSessionNames` — a different tag and a different subject, unrelated to this task and not
verifiable against the v0.12.0 checkout. Leave it alone.

### While you are in there — verify, do not assume

The two comments being fixed sit next to `send.rs:245`'s pre-existing `(:2392-2396)` span for "the
pane branch OUTRANKS the inferred-reply branch". That span covers the whole `content` block and is
defensible as written, so it needs no change — but confirm it against the checkout rather than
taking this brief's word for it, the same way `:2393` should have been confirmed before it was
written twice.

---

## 2. Definition of done

1. No `index.ts:2393` citation remains in `project_pane.rs` or `send.rs`; both read `:2394`, and each
   was checked against [`../../tmp/pi-intercom/index.ts`](../../tmp/pi-intercom/index.ts) with line
   numbers displayed — not inferred from a `sed` range.
2. `extension.rs:329` is unchanged.
3. `cargo clippy -p cyrup-intercom --all-targets` clean, `cargo doc -p cyrup-intercom --no-deps`
   exits 0, `cargo test -p cyrup-intercom` at 293 passing.
4. Nothing else changes. This is a two-character edit in two comments; if the diff is larger than
   that, something has gone wrong.

---

/home/user/cyrup/.flux/todo/ICOM-042_OPEN_PROJECT_PANE.md

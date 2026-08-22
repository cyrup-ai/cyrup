---
stage: qa
status: completed
updated: 2026-08-22 18:06
---

# Repair Three Stale file:line Citations in cyrup-core Docs

## Description

cyrup-core's docs lean heavily on `file:line` provenance pointers into sibling crates, and three of them now point at unrelated code after upstream refactors. crates/cyrup-core/src/message/stop_reason.rs:67 cites `entry.rs:262-285` for `Entry`'s `Deserialize`, which actually lives at crates/cyrup-session/src/entry.rs:295-317 (262-278 is `parent_id()`/`type_tag()`). stop_reason.rs:70 cites `manager.rs:826-831` for `entries_have_assistant`, which is at crates/cyrup-session/src/manager.rs:875-880 (826-831 is `session_file()`/`is_persisted()`). crates/cyrup-core/src/tool.rs:125 cites `host/live.rs:84` as where `prompt_guidelines` is copied, but :84 is the `capabilities.ui` gate in `ui_guest_of`; the real copy site is crates/cyrup-ext/src/host/live.rs:121 (the `registry.rs:27` half of that sentence is still correct). Comment-only; no behaviour and no serde bytes.

## Evidence

```
`grep -n "impl<'de> Deserialize" crates/cyrup-session/src/entry.rs` -> 295; `grep -rn "fn entries_have_assistant" crates/cyrup-session/src/` -> manager.rs:875; `grep -n "prompt_guidelines" crates/cyrup-ext/src/host/live.rs` -> first hit 121 (inside the ToolDescriptor construction). Cited text confirmed at crates/cyrup-core/src/message/stop_reason.rs:67,70 (file is 111 lines) and crates/cyrup-core/src/tool.rs:125.
```

## Acceptance Criteria

- [ ] crates/cyrup-core/src/message/stop_reason.rs:67 cites the real `impl<'de> Deserialize<'de> for Entry` site (crates/cyrup-session/src/entry.rs:295-317) or names the symbol without a line range.
- [ ] crates/cyrup-core/src/message/stop_reason.rs:70 cites crates/cyrup-session/src/manager.rs:875-880 or names `entries_have_assistant` without a line range.
- [ ] crates/cyrup-core/src/tool.rs:125 cites crates/cyrup-ext/src/host/live.rs:121, and the still-correct `registry.rs:27` reference is left unchanged.
- [ ] Each corrected pointer is confirmed against the current tree with a grep for the named symbol before the edit is made.
- [ ] `git diff` shows changes only inside doc comments in crates/cyrup-core/src/.

## Provenance

Found by the cyrup-core hygiene audit workflow (2026-08-22), dimension-fanned and adversarially
verified. Severity **low**, estimated effort **small**.

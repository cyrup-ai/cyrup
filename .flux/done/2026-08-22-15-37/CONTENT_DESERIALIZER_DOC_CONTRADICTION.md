---
stage: qa
status: completed
updated: 2026-08-22 18:06
---

# Fix the Content Enum Doc That Claims Off-Union Blocks Are Rejected

## Description

The rustdoc on `Content` in crates/cyrup-core/src/message/content.rs:9-12 states that the three per-role deserializers enforce Pi's per-role content unions and that an off-union block (an `Image` in an assistant turn, a `ToolCall`/`Thinking` in a user/toolResult turn) is "REJECTED on deserialize". The implementations do the exact opposite and say so in their own docs: `de_user_content` (content.rs:113-142), `de_tool_result_content` (:145-159) and `de_assistant_content` (:161-187) are bare tolerant deserializers with no role check, and two in-crate tests (`assistant_content_accepts_image_on_deserialize_like_pi`, `user_and_tool_result_content_accept_off_union_blocks_like_pi`) pin the tolerant behaviour. This is the highest-visibility copy of the falsehood because it is the first doc rustdoc renders for `Content`, and it invites an engineer to "restore" validation Pi never had — the exact R-00-013 wire-interop regression the deserializer docs at :120-123 warn against. Rewrite the paragraph to state the real contract (cyrup deliberately does not enforce the unions on read, because Pi's unions are compile-time TypeScript only and Pi's session read path is a bare JSON.parse), then close ledger row SESS-027. This is documentation only: no code, no serde bytes.

## Evidence

```
crates/cyrup-core/src/message/content.rs:9-12 (verbatim: "...is REJECTED on deserialize, exactly as Pi's typed unions reject it.") contradicted in the same file at :117-123, :148, :164-166; `cargo test -p cyrup-core --lib` -> 36 passed, including assistant_content_accepts_image_on_deserialize_like_pi and user_and_tool_result_content_accept_off_union_blocks_like_pi. Ledger: docs/gap-analysis/03-cyrup-session.md:178, :254 (SESS-027 still-open).
```

## Acceptance Criteria

- [ ] crates/cyrup-core/src/message/content.rs:9-12 no longer claims off-union blocks are rejected on deserialize, and instead states that the deserializers are read-tolerant by design with the Pi-interop rationale.
- [ ] The intra-doc links to `de_assistant_content`, `de_user_content` and `de_tool_result_content` and the "Producers still build the right variants by construction" sentence are preserved.
- [ ] No non-doc line in crates/cyrup-core/src/ is modified; `git diff` shows comment-only changes.
- [ ] `cargo test -p cyrup-core --lib` still passes with the same test count (36) and no test was edited.
- [ ] docs/gap-analysis/03-cyrup-session.md rows for SESS-027 (:178 and :254) are flipped from still-open to closed, and the stale path `cyrup-core/src/message.rs` in the :254 row is corrected to `cyrup-core/src/message/content.rs`.

## Provenance

Found by the cyrup-core hygiene audit workflow (2026-08-22), dimension-fanned and adversarially
verified. Severity **high**, estimated effort **small**.

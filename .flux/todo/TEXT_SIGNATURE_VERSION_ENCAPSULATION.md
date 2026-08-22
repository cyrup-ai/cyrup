---
stage: new
status: done
updated: 2026-08-22 17:24
---

# Make TextSignatureV1::v Private So the v == 1 Invariant Is Enforced

## Description

crates/cyrup-core/src/message/text_signature.rs:20-21 documents `v` as "always 1" but leaves the field `pub`, so the invariant is advisory. `new` (:29-31) pins `v: 1`, `parse` (:36-39) gates on `parsed.v == 1`, and `encode` (:42-43) serializes whatever is held — so a hand-built value with `v != 1` does leave the crate and then fails `parse` silently downstream: crates/cyrup-provider/src/api/openai_responses.rs falls through to its :919 arm, stuffing the whole JSON blob into `id` and dropping `phase` with no error and no log. There is exactly one struct-literal bypass in the workspace (openai_responses.rs:919-923, inside `parse_text_signature` at :913); the other external use at :1371 already goes through `TextSignatureV1::new`. Nothing produces `v != 1` today, so this is a latent-misuse guard, not a live bug. Serde reads and writes private fields, so the wire form is byte-identical.

## Evidence

```
crates/cyrup-core/src/message/text_signature.rs:20-21 (`/// Schema version — always `1` (Pi `v: 1`).` over `pub v: u8`), :29-31 (`new` pins 1), :36-39 (`parse` rejects non-1), :42-43 (`encode` serializes unconditionally). `grep -rn TextSignatureV1 crates/ | grep -v cyrup-core/src` -> 5 lines (openai_responses.rs:38, 913, 915, 919, 1371); the sole struct literal is :919-923.
```

## Acceptance Criteria

- [ ] `v` in crates/cyrup-core/src/message/text_signature.rs:21 is no longer `pub`, and its doc comment is preserved on the private field.
- [ ] A `pub fn version(&self) -> u8` accessor is added to the impl block at :27-45 so external readers are not blocked.
- [ ] crates/cyrup-provider/src/api/openai_responses.rs:919-923 is rewritten as a `TextSignatureV1::new(...)` call and is the only file outside cyrup-core that the change touches.
- [ ] `grep -rn TextSignatureV1 crates/ | grep -v cyrup-core/src` shows no remaining struct-literal construction of the type.
- [ ] The existing round-trip test `text_signature_v1_roundtrips_through_string_field` (text_signature.rs:52-64) passes unchanged, confirming `v`, `id` and the `final_answer` phase spelling are byte-stable.
- [ ] `cargo check -p cyrup-provider` and `cargo test -p cyrup-core` both pass.

## Provenance

Found by the cyrup-core hygiene audit workflow (2026-08-22), dimension-fanned and adversarially
verified. Severity **low**, estimated effort **small**.

---
stage: new
status: done
updated: 2026-08-22 17:24
---

# Pin the Untested Hand-Written Serialize Arms Against Pi's Wire Bytes

## Description

Three hand-written serialize paths in cyrup-core emit correct Pi bytes today but have zero forward-direction coverage, so a refactor could reorder or drop keys without any test failing — and PROV-020 (docs/gap-analysis/01-cyrup-core-and-provider.md:153, still open) is exactly a mis-ordered key in a sibling serializer, so the failure mode is real here. (1) `AssistantMessage`'s serializer (message/assistant.rs:128-183) emits `responseModel`, `responseId` and `diagnostics` conditionally at :155-165; both existing serialize tests build all-`None` messages and the field-order assertion at :283-291 lists only the eight always-present keys, so the optional arms are structurally unobservable — and `grep -rn '"responseModel"' --include=*.rs crates/` matches only the serializer itself, with no Pi fixture carrying the key. (2) `Content::Image` (message/content.rs:88-95) is the last hand-written arm with no serialize test; its `mimeType` key at :93 exists only there. (3) message/usage.rs has no `mod tests` at all, and `cacheWrite1h` is the non-obvious `rename_all="camelCase"` case (segment starting with a digit). These are pure pins that pass against current code — do not change any serializer.

## Evidence

```
crates/cyrup-core/src/message/assistant.rs:128-183 (serializer), :155-165 (uncovered arms), :237-241 and :265-269 (all-None tests), :283-291 (order test covering only 8 keys), :258 (the sole negative assertion). crates/cyrup-core/src/message/content.rs:88-95; the only Image test is deserialize-direction at :268-283. crates/cyrup-core/src/message/usage.rs:11-14 with no test module. Verified current bytes via throwaway integration tests (since removed): {"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"cacheWrite1h":400000,"reasoning":12,"totalTokens":0,"cost":{...}} and {"type":"image","data":"AAAA","mimeType":"image/png"}. `grep -rl 'responseModel' crates/cyrup-test-support/fixtures` -> no matches.
```

## Acceptance Criteria

- [ ] A test in crates/cyrup-core/src/message/assistant.rs builds an `AssistantMessage` with `response_model`, `response_id`, `diagnostics`, `deferred`, `error_message` and `raw_stop_reason` all `Some`, and asserts the 14 keys role, content, api, provider, model, responseModel, responseId, diagnostics, usage, stopReason, deferred, errorMessage, rawStopReason, timestamp appear at strictly increasing byte offsets in the serialized string, then asserts it deserializes back equal.
- [ ] A test in crates/cyrup-core/src/message/content.rs asserts `Content::Image { data: "AAAA", mime_type: "image/png" }` serializes to exactly {"type":"image","data":"AAAA","mimeType":"image/png"} and round-trips.
- [ ] crates/cyrup-core/src/message/usage.rs gains a `mod tests` asserting the exact serialized string for a `Usage` with `cache_write_1h: Some(400_000)` and `reasoning: Some(12)`, that both keys are absent when `None`, and that `Usage::default()` emits no optional keys.
- [ ] Zero lines of serialization or deserialization code are changed — `git diff` touches only `#[cfg(test)]` regions.
- [ ] `cargo test -p cyrup-core` passes on the first run without adjusting any expected byte string (the tests must pass against current behaviour, proving they are pins, not repairs).

## Provenance

Found by the cyrup-core hygiene audit workflow (2026-08-22), dimension-fanned and adversarially
verified. Severity **medium**, estimated effort **medium**.

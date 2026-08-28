---
title: Cross-reference CYRUP-DELTA sites (bookkeeping only)
priority: LOW
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: new
status: pending
updated: 2026-08-28
---

# Cross-reference delta sites

6 of the 31 capability-gap markers are cross-references to another site's
gap, listed by the audit so the count stayed honest. They need no independent fix —
they resolve when their parent does.

- `crates/cyrup-provider/src/api/openai_responses/params.rs:143`
  - Same observable as compat.rs:781 (cross-reference, same underlying gap, listed so the count stays honest). The same gate is applied in `api/azure_openai_responses.rs` and `api/openai_codex_responses/r
- `crates/cyrup-provider/src/auth/google_adc.rs:264`
  - Same gap as google_adc.rs:19 (cross-reference at the implementation site). Included for count completeness.
- `crates/cyrup-provider/src/providers/google_vertex.rs:44`
  - Same gap as google_adc.rs:19 (cross-reference). Listed for count completeness.
- `crates/cyrup-provider/src/providers/all.rs:77`
  - Same gap as google_adc.rs:19 (cross-reference). Listed for count completeness.
- `crates/cyrup-provider/src/api/google_vertex.rs:36`
  - Same gap as google_adc.rs:19 (cross-reference). Listed for count completeness.
- `crates/cyrup-provider/src/utils/retry.rs:83`
  - Same divergence as retry.rs:43 (the inline half of the same added literal). Listed for count completeness.
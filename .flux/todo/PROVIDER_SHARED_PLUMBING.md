---
stage: new
status: done
updated: 2026-08-22 22:40
---

# Collapse Copy-Pasted cyrup Plumbing Across The api Impls

## Description

Every free function in the 9 non-bedrock api impls was bucketed by name, then candidate bodies were
normalized (comments, blank lines and indentation stripped) and md5-hashed to prove **byte
identity** rather than eyeballed similarity.

**Most of what you might expect is already shared, and must not be touched:** SSE frame parsing is
already `crate::stream::sse::{SseFrame, open_sse}`, imported by all 9; retry/backoff is already
`crate::utils::provider_retry::ProviderRetry::from_options`; error-body normalization exists in
exactly one file; image/base64 block building is genuinely different per wire format;
`map_stop_reason` has 4 definitions with 3 different signatures over 4 protocol vocabularies.
`normalize_tool_call_id` and `decode_stream` were examined and are the "mirrors different upstream
sources" case — **explicitly not in scope, do not re-litigate them.**

What survived is copy-paste of **cyrup's own plumbing** — code with either no upstream counterpart
at all, or one upstream counterpart independently ported N times. Collapsing these *improves* the
1:1 pi->cyrup mapping the port discipline depends on rather than damaging it.

### 1. The SSE connect block — 6 api impls, 41 identical code lines each

246 identical code lines total (~295 raw, including the copy-pasted comments). Verified by re-run;
the one correction is that the raw blocks differ by comments in 2 of the 6, which strengthens the
case — the code is identical, only the commentary drifted.

### 2. `provider_env_value` — 5 byte-identical copies

All five body-md5 `78f3cfcf2cea`: `anthropic_messages.rs:371`, `openai_completions.rs:302`,
`openai_responses.rs:293` (`pub(crate)`), `pi_messages.rs:313`, `google_vertex.rs:262`. Each is
8 fn lines + 2 doc lines. These are 5 independent ports of pi's **single** `getProviderEnvValue`.
`src/auth/google_adc.rs:201` is correctly **excluded** — it takes a 3rd `ambient` parameter.

### 3. `resolve_cache_retention` — 5 copies, 3 of them identical

md5 `ee139316db90` at `anthropic_messages.rs:381` and `openai_completions.rs:...`. The 5th copy is
`bedrock_converse_stream/params.rs:111`, which the original scan missed and which takes
`env: &EnvSource<'_>` — check whether it can share before assuming it can.

### 4. `now_millis` — 8 identical copies in `src/api/`, 2 of which have already silently diverged

**This one was NOT independently verified** — its verifier hit the session usage limit. Re-measure
before acting on it. If the divergence claim holds it is the most interesting item here, because
divergence in a "trivial" copy-pasted helper is exactly the failure mode that dedup prevents.

## Coordination

Items 1-3 touch `anthropic_messages.rs`, `openai_completions.rs`, `openai_responses.rs` and
`pi_messages.rs`, which are also decomposition targets. **Do not run this concurrently with
DECOMPOSE_ANTHROPIC_MESSAGES / DECOMPOSE_OPENAI_COMPLETIONS / DECOMPOSE_OPENAI_RESPONSES.**
Running the dedup first is preferable — the split is pure movement and will carry the shared call
sites along.

## Acceptance Criteria

- [ ] The shared home for each helper is `api/compat.rs` or `utils/`, chosen to match where the existing shared helpers live — not a new top-level module
- [ ] Each collapsed helper keeps the port-fidelity comment naming its single upstream counterpart
- [ ] `provider_env_value`: 5 copies -> 1; `google_adc.rs:201` untouched
- [ ] SSE connect block: 6 copies -> 1 shared helper
- [ ] `now_millis`: claim re-measured first; if the 2 divergent copies are real, say in the task summary which behaviour was kept and why
- [ ] `cargo build -p cyrup-provider --all-targets` — 0 errors, 0 warnings
- [ ] `cargo clippy -p cyrup-provider --all-targets` — warning count not increased
- [ ] `cargo test -p cyrup-provider --lib` — 1118 pass, 0 fail
- [ ] `cargo build --workspace` clean

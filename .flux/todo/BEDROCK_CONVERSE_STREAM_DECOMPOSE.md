---
stage: aug
status: done
updated: 2026-08-22 17:05
---

# Decompose Bedrock Converse Stream Into Submodules

## Description

Split [`crates/cyrup-provider/src/api/bedrock_converse_stream.rs`](../../crates/cyrup-provider/src/api/bedrock_converse_stream.rs)
(4,721 lines / 190 KB — the largest single-concern file in `cyrup-provider/src/api`) into a
`bedrock_converse_stream/` module directory, one submodule per concern.

This is a **pure code move**. Not one expression, string literal, doc comment or test body changes.
The only edits permitted beyond relocation are (a) per-module `use` headers, (b) widening moved
items from private to `pub(super)`, and (c) the `mod` declarations + `pub use` re-exports in
`mod.rs`. Behaviour, wire format, error strings and the crate's public API are byte-identical
before and after.

**Note on path (the queued description was written from the workspace root):** the file is at
`crates/cyrup-provider/src/api/bedrock_converse_stream.rs`, not `cyrup-provider/src/...`.

## Objective

The file is a 1:1 behavioural port of pi's `packages/ai/src/api/bedrock-converse-stream.ts` and
carries an unusually dense layer of port-fidelity commentary (upstream line citations, PROV-xxx
delta notes). That commentary is the file's real value and MUST survive the move verbatim — it is
the reason the split has to be mechanical rather than a rewrite.

The file is already internally sectioned by the author with `// ---` banners. Those banners are the
seams; the decomposition follows them rather than inventing new ones. Each banner's title line
becomes the new module's `//!` header, so no new prose is authored.

## Current shape (verified)

| Region | Lines | Contents |
|---|---|---|
| Module doc | 1–50 | `//!` header: mechanism-divergence table (no AWS SDK), scope notes |
| Imports | 52–78 | 18 `use` statements |
| Consts | 80–108 | `API_ID`, `EMPTY_TEXT_PLACEHOLDER`, `BEDROCK_DATA_RETENTION_DOCS_URL`, `INTERLEAVED_THINKING_BETA`, `SIGV4_SERVICE`, `EVENT_STREAM_MEDIA_TYPE`, `BEDROCK_STANDARD_MODE_RETRIES`, `SKIP_AUTH_{ACCESS,SECRET}_KEY` |
| `// Typed options` | 110–224 | `BedrockThinkingDisplay`, `BedrockToolChoice`, `BedrockOptions` |
| `// ApiImpl` | 226–717 | api struct + `factory()` + `impl ApiImpl` (the catch arm), `BedrockFailure` + diagnostics, `run_inner`, `split_command_input` |
| `// Environment resolution` | 719–762 | `EnvSource` |
| `// Client configuration` | 764–1063 | `AwsCredentials`, `BedrockClientConfig`, `resolve_client_config` + 7 region/credential helpers |
| `// Headers` | 1065–1135 | `RESERVED_HEADER_EXACT`, `is_reserved_header`, `apply_custom_headers`, `authorize` |
| `// Request encoding` | 1137–1746 | `converse_stream_url`, `build_params`, cache points, capability predicates, message/tool conversion, `additionalModelRequestFields` |
| `// Errors` | 1748–1819 | prefix table, `data_retention_hint`, `format_bedrock_*`, `map_stop_reason` |
| `// Response decoding` | 1821–2248 | `Block`, `Decoder`, `blocks_to_content`, `dispatch_frame`, 3 `handle_content_block_*`, `handle_metadata` |
| `// AWS event-stream framing` | 2250–2403 | `EventFrame`, `EventStreamDecoder`, `be_u32`, `parse_event_headers`, `crc32` |
| `// SigV4` | 2405–2620 | `hmac_sha256`, `hex`, `uri_encode`, `url_*`, `now_*`, `sigv4_timestamps`, `civil_from_days`, `sign_sigv4`, `upper_first` |
| `mod tests` | 2621–4721 | 57 tests + helpers, itself banner-sectioned into 11 groups |

Three items sit in the wrong section today and are placed by **use site**, not by current position —
this is the one judgement call in the split, and it is what makes it a decomposition rather than a
`csplit`:

* `upper_first` (2610–2620) is filed under SigV4 but its only caller is `dispatch_frame` (line 1954),
  mapping an event-stream `:exception-type` onto the names `bedrock_error_prefix` is keyed by → goes
  to `errors.rs`.
* `now_millis` (2498–2504) is filed under SigV4 but its only caller is `Decoder::snapshot`
  (line 1900) → goes to `blocks.rs`. Its neighbour `now_unix_seconds` (2490–2496) genuinely is
  SigV4's (`authorize`, line 1134) → stays in `sigv4.rs`.
* `url_host` (2459–2469) is filed under SigV4 but is called by
  `standard_bedrock_endpoint_region` (line 927) as well as by signing → goes to the shared `url.rs`
  with `uri_encode`, which `converse_stream_url` (1148) and `sign_sigv4` (2580) both call.

## Target layout

```
crates/cyrup-provider/src/api/bedrock_converse_stream/
├── mod.rs           ~140   module doc, `mod` decls, `pub use`, API_ID, BedrockConverseStreamApi,
│                           Default/new, factory(), impl ApiImpl (the catch arm)
├── options.rs       ~130   BedrockThinkingDisplay, BedrockToolChoice, BedrockOptions
├── env.rs            ~50   EnvSource (the env-overlay/ambient test seam)
├── config.rs        ~320   AwsCredentials, BedrockClientConfig, resolve_client_config + helpers
├── headers.rs        ~80   reserved-header table, apply_custom_headers, authorize
├── sigv4.rs         ~155   hmac_sha256, hex, now_unix_seconds, sigv4_timestamps, civil_from_days, sign_sigv4
├── url.rs            ~65   uri_encode, url_host, url_authority, url_path, converse_stream_url
├── params.rs        ~225   build_params, resolve_cache_retention, cache_point,
│                           build_additional_model_request_fields, default_thinking_budget
├── capabilities.rs  ~130   model_match_candidates + the 7 model-capability predicates
├── convert.rs       ~290   system prompt, message/content conversion, tool config
├── errors.rs         ~95   prefix table, data_retention_hint, format_*, map_stop_reason, upper_first
├── blocks.rs        ~135   Block, Decoder, blocks_to_content, now_millis
├── events.rs        ~325   dispatch_frame + handle_content_block_{start,delta,stop} + handle_metadata
├── framing.rs       ~160   EventFrame, EventStreamDecoder, be_u32, parse_event_headers, crc32
├── failure.rs       ~140   BedrockFailure + normalize_diagnostic_value/extract_bedrock_error_code/append_*
├── driver.rs        ~330   run_inner (request → retry loop → frame loop → terminal), split_command_input
└── tests/           (see below)
```

Largest non-test module ≈ 330 lines; no module carries two concerns.

`mod.rs` (not `bedrock_converse_stream.rs` + directory) because the parent
[`api/`](../../crates/cyrup-provider/src/api/mod.rs) already uses the `mod.rs` form, as do
`auth/`, `images/`, `providers/`, `utils/`.

### Dependency direction

```
leaves        url, sigv4, framing, errors, capabilities, options, env
config    ──> env, url
headers   ──> sigv4, url
params    ──> capabilities, convert, options, env
convert   ──> options
events    ──> blocks, framing, errors
driver    ──> config, headers, params, convert, events, blocks, framing, failure, errors, url, env, options
mod.rs    ──> driver, failure, options
```

No cycles. `driver.rs` is the only module that reaches broadly — it is the orchestrator, and that
is exactly the concern it is left holding.

## Move map (exact, inclusive line ranges of the pre-split file)

Extract with `sed -n 'A,Bp'` so bytes are preserved literally.

| Destination | Source ranges |
|---|---|
| `mod.rs` | `1–50` (doc header, verbatim), `80–81` (`API_ID`), `229–294` |
| `options.rs` | `113–224` |
| `failure.rs` | `296–420` |
| `driver.rs` | `96–103` (`EVENT_STREAM_MEDIA_TYPE`, `BEDROCK_STANDARD_MODE_RETRIES`), `421–717` |
| `env.rs` | `723–762` |
| `config.rs` | `105–108` (`SKIP_AUTH_*`), `768–1063` |
| `headers.rs` | `1069–1135` |
| `url.rs` | `1141–1150` (`converse_stream_url`), `2442–2457` (`uri_encode`), `2459–2469`, `2471–2480`, `2482–2488` |
| `params.rs` | `90–91` (`INTERLEAVED_THINKING_BETA`), `1152–1239`, `1241–1268`, `1662–1746` |
| `capabilities.rs` | `1270–1389` |
| `convert.rs` | `83–84` (`EMPTY_TEXT_PLACEHOLDER`), `1391–1660` |
| `errors.rs` | `86–88` (`BEDROCK_DATA_RETENTION_DOCS_URL`), `1752–1819`, `2610–2620` (`upper_first`) |
| `blocks.rs` | `1825–1936`, `2498–2504` (`now_millis`) |
| `events.rs` | `1938–2248` |
| `framing.rs` | `2254–2403` |
| `sigv4.rs` | `93–94` (`SIGV4_SERVICE`), `2409–2435`, `2437–2440`, `2490–2496`, `2506–2537`, `2539–2608` |
| `tests/` | `2629–4720` (see test map) |

Each module opens with `//! <the banner title text it came from>` — e.g. `sigv4.rs` gets
`//! SigV4 (the signing the SDK performs for upstream).` The banner comment lines themselves
(`// ---...---`) are dropped; they were a poor man's module boundary and the module IS the boundary
now. Where a module is a sub-slice of a banner section (`params`/`capabilities`/`convert`/`url` all
come out of `// Request encoding`), give it a one-line header naming its slice.

The 50-line `//!` module doc (mechanism-divergence table, scope notes) moves to `mod.rs`
**verbatim**, including the `| SDK concern | here |` table whose right column names
`resolve_client_config`, `sign_sigv4`, `EventStreamDecoder`, `apply_custom_headers` etc. Those are
plain backticked names, not intra-doc links, so they survive the move without edit.

## Visibility rules

1. **Public surface is frozen.** `BedrockOptions`, `BedrockThinkingDisplay`, `BedrockToolChoice`
   stay `pub` in `options.rs` and are re-exported from `mod.rs`:
   ```rust
   pub use options::{BedrockOptions, BedrockThinkingDisplay, BedrockToolChoice};
   ```
   so `crate::api::bedrock_converse_stream::BedrockOptions` still resolves — it is named by
   [`stream.rs:284,339`](../../crates/cyrup-provider/src/stream.rs). `BedrockConverseStreamApi` and
   `factory()` are **defined in `mod.rs`**, so `bedrock_converse_stream::factory` keeps working for
   [`api/mod.rs:188`](../../crates/cyrup-provider/src/api/mod.rs) with no re-export at all.
2. **Every moved private item becomes `pub(super)`** — functions, structs, enums, consts, **and
   struct fields**. `pub(super)` from inside a submodule means "visible in
   `bedrock_converse_stream` and everything below it", which is exactly the old file's scope. Do
   NOT reach for `pub(crate)`: it would widen the surface beyond what the old file had.
3. Fields that specifically must be widened (they are read across the new boundaries):
   * `EnvSource { overlay, ambient }` — the tests construct it as a struct literal.
   * `AwsCredentials { access_key_id, secret_access_key, session_token }` — read by `sign_sigv4`.
   * `BedrockClientConfig { profile, region, endpoint, credentials, bearer_token }` — read by
     `authorize` and `run_inner`.
   * `Decoder { blocks, usage, stop_reason, raw_stop_reason, error_message }` — written by
     `events.rs`, read by `driver.rs`.
   * `EventFrame { headers, payload }` and `EventStreamDecoder { buffer }`.
   * `BedrockFailure { partial, stop_reason, message, status, error_code, request_id }` — the
     `impl ApiImpl` catch arm in `mod.rs` destructures it.
   * Inherent methods on those types (`EnvSource::{new,get,ambient}`, `Decoder::{position_of,
     snapshot}`, `EventFrame::{header,json}`, `EventStreamDecoder::{push,next_frame}`,
     `BedrockFailure::{errored,with_request_id,service_exception}`, `BedrockToolChoice::to_wire`)
     become `pub(super)` too.
   * `Block`'s variants need no annotation — enum variants inherit the enum's visibility.
4. **Modules are declared private** (`mod config;`, not `pub mod config;`). Nothing outside the
   subtree may name them, which is what keeps rule 1 true.

## Imports

Do not copy the 18-line import block into every file. Each module takes only what it uses, plus
explicit sibling paths — the sibling `use` list doubles as the dependency documentation:

```rust
// driver.rs (illustrative — the widest importer)
use super::blocks::Decoder;
use super::config::resolve_client_config;
use super::env::EnvSource;
use super::errors::{format_bedrock_error, format_bedrock_service_error};
use super::events::dispatch_frame;
use super::failure::{BedrockFailure, append_bedrock_failure_diagnostic, normalize_diagnostic_value};
use super::framing::EventStreamDecoder;
use super::headers::{apply_custom_headers, authorize};
use super::options::BedrockOptions;
use super::params::{build_params, resolve_cache_retention};
use super::url::converse_stream_url;
```

Crate-level imports, by owner (derived from the pre-split call sites):

| Import | Modules that need it |
|---|---|
| `crate::HeaderMap` | headers |
| `crate::api::compat::sanitize_surrogates` | convert |
| `crate::api::openai_completions::transform_messages_with` | convert |
| `crate::api::{ApiImpl, EventSink}` | mod (`ApiImpl`, `EventSink`), driver + events (`EventSink`) |
| `crate::auth::{AuthResult, ProviderEnv}` | mod, driver, config (`AuthResult`); env, config (`ProviderEnv`) |
| `crate::context::{Context, ToolDef}` | mod, driver, params (`Context`); convert (`ToolDef`) |
| `crate::error::ProviderError` | driver |
| `crate::model::Model` | almost all |
| `crate::stream::sse::build_client_for_target_forcing_http1` | driver |
| `crate::stream::{CacheRetention, StreamEvent, StreamOptions}` | params (`CacheRetention`), driver/events/mod (`StreamEvent`), mod/driver/params/convert/config (`StreamOptions`) |
| `crate::usage::compute_cost` | blocks |
| `crate::utils::constrained_sampling::{ConstrainedSamplingError, resolve_json_schema_strict_sampling}` | convert |
| `crate::utils::error_body::normalize_error_body` | driver |
| `crate::utils::provider_retry::{ProviderRetry, is_retryable_provider_error, retry_delay_ms}` | driver |
| `crate::utils::json_parse::parse_streaming_json_object` | blocks |
| `crate::utils::simple_options::{adjust_max_tokens_for_thinking, clamp_max_tokens_to_context}` | params |
| `base64::Engine as _` | convert |
| `cyrup_core::{ApiId, AssistantMessage, CancelToken, Content, Message, StopReason, ThinkingLevel, ToolCall, ToolCallId, Usage, diagnostics::create_assistant_message_diagnostic_from}` | split per use: `ApiId` mod/driver/blocks/events, `AssistantMessage` failure/blocks, `CancelToken` mod/driver, `Content` blocks/convert, `Message` convert, `StopReason` mod/driver/blocks/errors/events, `ThinkingLevel` params/capabilities, `ToolCall`+`ToolCallId` blocks/events, `Usage` blocks, `create_assistant_message_diagnostic_from` failure |
| `futures::StreamExt` | driver |
| `serde_json::{Map, Value, json}` | `Map` failure/convert, `Value`+`json` most |
| `std::collections::BTreeMap` | options, headers, sigv4, framing, driver |
| `std::sync::Arc` | mod (`factory`) |

Treat this table as the starting point, then let `cargo build` settle it — an unused import is a
warning and the acceptance bar is zero new warnings.

## Test tree

The `mod tests` block is 2,093 lines — larger than the code it covers, and itself banner-sectioned.
It becomes `tests/` mirroring the source modules. Test **bodies do not change**; only which file
they live in.

`tests/mod.rs` keeps the shared helpers (`model_with`, `sonnet_45`, `opus_48`, `user_ctx`,
`env_map`, `env_source`, `no_auth`, `opts_with_reasoning`, `payload`, source `2631–2724`) plus the
`mod` declarations. Helpers stay **private** — child modules reach them through `use super::*;`,
because a private item in an ancestor module is visible to its descendants. That is the whole
reason this split needs no helper churn.

| File | Source range(s) | Contents |
|---|---|---|
| `tests/mod.rs` | `2629–2724`, `4717–4720` | imports, 9 shared helpers, `the_factory_serves_the_bedrock_api_id` |
| `tests/errors.rs` | `2728–2759`, `3818–3854` | stop-reason table, error composition |
| `tests/config.rs` | `2761–2993`, `4659–4694` | region/endpoint/credential precedence, shared-credentials file |
| `tests/params.rs` | `2995–3184`, `3498–3613`, `3690–3772` | thinking payload, cache points, `inferenceConfig` |
| `tests/convert.rs` | `3186–3496`, `3615–3688` | message conversion, tool config |
| `tests/headers.rs` | `3774–3816` | custom-header injection VC1/VC2/VC3 |
| `tests/framing.rs` | `3856–3954` | `frame`/`event` builders, event-stream decoder |
| `tests/decode.rs` | `3955–4116` | `collect`/`kinds` helpers, upstream-order decode |
| `tests/driver.rs` | `4117–4537`, `4695–4716` | mock server, retry, force-http1, failure diagnostics, terminals, command-input split |
| `tests/sigv4.rs` | `4538–4658` | HMAC/RFC 4231, key derivation, timestamps, deterministic signing, missing credentials |

Largest test file ≈ 440 lines.

Two mechanics that must be got right:

1. **The lint allow moves with the tests.** Source `2621–2627` is
   `#[cfg(test)] #[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic,
   clippy::indexing_slicing)]` — the workspace *denies* all four
   ([`Cargo.toml` `[workspace.lints.clippy]`](../../Cargo.toml)). Put it back as an **inner**
   attribute at the top of `tests/mod.rs`:
   ```rust
   #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
   ```
   Lint levels propagate to nested modules, so the child test files are covered. `mod.rs` declares
   it as `#[cfg(test)] mod tests;`.
2. **`use super::*` no longer reaches the whole file.** In the old layout the test block globbed
   the single file's top-level imports. Now `tests/mod.rs` needs its own import header — the test
   *bodies* are unchanged, only this header is new:
   ```rust
   use super::*;                       // BedrockConverseStreamApi, factory, API_ID, the pub re-exports
   use super::blocks::*;
   use super::config::*;
   use super::convert::*;
   use super::driver::*;
   use super::env::*;
   use super::errors::*;
   use super::failure::*;
   use super::framing::*;
   use super::headers::*;
   use super::params::*;
   use super::sigv4::*;
   use super::url::*;
   use crate::HeaderMap;
   use crate::auth::{AuthResult, ProviderEnv};
   use crate::context::{Context, ToolDef};
   use crate::model::{Modality, Model, ModelCost};
   use crate::stream::sse::build_client_for_target_forcing_http1;
   use crate::stream::{CacheRetention, StreamEvent, StreamOptions};
   use cyrup_core::{
       ApiId, AssistantMessage, CancelToken, Content, Message, ModelThinkingLevel, StopReason,
       ToolCall, ToolCallId, Usage,
   };
   use serde_json::{Map, Value, json};
   use std::collections::BTreeMap;
   use std::sync::Arc;
   ```
   The `super::<module>::*` globs work because every moved item is `pub(super)` (rule 2) and
   `tests` is a descendant of `bedrock_converse_stream`. Prune whatever `cargo build` reports
   unused; a glob-imported name that an explicit `use` also names is shadowed by the explicit one,
   not a conflict.

## Procedure

Work module-by-module against `git`, not against a half-edited buffer:

```bash
cd crates/cyrup-provider/src/api
git show HEAD:crates/cyrup-provider/src/api/bedrock_converse_stream.rs > /tmp/bcs_old.rs
mkdir -p bedrock_converse_stream/tests
# per row of the move map, e.g.:
sed -n '2254,2403p' /tmp/bcs_old.rs >> bedrock_converse_stream/framing.rs
```

1. Snapshot the original to `/tmp/bcs_old.rs`; slice every module out of the snapshot with `sed`.
2. Prepend each file's `//!` header and `use` block; append `pub(super)` to the moved items.
3. Write `mod.rs` (doc header, 16 `mod` decls, `pub use options::{…}`, `API_ID`, the api struct,
   `factory`, `impl ApiImpl`, `#[cfg(test)] mod tests;`).
4. `git rm` the old `bedrock_converse_stream.rs` — `api/mod.rs:49`'s `pub mod
   bedrock_converse_stream;` is unchanged and now resolves to the directory. Having both the file
   and the directory present is an E0761 ambiguity, so the delete is part of the same step.
5. `cargo build -p cyrup-provider` and fix only visibility/import errors. **Any error that is not
   "unresolved import", "private item", or "unused import" means something was altered — revert to
   the snapshot slice instead of patching the symptom.**
6. `cargo clippy -p cyrup-provider --all-targets`, then `cargo test -p cyrup-provider --lib bedrock`.

## Pitfalls

* **Do not reorder or "tidy" anything while moving.** Not the match arms, not the `use` order
  inside a function body (`hmac_sha256` has an inner `use crate::auth::oauth::sha256::sha256;`),
  not the port-citation comments. A reviewer must be able to diff a slice of the old file against
  a new module and see zero content lines changed.
* **`docs/` prose cites the old path.** `docs/PARITY-PLAN.md` and five `docs/gap-analysis/*.md`
  mention `api/bedrock_converse_stream.rs`. Those are historical analysis records — out of scope,
  leave them alone.
* **`clippy::indexing_slicing` is denied workspace-wide.** `framing.rs` (`be_u32`,
  `parse_event_headers`) and `sigv4.rs` are written slice-getter-style specifically to satisfy it.
  Move them as-is; do not "simplify" a `.get(..)?` into an index.
* **Doc links.** Private-item docs are not link-checked by default `cargo doc`, but the module doc
  and the `pub` options types are. If `cargo doc -p cyrup-provider --no-deps` reports a broken
  intra-doc link (e.g. `[`is_reserved_header`]` cited from `sign_sigv4`'s docs, or
  `[`convert_tool_config`]` from `BedrockToolChoice::to_wire`), fix it by bringing the name into
  scope with a `use`, never by deleting the reference.
* **`EnvSource::new` vs the ambient seam.** `env.rs` is 40 lines and looks trivially inlinable into
  `config.rs`. It is not: it is the seam the whole config test suite injects through, and it is
  named by `driver.rs` too. It stays its own module.

## Definition of done

- [ ] `crates/cyrup-provider/src/api/bedrock_converse_stream.rs` is deleted; a
      `bedrock_converse_stream/` directory with `mod.rs` + 15 sibling modules + `tests/` replaces it
- [ ] No non-test module exceeds ~350 lines; each maps to exactly one row of the target layout
- [ ] `crate::api::bedrock_converse_stream::{factory, BedrockConverseStreamApi, BedrockOptions,
      BedrockThinkingDisplay, BedrockToolChoice}` all still resolve; `api/mod.rs` and `stream.rs`
      are untouched
- [ ] Every moved item is `pub(super)` at most; nothing new is `pub` or `pub(crate)`
- [ ] `git show HEAD:...bedrock_converse_stream.rs` and the concatenated new tree contain the same
      set of item names (verify: extract `fn`/`struct`/`enum`/`const` names from both, `sort`, `diff`
      — the only expected difference is zero)
- [ ] `cargo build -p cyrup-provider` clean
- [ ] `cargo clippy -p cyrup-provider --all-targets` — no new warnings
- [ ] `cargo test -p cyrup-provider --lib bedrock` — the same 57 tests run and pass
- [ ] `cargo doc -p cyrup-provider --no-deps` — no new warnings

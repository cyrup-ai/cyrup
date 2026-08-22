---
stage: qa
status: completed
updated: 2026-08-22 16:09
---

# Decompose cyrup-core message.rs Into Submodules

## Description

Split [`crates/cyrup-core/src/message.rs`](../../crates/cyrup-core/src/message.rs) — 1,289 lines,
the largest file in the crate and ~55% of its 2,348 `src` lines — into a `src/message/` module
directory, one submodule per concern, with a `mod.rs` that re-exports the existing public surface
unchanged.

**This is a pure code move.** No type, field, derive, serde attribute, impl body or doc sentence
changes meaning. The only edits that are not verbatim copies are the eight intra-doc link
rewrites enumerated in §5 and the per-module `use` headers in §3 — both are mechanical
consequences of items crossing a module boundary, and both are specified exactly below.

The file is ~75% doc comment by volume (the `StopReason` doc alone is 70 lines of port
provenance). Those docs are load-bearing — they are the record of *why* each hand-written serde
impl exists — so they move with their item, intact, never trimmed.

## Current shape (measured)

| region | lines | what |
|---|---:|---|
| module doc + imports | 1-7 | `//!` header, `use crate::…` |
| `UNRESOLVED_API` + `default_api` | 9-21 | assistant-api sentinel + its serde default |
| `ThinkingLevel`, `ModelThinkingLevel`, `From` | 23-89 | reasoning-effort ladder |
| `StopReason` + `impl` | 91-199 | terminal-reason enum (70-line provenance doc) |
| `TextPhase`, `TextSignatureV1` + `impl` | 201-242 | structured text-signature payload |
| `ToolCall` + hand-written `Serialize` | 244-292 | self-tagging tool call |
| `Content` + hand-written `Serialize` + ctors | 294-400 | typed content block |
| `Usage`, `Cost` | 402-426 | token + cost accounting |
| `AssistantMessage` | 428-504 | assistant turn |
| `DeferredHandle` | 506-530 | durable provider handle |
| `impl Serialize for AssistantMessage` | 532-588 | Pi field-order serializer |
| `impl AssistantMessage` | 590-634 | `model_ref`, `errored`, `append_diagnostic` |
| `Message` + hand-written `Serialize` | 636-762 | role-tagged conversation message |
| `de_user_content` / `de_tool_result_content` / `de_assistant_content` | 764-838 | per-role content deserializers |
| `mod tests` | 840-1289 | 20 `#[test]` fns, no shared helpers, only `use super::*;` |

## 1. Target layout

Create `crates/cyrup-core/src/message/` with **nine** files. Ranges are `sed`-extractable from the
current file and are inclusive; each range already begins at the item's first `///` doc line.

| new file | verbatim source ranges | ~lines |
|---|---|---:|
| `mod.rs` | `1-4` (module doc) + new `mod`/`pub use` block | ~60 |
| `thinking.rs` | `23-89` | ~100 |
| `stop_reason.rs` | `91-199` | ~120 |
| `text_signature.rs` | `201-242` | ~70 |
| `tool_call.rs` | `244-292` | ~90 |
| `content.rs` | `294-400` + `764-838` | ~305 |
| `usage.rs` | `402-426` | ~35 |
| `assistant.rs` | `9-21` + `428-504` + `506-530` + `532-588` + `590-634` | ~335 |
| `conversation.rs` | `636-762` | ~285 |

Largest module drops from 1,289 → ~335 lines (a 3.8× reduction), and the two biggest are the two
that carry the hand-written Pi-byte-order serializers, which is where the density belongs.

### Naming and boundary decisions — these are the required ones, not options

- **`conversation.rs`, not `message.rs`.** A `message::message` submodule trips
  `clippy::module_inception` and reads badly at every call site. The `Message` enum's own doc calls
  it "a conversation message (func-01 §4.2)".
- **The three `de_*` deserializers live in `content.rs`, not their own module.** They return
  `Vec<Content>`, and `Content`'s own doc links to all three by bare name. Keeping them beside
  `Content` is what makes those three links keep resolving with zero doc edits (§5). Splitting them
  out would force `content.rs` to `use` them purely for the doc links — an unused import.
- **`DeferredHandle` stays in `assistant.rs`.** It exists only as the payload of
  `AssistantMessage::deferred`, its doc links to `AssistantMessage` and `StopReason::Deferred`, and
  alone it is a 25-line file.
- **`UNRESOLVED_API` and `default_api` go to `assistant.rs`.** Both exist solely for
  `AssistantMessage::api`; `default_api` is named by a serde string attribute on that field, which
  only resolves in the same module.
- **Submodules are private (`mod x;`), items re-exported (`pub use x::…`).** `pub mod` would mint
  new public paths (`cyrup_core::message::content::Content`) and widen the API surface the task is
  supposed to leave untouched. Private-mod + `pub use` keeps `cyrup_core::message::Content` and
  `cyrup_core::Content` as the only two paths, exactly as today.
- **`mod.rs` style**, matching the crate-dominant convention (46 `mod.rs` files in the workspace vs
  3 sibling-file modules).

## 2. `mod.rs` — write it exactly like this

```rust
//! The message & content model (arch-00 §3.3; conformance: func-01 §4).
//!
//! Serde follows arch-00 §4: structs use `rename_all = "camelCase"`; tagged enums add
//! `rename_all_fields = "camelCase"` so payload fields are camelCase for Pi-interop (R-00-013).
//!
//! Split by concern; every public item is re-exported here, so `cyrup_core::message::X` and
//! `cyrup_core::X` resolve exactly as they did when this was one file:
//!
//! - [`thinking`] — the reasoning-effort ladder ([`ThinkingLevel`], [`ModelThinkingLevel`]).
//! - [`stop_reason`] — how a generation settled ([`StopReason`]).
//! - [`text_signature`] — the structured text-signature payload ([`TextPhase`],
//!   [`TextSignatureV1`]).
//! - [`tool_call`] — the self-tagging [`ToolCall`].
//! - [`content`] — the typed [`Content`] block and the per-role content deserializers.
//! - [`usage`] — token + cost accounting ([`Usage`], [`Cost`]).
//! - [`assistant`] — [`AssistantMessage`], [`DeferredHandle`], [`UNRESOLVED_API`].
//! - [`conversation`] — the role-tagged [`Message`] enum.

mod assistant;
mod content;
mod conversation;
mod stop_reason;
mod text_signature;
mod thinking;
mod tool_call;
mod usage;

pub use assistant::{AssistantMessage, DeferredHandle, UNRESOLVED_API};
pub use content::Content;
pub use conversation::Message;
pub use stop_reason::StopReason;
pub use text_signature::{TextPhase, TextSignatureV1};
pub use thinking::{ModelThinkingLevel, ThinkingLevel};
pub use tool_call::ToolCall;
pub use usage::{Cost, Usage};
```

**Caveat on the module doc:** the `[`thinking`]`-style links in that list point at *private*
modules, which would add eight new `private_intra_doc_links` warnings — and §7 requires the
warning count to stay at exactly 5. So write those eight bullets with plain code spans
(`` `thinking` ``) instead of link brackets. Everything else in the doc block above is verbatim.

## 3. Per-module `use` headers

Add exactly these, nothing more (an unused `use` is a warning, and §7 forbids new warnings):

| module | header |
|---|---|
| `thinking.rs` | *(none — no crate types)* |
| `stop_reason.rs` | *(none)* |
| `text_signature.rs` | *(none — `serde_json` is already fully qualified in the bodies)* |
| `tool_call.rs` | `use crate::ToolCallId;` |
| `content.rs` | `use super::tool_call::ToolCall;` |
| `usage.rs` | *(none)* |
| `assistant.rs` | `use super::content::{de_assistant_content, Content};`<br>`use super::stop_reason::StopReason;`<br>`use super::usage::Usage;`<br>`use crate::diagnostics::AssistantMessageDiagnostic;`<br>`use crate::{ApiId, ModelId, ModelRef, ProviderId};` |
| `conversation.rs` | `use super::assistant::AssistantMessage;`<br>`use super::content::{de_tool_result_content, de_user_content, Content};`<br>`use super::usage::Usage;`<br>`use crate::ToolCallId;` |

The original two-line import block (`use crate::diagnostics::AssistantMessageDiagnostic;` /
`use crate::{ApiId, ModelId, ModelRef, ProviderId, ToolCallId};`) is fully consumed by this table —
`ToolCallId` splits to `tool_call.rs` + `conversation.rs`, everything else lands in `assistant.rs`.

Give each file a one-line `//!` header naming its concern (`//! The reasoning-effort ladder …`).

### Why the `de_*` imports are mandatory, not stylistic

`AssistantMessage` and `Message` name the deserializers by **string**:

```rust
#[serde(default, deserialize_with = "de_assistant_content")]
```

serde expands that string as a path resolved in the *defining item's* module scope. Importing the
fn by name is what makes the bare string keep working — and, as a bonus, is what keeps the doc
links in §5 resolving. Do **not** "fix" it by rewriting the attribute to an absolute path; the
`use` is required for the doc links regardless, and two mechanisms for one dependency is worse.

## 4. Test placement — all 20 tests move, none is rewritten

Every test module gets the original gate verbatim:

```rust
#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;
    use crate::message::*;
```

`use super::*` alone is no longer sufficient (a `conversation.rs` test builds `Content` and `Usage`
values); the `crate::message::*` glob supplies every sibling type through the `mod.rs` re-exports.
Two overlapping globs resolving to the same items is not an ambiguity error, and rustc does not
lint unused glob imports — so this exact pair is safe in all six test modules.

| test (source line) | target |
|---|---|
| `model_thinking_level_splits_off_from_levels` (1043) | `thinking.rs` |
| `max_is_a_first_class_on_level` (1055) | `thinking.rs` |
| `text_signature_v1_roundtrips_through_string_field` (1029) | `text_signature.rs` |
| `bare_tool_call_self_tags_with_exactly_one_type_key` (1225) | `tool_call.rs` |
| `tool_call_arguments_reject_non_object` (1173) | `tool_call.rs` |
| `content_serializes_camelcase_tagged` (845) | `content.rs` |
| `thinking_redacted_omitted_when_false_emitted_when_true` (1204) | `content.rs` |
| `content_tool_call_flattens_single_type_key_no_duplicate` (1252) | `content.rs` |
| `user_content_accepts_bare_string_shorthand` (1013) | `content.rs` — exercises `de_user_content` |
| `assistant_content_accepts_image_on_deserialize_like_pi` (1100) | `content.rs` — exercises `de_assistant_content` |
| `user_and_tool_result_content_accept_off_union_blocks_like_pi` (1118) | `content.rs` — exercises the read-tolerance of both role deserializers |
| `assistant_message_roundtrips_and_tags_role` (988) | `assistant.rs` |
| `bare_assistant_message_role_first_pi_order_single_role_key` (1136) | `assistant.rs` |
| `assistant_message_api_is_required_on_the_wire` (1187) | `assistant.rs` |
| `assistant_append_diagnostic_accumulates` (1275) | `assistant.rs` |
| `tool_result_message_uses_camelcase_fields` (858) | `conversation.rs` |
| `tool_result_usage_and_added_tool_names_round_trip_byte_identically` (880) | `conversation.rs` |
| `old_shape_tool_result_reads_and_re_exports_unchanged` (917) | `conversation.rs` |
| `new_shape_tool_result_still_parses_under_the_pre_change_shape` (940) | `conversation.rs` |
| `user_content_serializes_single_text_as_array_like_pi` (1068) | `conversation.rs` |

`stop_reason.rs` and `usage.rs` get no `mod tests` — those types have no direct tests today, and
this task adds none.

The three deserializer tests land in `content.rs` because that is where their subject now lives,
even though they drive it through `Message`/`AssistantMessage`. That is the acceptance criterion
"tests move alongside the code they exercise", read literally.

## 5. The eight doc links that must be rewritten — and the twelve that must not

A bare intra-doc link resolves through the module's name-resolution scope. Links whose target is
either in the same new module or already imported by §3 keep working untouched. Only these eight
lose their scope; rewrite each to a `crate::`-rooted path (every one of these types is re-exported
at the crate root by `lib.rs`, so `crate::X` is stable and correct):

| source line | in new module | from | to |
|---:|---|---|---|
| 180 | `stop_reason.rs` | ``[`DeferredHandle`]`` | ``[`crate::DeferredHandle`]`` |
| 180 | `stop_reason.rs` | ``[`AssistantMessage::deferred`]`` | ``[`crate::AssistantMessage::deferred`]`` |
| 211 | `text_signature.rs` | ``[`Content::Text`]`` | ``[`crate::Content::Text`]`` |
| 249 | `tool_call.rs` | ``[`Content::ToolCall`]`` | ``[`crate::Content::ToolCall`]`` |
| 272 | `tool_call.rs` | ``[`Content::ToolCall`]`` | ``[`crate::Content::ToolCall`]`` |
| 308 | `content.rs` | ``[`TextSignatureV1`]`` | ``[`crate::TextSignatureV1`]`` |
| 309 | `content.rs` | ``[`TextSignatureV1::parse`]`` | ``[`crate::TextSignatureV1::parse`]`` |
| 396 | `content.rs` | ``[`TextSignatureV1`]`` | ``[`crate::TextSignatureV1`]`` |

**Leave these alone** — they still resolve after the move, and rewriting them is churn:
`[`ApiId`]`/`[`AssistantMessage`]`/`[`AssistantMessage::api`]`/`[`UNRESOLVED_API`]`/
`[`StopReason::Deferred`]`/`[`AssistantMessage::model_ref`]` (all in `assistant.rs`, all
same-module or imported), `[`ModelThinkingLevel`]`/`[`ThinkingLevel`]`/`[`ThinkingLevel::Max`]`
(`thinking.rs`), `[`StopReason::Pending`]`/`[`StopReason::Error`]`/`[`Self::Pending`]`
(`stop_reason.rs`), `[`TextSignatureV1::parse`]`/`[`TextSignatureV1::encode`]`
(`text_signature.rs`, same module), `[`Content`]`/`[`ToolCall`]`/`[`Content::Text`]`/the three
`[`de_*`]` links (`content.rs`), `[`AssistantMessage`]`/`[`Content::Text`]`/`[`de_user_content`]`/
`[`de_tool_result_content`]` (`conversation.rs`, all imported by §3), `[`serde::Serialize`]`/
`[`serde::Deserialize`]` (external, resolve anywhere), and `[`crate::ModelRef::api`]`/
`[`crate::StopReason::Deferred`]` (already rooted).

## 6. What must NOT change

- **`lib.rs` needs zero edits.** `pub mod message;` already resolves to the directory, and the
  `pub use message::{…}` list is satisfied by `mod.rs`'s re-exports.
- **No public API movement.** The full public inventory is these 13 items, all of which stay
  reachable at *both* `cyrup_core::X` and `cyrup_core::message::X`: `UNRESOLVED_API`,
  `ThinkingLevel`, `ModelThinkingLevel`, `StopReason`, `TextPhase`, `TextSignatureV1`, `ToolCall`,
  `Content`, `Usage`, `Cost`, `AssistantMessage`, `DeferredHandle`, `Message`.
- **The three `de_*` fns stay private.** Making them `pub(crate)`/`pub` to dodge the existing
  private-link doc warnings is a different task's decision — see `CARGO_DOC_WARNINGS.md`, which
  owns the "either make them `pub` or make the links plain code spans, decide once, apply
  uniformly" call for all 442 such links in the workspace.
- **Do not touch `docs/gap-analysis/*`.** Those ledgers cite `cyrup-core/src/message.rs:<line>`
  in a dozen places (SESS-001, SESS-027, PROV-002, PROV-009, PROV-020). They are a historical
  record of closed findings against the file as it stood; rewriting their citations is out of
  scope and would be a large, error-prone diff on a document this change does not alter the truth
  of.
- **Do not touch the two cross-crate comments** that name the file
  (`cyrup-provider/src/api/github_copilot_headers.rs:56`,
  `cyrup-session/src/tests/listing_unparseable_message.rs:19`). The second is already factually
  stale (it claims `de_user_content` rejects off-union arrays, which SESS-001 removed) and fixing
  it is a content fix, not a move.
- **Only one downstream file imports through the module path** —
  `cyrup-session-svc/src/tests/read_image_auto_resize.rs:25`
  (`use cyrup_core::message::{Content, Message};`). It must keep compiling untouched; that is the
  single sharpest check that the re-export layer is right.

## 7. Definition of done

Run from the repo root. Every one of these is a hard gate:

```bash
cargo build -p cyrup-core
cargo clippy -p cyrup-core --all-targets      # workspace lints deny unwrap/expect/panic/indexing
cargo test  -p cyrup-core                     # 20 message tests + 2 lib tests, all green
cargo doc   -p cyrup-core --no-deps
cargo build -p cyrup-session-svc -p cyrup-provider -p cyrup-session   # downstream path check
```

- `src/message.rs` is gone; `src/message/` holds the nine files of §1. Use `git rm` / `git add` so
  the split lands as one reviewable commit.
- `cargo test -p cyrup-core` runs **exactly the same 20 message test names** as before, all
  passing. No test is renamed, merged, dropped or added.
- `cargo clippy -p cyrup-core --all-targets` is clean — in particular no `unused_imports` (§3 is
  exhaustive for a reason) and no `module_inception`.
- `cargo doc -p cyrup-core --no-deps` emits **exactly 5 warnings**, the same 5 as the pre-change
  baseline measured 2026-08-22 (rustc 1.98.0), all of class
  `public documentation for X links to private item Y`:

  | item | private target |
  |---|---|
  | `Content` | `de_assistant_content` |
  | `Content` | `de_user_content` |
  | `Content` | `de_tool_result_content` |
  | `content` (the `Message::User` field) | `de_user_content` |
  | `content` (the `Message::ToolResult` field) | `de_tool_result_content` |

  **Zero `unresolved link` warnings.** An `unresolved link` is the failure signature of a missed
  §3 import or a missed §5 rewrite — it is the single most likely way to get this refactor subtly
  wrong, and the doc build is what catches it.
- `git diff --stat` shows the change as ~1,289 lines moved: no serde attribute, derive list, field
  name, impl body or test assertion differs from its original text. Verify by sorting the removed
  and added non-blank, non-`use`, non-`mod` lines and diffing the two sorted sets — the only
  residue should be the eight §5 link rewrites and the new `//!` module headers.

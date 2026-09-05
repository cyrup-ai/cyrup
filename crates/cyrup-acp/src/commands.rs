//! Slash-command translation: the prompt-block translator, the catalog projection, and the headless
//! built-ins with their dispatcher.
//!
//! **Owner: agent E (area 4e — `ACP-263`…`ACP-296`), plus `ACP-069`/`ACP-070`/`ACP-071` from 4b.**
//!
//! Port of pi-acp v0.0.33 `src/acp/translate/prompt.ts` (whole file), `src/acp/pi-commands.ts`
//! (`toAvailableCommandsFromPiGetCommands`, `describeFallback`), and `src/acp/agent.ts`'s
//! `builtinAvailableCommands`, `mergeCommands` and the eight-command dispatcher at the head of its
//! `prompt()`.
//!
//! # What this module is NOT, and it is most of upstream's file
//!
//! `src/acp/slash-commands.ts` (197 lines) has **no port**. Its stated premise — "pi RPC mode
//! disables slash command expansion, so we do it here" — is **false for cyrup**:
//! `AgentSession::prepare_and_assemble` (`crates/cyrup-session-svc/src/session/run.rs`) expands
//! unconditionally when `UserInput::expand_templates` is set. So `cyrup-acp` holds **no** template
//! state: no `fileCommands` field, no reload on `session/new` or `session/load`, no cwd-keyed
//! cache. It submits raw text (`ACP-266`), and the advertised list comes from
//! `AgentSession::slash_command_catalog` so it cannot disagree with what actually expands.
//!
//! **`ACP-266` inverts its upstream and the inversion is the unit.** A host-side expansion would
//! run *before* the core's, so the template body's own `$1`/`$@` would be substituted a second time
//! against the same argv — a template that emits `$1` literally, or that contains a `$` in a regex,
//! is silently corrupted. Nothing in this module looks a template up, and
//! `nothing_here_expands_a_template` is the assertion that keeps it that way.
//!
//! Two propagations that must not be lost with the cut: cyrup's project root is `.cyrup/prompts`,
//! not `.pi/prompts`; and cyrup's command names are **path-namespaced** (`flux/new`), so an ACP
//! client sees names containing `/` after the leading slash (`ACP-267`). Both are properties of
//! `cyrup_resources::discovery`, which this module reads through rather than reimplements.
//!
//! `ACP-295` — upstream's `loadSlashCommands` pushes **user** templates before **project** ones and
//! then de-dupes first-wins, so a user template shadows a project one; pi itself and cyrup both
//! resolve the other way (project wins). That inversion is recorded here as a defect the cut
//! deletes, not as behaviour with parity value: `ResourceSet::winners()` has already applied
//! cyrup's precedence before a row reaches [`project_catalog`], so there is nothing to re-decide.
//!
//! `ACP-296` — pi-acp's `pi-settings.ts` merges settings and *then* falls back from
//! `skills.enableSkillCommands` to the top-level key at read time; cyrup migrates the legacy key
//! into the top-level one at load (`cyrup_config::settings::migrate::migrate_settings` step 3) and
//! reads only `EffectiveSettings::enable_skill_commands`. The two differ in **order**, not in
//! strength: a global `{enableSkillCommands:true}` plus a project `{skills:{enableSkillCommands:
//! false}}` resolves to the project's `false` under cyrup, because migration rewrites each layer
//! before the merge. The residue that survives into this crate is one read site,
//! [`available_commands`], which goes through `EffectiveSettings` and nowhere else — never a free
//! `fn(cwd)` re-reading settings files, which would reintroduce the **trust bypass**
//! `pi-settings.ts` is cut for. The gate's own behaviour is pinned by
//! `the_skill_gate_removes_only_skill_rows_from_the_advertisement`; that the read happens once and
//! through the session is a property of [`available_commands`]' body.
//!
//! `pi-commands.ts`'s defensive layer — the `commands`/`data.commands` shape tolerance, the four
//! `typeof` guards, the `raw` return, and the `try/catch` fallback to file commands — is also cut:
//! `slash_command_catalog` is an infallible in-process call over data this workspace emits. **If it
//! is ever made fallible this cut must be revisited** rather than silently producing an empty menu.
//! The one probe that survives is over the row *values*, because `slash_command_catalog` returns
//! `Vec<serde_json::Value>`: this one seam really does require key-probing despite the in-process
//! narrative, and [`project_catalog`] is where it is confined.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use agent_client_protocol::schema::v1::{
    AvailableCommand, AvailableCommandInput, AvailableCommandsUpdate, ContentBlock, ContentChunk,
    EmbeddedResourceResource, ResourceLink, SessionUpdate, StopReason, TextContent,
    UnstructuredCommandInput,
};
use cyrup_core::Content;
use cyrup_session_svc::{AgentSession, QueueMode};

use crate::error::AcpFailure;
use crate::ids::{AbsCwd, AcpSessionId};

// ===================================================================================================
// `translate/prompt.ts` — `ACP-276` … `ACP-281`
// ===================================================================================================

/// Node's `Buffer.byteLength(str, 'base64')`, exactly.
///
/// `ACP-279`/`ACP-280` pin a **decoded** byte count against upstream's `3 bytes` goldens.
/// `base64::decoded_len_estimate` is an *estimate* and disagrees on padded input, so the count is
/// computed the way Node computes it: drop up to two trailing `=`, then `(len * 3) >> 2`. That is
/// Node's own `base64ByteLength`, including its indifference to invalid characters — reproducing
/// the indifference is what keeps the marker's number identical for the same input.
///
/// Character-counted, not byte-counted, so a non-ASCII character in a malformed payload cannot make
/// the two implementations disagree.
#[must_use]
pub fn base64_decoded_len(data: &str) -> usize {
    let mut len = data.chars().count();
    let mut trailing = data.chars().rev();
    if trailing.next() == Some('=') {
        len -= 1;
        if len > 1 && trailing.next() == Some('=') {
            len -= 1;
        }
    }
    (len * 3) / 4
}

/// Flatten an ACP prompt into the text cyrup submits plus its image content blocks.
///
/// Port of pi-acp v0.0.33 `src/acp/translate/prompt.ts`'s `promptToPiMessage`, whole. It is one
/// 71-line pure function with **no cyrup counterpart at all**, and every string it emits lands
/// verbatim in the model's context, so each arm is pinned byte-for-byte against
/// `test/unit/prompt-to-pi-message.test.ts`.
///
/// * `Text` (`ACP-276`) — concatenated with **no separator and no trimming**.
/// * `ResourceLink` (`ACP-277`) — appends `\n[Context] <uri>` from the raw `uri` alone, so a link
///   as the first block makes the message start with `\n`. That is upstream's behaviour and the
///   golden `Hello\n[Context] file:///tmp/foo.txt world` depends on it.
/// * `Image` (`ACP-278`) — contributes **nothing** to the text and pushes one
///   `cyrup_core::Content::Image { data, mime_type }`, which is exactly upstream's `PiImage`: raw
///   base64 in `data` with **no `data:<mime>;base64,` prefix**, passed through verbatim. Any `uri`
///   is dropped, as upstream drops it.
/// * `Resource` (`ACP-279`) — one of three shapes in a fixed order, with `text/plain` and
///   `application/octet-stream` as the two mime defaults and the blob's **decoded** byte count.
/// * `Audio` (`ACP-280`) — an explicit not-supported marker rather than a silent drop.
///
/// # [CYRUP-DELTA] — three forced or deliberate divergences
///
/// **What differs.** (1) The audio marker reads `not supported by cyrup-acp` where upstream reads
/// `not supported by pi-acp`. This is **the one string in this file that must change**, and it is
/// changed deliberately rather than incidentally. (2) Upstream defaults a missing `uri` to the
/// literal `(unknown)` and probes `typeof r?.text === 'string'` / `typeof r?.blob === 'string'` at
/// runtime; `EmbeddedResourceResource` is a typed two-variant enum with a non-optional `uri`, so
/// the `(unknown)` default and the "neither text nor blob" third shape are only reachable through
/// the enum's `#[non_exhaustive]` catch-all — which a future schema variant would enter. The arm is
/// written, with upstream's bare-uri line, so that variant degrades the way upstream degraded.
/// (3) `ACP-281`: upstream's `default: break` silently drops an unknown block type. Here an unknown
/// `type` fails deserialization of the **entire `PromptRequest`** before this function is reached —
/// `PromptRequest.prompt` is a bare `Vec<ContentBlock>` with neither `VecSkipError` nor
/// `DefaultOnError`, and `ContentBlock` has no `#[serde(other)]` variant.
///
/// **What it costs.** (1) A model reading the marker sees cyrup's name, which is correct. (2)
/// Nothing today; the arm is unreachable for wire-sourced blocks and carries a `tracing::debug!` so
/// the day it becomes reachable is visible in a log. (3) **This is a real client-visible
/// divergence**: a client that sends one unrecognised block gets its whole turn rejected with
/// invalid-params where pi-acp dropped the block and carried on. **Decision: accept the
/// rejection.** The alternative — a `Vec<serde_json::Value>` shim that re-parses each block — puts
/// a hand-written deserializer in front of the protocol's own types, which is exactly the
/// key-probing this port exists to delete, and it trades a loud failure for a silently truncated
/// prompt. A truncated prompt is the worse outcome: the model answers a question the user did not
/// ask.
///
/// **Invariant, recorded because the code cannot enforce it.** The set of block types this function
/// handles meaningfully and the set `initialize` advertises in `promptCapabilities`
/// ([`crate::config_options::agent_capabilities`]) are two views of one fact declared in two files.
/// `audio` is `false` there and this function's `Audio` arm emits a marker rather than content;
/// `image` is `true` and this function produces a real `Content::Image`. Asserted together by
/// `the_audio_arm_and_the_advertised_capability_agree`.
#[must_use]
pub fn prompt_to_user_input(blocks: &[ContentBlock]) -> (String, Vec<Content>) {
    let mut message = String::new();
    let mut images: Vec<Content> = Vec::new();

    for block in blocks {
        match block {
            // `ACP-276` — no separator, no trimming.
            ContentBlock::Text(TextContent { text, .. }) => message.push_str(text),

            // `ACP-277` — "a lightweight, human-readable hint for the LLM", upstream's words. Only
            // the raw `uri`; `name`, `title` and `description` are deliberately not used.
            ContentBlock::ResourceLink(ResourceLink { uri, .. }) => {
                message.push_str("\n[Context] ");
                message.push_str(uri);
            }

            // `ACP-278` — no text contribution; `data` passes through verbatim.
            ContentBlock::Image(image) => images.push(Content::Image {
                data: image.data.clone(),
                mime_type: image.mime_type.clone(),
            }),

            // `ACP-279` — three shapes, fixed order, two mime defaults, decoded byte count.
            ContentBlock::Resource(resource) => match &resource.resource {
                EmbeddedResourceResource::TextResourceContents(text) => {
                    let mime = text.mime_type.as_deref().unwrap_or("text/plain");
                    message.push_str(&format!(
                        "\n[Embedded Context] {} ({})\n{}",
                        text.uri, mime, text.text
                    ));
                }
                EmbeddedResourceResource::BlobResourceContents(blob) => {
                    let mime = blob
                        .mime_type
                        .as_deref()
                        .unwrap_or("application/octet-stream");
                    message.push_str(&format!(
                        "\n[Embedded Context] {} ({}, {} bytes)",
                        blob.uri,
                        mime,
                        base64_decoded_len(&blob.blob)
                    ));
                }
                // Upstream's `else` branch: the bare uri line. Unreachable for the two variants
                // 1.7.0 declares, and written because `EmbeddedResourceResource` is
                // `#[non_exhaustive]` and a third variant must degrade the way upstream degraded
                // rather than vanish from the model's context.
                _ => {
                    tracing::debug!(
                        "ACP-279: an unmodelled embedded-resource shape reached the translator"
                    );
                    message.push_str("\n[Embedded Context] (unknown)");
                }
            },

            // `ACP-280` — upstream's comment: "Not supported by pi. Provide a marker so we don't
            // silently drop context."
            ContentBlock::Audio(audio) => {
                message.push_str(&format!(
                    "\n[Audio] ({}, {} bytes) not supported by cyrup-acp",
                    audio.mime_type,
                    base64_decoded_len(&audio.data)
                ));
            }

            // `ACP-281` — unreachable for wire-sourced blocks (see the delta above); mandatory to
            // compile against `#[non_exhaustive]`. A `debug!` is the only observability this path
            // can have, and it costs nothing.
            _ => {
                tracing::debug!("ACP-281: an unmodelled prompt content block was ignored");
            }
        }
    }

    (message, images)
}

// ===================================================================================================
// The built-ins — `ACP-070`, `ACP-272`, `ACP-282` … `ACP-292`
// ===================================================================================================

/// The stop reason every built-in produces.
///
/// Upstream's eight arms each `return { stopReason: 'end_turn' }`; there is no arm that returns
/// anything else, and an error in a built-in rejects the whole `prompt()` request rather than
/// changing the stop reason. A `const` rather than a field on the outcome, because a field would
/// invite a caller to think the value varies.
pub const BUILTIN_STOP_REASON: StopReason = StopReason::EndTurn;

/// The built-ins the ACP host intercepts (`ACP-070`, `ACP-272`).
///
/// They must be intercepted here because they are **not** extension commands: the session core
/// would send them to the model as literal prompt text.
///
/// `ACP-272`'s verify is a **both-directions** identity — `BUILTINS.iter().map(name)` equals the
/// dispatcher's accepted-name set exactly — so adding a variant to one without the other fails a
/// test. That is why this is an enum with a `name()` rather than two lists: upstream's two lists
/// are ~450 lines apart in one file with nothing relating them, and a Rust port that normalises
/// `follow-up` to `FollowUp` and derives `"follow_up"` on one side while matching `"follow-up"` on
/// the other produces a menu entry that silently becomes a literal user message.
///
/// # `ACP-Q18`, decided — hand-written, not derived from the TUI's registry
///
/// Deriving from `cyrup_tui::BUILTIN_SLASH_COMMANDS` filtered to a headless-safe subset would
/// prevent drift and **change all eight strings**, and it would put a `cyrup-tui` dependency on the
/// ACP adapter for a list of eight literals. Hand-writing preserves the strings (house rule 4) and
/// the drift it "guarantees" is guaranteed only between this list and the TUI's — not between the
/// advertisement and the dispatcher, which is the drift that breaks a client, and which the enum
/// closes.
///
/// # `ACP-Q40`, decided — the ACP front-end advertises three commands the TUI does not
///
/// `/steering`, `/follow-up` and `/autocompact` have no TUI builtin. **They stay**, because each
/// maps 1:1 onto real session state (`set_steering_mode`, `set_follow_up_mode`,
/// `set_auto_compaction_enabled`) that a headless client otherwise has no way to reach — the TUI
/// user can at least edit settings. The cost is that the two front-ends disagree about what
/// commands exist, which is an asymmetry pi-acp introduced and which is resolved properly by adding
/// them to the TUI, not by removing them here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Builtin {
    /// `ACP-283`.
    Compact,
    /// `ACP-289`.
    Autocompact,
    /// `ACP-288` / `ACP-291`.
    Export,
    /// `ACP-284`.
    Session,
    /// `ACP-285`.
    Name,
    /// `ACP-286`.
    Steering,
    /// `ACP-287`.
    FollowUp,
}

/// Every built-in, **in upstream's advertisement order** (`ACP-070`).
///
/// The order is `builtinAvailableCommands`'s array order, which is what a client renders, so it is
/// pinned by `the_advertised_list_is_upstreams_fixture`.
///
/// # [CYRUP-DELTA] — `/changelog` is dropped, and that is a decision (`ACP-070`, `ACP-272`)
///
/// **What differs.** Upstream advertises a ninth command, `changelog` / `Show pi changelog`, whose
/// implementation runs `which pi` and `npm root -g`, walks the resolved package root for a
/// `CHANGELOG.md`, and truncates it at 20 000 characters. None of that has a Rust analogue, and
/// reproducing it would reintroduce a subprocess dependency in a design whose premise is having
/// none. cyrup's own `/changelog` is a TUI-local effect
/// (`crates/cyrup-tui/src/app/submit.rs`) whose entire answer today is the block
/// `What's New` / `No changelog entries found.` — there is no changelog source anywhere in this
/// workspace for the ACP host to read.
///
/// **What it costs.** A Zed user's palette has no `/changelog`. The two alternatives are both
/// worse: advertising a command that always answers "nothing" is a dead palette row, and copying
/// the TUI's two literals into this crate creates a second source of truth for a string neither
/// front-end computes. **Re-add condition, stated so this is reversible:** when cyrup ships a real
/// changelog source reachable from `AgentSession` or `cyrup_resources`, add the variant — both
/// halves (`name` and the dispatch arm) come back together because they are one enum, and no string
/// here mentions pi or npm.
pub const BUILTINS: [Builtin; 7] = [
    Builtin::Compact,
    Builtin::Autocompact,
    Builtin::Export,
    Builtin::Session,
    Builtin::Name,
    Builtin::Steering,
    Builtin::FollowUp,
];

impl Builtin {
    /// The command name as it appears after the leading `/`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Builtin::Compact => "compact",
            Builtin::Autocompact => "autocompact",
            Builtin::Export => "export",
            Builtin::Session => "session",
            Builtin::Name => "name",
            Builtin::Steering => "steering",
            Builtin::FollowUp => "follow-up",
        }
    }

    /// The advertised description (`ACP-070`), byte-for-byte upstream's except where noted.
    ///
    /// # [CYRUP-DELTA] — the two `pi` occurrences are reworded
    ///
    /// **What differs.** `steering` and `follow-up` read `Get/set pi <kind> message delivery mode`
    /// upstream; here they read `Get/set cyrup <kind> message delivery mode`. The parenthetical
    /// clause after each is unchanged.
    ///
    /// **What it costs.** Nothing — the strings describe this agent, and naming a different product
    /// in a palette entry is a defect, not parity.
    #[must_use]
    pub fn description(self) -> &'static str {
        match self {
            Builtin::Compact => "Manually compact the session context",
            Builtin::Autocompact => "Toggle automatic context compaction",
            Builtin::Export => "Export session to an HTML file in the session cwd",
            Builtin::Session => "Show session stats (messages, tokens, cost, session file)",
            Builtin::Name => "Set session display name",
            Builtin::Steering => {
                "Get/set cyrup steering message delivery mode (how queued steering messages are delivered)"
            }
            Builtin::FollowUp => {
                "Get/set cyrup follow-up message delivery mode (how queued follow-up messages are delivered)"
            }
        }
    }

    /// The `input.hint` upstream advertises, if any. `export` and `session` take no argument.
    #[must_use]
    pub fn hint(self) -> Option<&'static str> {
        match self {
            Builtin::Compact => Some("optional custom instructions"),
            Builtin::Autocompact => Some("on|off|toggle"),
            Builtin::Name => Some("<name>"),
            Builtin::Steering | Builtin::FollowUp => Some("(no args to show) all | one-at-a-time"),
            Builtin::Export | Builtin::Session => None,
        }
    }

    /// Parse a bare command name. The inverse of [`Builtin::name`], and `ACP-272` asserts they are
    /// mutual inverses over [`BUILTINS`].
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        BUILTINS.into_iter().find(|b| b.name() == name)
    }

    /// This built-in as the client sees it in `availableCommands`.
    #[must_use]
    pub fn advertised(self) -> AvailableCommand {
        let command = AvailableCommand::new(self.name(), self.description());
        match self.hint() {
            Some(hint) => command.input(AvailableCommandInput::Unstructured(
                UnstructuredCommandInput::new(hint),
            )),
            None => command,
        }
    }
}

/// Every built-in as an `AvailableCommand`, in advertisement order (`ACP-070`).
#[must_use]
pub fn builtin_commands() -> Vec<AvailableCommand> {
    BUILTINS.into_iter().map(Builtin::advertised).collect()
}

/// Should this prompt be intercepted as a built-in, and if so which and with what argument?
///
/// `ACP-282` — port of pi-acp v0.0.33 `agent.ts`'s `images.length === 0 && startsWith('/')` guard
/// plus its argument split. Takes the already-flattened prompt text (from
/// [`prompt_to_user_input`]) and whether the request carried any image, so it stays pure and
/// table-testable.
///
/// Four behaviours a naive `starts_with` gets wrong, each of which the verify pins: `/compact`
/// **with an attached image** is NOT intercepted (upstream's `images.length === 0`); `/session`
/// with trailing whitespace IS (upstream trims first); `/compactfoo` is NOT (the name is compared
/// whole); and a prompt template named `session` is **shadowed**.
///
/// # [CYRUP-DELTA] — the name split is on a literal space, not on whitespace
///
/// **What differs.** Upstream splits on the **first literal space only** — `indexOf(' ')` — so a
/// tab after the command name lands *inside* `cmd` and `/compact\tfoo` is not a command at all.
/// `cyrup_resources::prompt::split_command` splits on any whitespace and would intercept it. The
/// literal-space split is kept, deliberately, so the two hosts classify the same text identically.
/// The **arguments** are then tokenised by `cyrup_resources::parse_command_args`, which is
/// upstream's quote-aware parser line for line (verified: same algorithm, same quote semantics, no
/// escapes, an unterminated quote absorbs the remainder), so that half is a call, not a port.
///
/// **What it costs.** `/compact\tfoo` reaches the model as literal text under ACP and would be a
/// command in cyrup's TUI. That is upstream's behaviour and it is the safer of the two: a
/// misinterpreted tab silently compacts a session.
///
/// # `ACP-Q41`, decided — the built-ins shadow user commands of the same name
///
/// The ACP host dispatches **before** `AgentSession::prepare`, so a built-in shadows an extension
/// command or a prompt template of the same name — exactly as pi's TUI builtins do, and exactly as
/// pi-acp does. A user with a `/session` template loses it under ACP and keeps it in the TUI.
/// **Kept**, because the alternative makes the meaning of `/compact` depend on whether a file
/// exists in the user's project, and a client cannot see that. Note upstream is internally
/// inconsistent about it — `mergeCommands(piCommands, builtins)` lets a user `compact` shadow the
/// builtin in the *advertised* list while `prompt()`'s if-chain still intercepts it — which is the
/// inconsistency [`merge_commands`] fixes.
#[must_use]
pub fn intercept(text: &str, has_image: bool) -> Option<(Builtin, String)> {
    if has_image || !text.trim_start().starts_with('/') {
        return None;
    }
    let trimmed = text.trim();
    let (name, args) = match trimmed.find(' ') {
        Some(space) => (
            trimmed.get(1..space)?,
            trimmed.get(space + 1..).unwrap_or_default(),
        ),
        None => (trimmed.get(1..)?, ""),
    };
    Builtin::parse(name).map(|builtin| (builtin, args.to_string()))
}

/// Run a built-in.
///
/// Port of the eight-command dispatcher at the head of pi-acp v0.0.33 `agent.ts`'s `prompt()`.
/// Returns the `session/update`s to send; the caller answers the `session/prompt` with
/// [`BUILTIN_STOP_REASON`] **after** sending them, which is upstream's order (`await
/// conn.sessionUpdate(...)` then `return { stopReason }`).
///
/// `session_id` is the **checked** [`AcpSessionId`] and `cwd` the checked [`AbsCwd`], which is what
/// [`Builtin::Export`] needs: `ACP-291` is the one unguarded destructive path in the whole port,
/// and `AcpSessionId::export_path_in` is the only constructor of an export path.
///
/// # `ACP-292`, decided — `/compact` refuses while a turn is running
///
/// The dispatcher sits **above** the turn queue, so a built-in issued while a turn is streaming
/// executes immediately. Harmless for six of the seven. **Not for `/compact`**:
/// `AgentSession::compact` (`crates/cyrup-session-svc/src/session/compaction.rs`) opens with
/// `self.abort_and_settle().await`, so a client that pipelines `/compact` behind a running prompt
/// **kills the running turn**, whose own `session/prompt` then resolves `cancelled` — a data-losing
/// side effect of a command the user thinks is housekeeping.
///
/// **Decision: refuse, do not queue and do not abort.** Queuing would need a second queue beside
/// the one `crate::turn` owns and would make `/compact` block on a turn that may take minutes;
/// aborting is the destructive default. The refusal is a chunk, not an error, so the client
/// renders it like any other command output and the turn keeps streaming.
///
/// **`CYRUP-DELTA`:** the refusal string `Cannot compact while a turn is running. Cancel it first.`
/// has **no upstream counterpart** — upstream has no refusal because it aborts. It is a new
/// user-visible string, recorded here as the deliberate exception to house rule 4.
///
/// `/autocompact` and `/name` also mutate live session state mid-turn; both are flag writes that
/// the running turn reads on its next boundary, so they are allowed to proceed, as upstream allows
/// them.
///
/// # The tokeniser (`ACP-264`, `ACP-282`)
///
/// `args` is the **raw remainder** [`intercept`] returned, and this function splits it with
/// `cyrup_resources::parse_command_args` — cyrup's own quote-aware parser, which `ACP-264` struck
/// as already present after verifying it line for line against upstream's (same algorithm, same
/// quote semantics, no escapes, an unterminated quote absorbs the remainder). Splitting here rather
/// than at the call site is what makes "there is exactly one tokeniser" checkable by reading one
/// function: a caller handed a `&[String]` can always have built it some other way.
///
/// # Errors
///
/// [`AcpFailure`] for anything the client should see as an error frame rather than as output.
/// Following upstream, a failing `compact` or `export` **rejects the request** rather than
/// answering with an error chunk — upstream has no try/catch on the compact arm, and its export
/// arm's catch is replaced here by the typed `Result`, so the client can tell a failed command from
/// a command that reported a failure.
///
/// Those two are the **only** rejecting arms, and the list is exhaustive rather than illustrative:
/// upstream's `name` arm *does* catch and answer with a chunk, so a failing `/name` completes the
/// turn here too (`NEW`, area 4e — see [`name_failure_updates`]), and the remaining four arms
/// cannot fail at all.
pub async fn dispatch(
    builtin: Builtin,
    args: &str,
    session_id: &AcpSessionId,
    cwd: &AbsCwd,
    session: &AgentSession,
    rename_echo: &RenameEcho,
) -> Result<Vec<SessionUpdate>, AcpFailure> {
    // `ACP-264` — the ONE tokeniser. Never re-implemented here; see the doc above.
    let tokens = cyrup_resources::parse_command_args(args);
    match builtin {
        Builtin::Compact => compact(&tokens, session).await,
        Builtin::Autocompact => Ok(autocompact(&tokens, session)),
        Builtin::Export => export(session_id, cwd, session).await,
        Builtin::Session => Ok(session_stats(session).await),
        Builtin::Name => Ok(name(&tokens, session, rename_echo).await),
        Builtin::Steering => Ok(queue_mode(QueueKind::Steering, &tokens, session)),
        Builtin::FollowUp => Ok(queue_mode(QueueKind::FollowUp, &tokens, session)),
    }
}

/// One `agent_message_chunk` carrying plain text — the shape every built-in answers with.
fn chunk(text: impl Into<String>) -> SessionUpdate {
    SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
        text.into(),
    ))))
}

/// `/compact` (`ACP-283`).
///
/// Port of pi-acp v0.0.33 `agent.ts`'s `compact` arm: `args.join(' ').trim() || undefined`, then
/// `` `Compaction completed.${custom ? ' (custom instructions applied)' : ''}` ``, then
/// `` `Tokens before: ${n}` ``, joined with `\n`, then `\n\n${summary}` when the summary is
/// truthy. One chunk.
///
/// # [CYRUP-DELTA] — two `typeof` guards collapse, and one branch becomes unreachable
///
/// **What differs.** `CompactionResult` (`crates/cyrup-session-svc/src/state.rs`) is typed:
/// `summary: String` and `tokens_before: u64` are **both non-optional**, so upstream's
/// `typeof r?.tokensBefore === 'number'` and `typeof r?.summary === 'string'` guards have nothing
/// to test. `Tokens before:` is therefore **always** emitted, where upstream omitted the line when
/// pi's reply lacked the key. The summary block is still conditional, on `summary` being non-empty
/// — that is JS truthiness on `''`, preserved deliberately.
///
/// **What it costs.** A client that treated a missing `Tokens before:` line as "unknown" now always
/// sees a number. There is no case where the number is unavailable, so the line cannot be wrong.
///
/// # `ACP-Q42`, decided — the summary chunk stays, and the events are the translator's business
///
/// `AgentSession::compact` emits `CompactionStart`/`CompactionEnd` on the same operation, so if the
/// event translator also projects those (`ACP-143`) the client sees both the events and this
/// summary. **The chunk stays**: it is this command's *answer*, it is the only place the summary
/// text and the token count reach the client, and a `/compact` that produced no visible output
/// would read as a no-op. The events describe progress; the chunk describes the result. That is one
/// decision and `crate::translate` is where its other half is enforced.
async fn compact(
    tokens: &[String],
    session: &AgentSession,
) -> Result<Vec<SessionUpdate>, AcpFailure> {
    // `ACP-292` — see `dispatch`'s doc and [`compact_refusal`]. Refuse rather than abort the
    // running turn: `AgentSession::compact` opens with `abort_and_settle()`, so without this the
    // user's streaming `session/prompt` resolves `cancelled` because they typed `/compact`.
    if let Some(refusal) = compact_refusal(session.is_run_active()) {
        return Ok(refusal);
    }

    let joined = tokens.join(" ");
    let custom = joined.trim();
    let custom = (!custom.is_empty()).then(|| custom.to_string());

    let result = session
        .compact(custom.clone())
        .await
        .map_err(|e| AcpFailure::classify(&e))?;

    Ok(vec![chunk(compaction_text(
        custom.as_deref(),
        result.tokens_before,
        &result.summary,
    ))])
}

/// `ACP-283`'s three format decisions, as a value.
///
/// Split out of [`compact`] for the reason [`compact_refusal`] was: every other line of that
/// function needs a live `AgentSession`, which no unit test can build, so the string the user
/// actually reads had no assertion anywhere and each of its three decisions was one character from
/// a silent divergence —
///
/// 1. the parenthetical's **leading space** (`` `Compaction completed.${custom ? ' (custom
///    instructions applied)' : ''}` ``),
/// 2. the single `\n` before `Tokens before:` (upstream's `headerLines.join('\n')`), and
/// 3. the `\n\n` before the summary **and its JS-truthiness guard**: an empty summary omits the
///    whole block, because upstream's `summary ? … : ''` treats `''` as falsy. Preserved
///    deliberately; see [`compact`]'s delta for why the `Tokens before:` line is *not* conditional
///    here where upstream's was.
#[must_use]
fn compaction_text(custom: Option<&str>, tokens_before: u64, summary: &str) -> String {
    let mut text = format!(
        "Compaction completed.{}",
        if custom.is_some() {
            " (custom instructions applied)"
        } else {
            ""
        }
    );
    text.push_str(&format!("\nTokens before: {tokens_before}"));
    if !summary.is_empty() {
        text.push_str(&format!("\n\n{summary}"));
    }
    text
}

/// `ACP-292`'s refusal, byte-for-byte as the user reads it.
///
/// # [CYRUP-DELTA] — this string has no upstream counterpart
///
/// **What differs.** pi-acp's `compact` arm has no guard at all: it calls
/// `proc.sendCommand('compact')` whatever the session is doing. The refusal, and therefore its
/// wording, is cyrup's.
///
/// **What it costs.** It is the deliberate exception to the byte-for-byte rule for user-visible
/// strings, and it is forced: `AgentSession::compact` opens with `abort_and_settle()`, so the
/// faithful port would silently resolve a streaming `session/prompt` as `cancelled` because the
/// user typed `/compact` in a second editor pane. Refusing needs a sentence, and a sentence
/// upstream never wrote cannot be copied from it. It is a `const` so a reword is one edit and one
/// failing assertion rather than a silent change.
pub const COMPACT_BUSY_MESSAGE: &str = "Cannot compact while a turn is running. Cancel it first.";

/// `ACP-292`'s decision, as a value.
///
/// Split out from [`compact`] because every other part of that function needs a live
/// `AgentSession`, which no unit test can build — so without this the rule and its string had no
/// assertion anywhere, and disabling the guard left the suite green. It is a refusal *chunk*, not
/// an `Err`: the turn that is running keeps streaming, and an error frame on the `/compact`
/// request would read to a client as the session being broken.
#[must_use]
fn compact_refusal(run_active: bool) -> Option<Vec<SessionUpdate>> {
    run_active.then(|| vec![chunk(COMPACT_BUSY_MESSAGE)])
}

/// `/session` (`ACP-284`).
///
/// Port of pi-acp v0.0.33 `agent.ts`'s `session` arm, whose lines are built conditionally in a
/// fixed order: `Session:`, `Session file:`, `Messages:`, `Cost:`, then `Tokens: ` plus a
/// `, `-joined list of the present sub-parts in order.
///
/// # [CYRUP-DELTA] — the dead fallback is dropped and the cost format is cyrup's
///
/// **What differs.** (1) `SessionStats` (`crates/cyrup-session-svc/src/state.rs`) is typed and only
/// `session_file` is an `Option`; every other guard upstream writes collapses, and its
/// `` `Session stats:\n${JSON.stringify(stats, null, 2)}` `` fallback — reached only when *no* line
/// was produced — becomes **unreachable**. It is dropped rather than ported: a dead branch that
/// serializes a Rust struct with JS field names would be a lie the first time it ran.
/// (2) `cost` is an `f64`. pi-acp prints JS default number formatting (`Cost: 0.0123456`); this
/// prints `Cost: $0.012`, which is the format cyrup's own TUI uses for the same figure
/// (`crates/cyrup-tui/src/app/execute_session.rs` renders `${:.3}`).
///
/// **What it costs.** (1) A client parsing the JSON fallback has nothing to parse — but nothing
/// could ever produce it. (2) The cost string differs from pi-acp's byte-for-byte, deliberately, so
/// that a user reading `/session` in Zed and in the TUI sees the same number in the same shape. Sub
/// tenth-of-a-cent costs render as `$0.000`; the exact figure is in the session file.
///
/// # `ACP-Q43`, decided — the five lines only, no extension
///
/// `SessionStats` additionally carries `user_messages`, `assistant_messages`, `tool_calls`,
/// `tool_results` and `context_usage`, all of which the TUI shows. **Not added**: `ACP-284`'s
/// verify pins the exact five-line shape, extending it is a superset enhancement rather than a port
/// unit, and the natural home for the richer view is `UsageUpdate` (which the ACP schema already
/// has and which the event translator owns) rather than a hand-formatted text block.
async fn session_stats(session: &AgentSession) -> Vec<SessionUpdate> {
    vec![chunk(stats_text(&session.session_stats().await))]
}

/// `ACP-284`'s five-line shape, as a value.
///
/// Split out of [`session_stats`] for [`compact_refusal`]'s reason: `session_stats` takes an
/// `&AgentSession`, so no unit test could reach the composition, and the integration case
/// (`crates/cyrup-it/tests/bin/acp_session.rs`) asserts three of the five lines. The two that were
/// unasserted in either direction are exactly the two that can silently go wrong:
///
/// * the `Tokens:` line's **five comma-joined sub-parts in order** — a reorder or a renamed
///   sub-label reads as a different wire string to a client that parses it, and
/// * the `Session file:` conditional, the one genuinely conditional line after the typed-stats
///   collapse (see this arm's delta): an `Option` that became always-`Some` would add a line to
///   every in-memory session's output with nothing failing.
///
/// The integration case stays as the proof that the arm is reached above the turn queue; this is
/// the proof of what it says.
#[must_use]
fn stats_text(stats: &cyrup_session_svc::SessionStats) -> String {
    let mut lines = vec![format!("Session: {}", stats.session_id)];
    // The one genuinely conditional line: an in-memory session has no file.
    if let Some(file) = &stats.session_file {
        lines.push(format!("Session file: {file}"));
    }
    lines.push(format!("Messages: {}", stats.total_messages));
    lines.push(format!("Cost: ${:.3}", stats.cost));
    let t = &stats.tokens;
    lines.push(format!(
        "Tokens: in {}, out {}, cache read {}, cache write {}, total {}",
        t.input, t.output, t.cache_read, t.cache_write, t.total
    ));
    lines.join("\n")
}

/// `/name` (`ACP-285`).
///
/// Port of pi-acp v0.0.33 `agent.ts`'s `name` arm. Empty argument → one chunk `Usage: /name <name>`.
/// Otherwise set the name, then answer `Session name set: <name>`. Note `args.join(' ')` re-joins
/// the quote-stripped tokens, so `/name "a  b"` becomes `a b` — preserved exactly.
///
/// # [CYRUP-DELTA] — the `session_info_update` is emitted **here**, and the pump's copy is claimed
///
/// **What differs.** Upstream's arm emits two updates and its setter emits nothing, so the two
/// halves are independent. Here `AgentSession::set_session_name` *also* fans out
/// `AgentSessionEvent::SessionInfoChanged { name }` to every subscriber and dispatches a
/// `HostEvent::SessionInfoChanged` to extensions, which `crate::sessions`' `config_pump` — a
/// **different task** — would project into a second `session_info_update` that always lost the race
/// with this command's own response (`ACP-122`; see [`RenameEcho`] for the observed 9-of-9 wire
/// order). So the causer emits upstream's update itself, in its own ordered output, and
/// [`RenameEcho::claim`] makes the pump swallow the copy the fanout is about to produce.
///
/// **The claim and the emission are ONE unit and neither may be removed alone.** Deleting the
/// `session_info_update` from the returned vec while leaving `claim.keep()` in place makes the
/// pump consume the claim and emit nothing, so a rename produces **zero** `session_info_update`s
/// with no error anywhere. Deleting the claim while leaving the emission produces two.
///
/// **What it costs.** `ACP-285`'s verify — **exactly one** `session_info_update` with a parseable
/// ISO-8601 `updatedAt` — is a property of this function *plus* the pump, so it is asserted end to
/// end in `crates/cyrup-it` as well as here ([`name_updates`] pins this half). In exchange a
/// rename originating from an extension, from the TUI on the same session file, or from any other
/// front-end still reaches the client through the pump, identically — which upstream's
/// setter-local emission cannot do.
///
/// # [CYRUP-DELTA] — an empty name is refused before the setter, and the skew hint is cut
///
/// **What differs.** Upstream's arm has a `catch` that appends *"This requires a newer pi version
/// that supports `set_session_name` in RPC mode"* when the error message matches
/// `/set_session_name/i`. There is no version skew in-process; the hint is cut. The RPC precedent
/// (`cyrup_modes::rpc`'s `SessionCommand::SetSessionName`) refuses an empty name with
/// `Session name cannot be empty`, which the usage line already covers here.
///
/// **What it costs.** Nothing: the cut branch describes a failure mode that cannot occur. The
/// `catch` **itself** is ported — see [`name_failure_updates`] — because `/name` is the one arm
/// where upstream answers a failure instead of rejecting the request.
async fn name(
    tokens: &[String],
    session: &AgentSession,
    rename_echo: &RenameEcho,
) -> Vec<SessionUpdate> {
    let joined = tokens.join(" ");
    let name = joined.trim();
    if name.is_empty() {
        // `ACP-Q44`, decided: the usage line, not the TUI's "show the current name". A client that
        // wants the name has `session/list`'s `SessionInfo.title` and the pump's
        // `session_info_update`; overloading a setter to be a getter is what makes `/name` with a
        // typo'd argument silently do nothing.
        return vec![chunk(NAME_USAGE)];
    }

    // `ACP-122` — claimed BEFORE the mutation, because `set_session_name` emits
    // `SessionInfoChanged` synchronously into the fanout and the pump is a different task: a claim
    // taken afterwards could lose the race and the client would get the update twice. See
    // [`RenameEcho`].
    let claim = rename_echo.claim();
    match session.set_session_name(name).await {
        Ok(()) => {
            claim.keep();
            name_updates(name, crate::sessions::now_iso8601_millis())
        }
        // The rename did not happen, so nothing will arrive for the pump to suppress. `claim` is
        // dropped un-kept and releases itself.
        Err(e) => name_failure_updates(&e.to_string()),
    }
}

/// `/name`'s usage line, for the empty argument (`ACP-Q44`). Upstream's string.
const NAME_USAGE: &str = "Usage: /name <name>";

/// `ACP-285`'s two updates, in upstream's order, as a value.
///
/// The `session_info_update` first, then the confirmation the user reads — upstream's order, and
/// the order that matters to a client which renders the title from the notification and the
/// transcript from the chunk. Split out of [`name`] for [`compact_refusal`]'s reason (the rest of
/// that function needs a live `AgentSession`) and because the delta above turns on there being
/// **exactly one** update of each kind here: this is where that is asserted.
#[must_use]
fn name_updates(name: &str, updated_at: String) -> Vec<SessionUpdate> {
    vec![
        crate::config_options::session_info_update(Some(name.to_string()), updated_at),
        chunk(format!("Session name set: {name}")),
    ]
}

/// Upstream's `catch` on the `/name` arm, ported (`NEW`, area 4e).
///
/// pi-acp v0.0.33 `agent.ts`'s `name` arm wraps `setSessionName` in a try/catch that emits
/// `` `Failed to set session name: ${msg}${hint}` `` as an `agent_message_chunk` and returns
/// `{stopReason:'end_turn'}`: the turn completes and the user reads the failure in the transcript.
/// This crate previously answered `Err(AcpFailure::classify(&e))` instead, which
/// `SessionManager::serve_builtin` turns into a JSON-RPC error frame on the
/// `session/prompt` — so a disk error on a rename surfaced in Zed as a **failed request**, and
/// [`dispatch`]'s "following upstream, a failing command rejects the request" is true of `compact`
/// and `export` (upstream has no catch on the first and its catch is replaced by the typed `Result`
/// on the second) but was **false** for the one arm where upstream does catch and answer. Restored
/// to upstream's behaviour, minus the version-skew `hint` this port cut for the reason in [`name`]'s
/// second delta.
///
/// The message is the error's `Display`, which is upstream's `String(e?.message ?? e)`.
#[must_use]
fn name_failure_updates(error: &str) -> Vec<SessionUpdate> {
    vec![chunk(format!("Failed to set session name: {error}"))]
}

/// The one-emitter rule (`ACP-Q20`) reconciled with the response-follows-notifications rule
/// (`ACP-122`), for the one built-in that mutates a fact the session-wide pump also reports.
///
/// # The problem this exists for
///
/// `/name` is answered **above** the turn queue (`ACP-282`, upstream's dispatcher position): the
/// arm mutates, `SessionManager::serve_builtin` writes the arm's updates and then answers the
/// `session/prompt`. The rename's `session_info_update`, however, was derived by
/// `crate::sessions::config_pump` from `AgentSessionEvent::SessionInfoChanged` — a *different
/// task* — so it always lost the race. The observed wire order was
/// `agent_message_chunk` → `{"stopReason":"end_turn"}` → `session_info_update`, 9 runs out of 9:
/// a client that treats the prompt response as the end of the turn attributes the rename to the
/// next turn, or drops it.
///
/// # The mechanism
///
/// The causer claims the next `SessionInfoChanged` *before* it mutates, emits the update itself in
/// its own ordered output, and the pump consumes the claim and emits nothing. Exactly one update
/// reaches the client, and for the built-in it reaches it before the response.
///
/// A counter rather than a slot: two `/name`s can overlap (`serve_prompt` is spawned per request),
/// and a slot would lose one. It is deliberately **not** keyed by name — an extension renaming
/// concurrently could consume a `/name`'s claim and vice versa, which changes which *route* emits
/// each update but not how many are emitted, and the built-in's own update is written before its
/// own response either way. Keying by name would trade that for a worse failure: two renames to
/// the same string, one of them silently doubled.
///
/// A claim is released if the mutation fails, so a rejected `/name` does not swallow an unrelated
/// rename that arrives later.
#[derive(Clone, Default)]
pub struct RenameEcho {
    outstanding: Arc<AtomicUsize>,
}

impl RenameEcho {
    /// Claim the next `SessionInfoChanged`. Call **before** the mutation.
    ///
    /// The returned guard releases the claim on drop unless [`RenameClaim::keep`] is called, so a
    /// mutation that fails — or an early `?` added later — cannot leave a claim behind that
    /// silences someone else's rename.
    #[must_use]
    pub fn claim(&self) -> RenameClaim<'_> {
        self.outstanding.fetch_add(1, Ordering::SeqCst);
        RenameClaim { echo: Some(self) }
    }

    /// The pump's question: has this event already been emitted by whatever caused it?
    ///
    /// Consumes one claim when the answer is yes.
    #[must_use]
    pub fn take(&self) -> bool {
        self.outstanding
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
    }
}

/// One outstanding [`RenameEcho`] claim. See that type.
pub struct RenameClaim<'a> {
    echo: Option<&'a RenameEcho>,
}

impl RenameClaim<'_> {
    /// The mutation succeeded, so the event IS coming and the claim must stand.
    pub fn keep(mut self) {
        self.echo = None;
    }
}

impl Drop for RenameClaim<'_> {
    fn drop(&mut self) {
        if let Some(echo) = self.echo {
            // Saturating: `take` may already have consumed this claim on behalf of some other
            // rename, and going negative is not representable.
            let _ = echo
                .outstanding
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
        }
    }
}

/// Which of the two identically-shaped queue-mode commands is being run.
///
/// `ACP-287`'s verify is that the two write to **different** modes — the failure this enum makes
/// unwritable is a copy-paste of the `steering` arm that leaves `setSteeringMode` in the
/// `follow-up` branch, which upstream's two near-identical 40-line blocks invite.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QueueKind {
    Steering,
    FollowUp,
}

impl QueueKind {
    /// The label in every user-visible string of this arm.
    fn label(self) -> &'static str {
        match self {
            QueueKind::Steering => "Steering",
            QueueKind::FollowUp => "Follow-up",
        }
    }

    /// The command name, for the usage line.
    fn command(self) -> &'static str {
        match self {
            QueueKind::Steering => "steering",
            QueueKind::FollowUp => "follow-up",
        }
    }

    fn read(self, session: &AgentSession) -> QueueMode {
        match self {
            QueueKind::Steering => session.steering_mode(),
            QueueKind::FollowUp => session.follow_up_mode(),
        }
    }

    fn write(self, session: &AgentSession, mode: QueueMode) {
        match self {
            QueueKind::Steering => session.set_steering_mode(mode),
            QueueKind::FollowUp => session.set_follow_up_mode(mode),
        }
    }
}

/// The wire spelling of a queue mode — pi's two strings (`settings-manager.ts`), which
/// `cyrup_session_svc::QueueMode`'s own `FromStr` accepts and which the RPC arm already emits.
///
/// Written out rather than routed through a `Display` because `QueueMode` has none; a `Display`
/// added later must agree with this, which `the_queue_mode_spelling_round_trips` asserts.
fn queue_mode_id(mode: QueueMode) -> &'static str {
    match mode {
        QueueMode::All => "all",
        QueueMode::OneAtATime => "one-at-a-time",
    }
}

/// `/steering` (`ACP-286`) and `/follow-up` (`ACP-287`) — one arm, two modes.
///
/// Port of pi-acp v0.0.33 `agent.ts`'s two near-identical blocks: no argument reports the current
/// mode; `all` or `one-at-a-time` sets it; anything else prints the usage line. All three strings
/// are upstream's.
///
/// # [CYRUP-DELTA] — `unknown` is unreachable, and the two blocks are one function
///
/// **What differs.** Upstream reads `String(state?.steeringMode ?? '')` off an untyped RPC reply
/// and prints `Steering mode: unknown` when the key is missing. `AgentSession::steering_mode()`
/// returns a typed `QueueMode` that always has a value, so the `|| 'unknown'` fallback is
/// unreachable and is dropped. And upstream's two blocks are one function here, parameterised by
/// [`QueueKind`], because they differed only in four literals and one setter.
///
/// **What it costs.** Nothing observable: `unknown` was only ever printed when pi's reply was
/// malformed.
fn queue_mode(kind: QueueKind, tokens: &[String], session: &AgentSession) -> Vec<SessionUpdate> {
    let (updates, write) =
        queue_mode_updates(kind, tokens.first().map(String::as_str), kind.read(session));
    // The write is the pure half's *decision*, applied here — never a second parse of `raw`.
    if let Some(mode) = write {
        kind.write(session, mode);
    }
    updates
}

/// `ACP-286`/`ACP-287`'s three branches, three strings and one write decision, as values.
///
/// Split out of [`queue_mode`] for [`compact_refusal`]'s reason — that function takes an
/// `&AgentSession` purely to reach `kind.read`/`kind.write`, so nothing could reach the composition
/// from a unit test and the three composed strings (`"{label} mode: {id}"`,
/// `"{label} mode set to: {id}"` and `"Usage: /{cmd} all | /{cmd} one-at-a-time"`) were unpinned
/// while only their *pieces* were asserted.
///
/// The returned `Option<QueueMode>` is the whole write decision, and it is upstream's own test
/// assertion made checkable: `test/component/agent-steering-followup-modes.test.ts` asserts
/// `setTo === 'one-at-a-time'` on the set branch and `called === false` on the invalid one, i.e.
/// exactly `Some(OneAtATime)` and `None` here. A read or a usage line that started writing is a
/// failing test rather than a silent mode change.
///
/// `arg` is the raw token; the `toLowerCase()` is upstream's and lives **here** so the case table
/// is pinned with the strings rather than at the one call site.
#[must_use]
fn queue_mode_updates(
    kind: QueueKind,
    arg: Option<&str>,
    current: QueueMode,
) -> (Vec<SessionUpdate>, Option<QueueMode>) {
    // Upstream's `String(args[0] ?? '').toLowerCase()`.
    let raw = arg.map(str::to_lowercase).unwrap_or_default();
    if raw.is_empty() {
        return (
            vec![chunk(format!(
                "{} mode: {}",
                kind.label(),
                queue_mode_id(current)
            ))],
            None,
        );
    }
    let mode = match raw.as_str() {
        "all" => QueueMode::All,
        "one-at-a-time" => QueueMode::OneAtATime,
        _ => {
            return (
                vec![chunk(format!(
                    "Usage: /{cmd} all | /{cmd} one-at-a-time",
                    cmd = kind.command()
                ))],
                None,
            );
        }
    };
    (
        vec![chunk(format!(
            "{} mode set to: {}",
            kind.label(),
            queue_mode_id(mode)
        ))],
        Some(mode),
    )
}

/// `/autocompact` (`ACP-289`).
///
/// Port of pi-acp v0.0.33 `agent.ts`'s `autocompact` arm: `args[0] ?? 'toggle'`, lowercased;
/// `on`/`true`/`enable`/`enabled` → true; `off`/`false`/`disable`/`disabled` → false; **anything
/// else, including a typo and the literal `toggle`, inverts the current state**. The answer is
/// `Auto-compaction enabled.` or `Auto-compaction disabled.`, byte-for-byte.
///
/// The "a typo toggles" behaviour is upstream's and is preserved: the `enabled === null` branch is
/// reached by every unrecognised word, not only by `toggle`. It is worth knowing when reading the
/// unit's table — `/autocompact onn` flips the flag rather than printing a usage line.
fn autocompact(tokens: &[String], session: &AgentSession) -> Vec<SessionUpdate> {
    let enabled = autocompact_target(
        tokens.first().map(String::as_str),
        session.auto_compaction_enabled(),
    );
    session.set_auto_compaction_enabled(enabled);
    vec![chunk(autocompact_text(enabled))]
}

/// `ACP-289`'s accept table and its one surprise, as a value.
///
/// Split out of [`autocompact`] for [`compact_refusal`]'s reason — the arm needs a live
/// `AgentSession` for both the read and the write, so **none** of the seven inputs `ACP-289`'s
/// verify enumerates (`on`/`enabled`/`off`/`disabled`/no-arg/`toggle`/a typo) was asserted anywhere.
///
/// The surprise is the last one: upstream's `enabled === null` branch is reached by **every**
/// unrecognised word, not only by the literal `toggle`, so `/autocompact onn` **inverts** the flag
/// rather than printing a usage line. That is the behaviour a later reader will "fix" into a usage
/// line; it is upstream's, it is preserved, and it is now a failing test rather than a review note.
///
/// The lowercasing is upstream's `(args[0] ?? 'toggle').toLowerCase()` and lives here so the
/// 8-word table is pinned with the inputs that reach it.
#[must_use]
fn autocompact_target(arg: Option<&str>, current: bool) -> bool {
    let mode = arg.map_or_else(|| "toggle".to_string(), str::to_lowercase);
    match mode.as_str() {
        "on" | "true" | "enable" | "enabled" => true,
        "off" | "false" | "disable" | "disabled" => false,
        // Upstream's `enabled === null` branch: `toggle`, and every typo, inverts.
        _ => !current,
    }
}

/// `ACP-289`'s answer, byte-for-byte — upstream's
/// `` `Auto-compaction ${enabled ? 'enabled' : 'disabled'}.` ``, trailing period included.
#[must_use]
fn autocompact_text(enabled: bool) -> String {
    format!(
        "Auto-compaction {}.",
        if enabled { "enabled" } else { "disabled" }
    )
}

/// `/export` (`ACP-288`) — and the security control that makes it safe (`ACP-291`).
///
/// Port of pi-acp v0.0.33 `agent.ts`'s `export` arm. Emits **two** chunks: a text chunk equal to
/// `"Session exported: "` — trailing space, no newline, which upstream's comment explains avoids
/// the "link + duplicate plain text" look in clients that concatenate chunks — then a
/// `resource_link` with `name`, `uri: file://…`, `mimeType: "text/html"` and
/// `title: "Session exported"`.
///
/// # `ACP-291` — the sanitiser is a boundary check, not decoration
///
/// Upstream computes `safeSessionId = sessionId.replace(/[^a-zA-Z0-9_-]/g,'_')` and joins it into
/// the cwd. **That regex is the only thing standing between a client-supplied id and an arbitrary
/// file write**: `session/load` takes the id from the client and every `session/prompt` carries it,
/// and `PathBuf::join` with an absolute component **replaces** the base, so
/// `cwd.join("cyrup-session-/etc/x.html")` is `/etc/x.html`. On the cyrup side there is no second
/// line of defence — `AgentSession::export_to_html` takes the caller's path verbatim and ends in a
/// bare `std::fs::write` with no normalisation, no containment check and no consultation of
/// `cyrup-permission-system`.
///
/// Here the id has already been through `cyrup_session::validate_session_id` at the handler
/// boundary ([`AcpSessionId::parse`]), and [`AcpSessionId::export_path_in`] is the **only**
/// constructor of an export path — it re-checks `parent() == Some(dir)` after composing, so the
/// containment cannot be simplified away into a `format!`. `ACP-Q45` and `ACP-Q46` are pre-decided
/// and are not reversible here: **no client-supplied path is accepted**, and containment is a real
/// boundary check.
///
/// The *directory* that check confines the write to is [`export_dir`]'s, not the caller's — see
/// that function for the slot race a containment check cannot see.
///
/// # [CYRUP-DELTA] — the guard branches and the empty-path branch are gone; overwrite is parity
///
/// **What differs.** Upstream guards with three pre-flight branches (`!sessionFile ||
/// messageCount === 0 || !existsSync`, an empty-file read, and an unreadable-file catch) because
/// pi's `export_html` throws on an empty JSONL and RPC mode then emits an **uncorrelated** parse
/// error with no id, which would hang the request. In-process there is no correlation to lose:
/// `export_to_html` reads the live branch through `SessionManager::export_jsonl` and returns a
/// typed `Result`, so the three guards and the `no output path returned by pi` branch (the return
/// is a `PathBuf` and can never be empty) are all dropped. The artefact is renamed
/// `cyrup-session-<id>.html` to match cyrup's own default. An existing file at that path is
/// **overwritten** — that half is parity, pi-acp overwrote too, and it is recorded rather than
/// treated as the regression.
///
/// **What it costs.** Exporting a session with no messages now produces a valid but nearly empty
/// HTML document instead of the message `Nothing to export yet (no session messages). Send a prompt
/// first.` Upstream's three strings are dropped with their branches; keeping them would mean
/// re-deriving "is this session empty" from `session_stats()` to reproduce a guard against a
/// failure mode that does not exist here.
///
/// # [CYRUP-DELTA] — the `file://` URI assumes a UTF-8 path
///
/// **What differs.** `PathBuf` is not UTF-8-guaranteed and the ACP `uri` is a `String`. A non-UTF-8
/// path is rendered with `Path::display`, i.e. with replacement characters, rather than
/// percent-encoded.
///
/// **What it costs.** On such a path the link is unopenable — but the file is written correctly and
/// its name is in the chunk. The alternative (fail the export) loses the artefact over a display
/// concern.
async fn export(
    session_id: &AcpSessionId,
    cwd: &AbsCwd,
    session: &AgentSession,
) -> Result<Vec<SessionUpdate>, AcpFailure> {
    // `ACP-291` — the ONLY constructor of an [`crate::ids::ExportPath`], which is the only type
    // this function can hand to `export_to_html`. Substituting a bare
    // `cwd.join(format!("cyrup-session-{session_id}.html"))` here does not compile, which is the
    // point: `ACP-Q45`'s rule ("`/export` composes its own path and containment-checks it") is a
    // property of the types rather than of this line staying as written.
    let path = session_id.export_path_in(export_dir(cwd, &session.services().cwd))?;
    let written = session
        .export_to_html(Some(path.as_path()))
        .await
        .map_err(|e| AcpFailure::classify(&e))?;

    let name = written
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        // Unreachable: `export_path_in` composes `<dir>/cyrup-session-<id>.html`, which always has
        // a file name. The fallback is the id rather than a panic or an `unwrap`.
        .unwrap_or_else(|| format!("cyrup-session-{session_id}.html"));

    Ok(export_updates(name, file_uri(&written)))
}

/// Which directory `/export` writes into (`NEW`, area 4e — the ACP-291 failure class through the
/// cwd rather than through the id).
///
/// **The session's own cwd wins, always.** Upstream is `join(session.cwd, …)` where `session` is
/// the *resolved binding*; the `cwd` this function is handed comes from
/// [`crate::sessions::SessionManager`]'s live slot, which is the one slot read on this path that
/// does not filter on the session id — `get`, `take_live`, `turn_for` and `rename_echo_for` all do.
/// `serve_prompt` resolves the session and then runs spawned, while `session/new` and
/// `session/load` replace the slot from a different task, so a client that pipelines a
/// `session/new` for project **B** behind a `/export` prompt for session **A** would otherwise get
/// `cyrup-session-<A-id>.html` written into **B**'s working directory. Containment still held —
/// [`AcpSessionId::export_path_in`] confines the write to the directory it is handed — but it held
/// against the *wrong* directory, so it is a mislocated file rather than an escape.
///
/// The connection's cwd is kept as a **cross-check** rather than dropped: the two agree on every
/// non-racing path (a session is built by `build_and_install` with exactly the `AbsCwd` it stores
/// in the slot), so a disagreement is the race itself and is worth a log line. It is deliberately
/// *not* a refusal — the session's cwd is the correct destination either way, and refusing would
/// turn a benign slot move into a failed export.
fn export_dir<'a>(connection_cwd: &'a AbsCwd, session_cwd: &'a Path) -> &'a Path {
    if connection_cwd.as_path() != session_cwd {
        tracing::warn!(
            slot_cwd = %connection_cwd.as_path().display(),
            session_cwd = %session_cwd.display(),
            "ACP-288: the live-session slot moved under /export; exporting into the session's own cwd"
        );
    }
    session_cwd
}

/// `ACP-288`'s wire shape, as a value.
///
/// Split out of [`export`] for [`compact_refusal`]'s reason — the rest of that function needs a
/// live `AgentSession` — and because every part of this emission is one character from a
/// divergence that no test would have caught: the **two** updates and their order, the trailing
/// space in `"Session exported: "` (upstream's own comment explains it: clients concatenate chunks,
/// so a newline or a missing space produces the "link + duplicate plain text" look), and the
/// resource link's `mimeType: "text/html"` and `title: "Session exported"`.
#[must_use]
fn export_updates(name: String, uri: String) -> Vec<SessionUpdate> {
    vec![
        // Upstream's exact prefix, trailing space preserved and NO newline.
        chunk("Session exported: "),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::ResourceLink(
            ResourceLink::new(name, uri)
                .mime_type("text/html".to_string())
                .title("Session exported".to_string()),
        ))),
    ]
}

/// `file://<absolute path>` — upstream's `` `file://${resultPath}` ``, unencoded.
fn file_uri(path: &Path) -> String {
    format!("file://{}", path.display())
}

// ===================================================================================================
// The catalog projection — `ACP-263`, `ACP-267` … `ACP-272`, `ACP-290`
// ===================================================================================================

/// Provenance for a row whose description is empty or absent (`ACP-263`, `ACP-080`).
///
/// Port of pi-acp v0.0.33 `pi-commands.ts`'s `describeFallback`: `(${[source, location].join(':')})`
/// when either is present, else the literal `(command)`.
///
/// # [CYRUP-DELTA] — `location` becomes `sourceInfo.scope`, because `location` does not exist
///
/// **What differs.** Upstream reads a top-level `location` key. `ACP-270` established that pi's own
/// `get_commands` never emits one either, so upstream's `(prompt:project)` shape fires **only in
/// its own unit test** and a real pi always produced `(prompt)`. cyrup's rows carry the provenance
/// under `sourceInfo` (`scope` is `user`/`project`/`temporary`, from
/// `cyrup_resources::ResourceOrigin::source_info_json`), so the second part is read from there —
/// which produces the shape upstream's test asserts, against real data, for the first time.
///
/// **What it costs.** A row's fallback description reads `(prompt:user)` where a byte-faithful port
/// of the live path would read `(prompt)`. `ACP-263`'s verify asks for exactly this: a row with
/// `sourceInfo.scope == "user"` and an empty description projects to a **non-empty** description
/// carrying **exactly one** provenance marker.
#[must_use]
fn describe_fallback(row: &serde_json::Value) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if let Some(source) = row.get("source").and_then(serde_json::Value::as_str)
        && !source.is_empty()
    {
        parts.push(source);
    }
    if let Some(scope) = row
        .get("sourceInfo")
        .and_then(|s| s.get("scope"))
        .and_then(serde_json::Value::as_str)
        && !scope.is_empty()
    {
        parts.push(scope);
    }
    if parts.is_empty() {
        "(command)".to_string()
    } else {
        format!("({})", parts.join(":"))
    }
}

/// Project `AgentSession::slash_command_catalog()` rows onto ACP `AvailableCommand`s (`ACP-267`).
///
/// Port of pi-acp v0.0.33 `pi-commands.ts`'s `toAvailableCommandsFromPiGetCommands` and
/// `slash-commands.ts`'s `toAvailableCommands`, merged: those two functions produced the same
/// output from two sources, and cyrup has one source.
///
/// Three rules, all of which have a verify: a nameless row is **dropped** (`ACP-Q38`, decided —
/// **keep the guard**, because it is one `is_empty()` over an untyped `Value` and its cost is a
/// branch, while its absence would put an unaddressable empty-named entry in the client's palette);
/// a description-less row carries [`describe_fallback`] rather than `""` (`AvailableCommand
/// ::description` is a **required `String`** in Rust, so `""` is representable and is exactly the
/// wrong answer); and de-duplication is by name, **first wins**.
///
/// `enable_skill_commands` gates `skill:`-prefixed rows (`ACP-268`). It is **advertisement-only**:
/// a gated-out `/skill:<name>` still expands when submitted, because expansion is
/// `AgentSession::prepare_and_assemble`'s and this function is not in that path. Read it off the
/// session — `session.services().settings.effective().enable_skill_commands()` — never from a free
/// `fn(cwd)` re-reading settings files, which would reintroduce a **trust bypass**
/// (`pi-settings.ts` is cut for exactly that reason). [`available_commands`] is the call site that
/// does this correctly.
///
/// # [CYRUP-DELTA] — `ACP-269`: extension commands are advertised, reversing upstream
///
/// **What differs.** `pi-commands.ts` removes every `source === 'extension'` row unless the caller
/// opts in, and `agent.ts` never opts in. **cyrup includes them.** The exclusion is an
/// out-of-process workaround: pi-acp cannot know whether an extension command needs UI it cannot
/// serve, and its `prompt()` hands text to a subprocess. In cyrup the same submission reaches
/// `AgentSession::prepare`, whose step 0 is `try_execute_extension_command` — the command runs, its
/// outcome is surfaced through `surface_command_outcome` → `HostServices::notify` →
/// `UiEffect::Notify`, and the ACP host renders that as an `agent_message_chunk` with no model
/// call. cyrup's TUI already advertises them (`dynamic_commands_from_catalog_gated`,
/// `crates/cyrup-tui/src/commands.rs`), so excluding them here would make one front-end show
/// strictly less than another off the same session. **`ACP-Q17` is settled by this**, and
/// `ACP-069`'s `includeExtensionCommands: false` follows it rather than the reverse.
///
/// **What it costs.** `ACP-Q39`: an extension command that opens a `UiKind::Editor` dialog has no
/// faithful ACP rendering, so it degrades — that degrades the command, it does not make advertising
/// it wrong. If `Editor` ends up cancelling to `Text(None)`, a per-command capability filter may be
/// worth adding; the hook for it is this function, and it is not added speculatively.
///
/// # [CYRUP-DELTA] — `ACP-271`: `argumentHint` is carried where upstream omitted `input`
///
/// **What differs.** `slash-commands.ts` says `// input: omitted for now (pi commands don't specify
/// this)`. cyrup's catalog carries `argumentHint` for a prompt template whose frontmatter declares
/// one, so it is projected to `AvailableCommandInput::Unstructured`, which is the same shape the
/// built-ins use.
///
/// **What it costs.** Nothing: a row without the key still projects to `input: None`.
///
/// # [CYRUP-DELTA] — `ACP-290`: the projection is sorted, because the catalog is not ordered
///
/// **What differs.** pi-acp's list is deterministic (user templates in directory order, then
/// project). cyrup's is **not**: `slash_command_catalog` sources its prompt and skill rows from
/// `ResourceSet::winners()`, which is `by_key.values()` over a `std::collections::HashMap` whose
/// own doc says *"order unspecified"* — so the `available_commands_update` payload would reorder
/// its prompt and skill rows on every process start, shuffling the Zed command menu between
/// launches for no user-visible reason and making any golden over it flaky by construction. Only
/// the extension arm is ordered (extension load order, which is meaningful).
///
/// This function therefore sorts **within each provenance group, by name**, and preserves the group
/// order the catalog emits (extension, then prompt, then skill). The extension group keeps its load
/// order untouched.
///
/// **What it costs.** A user's prompt templates are advertised alphabetically rather than in
/// directory order — which is what upstream's directory order approximated anyway, and is the only
/// order a `HashMap` source can offer. The proper fix is an ordered `winners()` in
/// `cyrup-resources`; **the ACP host must not assume the order it is given**, which is what this
/// sort encodes.
#[must_use]
pub fn project_catalog(
    rows: &[serde_json::Value],
    enable_skill_commands: bool,
) -> Vec<AvailableCommand> {
    /// The group order the catalog emits, made explicit so the sort cannot reorder groups.
    fn group_rank(row: &serde_json::Value) -> u8 {
        match row.get("source").and_then(serde_json::Value::as_str) {
            Some("extension") => 0,
            Some("prompt") => 1,
            Some("skill") => 2,
            _ => 3,
        }
    }

    let mut ordered: Vec<&serde_json::Value> = rows.iter().collect();
    // A STABLE sort, so the extension group — whose rank is equal for every member — keeps the load
    // order `resolved_commands()` gave it, while the two `HashMap`-sourced groups get a total order
    // from the name.
    ordered.sort_by(|a, b| {
        let by_group = group_rank(a).cmp(&group_rank(b));
        if by_group != std::cmp::Ordering::Equal {
            return by_group;
        }
        if group_rank(a) == 0 {
            // Extension load order is meaningful; do not impose a name order on it.
            return std::cmp::Ordering::Equal;
        }
        let name = |v: &serde_json::Value| {
            v.get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        name(a).cmp(&name(b))
    });

    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out = Vec::new();
    for row in ordered {
        let name = row
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .trim();
        // `ACP-Q38` — the nameless-row guard, kept.
        if name.is_empty() {
            continue;
        }
        // `ACP-268` — advertisement-only.
        if !enable_skill_commands && name.starts_with("skill:") {
            continue;
        }
        if !seen.insert(name.to_string()) {
            continue;
        }
        let description = row
            .get("description")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        let description = if description.is_empty() {
            describe_fallback(row)
        } else {
            description
        };
        let command = AvailableCommand::new(name, description);
        // `ACP-271`.
        let command = match row.get("argumentHint").and_then(serde_json::Value::as_str) {
            Some(hint) if !hint.is_empty() => command.input(AvailableCommandInput::Unstructured(
                UnstructuredCommandInput::new(hint),
            )),
            _ => command,
        };
        out.push(command);
    }
    out
}

/// Merge the built-ins with the catalog projection — **first wins, order preserved** (`ACP-071`,
/// `ACP-272`).
///
/// Port of pi-acp v0.0.33 `agent.ts`'s `mergeCommands`, **with its argument order corrected**.
///
/// # [CYRUP-DELTA] — built-ins first, where upstream put them last
///
/// **What differs.** Upstream calls `mergeCommands(piCommands, builtins)`, so a user command named
/// `compact` **shadows the builtin in the advertised list** while `prompt()`'s if-chain still
/// intercepts `/compact` as the builtin. The advertised menu and the dispatcher therefore disagree
/// upstream: the palette shows the user's description and running it does something else. Here the
/// built-ins come first, so the advertised list matches what [`intercept`] actually dispatches.
///
/// **What it costs.** Two things, both deliberate. (1) The advertised **order** differs from
/// pi-acp's: built-ins lead. (2) `ACP-071`'s "a user command named `compact` shadows the builtin"
/// no longer holds — the builtin wins, which is the point. The unit's other half, `1 +
/// BUILTINS.len() - 1` total entries for one colliding user command, holds either way and is what
/// `a_colliding_user_command_is_dropped_not_duplicated` asserts.
#[must_use]
pub fn merge_commands(user: Vec<AvailableCommand>) -> Vec<AvailableCommand> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(BUILTINS.len() + user.len());
    for command in builtin_commands().into_iter().chain(user) {
        if seen.insert(command.name.clone()) {
            out.push(command);
        }
    }
    out
}

/// The whole advertised command set for a live session (`ACP-069`, `ACP-267`…`ACP-272`).
///
/// The **one** function both `session/new` and `session/load` call, which is `ACP-069`'s deliverable:
/// upstream duplicates the block in `newSession` and `loadSession` with a shortened comment, and a
/// port that gets the ordering right on one and reuses a plain `send_notification` on the other
/// produces a session whose command menu is empty for reasons no log shows.
///
/// The skill gate is read off the session here, once — see [`project_catalog`] for why it must not
/// be re-derived from the filesystem.
#[must_use]
pub fn available_commands(session: &AgentSession) -> Vec<AvailableCommand> {
    let enable_skill_commands = session
        .services()
        .settings
        .effective()
        .enable_skill_commands();
    merge_commands(project_catalog(
        &session.slash_command_catalog(),
        enable_skill_commands,
    ))
}

/// [`available_commands`] as the `session/update` both handlers send **after** their response
/// (`ACP-069`, `ACP-293`).
///
/// The ordering is not this function's to enforce — it is [`crate::HandlerOutcome`]'s, and the
/// reason is upstream's own comment on the `setTimeout` this replaces: *"some clients (e.g. Zed)
/// will ignore notifications for an unknown sessionId. So we must send this after the session/new
/// response has been delivered."* Returning the value rather than sending it is what keeps that
/// true without a timer.
#[must_use]
pub fn available_commands_update(session: &AgentSession) -> SessionUpdate {
    SessionUpdate::AvailableCommandsUpdate(AvailableCommandsUpdate::new(available_commands(
        session,
    )))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{
        AudioContent, BlobResourceContents, EmbeddedResource, ImageContent, TextResourceContents,
    };

    fn text(s: &str) -> ContentBlock {
        ContentBlock::Text(TextContent::new(s))
    }

    // ---- ACP-276 … ACP-281 ---------------------------------------------------------------------

    /// ACP-276 / ACP-277 — `test/unit/prompt-to-pi-message.test.ts`'s first case, byte-for-byte.
    /// Text concatenates with no separator and no trimming; a resource link contributes
    /// `\n[Context] <uri>` from the raw uri alone.
    #[test]
    fn text_and_resource_links_concatenate_exactly_as_upstream() {
        let (message, images) = prompt_to_user_input(&[
            text("Hello"),
            ContentBlock::ResourceLink(ResourceLink::new("foo", "file:///tmp/foo.txt")),
            text(" world"),
        ]);
        assert_eq!(message, "Hello\n[Context] file:///tmp/foo.txt world");
        assert!(images.is_empty());

        // A link as the FIRST block makes the message start with `\n`. That is upstream's
        // behaviour and the golden above depends on it, so it is pinned separately.
        let (message, _) = prompt_to_user_input(&[ContentBlock::ResourceLink(ResourceLink::new(
            "foo",
            "file:///a",
        ))]);
        assert_eq!(message, "\n[Context] file:///a");

        // No trimming, no separator: two adjacent text blocks are one string.
        let (message, _) = prompt_to_user_input(&[text("  a  "), text("b  ")]);
        assert_eq!(message, "  a  b  ");
    }

    /// ACP-279 — both upstream goldens byte-for-byte, plus the mime defaults.
    #[test]
    fn an_embedded_resource_renders_in_three_shapes() {
        let (message, images) = prompt_to_user_input(&[ContentBlock::Resource(
            EmbeddedResource::new(EmbeddedResourceResource::TextResourceContents(
                TextResourceContents::new("hi", "file:///tmp/a.txt")
                    .mime_type("text/plain".to_string()),
            )),
        )]);
        assert_eq!(
            message,
            "\n[Embedded Context] file:///tmp/a.txt (text/plain)\nhi"
        );
        assert!(images.is_empty());

        // `Buffer.from('xyz').toString('base64')` is `eHl6`, and the golden is `3 bytes`.
        let (message, _) = prompt_to_user_input(&[ContentBlock::Resource(EmbeddedResource::new(
            EmbeddedResourceResource::BlobResourceContents(
                BlobResourceContents::new("eHl6", "file:///tmp/a.bin")
                    .mime_type("application/octet-stream".to_string()),
            ),
        ))]);
        assert_eq!(
            message,
            "\n[Embedded Context] file:///tmp/a.bin (application/octet-stream, 3 bytes)"
        );

        // The two mime defaults, applied explicitly where upstream's `typeof` guard defaulted.
        let (message, _) = prompt_to_user_input(&[ContentBlock::Resource(EmbeddedResource::new(
            EmbeddedResourceResource::TextResourceContents(TextResourceContents::new("hi", "u")),
        ))]);
        assert_eq!(message, "\n[Embedded Context] u (text/plain)\nhi");
        let (message, _) = prompt_to_user_input(&[ContentBlock::Resource(EmbeddedResource::new(
            EmbeddedResourceResource::BlobResourceContents(BlobResourceContents::new("", "u")),
        ))]);
        assert_eq!(
            message,
            "\n[Embedded Context] u (application/octet-stream, 0 bytes)"
        );
    }

    /// ACP-279 — the byte count is the DECODED length and is exact on padded input, where
    /// `base64::decoded_len_estimate` disagrees.
    #[test]
    fn the_blob_byte_count_is_the_decoded_length_including_padding() {
        // "abc" -> "YWJj" (no padding), "ab" -> "YWI=", "a" -> "YQ==".
        assert_eq!(base64_decoded_len("YWJj"), 3);
        assert_eq!(base64_decoded_len("YWI="), 2);
        assert_eq!(base64_decoded_len("YQ=="), 1);
        assert_eq!(base64_decoded_len(""), 0);
        // And it reaches the marker.
        let (message, _) = prompt_to_user_input(&[ContentBlock::Resource(EmbeddedResource::new(
            EmbeddedResourceResource::BlobResourceContents(BlobResourceContents::new("YQ==", "u")),
        ))]);
        assert!(message.ends_with("1 bytes)"), "{message}");
    }

    /// ACP-278 — an image contributes nothing to the text and one `Content::Image` whose `data` is
    /// byte-identical to the input, with no data-url prefix and the `uri` dropped.
    #[test]
    fn an_image_becomes_a_content_block_and_no_text() {
        let base64 = "YWJj";
        let (message, images) = prompt_to_user_input(&[
            text("see"),
            ContentBlock::Image(ImageContent::new(base64, "image/png").uri("img-1".to_string())),
        ]);
        assert_eq!(message, "see");
        assert_eq!(images.len(), 1);
        match &images[0] {
            Content::Image { data, mime_type } => {
                assert_eq!(data, base64, "verbatim: no `data:<mime>;base64,` prefix");
                assert_eq!(mime_type, "image/png");
            }
            other => panic!("expected an image, got {other:?}"),
        }
    }

    /// ACP-280 — the marker, with the one string that changes, and the invariant that the
    /// advertised `promptCapabilities.audio` agrees with this arm.
    #[test]
    fn the_audio_arm_and_the_advertised_capability_agree() {
        let (message, images) =
            prompt_to_user_input(&[ContentBlock::Audio(AudioContent::new("YWJj", "audio/wav"))]);
        assert_eq!(
            message,
            "\n[Audio] (audio/wav, 3 bytes) not supported by cyrup-acp"
        );
        assert!(images.is_empty());
        assert!(
            !message.contains("pi-acp"),
            "the one string in this file that must change"
        );

        // The other half of the invariant, declared in another file: this arm emits a marker
        // rather than content, so the capability must be false.
        let caps = serde_json::to_value(crate::config_options::agent_capabilities(false)).unwrap();
        assert_eq!(
            caps["promptCapabilities"]["audio"],
            serde_json::json!(false)
        );
        // And an image IS produced, so that capability is true.
        assert_eq!(caps["promptCapabilities"]["image"], serde_json::json!(true));
    }

    /// ACP-281 — an unknown block type cannot reach the translator: the SDK rejects the whole
    /// request first. This asserts the mechanism the delta describes, so nobody "fixes" the
    /// unreachable arm by adding a shim that changes it.
    #[test]
    fn an_unknown_block_type_is_rejected_by_deserialization_not_dropped() {
        let err = serde_json::from_value::<ContentBlock>(serde_json::json!({
            "type": "video",
            "data": "x"
        }))
        .unwrap_err();
        assert!(
            err.to_string().contains("video") || err.to_string().contains("variant"),
            "unexpected error: {err}"
        );
        // The whole prompt list goes with it — there is no `VecSkipError` on `PromptRequest.prompt`.
        assert!(
            serde_json::from_value::<Vec<ContentBlock>>(serde_json::json!([
                {"type": "text", "text": "keep me"},
                {"type": "video", "data": "x"}
            ]))
            .is_err()
        );
    }

    // ---- ACP-070 / ACP-071 / ACP-272 -----------------------------------------------------------

    /// ACP-272 — the advertised set and the dispatchable set are the same set, **both
    /// directions**. This is the assertion that fails when someone adds a name to one list and
    /// forgets the other, and it is green by construction because they are one enum.
    #[test]
    fn every_builtin_name_round_trips() {
        for builtin in BUILTINS {
            assert_eq!(
                Builtin::parse(builtin.name()),
                Some(builtin),
                "{} does not round-trip",
                builtin.name()
            );
        }
        assert!(Builtin::parse("nope").is_none());
        assert!(
            Builtin::parse("compactfoo").is_none(),
            "ACP-282: a prefix match is not a command"
        );
        // `/changelog` is deliberately absent — see BUILTINS' delta. Pinned so re-adding it is a
        // deliberate act with a test to change.
        assert!(
            Builtin::parse("changelog").is_none(),
            "ACP-070: /changelog is dropped, not forgotten"
        );
        // Every advertised row is dispatchable, which is the half of ACP-070's verify that fails
        // for a hand-written pair of lists.
        for command in builtin_commands() {
            assert!(
                Builtin::parse(&command.name).is_some(),
                "advertised `{}` has no dispatch arm",
                command.name
            );
        }
    }

    /// ACP-070 — the advertised list is upstream's fixture: names, descriptions, hints and order.
    #[test]
    fn the_advertised_list_is_upstreams_fixture() {
        let json = serde_json::to_value(builtin_commands()).unwrap();
        let expected = serde_json::json!([
            {
                "name": "compact",
                "description": "Manually compact the session context",
                "input": {"hint": "optional custom instructions"}
            },
            {
                "name": "autocompact",
                "description": "Toggle automatic context compaction",
                "input": {"hint": "on|off|toggle"}
            },
            {
                "name": "export",
                "description": "Export session to an HTML file in the session cwd"
            },
            {
                "name": "session",
                "description": "Show session stats (messages, tokens, cost, session file)"
            },
            {
                "name": "name",
                "description": "Set session display name",
                "input": {"hint": "<name>"}
            },
            {
                "name": "steering",
                "description": "Get/set cyrup steering message delivery mode (how queued steering messages are delivered)",
                "input": {"hint": "(no args to show) all | one-at-a-time"}
            },
            {
                "name": "follow-up",
                "description": "Get/set cyrup follow-up message delivery mode (how queued follow-up messages are delivered)",
                "input": {"hint": "(no args to show) all | one-at-a-time"}
            }
        ]);
        assert_eq!(json, expected);
        // The two reworded strings, asserted as a rule rather than as two literals.
        for command in builtin_commands() {
            assert!(
                !command.description.contains("pi "),
                "`{}` names another product: {}",
                command.name,
                command.description
            );
        }
    }

    /// ACP-071 / ACP-272 — first wins, order preserved, and the BUILTINS lead so the advertised
    /// list matches what `intercept` dispatches.
    #[test]
    fn a_colliding_user_command_is_dropped_not_duplicated() {
        let user = vec![
            AvailableCommand::new("compact", "my own compact"),
            AvailableCommand::new("deploy", "ship it"),
        ];
        let merged = merge_commands(user);
        assert_eq!(
            merged.len(),
            BUILTINS.len() + 1,
            "ACP-071: `1 + builtins - 1` for one collision"
        );
        // The builtin won, which is the corrected order.
        let compact = merged.iter().find(|c| c.name == "compact").unwrap();
        assert_eq!(compact.description, "Manually compact the session context");
        // Exactly one entry per name.
        assert_eq!(merged.iter().filter(|c| c.name == "compact").count(), 1);
        // Order: builtins in their order, then the surviving user commands in theirs.
        assert_eq!(merged[0].name, "compact");
        assert_eq!(merged[BUILTINS.len()].name, "deploy");
    }

    // ---- ACP-282 -------------------------------------------------------------------------------

    /// ACP-282 — the four behaviours a naive `starts_with` gets wrong.
    #[test]
    fn the_interception_gate_matches_upstream_exactly() {
        // Intercepted, with the argument split on the first literal space.
        assert_eq!(
            intercept("/compact tighten it", false),
            Some((Builtin::Compact, "tighten it".to_string()))
        );
        // Trailing whitespace is trimmed first.
        assert_eq!(
            intercept("/session   ", false),
            Some((Builtin::Session, String::new()))
        );
        assert_eq!(
            intercept("  /session", false),
            Some((Builtin::Session, String::new()))
        );
        // A prefix is not a command.
        assert_eq!(intercept("/compactfoo", false), None);
        // An attached image suppresses interception entirely — upstream's `images.length === 0`.
        assert_eq!(intercept("/compact", true), None);
        // Not a command at all.
        assert_eq!(intercept("compact", false), None);
        assert_eq!(intercept("", false), None);
        assert_eq!(intercept("/", false), None);
        // A prompt template named `session` is SHADOWED (ACP-Q41), and one named `deploy` is not.
        assert!(intercept("/session", false).is_some());
        assert_eq!(intercept("/deploy prod", false), None);
    }

    /// ACP-282's delta — the name split is on a literal SPACE, so a tab lands inside the name and
    /// the text is not a command. `cyrup_resources::prompt::split_command` would disagree.
    #[test]
    fn a_tab_after_the_command_name_is_not_a_command() {
        assert_eq!(intercept("/compact\tfoo", false), None);
        // But a tab in the ARGUMENTS is fine — the split already happened.
        assert_eq!(
            intercept("/compact a\tb", false),
            Some((Builtin::Compact, "a\tb".to_string()))
        );
    }

    /// ACP-266 — the invariant this module exists to hold: nothing here looks a template up, so a
    /// `/tpl` submission reaches `AgentSession::prepare_and_assemble` verbatim and is expanded
    /// exactly once. A host-side expansion would substitute the template body's own `$1`/`$@` a
    /// second time against the same argv.
    #[test]
    fn nothing_here_expands_a_template() {
        let submitted = "/tpl $1 and $@";
        assert_eq!(
            intercept(submitted, false),
            None,
            "a non-builtin falls through to the core, unchanged"
        );
        // The text a caller submits is exactly the text it received: the translator does not
        // rewrite a leading slash, and there is no `substitute_args` call anywhere in this module.
        let (message, _) = prompt_to_user_input(&[text(submitted)]);
        assert_eq!(message, submitted);
        assert!(message.starts_with("/tpl"));
        assert!(message.contains("$1") && message.contains("$@"));
    }

    // ---- ACP-263 / ACP-267 … ACP-271 / ACP-290 -------------------------------------------------

    fn row(json: serde_json::Value) -> serde_json::Value {
        json
    }

    /// ACP-267 — the three projection rules, on one fixture: a duplicate name collapses first-wins,
    /// a nameless row is dropped, and a description-less row carries the fallback rather than `""`.
    #[test]
    fn the_projection_drops_nameless_rows_dedupes_and_never_emits_an_empty_description() {
        let rows = vec![
            row(
                serde_json::json!({"name": "deploy", "description": "ship", "source": "extension"}),
            ),
            row(
                serde_json::json!({"name": "deploy", "description": "a second deploy", "source": "extension"}),
            ),
            row(serde_json::json!({"name": "", "description": "nameless", "source": "extension"})),
            row(serde_json::json!({"source": "extension"})),
            row(serde_json::json!({
                "name": "review",
                "source": "prompt",
                "sourceInfo": {"scope": "user", "source": "loose"}
            })),
        ];
        let out = project_catalog(&rows, true);
        let names: Vec<&str> = out.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["deploy", "review"]);
        assert_eq!(out[0].description, "ship", "first wins");
        // ACP-263 / ACP-080 — non-empty, and exactly one provenance marker.
        assert_eq!(out[1].description, "(prompt:user)");
        assert!(!out[1].description.is_empty());
        assert_eq!(out[1].description.matches('(').count(), 1);
    }

    /// ACP-263 — the fallback's shapes, including upstream's `(command)` for a row with no
    /// provenance at all.
    #[test]
    fn the_provenance_fallback_has_upstreams_shape() {
        assert_eq!(
            describe_fallback(
                &serde_json::json!({"source": "prompt", "sourceInfo": {"scope": "project"}})
            ),
            "(prompt:project)"
        );
        assert_eq!(
            describe_fallback(&serde_json::json!({"source": "skill"})),
            "(skill)"
        );
        assert_eq!(
            describe_fallback(&serde_json::json!({"sourceInfo": {"scope": "user"}})),
            "(user)"
        );
        assert_eq!(describe_fallback(&serde_json::json!({})), "(command)");
    }

    /// ACP-268 — the skill gate is advertisement-only, and it gates by the `skill:` name prefix
    /// exactly as upstream does.
    #[test]
    fn the_skill_gate_removes_only_skill_rows_from_the_advertisement() {
        let rows = vec![
            row(serde_json::json!({"name": "review", "description": "r", "source": "prompt"})),
            row(serde_json::json!({"name": "skill:pdf", "description": "s", "source": "skill"})),
        ];
        let on: Vec<String> = project_catalog(&rows, true)
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(on, ["review", "skill:pdf"]);
        let off: Vec<String> = project_catalog(&rows, false)
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(off, ["review"], "only the prompt row is advertised");
        // The gate is advertisement-only: nothing in this module can stop `/skill:pdf` expanding,
        // because expansion is `AgentSession::prepare_and_assemble`'s. `intercept` proves the
        // submission is untouched.
        assert_eq!(intercept("/skill:pdf", false), None);
    }

    /// ACP-269 — extension rows are INCLUDED, reversing upstream's silent exclusion. This is the
    /// assertion that fails if someone ports `includeExtensionCommands`.
    #[test]
    fn extension_commands_are_advertised() {
        let rows = vec![row(serde_json::json!({
            "name": "deploy",
            "description": "ship it",
            "source": "extension",
            "sourceInfo": {"path": "deployer", "source": "local"}
        }))];
        let out = project_catalog(&rows, true);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "deploy");
        // And it survives the merge, since no builtin is named `deploy`.
        assert!(merge_commands(out).iter().any(|c| c.name == "deploy"));
    }

    /// ACP-271 — `argumentHint` becomes `Unstructured(hint)`; a row without it projects to
    /// `input: None`.
    #[test]
    fn an_argument_hint_becomes_unstructured_input() {
        let rows = vec![
            row(serde_json::json!({
                "name": "fix", "description": "d", "source": "prompt", "argumentHint": "<file>"
            })),
            row(serde_json::json!({"name": "grep", "description": "d", "source": "prompt"})),
            row(serde_json::json!({
                "name": "empty", "description": "d", "source": "prompt", "argumentHint": ""
            })),
        ];
        let out = project_catalog(&rows, true);
        let by_name = |n: &str| out.iter().find(|c| c.name == n).unwrap().clone();
        match by_name("fix").input {
            Some(AvailableCommandInput::Unstructured(input)) => assert_eq!(input.hint, "<file>"),
            other => panic!("expected an unstructured hint, got {other:?}"),
        }
        assert!(by_name("grep").input.is_none());
        assert!(by_name("empty").input.is_none(), "an empty hint is no hint");
    }

    /// ACP-290 — the projection is order-stable against a catalog that is not. Shuffling the
    /// `HashMap`-sourced groups must not change the output, while the extension group keeps its
    /// load order.
    #[test]
    fn the_projection_is_deterministic_whatever_order_the_catalog_arrives_in() {
        let ext_b =
            row(serde_json::json!({"name": "b-ext", "description": "d", "source": "extension"}));
        let ext_a =
            row(serde_json::json!({"name": "a-ext", "description": "d", "source": "extension"}));
        let p_z =
            row(serde_json::json!({"name": "z-prompt", "description": "d", "source": "prompt"}));
        let p_a =
            row(serde_json::json!({"name": "a-prompt", "description": "d", "source": "prompt"}));
        let s_z =
            row(serde_json::json!({"name": "skill:z", "description": "d", "source": "skill"}));
        let s_a =
            row(serde_json::json!({"name": "skill:a", "description": "d", "source": "skill"}));

        // Extension load order is B then A and MUST be preserved; the other two groups sort.
        let first = project_catalog(
            &[
                ext_b.clone(),
                ext_a.clone(),
                p_z.clone(),
                p_a.clone(),
                s_z.clone(),
                s_a.clone(),
            ],
            true,
        );
        let names: Vec<&str> = first.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "b-ext", "a-ext", "a-prompt", "z-prompt", "skill:a", "skill:z"
            ]
        );

        // The same catalog with the two unordered groups shuffled projects identically.
        let shuffled = project_catalog(&[ext_b, ext_a, p_a, p_z, s_a, s_z], true);
        assert_eq!(first, shuffled);
    }

    // ---- the arms that need no session ----------------------------------------------------------

    /// The plain text of an `agent_message_chunk` — the shape every built-in answers with. Panics
    /// naming what actually arrived, so a wrong update type reads as a wrong update type rather
    /// than as a string mismatch.
    #[track_caller]
    fn chunk_text(update: &SessionUpdate) -> &str {
        let SessionUpdate::AgentMessageChunk(chunk) = update else {
            panic!("expected an agent_message_chunk, got {update:?}");
        };
        let ContentBlock::Text(text) = &chunk.content else {
            panic!("expected plain text, got {:?}", chunk.content);
        };
        &text.text
    }

    /// The single chunk of a one-update answer.
    #[track_caller]
    fn only_chunk(updates: &[SessionUpdate]) -> &str {
        assert_eq!(
            updates.len(),
            1,
            "one chunk, not {}: {updates:?}",
            updates.len()
        );
        chunk_text(&updates[0])
    }

    /// **ACP-283** — `/compact`'s output string, whose three format decisions are each one
    /// character from a divergence and none of which was asserted anywhere.
    ///
    /// `ACP-283`'s verify, verbatim: "`/compact tighten it` emits exactly `"Compaction completed.
    /// (custom instructions applied)\nTokens before: <n>\n\n<summary>"` as one chunk; no args omits
    /// the parenthetical".
    #[test]
    fn the_compaction_summary_has_upstreams_exact_shape() {
        // Custom instructions + a summary: the parenthetical's LEADING SPACE, one `\n` before the
        // token line, two before the summary.
        assert_eq!(
            compaction_text(Some("tighten it"), 1234, "It was tightened."),
            "Compaction completed. (custom instructions applied)\nTokens before: 1234\n\nIt was tightened."
        );
        // No argument omits the parenthetical and nothing else.
        assert_eq!(
            compaction_text(None, 1234, "It was tightened."),
            "Compaction completed.\nTokens before: 1234\n\nIt was tightened."
        );
        // JS truthiness on `''`, preserved: an empty summary omits the block AND its blank line.
        assert_eq!(
            compaction_text(None, 0, ""),
            "Compaction completed.\nTokens before: 0"
        );
        assert!(
            !compaction_text(None, 0, "").ends_with('\n'),
            "an empty summary must not leave a trailing separator"
        );
        // The `Tokens before:` line is unconditional here — see the arm's delta (upstream omitted
        // it when pi's untyped reply lacked the key; `CompactionResult::tokens_before` is a `u64`).
        for summary in ["", "s"] {
            assert!(compaction_text(Some("c"), 7, summary).contains("\nTokens before: 7"));
        }
    }

    /// **ACP-284** — `/session`'s five-line shape, including the `Tokens:` line's five sub-parts in
    /// order and the one genuinely conditional line.
    ///
    /// The integration case (`crates/cyrup-it/tests/bin/acp_session.rs`) asserts the `Session:`
    /// prefix, `\nMessages: ` and `\nCost: $` and stops, so a reordering of the token sub-parts, or
    /// a `session_file` that became always-`Some`, passed in both directions.
    #[test]
    fn the_session_stats_block_is_five_lines_in_upstreams_order() {
        let stats = cyrup_session_svc::SessionStats {
            session_file: Some("/tmp/p/.cyrup/sessions/s1.jsonl".to_string()),
            session_id: "s1".to_string(),
            total_messages: 12,
            cost: 0.012_345,
            tokens: cyrup_session_svc::StatsTokens {
                input: 1,
                output: 2,
                cache_read: 3,
                cache_write: 4,
                total: 10,
            },
            ..Default::default()
        };
        assert_eq!(
            stats_text(&stats),
            "Session: s1\n\
             Session file: /tmp/p/.cyrup/sessions/s1.jsonl\n\
             Messages: 12\n\
             Cost: $0.012\n\
             Tokens: in 1, out 2, cache read 3, cache write 4, total 10"
        );

        // The ONE conditional line: an in-memory session omits `Session file:` and nothing else,
        // so the block is four lines with the other four unchanged.
        let in_memory = cyrup_session_svc::SessionStats {
            session_file: None,
            ..stats.clone()
        };
        let text = stats_text(&in_memory);
        assert!(!text.contains("Session file:"), "{text}");
        assert_eq!(text.lines().count(), 4, "{text}");
        assert_eq!(
            text,
            "Session: s1\nMessages: 12\nCost: $0.012\n\
             Tokens: in 1, out 2, cache read 3, cache write 4, total 10"
        );
        // The `$` and the three decimals are cyrup's, deliberately (see the arm's delta): pi-acp
        // printed JS default number formatting. Sub tenth-of-a-cent renders as `$0.000`.
        assert!(stats_text(&stats).contains("\nCost: $0.012"));
        assert!(
            stats_text(&cyrup_session_svc::SessionStats {
                cost: 0.000_04,
                ..stats
            })
            .contains("\nCost: $0.000")
        );
    }

    /// **ACP-285** — `/name` emits the `session_info_update` **here**, before its own chunk, and
    /// exactly one of each.
    ///
    /// This is the half of `ACP-285`'s verify that the pump cannot carry. The two updates and the
    /// [`RenameEcho`] claim are one unit: deleting this emission while leaving `claim.keep()` in
    /// place makes the pump swallow the fanout's copy and the client sees **zero** renames. The
    /// count and the order are asserted on the wire shape, which is what a client parses.
    #[test]
    fn a_rename_emits_one_titled_update_then_the_confirmation() {
        let updates = name_updates("my project", "2026-09-05T12:00:00.000Z".to_string());
        assert_eq!(updates.len(), 2, "exactly one of each: {updates:?}");

        // Upstream's order: the update first, the confirmation second.
        let wire = serde_json::to_value(&updates[0]).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({
                "sessionUpdate": "session_info_update",
                "title": "my project",
                "updatedAt": "2026-09-05T12:00:00.000Z"
            }),
            "the causer's own session_info_update"
        );
        assert_eq!(chunk_text(&updates[1]), "Session name set: my project");

        // Exactly one titled update, from this function — not zero (the hazard the delta names).
        assert_eq!(
            updates
                .iter()
                .filter(|u| matches!(u, SessionUpdate::SessionInfoUpdate(_)))
                .count(),
            1
        );
    }

    /// **NEW (area 4e)** — a failing `/name` answers with a chunk and ends the turn, as upstream's
    /// `catch` does, instead of rejecting the whole `session/prompt`.
    ///
    /// The rejection sent a JSON-RPC error frame for a disk error on a rename, so a Zed user saw a
    /// failed request rather than a message. `dispatch`'s `# Errors` paragraph now names `compact`
    /// and `export` as the only rejecting arms and this is what makes that exhaustive: `name`
    /// returns `Vec<SessionUpdate>`, so an `Err` from it is not representable.
    #[test]
    fn a_failing_rename_is_answered_not_rejected() {
        let updates = name_failure_updates("permission denied (os error 13)");
        assert_eq!(
            only_chunk(&updates),
            "Failed to set session name: permission denied (os error 13)"
        );
        // Upstream's version-skew hint is cut (see the arm's second delta), so the message is the
        // error's own text and nothing else.
        assert!(!only_chunk(&updates).contains("newer pi version"));
        // The usage line is upstream's too, and is the empty-argument answer rather than a getter.
        assert_eq!(NAME_USAGE, "Usage: /name <name>");
    }

    /// **ACP-286 / ACP-287** — the three branches, their three exact strings, and the write
    /// decision, for both commands.
    ///
    /// Upstream's own component test (`test/component/agent-steering-followup-modes.test.ts`)
    /// asserts `setTo === 'one-at-a-time'` on the set branch and `called === false` on the invalid
    /// one; here those are the returned `Option<QueueMode>`, so a read or a usage line that started
    /// writing fails rather than silently changing a mode.
    #[test]
    fn the_queue_mode_arm_has_three_branches_and_three_strings() {
        // No argument reports the current mode — `ACP-286`'s `Steering mode: all` verbatim.
        let (updates, write) = queue_mode_updates(QueueKind::Steering, None, QueueMode::All);
        assert_eq!(only_chunk(&updates), "Steering mode: all");
        assert_eq!(write, None, "a read must not write");
        let (updates, write) = queue_mode_updates(QueueKind::FollowUp, None, QueueMode::OneAtATime);
        assert_eq!(only_chunk(&updates), "Follow-up mode: one-at-a-time");
        assert_eq!(write, None);

        // Setting: the string and the mode that is actually written.
        let (updates, write) =
            queue_mode_updates(QueueKind::Steering, Some("one-at-a-time"), QueueMode::All);
        assert_eq!(only_chunk(&updates), "Steering mode set to: one-at-a-time");
        assert_eq!(write, Some(QueueMode::OneAtATime));
        let (updates, write) =
            queue_mode_updates(QueueKind::FollowUp, Some("all"), QueueMode::OneAtATime);
        assert_eq!(only_chunk(&updates), "Follow-up mode set to: all");
        assert_eq!(write, Some(QueueMode::All));

        // Upstream's `toLowerCase()` on the argument, and only on the argument.
        let (updates, write) =
            queue_mode_updates(QueueKind::Steering, Some("ALL"), QueueMode::OneAtATime);
        assert_eq!(only_chunk(&updates), "Steering mode set to: all");
        assert_eq!(write, Some(QueueMode::All));

        // Anything else is the usage line, and writes NOTHING (upstream's `called === false`).
        for bad in ["nope", "???", "one at a time", ""] {
            let (updates, write) =
                queue_mode_updates(QueueKind::Steering, Some(bad), QueueMode::All);
            assert_eq!(write, None, "`{bad}` must not write a mode");
            if bad.is_empty() {
                // An empty argument is upstream's `String(args[0] ?? '')` read branch, not usage.
                assert_eq!(only_chunk(&updates), "Steering mode: all");
            } else {
                assert_eq!(
                    only_chunk(&updates),
                    "Usage: /steering all | /steering one-at-a-time"
                );
            }
        }
        let (updates, write) =
            queue_mode_updates(QueueKind::FollowUp, Some("nope"), QueueMode::All);
        assert_eq!(
            only_chunk(&updates),
            "Usage: /follow-up all | /follow-up one-at-a-time"
        );
        assert_eq!(write, None);
    }

    /// **ACP-289** — `/autocompact`'s seven inputs, including the upstream quirk that a typo
    /// toggles, and the two answer strings.
    ///
    /// The arm had no test at all. The quirk is the one a later reader will "fix" into a usage
    /// line: upstream's `enabled === null` branch is reached by every unrecognised word.
    #[test]
    fn autocompact_accepts_eight_words_and_toggles_on_everything_else() {
        // The accept table, from both starting states so a table entry that fell through to the
        // inversion cannot pass by coincidence.
        for word in ["on", "true", "enable", "enabled"] {
            assert!(autocompact_target(Some(word), false), "`{word}`");
            assert!(autocompact_target(Some(word), true), "`{word}`");
        }
        for word in ["off", "false", "disable", "disabled"] {
            assert!(!autocompact_target(Some(word), false), "`{word}`");
            assert!(!autocompact_target(Some(word), true), "`{word}`");
        }
        // Upstream lowercases the argument.
        assert!(autocompact_target(Some("ON"), false));
        assert!(!autocompact_target(Some("Disabled"), true));

        // No argument is upstream's `args[0] ?? 'toggle'` — it inverts.
        assert!(autocompact_target(None, false));
        assert!(!autocompact_target(None, true));
        assert!(autocompact_target(Some("toggle"), false));

        // THE QUIRK: every unrecognised word inverts rather than printing usage. `/autocompact
        // onn` flips the flag. Upstream's behaviour, preserved deliberately.
        for typo in ["onn", "offf", "yes", "??"] {
            assert!(autocompact_target(Some(typo), false), "`{typo}` toggles on");
            assert!(
                !autocompact_target(Some(typo), true),
                "`{typo}` toggles off"
            );
        }

        // The two answers, byte-for-byte, trailing period included.
        assert_eq!(autocompact_text(true), "Auto-compaction enabled.");
        assert_eq!(autocompact_text(false), "Auto-compaction disabled.");
    }

    /// **ACP-288** — `/export` emits exactly two chunks: the trailing-space prefix, then a
    /// `resource_link` with the `text/html` mime type and the `Session exported` title.
    ///
    /// Asserted on the wire shape because that is what the client concatenates: upstream's comment
    /// on this arm explains that the trailing space (rather than a newline, and rather than
    /// nothing) is what avoids the "link + duplicate plain text" look.
    #[test]
    fn the_export_emits_the_prefix_chunk_then_the_resource_link() {
        let updates = export_updates(
            "cyrup-session-2026-09-05_abc123.html".to_string(),
            "file:///tmp/project/cyrup-session-2026-09-05_abc123.html".to_string(),
        );
        assert_eq!(updates.len(), 2, "exactly two chunks: {updates:?}");
        assert_eq!(
            chunk_text(&updates[0]),
            "Session exported: ",
            "the trailing space is load-bearing and there is NO newline"
        );
        assert_eq!(
            serde_json::to_value(&updates[1]).unwrap(),
            serde_json::json!({
                "sessionUpdate": "agent_message_chunk",
                "content": {
                    "type": "resource_link",
                    "name": "cyrup-session-2026-09-05_abc123.html",
                    "uri": "file:///tmp/project/cyrup-session-2026-09-05_abc123.html",
                    "mimeType": "text/html",
                    "title": "Session exported"
                }
            })
        );
    }

    /// **NEW (area 4e)** — `/export` writes into the **session's own** cwd, never the live slot's.
    ///
    /// The slot read in `SessionManager::serve_builtin` is the one on this path that does not
    /// filter on the session id, and `session/new` replaces it from a different task while
    /// `serve_prompt` runs spawned. A client that pipelines a `session/new` for project B behind a
    /// `/export` for session A would otherwise get A's HTML written into B's directory —
    /// containment holds, against the wrong directory.
    #[test]
    fn the_export_directory_is_the_sessions_own_cwd_not_the_slot() {
        let session_cwd = std::path::Path::new("/tmp/project-a");
        // The agreeing case: identical, and the session's is returned.
        let agreeing = AbsCwd::parse("/tmp/project-a").unwrap();
        assert_eq!(export_dir(&agreeing, session_cwd), session_cwd);
        // The race: the slot has moved to another project. The SESSION wins.
        let moved = AbsCwd::parse("/tmp/project-b").unwrap();
        assert_eq!(
            export_dir(&moved, session_cwd),
            session_cwd,
            "the slot's cwd must never decide where a session's export lands"
        );
        // And the composed path follows it, so B's directory is never written to.
        let id = AcpSessionId::parse("2026-09-05_abc123").unwrap();
        let path = id.export_path_in(export_dir(&moved, session_cwd)).unwrap();
        assert_eq!(
            path.as_path(),
            std::path::Path::new("/tmp/project-a/cyrup-session-2026-09-05_abc123.html")
        );
        assert!(!path.as_path().starts_with("/tmp/project-b"));
    }

    /// ACP-286 / ACP-287 — the wire spelling of a queue mode, and the two labels that make the two
    /// commands distinguishable. The three composed strings and the write *decision* are
    /// `the_queue_mode_arm_has_three_branches_and_three_strings`'; what is left over is
    /// [`QueueKind::write`]'s mapping onto `set_steering_mode`/`set_follow_up_mode`, which needs a
    /// live session and is a `cyrup-it` assertion. This pins the pieces both of those read.
    #[test]
    fn the_queue_mode_spelling_round_trips() {
        for mode in [QueueMode::All, QueueMode::OneAtATime] {
            let id = queue_mode_id(mode);
            assert_eq!(
                id.parse::<QueueMode>().ok(),
                Some(mode),
                "`{id}` must be what QueueMode::from_str accepts"
            );
        }
        assert_eq!(queue_mode_id(QueueMode::All), "all");
        assert_eq!(QueueKind::Steering.label(), "Steering");
        assert_eq!(QueueKind::FollowUp.label(), "Follow-up");
        assert_eq!(QueueKind::FollowUp.command(), "follow-up");
        assert_ne!(QueueKind::Steering.command(), QueueKind::FollowUp.command());
    }

    /// ACP-288 / ACP-291 — the export path is composed by the agent from a validated id, and the
    /// `file://` URI and link name are built from it. The write itself needs a live session and is
    /// a `cyrup-it` assertion; the composition is what carries the security property.
    #[test]
    fn the_export_path_is_agent_composed_and_contained() {
        let cwd = AbsCwd::parse("/tmp/project").unwrap();
        let id = AcpSessionId::parse("2026-09-05_abc123").unwrap();
        let path = id.export_path_in(cwd.as_path()).unwrap();
        assert_eq!(
            path.as_path(),
            std::path::Path::new("/tmp/project/cyrup-session-2026-09-05_abc123.html")
        );
        assert_eq!(
            file_uri(path.as_path()),
            "file:///tmp/project/cyrup-session-2026-09-05_abc123.html"
        );
        // A hostile id never reaches a path at all: the boundary rejects it.
        for hostile in ["../../etc/passwd", "/etc/passwd", "a/b", ".."] {
            assert!(
                AcpSessionId::parse(hostile).is_err(),
                "`{hostile}` must not become a session id"
            );
        }
    }

    /// **ACP-292** — `/compact` refuses while a turn is running, and the refusal is a chunk.
    ///
    /// The arm this pins is the one whose absence silently kills a running turn:
    /// `AgentSession::compact` opens with `abort_and_settle()`, so without the guard a pipelined
    /// `/compact` resolves the user's streaming `session/prompt` as `cancelled`. The string is
    /// cyrup-invented (see [`COMPACT_BUSY_MESSAGE`]), so it is pinned here rather than left to a
    /// reviewer to notice a reword.
    #[test]
    fn compact_refuses_a_running_turn_and_says_so_in_one_chunk() {
        assert_eq!(compact_refusal(false), None, "an idle session compacts");

        let refusal = compact_refusal(true).expect("a running turn is refused");
        assert_eq!(
            refusal.len(),
            1,
            "one chunk, not an error frame: {refusal:?}"
        );
        let SessionUpdate::AgentMessageChunk(chunk) = &refusal[0] else {
            panic!("the refusal is an agent_message_chunk: {refusal:?}");
        };
        let ContentBlock::Text(text) = &chunk.content else {
            panic!("plain text: {chunk:?}");
        };
        assert_eq!(
            text.text,
            "Cannot compact while a turn is running. Cancel it first."
        );
        assert_eq!(text.text, COMPACT_BUSY_MESSAGE);
    }

    /// Every built-in answers with the stop reason upstream's arms all return.
    #[test]
    fn a_builtin_always_ends_the_turn() {
        assert_eq!(BUILTIN_STOP_REASON, StopReason::EndTurn);
        let json = serde_json::to_value(BUILTIN_STOP_REASON).unwrap();
        assert_eq!(json, serde_json::json!("end_turn"));
    }
}

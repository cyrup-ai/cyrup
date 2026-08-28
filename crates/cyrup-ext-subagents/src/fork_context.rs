//! Fork-context: the CANONICAL design (sole owner; arch-SA §6.6). `exec/`, `spawn/`, and
//! `background/` all call into this module's `ForkContextResolver`, never re-derive it.
//!
//! Fork-context is a plain, direct, synchronous Cargo dependency of this crate on
//! `cyrup-session` (never routed through the `cyrup-ext` capability system): branching happens
//! via `cyrup_session::SessionManager::create_branched_session` on a THROWAWAY handle opened
//! against the parent's persisted file (`SessionManager::open`) — the orchestrator's live,
//! in-memory session manager is never mutated in place by this call (R-SA-139/DI-SA-6).
//!
//! Three corrections against the architecture document's illustrative code, verified live
//! against the real `cyrup-session` source (`crates/cyrup-session/src/manager.rs`):
//!
//! 1. There is no `SessionManager::clone_at` method anywhere in `cyrup-session`. The real
//!    primitive is `create_branched_session(&mut self, leaf_id: &EntryId, layout: &SessionLayout)
//!    -> Result<Option<PathBuf>, SessionError>` (around `manager.rs:201`).
//! 2. There is no `SessionManager::persisted_path()` accessor. The real accessor is
//!    `session_file(&self) -> Option<&Path>`.
//! 3. `create_branched_session` returns `Ok(None)` (not an error) when the branch is created on
//!    an in-memory (never-persisted) session, AND when a persisted session's branched path has no
//!    assistant message yet (the write is deferred until the first assistant append, mirroring
//!    the parent's own deferred-flush semantics — see `manager.rs:294-303`). Both cases are
//!    "success but no path" from the primitive's point of view; this resolver treats a `None`
//!    result as `SubagentError::ForkFailed` since fork-context (R-SA-137/DI-SA-2) requires a
//!    concrete `session_file_path` to hand to the spawned child's `--session` argument — a
//!    fork-context resolution that produces no path is, by definition, not usable and MUST fail
//!    hard rather than silently downgrading to fresh context.
//!
//! Lineage provenance (R-SA-143) requires no additional code in this module:
//! `create_branched_session` itself records `parentSession` on the forked child's header
//! (`manager.rs:276-283`) whenever branching a persisted parent, which is the only case this
//! resolver ever reaches (the persisted-parent precondition is enforced below, before branching).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};

use cyrup_core::{AssistantMessage, Content, EntryId, Message};
use cyrup_session::{AgentMessage, Entry, EntryBase, KnownEntry, SessionLayout, SessionManager};
use tokio::sync::Mutex as AsyncMutex;

use crate::error::SubagentError;

/// Whether a subagent run continues the parent conversation's session state (`Fork`) or starts
/// from a blank slate (`Fresh`). Canonical home: this module (arch-SA §6.6) — `discovery/types.rs`
/// (`AgentDefinition::default_context`), `exec/mod.rs` (`RunOptions::context`), and
/// `tui/mod.rs` (`SubagentProgressSnapshot::context`) all reference `crate::fork_context::ContextMode`
/// rather than re-declaring it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextMode {
    /// Start the subagent with no inherited conversation state (the default).
    #[default]
    Fresh,
    /// Branch the parent session's current leaf into a new, independent session file and hand
    /// that file's path to the child (R-SA-137/138/139).
    Fork,
}

/// Resolve one call site's (possibly omitted) `context` request against the target agent's own
/// persona-level `defaultContext` (func-SA §4.1 `AgentDefinition::default_context`; DI-SA-3,
/// R-SA-138/R-SA-111).
///
/// This is the ONE place "an omitted call-site `context` falls back to the agent's own default"
/// is decided — every caller building a batch of sibling tasks (a `ParallelGroup`/`DynamicGroup`
/// fan-out, or the `subagent` tool's parallel-shape parameter surface once wired) MUST call this
/// once per task, independently, rather than resolving a single shared default for the whole
/// batch. Per DI-SA-3 ("Context-mode independence"): when `context` is omitted at a call site
/// covering multiple tasks in one batch, each task resolves its OWN persona-level
/// `defaultContext` independently — one sibling's resolved default MUST NOT leak into another
/// sibling's resolution. Calling this function once per task, using that task's own
/// `agent_default_context` argument, is what makes that independence hold: there is no shared,
/// batch-wide state here at all — each call is a pure function of its own two arguments only.
///
/// Precedence, highest to lowest:
/// 1. `call_site_context` — an explicit `context` on the task/step itself (e.g. `SingleStepSpec::
///    context`) always wins when present.
/// 2. `agent_default_context` — the resolved agent's own `AgentDefinition::default_context`
///    frontmatter field, consulted only when (1) is `None`.
/// 3. [`ContextMode::default`] (`Fresh`) — when neither is present.
#[must_use]
pub fn resolve_effective_context(
    call_site_context: Option<ContextMode>,
    agent_default_context: Option<ContextMode>,
) -> ContextMode {
    call_site_context
        .or(agent_default_context)
        .unwrap_or_default()
}

/// The resolved outcome of a fork-context request: either `Fresh` (no session file), or `Fork`
/// with a concrete, on-disk session-file path ready to hand to a spawned child via `--session`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForkContext {
    pub mode: ContextMode,
    pub session_file_path: Option<PathBuf>,
    /// SUBA-075 — pi's `ForkContextResolution.thinkingOverride` (`shared/fork-context.ts:273`
    /// @v0.57.0): `Some("off")` when the child must launch with reasoning DISABLED, overriding
    /// whatever `thinking:` its persona declares, `None` when nothing forces the issue.
    ///
    /// Doubly gated, exactly as upstream is (see [`ForkContextResolver::resolve`]'s
    /// three-outcome contract): the branch must have actually had an unsafe thinking block
    /// stripped from it AND the resolved child model must require thinking off. A fork that
    /// sanitized nothing never carries an override.
    pub thinking_override: Option<String>,
}

impl ForkContext {
    /// The `Fresh` outcome: no session file, nothing to branch.
    pub fn fresh() -> Self {
        Self {
            mode: ContextMode::Fresh,
            session_file_path: None,
            thinking_override: None,
        }
    }
}

// ------------------------------------------------------------------ SUBA-075: fork sanitization ---
//
// pi `shared/fork-context.ts:105-178` @v0.57.0. A forked child inherits the parent's transcript
// verbatim, and Anthropic REJECTS a request carrying thinking blocks whose signatures were minted
// for a different request context — so an unsanitized fork does not degrade, it hard-fails at the
// provider on the ordinary `context: fork` path. Stripping those blocks is therefore unconditional;
// the thinking-off override that accompanies it is not (see [`ForkContextResolver::resolve`]).

/// pi `forkedChildRequiresThinkingOff` (`shared/fork-context.ts:105-115` @v0.57.0) — does the
/// resolved child model speak Anthropic's provider or message api, and therefore need the
/// sanitized branch to run with reasoning disabled?
///
/// Conservative by construction: an absent/empty model, or one the registry cannot resolve
/// UNAMBIGUOUSLY, answers `true` (upstream's `if (!model) return true` / `if (!info) return true`).
/// The asymmetry is deliberate — forcing thinking off on a model that did not need it costs
/// reasoning depth for one run, while failing to force it on a model that did means the child's
/// very first request carries inherited signed thinking blocks and is rejected outright.
///
/// Callers compute the gate and hand the result to [`ForkContextResolver::resolve`]; this is
/// upstream's `forceThinkingOffForIndex` callback seam, kept as a caller-side decision so this
/// module never reaches into the model registry mid-branch.
#[must_use]
pub fn forked_child_requires_thinking_off(
    model: Option<&str>,
    preferred_provider: Option<&str>,
) -> bool {
    // `!model` upstream is truthiness, so it catches `undefined` AND `""`; no trimming happens.
    let Some(model) = model.filter(|m| !m.is_empty()) else {
        return true;
    };
    let Some(info) = find_model_info(model, preferred_provider) else {
        return true;
    };
    info.provider.as_str().eq_ignore_ascii_case("anthropic")
        || info.api.as_str().eq_ignore_ascii_case("anthropic-messages")
}

/// pi `findModelInfo` (`shared/model-info.ts:74-86` @v0.57.0), resolved against
/// [`crate::extension::models::registry_models`] — this crate's standing binding for pi's
/// `ctx.modelRegistry.getAvailable()`.
///
/// Upstream's exact order, including its refusal to guess: strip any known `:<level>` suffix; try
/// an exact `provider/id` match; then collect the bare-`id` matches and narrow them by
/// `preferred_provider`; and when several providers offer that bare id with no preferred provider
/// to break the tie, return `None` rather than picking one. An ambiguous id is left UNKNOWN, which
/// [`forked_child_requires_thinking_off`] then treats conservatively — upstream's
/// `matches.length === 1 ? matches[0] : undefined`.
///
/// Id matching is case-SENSITIVE (upstream compares with `===`); only the provider/api test in
/// [`forked_child_requires_thinking_off`] lowercases, which is likewise upstream's own split.
fn find_model_info(
    model: &str,
    preferred_provider: Option<&str>,
) -> Option<&'static cyrup_provider::Model> {
    let (base_model, _) = crate::exec::spawn_plan::split_known_thinking_suffix(model);
    let models = crate::extension::models::registry_models();

    // Upstream's `entry.fullId === baseModel`. `fullId` is exactly `${provider}/${id}`, so
    // splitting on the FIRST `/` (provider names never contain one) compares the same two halves
    // without formatting a throwaway string per catalog entry.
    if let Some((provider, id)) = base_model.split_once('/')
        && let Some(exact) = models
            .iter()
            .find(|m| m.provider.as_str() == provider && m.id.as_str() == id)
    {
        return Some(exact);
    }

    let matches: Vec<&'static cyrup_provider::Model> =
        models.iter().filter(|m| m.id.as_str() == base_model).collect();
    if let Some(preferred) = preferred_provider
        && let Some(hit) = matches.iter().copied().find(|m| m.provider.as_str() == preferred)
    {
        return Some(hit);
    }
    if matches.len() == 1 {
        matches.first().copied()
    } else {
        None
    }
}

/// pi `isUnsafeAnthropicThinkingBlock` (`shared/fork-context.ts:117-127` @v0.57.0).
///
/// [CYRUP-DELTA] Upstream tests TWO wire types: `redacted_thinking` is unsafe unconditionally,
/// while a `thinking` block is unsafe only on an Anthropic turn and only when it carries
/// `redacted: true` or a non-empty signature. A cyrup session file never holds a
/// `redacted_thinking` block — the Anthropic adapter DECODES that wire type into
/// [`Content::Thinking`] with `redacted: true` on the way in
/// (`cyrup-provider/src/api/anthropic_messages/events.rs:73`) and re-encodes it on the way out
/// (`messages.rs:152`). So `redacted == true` IS upstream's `redacted_thinking` and keeps its
/// unconditional treatment; the signature test keeps upstream's Anthropic gate.
///
/// Takes the three message fields rather than `&AssistantMessage` because its only caller runs it
/// inside a `Vec::retain` over `content`, which holds that field mutably borrowed for the closure.
fn is_unsafe_thinking_block(provider: &str, api: &str, model: &str, block: &Content) -> bool {
    let Content::Thinking { thinking_signature, redacted, .. } = block else {
        return false;
    };
    if *redacted {
        return true;
    }
    let is_anthropic = provider.eq_ignore_ascii_case("anthropic")
        || api.eq_ignore_ascii_case("anthropic-messages")
        || model.to_lowercase().starts_with("anthropic/");
    is_anthropic && thinking_signature.as_deref().is_some_and(|sig| !sig.is_empty())
}

/// One line of a branched session file, carried as BOTH its original text and its parsed form.
///
/// The raw text is what gets written back for any line this pass did not change. Re-serializing an
/// untouched line would be lossless in CONTENT but not in bytes: `Entry::Unknown` holds a
/// [`serde_json::Value`], and this workspace's `serde_json` is built without `preserve_order`, so
/// its object maps are `BTreeMap`s that alphabetize keys on the way out. pi has no such problem —
/// `JSON.stringify` emits insertion order — so echoing the original text is what keeps a rewritten
/// branch byte-comparable with the one pi would have produced, and keeps an entry annotated by a
/// newer writer or an extension exactly as its author wrote it.
struct BranchLine {
    /// `None` once this line's entry has been modified (or synthesized), meaning it must be
    /// re-serialized from [`Self::entry`] rather than echoed.
    raw: Option<String>,
    entry: Entry,
}

/// pi `sanitizeUnsafeThinkingBlocks` (`shared/fork-context.ts:152-163` @v0.57.0): strip every
/// unsafe thinking block from every assistant entry, in place. Returns whether anything was
/// removed — the value that gates BOTH the thinking-off entry and the file rewrite.
fn sanitize_unsafe_thinking_blocks(lines: &mut [BranchLine]) -> bool {
    let mut sanitized = false;
    for line in lines {
        let Entry::Known(KnownEntry::Message {
            message: AgentMessage::Core(Message::Assistant(assistant)),
            ..
        }) = &mut line.entry
        else {
            continue;
        };
        // Destructured so `content`'s mutable borrow and the three read-only fields the predicate
        // needs are disjoint field borrows rather than two borrows of the whole message.
        let AssistantMessage { content, provider, api, model, .. } = assistant;
        let before = content.len();
        content.retain(|block| {
            !is_unsafe_thinking_block(provider.as_str(), api.as_str(), model, block)
        });
        if content.len() != before {
            sanitized = true;
            line.raw = None;
        }
    }
    sanitized
}

/// pi `createEntryId` (`shared/fork-context.ts:129-137` @v0.57.0): a short id colliding with
/// nothing already in the file, retried up to 100 times.
///
/// [`cyrup_session::ids::gen_short_id`] is the tail of a uuid-v7, so its time component already
/// makes a within-file collision effectively impossible; the guard is kept because upstream keeps
/// it and because `SessionManager::mint_id` — the same loop — is private to its own crate.
fn mint_entry_id(lines: &[BranchLine]) -> EntryId {
    let existing: std::collections::HashSet<EntryId> =
        lines.iter().map(|line| line.entry.id()).collect();
    let mut id = cyrup_session::ids::gen_short_id();
    for _ in 0..100 {
        if !existing.contains(&id) {
            break;
        }
        id = cyrup_session::ids::gen_short_id();
    }
    id
}

/// pi `appendThinkingOffEntry` (`shared/fork-context.ts:139-150` @v0.57.0): record the child's
/// reasoning level as `off` IN the branched transcript, so the level is part of the session the
/// child resumes rather than only of the argv it is launched with.
fn append_thinking_off_entry(lines: &mut Vec<BranchLine>) {
    if let Some(BranchLine {
        entry: Entry::Known(KnownEntry::ThinkingLevelChange { thinking_level, .. }),
        ..
    }) = lines.last()
        && thinking_level == "off"
    {
        return;
    }
    let id = mint_entry_id(lines);
    // Upstream `[...entries].reverse().find((entry) => typeof entry.id === "string")` — the last
    // entry carrying a REAL id. [`Entry::id`] synthesizes a placeholder for an `Unknown` entry that
    // has none (so tree indexing never panics), which is exactly the entry upstream skips; hence
    // the explicit test rather than a bare `lines.last().map(|l| l.entry.id())`.
    let parent_id = lines.iter().rev().find_map(|line| match &line.entry {
        Entry::Known(known) => Some(known.base().id.clone()),
        Entry::Unknown(value) => value
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(EntryId::from),
    });
    lines.push(BranchLine {
        raw: None,
        entry: Entry::known(KnownEntry::ThinkingLevelChange {
            base: EntryBase {
                id,
                parent_id,
                timestamp: cyrup_session::ids::now_ts(),
                extra: serde_json::Map::new(),
            },
            thinking_level: "off".to_string(),
        }),
    });
}

/// Read a branched session file for in-place rewriting: the header line kept VERBATIM as text, and
/// every subsequent non-blank line parsed as a typed [`Entry`] while retaining its original text
/// (see [`BranchLine`] for why both).
///
/// The header is never parsed at all. pi's `readSessionEntries`
/// (`shared/fork-context.ts:165-176`) treats it as just another entry and re-`JSON.stringify`s it;
/// holding it as an opaque string makes it byte-identical across the rewrite instead of merely
/// equivalent, since cyrup's [`cyrup_session::SessionHeader`] is a typed struct that a round trip
/// could reorder or, for a key a newer writer added, drop outright.
///
/// Every failure here collapses to [`SubagentError::ForkFailed`], the documented catch-all for
/// "branch creation failed for any other reason". The cause is lost, but the fail-HARD contract
/// (R-SA-137/DI-SA-2 — never silently downgrade a requested fork to `Fresh`) is what this path owes
/// its caller, and `ForkFailed` is the variant `resolve` already fails with alongside it.
fn read_session_entries(path: &Path) -> Result<(String, Vec<BranchLine>), SubagentError> {
    let raw = std::fs::read_to_string(path).map_err(|_| SubagentError::ForkFailed)?;
    let mut lines = raw.lines().filter(|line| !line.trim().is_empty());
    let header = lines.next().unwrap_or_default().to_string();
    let entries = lines
        .map(|line| {
            serde_json::from_str::<Entry>(line)
                .map(|entry| BranchLine { raw: Some(line.to_string()), entry })
                .map_err(|_| SubagentError::ForkFailed)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((header, entries))
}

/// Write back what [`read_session_entries`] read — pi's
/// `${entries.map((entry) => JSON.stringify(entry)).join("\n")}\n`, with the header restored as the
/// first line and every untouched line echoed rather than re-encoded.
fn write_session_entries(
    path: &Path,
    header: &str,
    lines: &[BranchLine],
) -> Result<(), SubagentError> {
    let mut buf = String::new();
    buf.push_str(header);
    buf.push('\n');
    for line in lines {
        match &line.raw {
            Some(raw) => buf.push_str(raw),
            None => buf.push_str(&line.entry.to_line().map_err(|_| SubagentError::ForkFailed)?),
        }
        buf.push('\n');
    }
    std::fs::write(path, buf).map_err(|_| SubagentError::ForkFailed)
}

/// Resolves `context: "fork"` requests into a concrete, branched session-file path — the sole
/// owner of fork-context logic in this crate (arch-SA §6.6). Every subsystem that needs
/// fork-context (`exec/` foreground execution, `spawn/` the OS-subprocess boundary, `background/`
/// hand-off to the detached runner) calls [`ForkContextResolver::resolve`]; none re-derive any
/// part of this algorithm.
///
/// Per-index caching (`cached`) makes `resolve` idempotent across repeated calls for the same
/// batch-step index — required by the eager whole-batch validation algorithm (R-SA-137, §6.6):
/// `SubagentExecutor::plan_batch()` resolves every step's `ForkContext` up front, and a later
/// re-resolution for the same index (e.g. a retry path) MUST return the same branched session
/// file rather than creating a second, divergent branch.
pub struct ForkContextResolver {
    /// The orchestrator's LIVE session manager — read from only to obtain the current leaf id
    /// and confirm persistence. Branching itself NEVER touches this handle's in-memory state
    /// (R-SA-139/DI-SA-6); see [`ForkContextResolver::resolve`]'s throwaway-handle step.
    manager: Arc<AsyncMutex<SessionManager>>,
    layout: SessionLayout,
    /// Per-index resolution memo. SUBA-075: the value is the WHOLE resolution — branched path AND
    /// the thinking override that branch was sanitized into — because a repeat `resolve` for an
    /// index must reproduce the first one exactly. Caching only the path silently dropped the
    /// override on every call after the first. pi caches the same pair, and for the same reason
    /// (`cachedResolutions: Map<number, ForkContextResolution>`, `shared/fork-context.ts:231,272`).
    cached: StdMutex<HashMap<u32, (PathBuf, Option<String>)>>,
}

impl ForkContextResolver {
    /// Construct a resolver over the orchestrator's live session manager and the session-file
    /// layout to branch new sessions into (normally the same layout the parent session itself
    /// was created/opened with, so the branched file lands alongside its parent).
    pub fn new(manager: Arc<AsyncMutex<SessionManager>>, layout: SessionLayout) -> Self {
        Self {
            manager,
            layout,
            cached: StdMutex::new(HashMap::new()),
        }
    }

    /// Resolve one batch-step's requested context mode into a concrete [`ForkContext`].
    ///
    /// Fails fast (R-SA-137/DI-SA-2): NEVER falls back to `Fresh` when `Fork` was requested and
    /// branching cannot proceed. Callers (notably `exec::plan_batch`) MUST resolve every step in
    /// a batch before spawning any child process for that batch, so a later step's fork failure
    /// is discovered before any earlier step's subprocess has started.
    ///
    /// # SUBA-075 — the branch is sanitized before the child ever sees it
    ///
    /// pi `resolveFork` (`shared/fork-context.ts:246-272` @v0.57.0) has THREE outcomes, and so
    /// does this, because the two gates are independent:
    ///
    /// | parent transcript | `force_thinking_off` | result |
    /// |---|---|---|
    /// | no unsafe block | not consulted | file left untouched, no override |
    /// | unsafe blocks | `false` | blocks stripped and the file rewritten, but NO override and no `thinking_level_change` entry |
    /// | unsafe blocks | `true` | stripped, `thinking_level_change: off` appended, rewritten, `thinking_override: Some("off")` |
    ///
    /// Sanitization itself is unconditional (an inherited Anthropic thinking block whose signature
    /// was minted for another request context is rejected by the provider, so leaving one in place
    /// breaks the run outright); the override is what the model gate decides.
    ///
    /// `force_thinking_off` is upstream's `options.forceThinkingOffForIndex?.(index) ?? true`,
    /// hoisted to a parameter: the model ladder this branch's child will run is resolved by the
    /// CALLER, well after the fork is requested, so the decision cannot be made here. Callers that
    /// have the ladder compute it with [`forked_child_requires_thinking_off`]; callers that do not
    /// pass `true`, which is upstream's own `?? true` default and the conservative direction.
    pub async fn resolve(
        &self,
        requested: ContextMode,
        index: u32,
        force_thinking_off: bool,
    ) -> Result<ForkContext, SubagentError> {
        if requested != ContextMode::Fork {
            return Ok(ForkContext::fresh());
        }

        if let Some((cached_path, cached_override)) = self
            .cached
            .lock()
            .map_err(|_| SubagentError::ForkFailed)?
            .get(&index)
        {
            return Ok(ForkContext {
                mode: ContextMode::Fork,
                session_file_path: Some(cached_path.clone()),
                thinking_override: cached_override.clone(),
            });
        }

        // Read only the current leaf id and the persisted-file path from the LIVE parent
        // manager; the guard is dropped at the end of this block, before any branching happens,
        // so branching never observes (or mutates) the live manager's in-memory state
        // (R-SA-139/DI-SA-6).
        let (leaf, persisted_path) = {
            let guard = self.manager.lock().await;
            if !guard.is_persisted() {
                return Err(SubagentError::ForkRequiresPersistedParent);
            }
            let leaf: EntryId = guard
                .leaf_id()
                .cloned()
                .ok_or(SubagentError::ForkRequiresLeaf)?;
            // `is_persisted()` was true above, so `session_file()` is expected to be `Some`; a
            // `None` here would mean the store reports itself persisted without a backing path,
            // an internal inconsistency this resolver treats identically to "not persisted"
            // rather than panicking or indexing into an absent value.
            let persisted_path = guard
                .session_file()
                .ok_or(SubagentError::ForkRequiresPersistedParent)?
                .to_path_buf();
            (leaf, persisted_path)
        };

        // Open a THROWAWAY handle on the parent's PERSISTED file on disk — never the live
        // manager. This is a brand-new `SessionManager` instance, used exactly once for this one
        // branch call, then dropped; it never becomes "the" session manager for anything, and the
        // orchestrator's own live manager is left completely untouched (R-SA-139/DI-SA-6).
        let mut throwaway = SessionManager::open(&persisted_path)?;
        let branched_path = throwaway
            .create_branched_session(&leaf, &self.layout)?
            .ok_or(SubagentError::ForkFailed)?;
        drop(throwaway);

        // SUBA-075 / pi `resolveFork`'s live arm (`shared/fork-context.ts:255-262` @v0.57.0). Only
        // that arm has a cyrup analogue: upstream's `!fs.existsSync(sessionFile)` fallback covers a
        // session manager that names a branch file it did not write, whereas
        // `create_branched_session` returns `Ok(None)` in that situation and the `ok_or` above has
        // already turned it into a hard error. So by here the file exists.
        //
        // Note what is NOT rewritten: when nothing was sanitized the file is left exactly as
        // `create_branched_session` wrote it — upstream keeps `writeFileSync` INSIDE the `if
        // (sanitizeUnsafeThinkingBlocks(entries))` block, and a no-op rewrite would still churn the
        // file's bytes and mtime for every fork of a clean transcript.
        let mut thinking_override = None;
        let (header, mut lines) = read_session_entries(&branched_path)?;
        if sanitize_unsafe_thinking_blocks(&mut lines) {
            if force_thinking_off {
                append_thinking_off_entry(&mut lines);
                thinking_override = Some("off".to_string());
            }
            write_session_entries(&branched_path, &header, &lines)?;
        }

        self.cached
            .lock()
            .map_err(|_| SubagentError::ForkFailed)?
            .insert(index, (branched_path.clone(), thinking_override.clone()));

        Ok(ForkContext {
            mode: ContextMode::Fork,
            session_file_path: Some(branched_path),
            thinking_override,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use std::path::Path;

    use cyrup_core::{AssistantMessage, Content, Message, StopReason, Usage};
    use cyrup_session::NewSessionOpts;

    use super::*;

    fn layout(root: &Path, cwd: &Path) -> SessionLayout {
        SessionLayout::new(root.to_path_buf(), cwd.to_path_buf())
    }

    fn user(s: &str) -> Message {
        Message::User {
            content: vec![Content::text(s)],
            timestamp: 0,
        }
    }

    fn assistant(s: &str) -> Message {
        Message::Assistant(AssistantMessage {
            content: vec![Content::text(s)],
            provider: "faux".into(),
            model: "faux-1".into(),
            api: "faux".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            timestamp: 0,
        })
    }

    // ---------------------------------------------------------------------------------------
    // resolve_effective_context: A-SA-5 (context independence, DI-SA-3, R-SA-138/R-SA-111)
    // ---------------------------------------------------------------------------------------

    #[test]
    fn resolve_effective_context_prefers_an_explicit_call_site_value_over_the_agent_default() {
        assert_eq!(
            resolve_effective_context(Some(ContextMode::Fork), Some(ContextMode::Fresh)),
            ContextMode::Fork,
            "an explicit call-site context must win even when it disagrees with the agent's own default"
        );
    }

    #[test]
    fn resolve_effective_context_falls_back_to_the_agent_default_when_call_site_omits_it() {
        assert_eq!(
            resolve_effective_context(None, Some(ContextMode::Fork)),
            ContextMode::Fork
        );
        assert_eq!(
            resolve_effective_context(None, Some(ContextMode::Fresh)),
            ContextMode::Fresh
        );
    }

    #[test]
    fn resolve_effective_context_defaults_to_fresh_when_neither_is_present() {
        assert_eq!(resolve_effective_context(None, None), ContextMode::Fresh);
    }

    /// A-SA-5, the exact scenario: in one parallel/batch call with `context` omitted at every call
    /// site, a sibling task whose agent persona defaults to `fresh` and a sibling task whose agent
    /// persona defaults to `fork` each resolve independently — resolving one MUST NOT be
    /// influenced by, or leak into, resolving the other. Modeled here as two independent calls
    /// into the same pure function (exactly how a real batch-builder is required to invoke it: once
    /// per task, never once for the whole batch) sharing no state whatsoever between them, which is
    /// what makes cross-sibling leakage structurally impossible rather than merely untested.
    #[test]
    fn resolve_effective_context_resolves_each_sibling_in_one_batch_independently() {
        // Sibling A: agent persona's own default_context is Fresh, call site omits context.
        let sibling_a_agent_default = Some(ContextMode::Fresh);
        // Sibling B: agent persona's own default_context is Fork, call site omits context.
        let sibling_b_agent_default = Some(ContextMode::Fork);

        let resolved_a = resolve_effective_context(None, sibling_a_agent_default);
        let resolved_b = resolve_effective_context(None, sibling_b_agent_default);

        assert_eq!(
            resolved_a,
            ContextMode::Fresh,
            "sibling A's own persona default (fresh) must be what it resolves to"
        );
        assert_eq!(
            resolved_b,
            ContextMode::Fork,
            "sibling B's own persona default (fork) must be what it resolves to, unaffected by \
             sibling A ever having been resolved to fresh in the same batch"
        );

        // Order independence: resolving B before A must produce the identical pair of outcomes —
        // proof there is no hidden shared/mutable state a call ordering could leak through.
        let resolved_b_first = resolve_effective_context(None, sibling_b_agent_default);
        let resolved_a_second = resolve_effective_context(None, sibling_a_agent_default);
        assert_eq!(resolved_b_first, ContextMode::Fork);
        assert_eq!(resolved_a_second, ContextMode::Fresh);
    }

    /// A real, persisted parent session (tempdir-backed, on-disk JSONL — never mocked) branches
    /// successfully: `resolve(Fork, _)` produces a genuine new session file on disk, distinct from
    /// the parent's file, and the parent's own live manager and its on-disk file are both left
    /// completely untouched (R-SA-139/DI-SA-6).
    #[tokio::test]
    async fn fork_resolve_produces_a_real_new_session_file_without_mutating_the_live_parent() {
        let root = tempfile::tempdir().expect("tempdir");
        let cwd = PathBuf::from("/proj/fork-context-test");
        let lay = layout(root.path(), &cwd);

        let mut parent = SessionManager::create(&cwd, &lay, NewSessionOpts::default())
            .expect("create parent session");
        parent.append_message(user("hello")).expect("append user");
        parent
            .append_message(assistant("hi there"))
            .expect("append assistant");
        let parent_path = parent
            .session_file()
            .expect("parent persisted")
            .to_path_buf();
        let parent_leaf_before = parent.leaf_id().cloned().expect("parent has a leaf");
        let parent_entry_count_before = parent.entries().len();

        let manager = Arc::new(AsyncMutex::new(parent));
        let resolver = ForkContextResolver::new(Arc::clone(&manager), lay.clone());

        let resolved = resolver
            .resolve(ContextMode::Fork, 0, true)
            .await
            .expect("fork resolves");
        assert_eq!(resolved.mode, ContextMode::Fork);
        let forked_path = resolved
            .session_file_path
            .expect("fork produces a session file path");

        // A real, distinct file exists on disk with real JSONL content.
        assert!(
            forked_path.exists(),
            "branched session file must exist on disk"
        );
        assert_ne!(
            forked_path, parent_path,
            "branched file must differ from the parent's file"
        );
        let forked_contents = std::fs::read_to_string(&forked_path).expect("read forked file");
        assert!(
            forked_contents.lines().count() >= 2,
            "forked file must contain a real header + entries"
        );

        // Lineage provenance (R-SA-143): the forked file's header records the parent's path.
        let reopened = SessionManager::open(&forked_path).expect("reopen forked session");
        assert_eq!(
            reopened.header().parent_session.as_deref(),
            Some(parent_path.to_string_lossy().as_ref()),
            "forked session header must record parentSession provenance"
        );

        // The live parent handle held by this resolver was NEVER mutated in place: same leaf,
        // same entry count, same file identity (R-SA-139/DI-SA-6).
        let guard = manager.lock().await;
        assert_eq!(
            guard.leaf_id().cloned(),
            Some(parent_leaf_before),
            "live parent leaf unchanged"
        );
        assert_eq!(
            guard.entries().len(),
            parent_entry_count_before,
            "live parent entries unchanged"
        );
        assert_eq!(
            guard.session_file().map(|p| p.to_path_buf()),
            Some(parent_path.clone()),
            "live parent still points at its own original file"
        );
        drop(guard);

        // On-disk parent file is untouched: reopening it independently yields the same content.
        let parent_reopened = SessionManager::open(&parent_path).expect("reopen parent file");
        assert_eq!(parent_reopened.entries().len(), parent_entry_count_before);
    }

    /// `ContextMode::Fresh` never touches the session manager or filesystem at all.
    #[tokio::test]
    async fn fresh_resolve_never_touches_the_session_manager() {
        let root = tempfile::tempdir().expect("tempdir");
        let cwd = PathBuf::from("/proj/fresh-context-test");
        let lay = layout(root.path(), &cwd);

        // An UNPERSISTED in-memory manager: if `resolve` incorrectly touched it for a `Fresh`
        // request, this would panic/error since it has no leaf and is not persisted.
        let manager = SessionManager::in_memory(&cwd, NewSessionOpts::default())
            .expect("create in-memory session");
        let manager = Arc::new(AsyncMutex::new(manager));
        let resolver = ForkContextResolver::new(Arc::clone(&manager), lay);

        let resolved = resolver
            .resolve(ContextMode::Fresh, 0, true)
            .await
            .expect("fresh always resolves");
        assert_eq!(resolved, ForkContext::fresh());
    }

    /// Fail-hard behavior (R-SA-137/DI-SA-2): an UNPERSISTED parent session (no file ever
    /// written) requesting `Fork` MUST return `ForkRequiresPersistedParent` — never silently
    /// downgrade to `Fresh`, and never reach `create_branched_session` at all (verified here by
    /// the fact that no session file appears anywhere under `root` after the call).
    #[tokio::test]
    async fn fork_on_unpersisted_parent_fails_hard_without_calling_create_branched_session() {
        let root = tempfile::tempdir().expect("tempdir");
        let cwd = PathBuf::from("/proj/unpersisted-test");
        let lay = layout(root.path(), &cwd);

        // In-memory session: never persisted, `is_persisted()` is false.
        let manager = SessionManager::in_memory(&cwd, NewSessionOpts::default())
            .expect("create in-memory session");
        assert!(
            !manager.is_persisted(),
            "precondition: in-memory session is not persisted"
        );

        let manager = Arc::new(AsyncMutex::new(manager));
        let resolver = ForkContextResolver::new(Arc::clone(&manager), lay);

        let err = resolver
            .resolve(ContextMode::Fork, 0, true)
            .await
            .expect_err("fork against an unpersisted parent must fail hard");
        assert!(
            matches!(err, SubagentError::ForkRequiresPersistedParent),
            "expected ForkRequiresPersistedParent, got: {err:?}"
        );

        // No session file was ever created anywhere under the sessions root — proof that
        // `create_branched_session` (and even `SessionManager::open`) was never reached.
        let any_files_created = std::fs::read_dir(root.path())
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false);
        assert!(
            !any_files_created,
            "no filesystem state should be created when fork fails hard pre-branch"
        );
    }

    /// Fail-hard behavior (R-SA-137/DI-SA-2): a PERSISTED parent session with NO resolvable leaf
    /// (a freshly-created session with zero appended messages — `leaf_id()` is `None`) requesting
    /// `Fork` MUST return `ForkRequiresLeaf` — never silently downgrade to `Fresh`.
    #[tokio::test]
    async fn fork_with_no_resolvable_leaf_fails_hard() {
        let root = tempfile::tempdir().expect("tempdir");
        let cwd = PathBuf::from("/proj/no-leaf-test");
        let lay = layout(root.path(), &cwd);

        // A brand-new session with zero entries has no leaf, regardless of persistence.
        let manager = SessionManager::create(&cwd, &lay, NewSessionOpts::default())
            .expect("create parent session");
        assert!(
            manager.leaf_id().is_none(),
            "precondition: fresh session has no leaf"
        );

        let manager = Arc::new(AsyncMutex::new(manager));
        let resolver = ForkContextResolver::new(Arc::clone(&manager), lay);

        let err = resolver
            .resolve(ContextMode::Fork, 0, true)
            .await
            .expect_err("fork against a leafless parent must fail hard");
        // A leafless session is also not-yet-persisted in this implementation (the file write is
        // deferred until the first assistant message), so either fail-hard variant is acceptable
        // here as long as it is NOT a silent Fresh downgrade and NOT a success.
        assert!(
            matches!(
                err,
                SubagentError::ForkRequiresLeaf | SubagentError::ForkRequiresPersistedParent
            ),
            "expected a fail-hard fork error, got: {err:?}"
        );
    }

    // ---------------------------------------------------------------------------------------
    // SUBA-075 — fork sanitization (pi `shared/fork-context.ts:105-178` @v0.57.0)
    // ---------------------------------------------------------------------------------------

    /// Concrete catalog ids the gate tests pin to. Each is asserted to still have the shape the
    /// test depends on before it is used, so a catalog change fails with "this fixture moved"
    /// rather than silently inverting the assertion it was chosen to make.
    const ANTHROPIC_QUALIFIED: &str = "anthropic/claude-opus-4-6";
    /// `anthropic-messages` api under a provider that is NOT `anthropic` — the only fixture that
    /// can tell the two arms of the gate apart.
    const ANTHROPIC_API_OTHER_PROVIDER: &str = "cloudflare-ai-gateway/claude-3-opus";
    /// Neither Anthropic axis. Its `:0` tail also proves the suffix strip only fires on a
    /// recognized THINKING level.
    const NON_ANTHROPIC_QUALIFIED: &str = "amazon-bedrock/amazon.nova-pro-v1:0";
    /// A bare id several providers offer, none of them Anthropic on either axis.
    const AMBIGUOUS_BARE: &str = "deepseek-v4-flash";

    fn catalog_entry(qualified: &str) -> &'static cyrup_provider::Model {
        let (provider, id) = qualified.split_once('/').expect("fixture is provider-qualified");
        let found = crate::extension::models::registry_models()
            .iter()
            .find(|m| m.provider.as_str() == provider && m.id.as_str() == id);
        assert!(
            found.is_some(),
            "catalog fixture {qualified} is no longer in the registry; pick a live id rather than \
             letting the assertions below pass vacuously"
        );
        found.expect("asserted present immediately above")
    }

    fn thinking(signature: Option<&str>, redacted: bool) -> Content {
        Content::Thinking {
            thinking: "chain of thought".to_string(),
            thinking_signature: signature.map(str::to_string),
            redacted,
        }
    }

    /// An assistant turn with an explicit provider/api/model triple and arbitrary content — the
    /// three fields `is_unsafe_thinking_block` reads.
    fn assistant_from(provider: &str, api: &str, model: &str, content: Vec<Content>) -> Message {
        Message::Assistant(AssistantMessage {
            content,
            provider: provider.into(),
            model: model.to_string(),
            api: api.into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            timestamp: 0,
        })
    }

    /// A persisted parent whose single assistant turn carries `content`, plus a resolver over it.
    async fn parent_with_assistant(
        root: &Path,
        cwd: &Path,
        message: Message,
    ) -> ForkContextResolver {
        let lay = layout(root, cwd);
        let mut parent = SessionManager::create(cwd, &lay, NewSessionOpts::default())
            .expect("create parent session");
        parent.append_message(user("go")).expect("append user");
        parent.append_message(message).expect("append assistant");
        ForkContextResolver::new(Arc::new(AsyncMutex::new(parent)), lay)
    }

    /// Every thinking block left in a branched file, as `(signature, redacted)`.
    fn surviving_thinking(lines: &[BranchLine]) -> Vec<(Option<String>, bool)> {
        lines
            .iter()
            .filter_map(|line| match &line.entry {
                Entry::Known(KnownEntry::Message {
                    message: AgentMessage::Core(Message::Assistant(a)),
                    ..
                }) => Some(&a.content),
                _ => None,
            })
            .flatten()
            .filter_map(|block| match block {
                Content::Thinking { thinking_signature, redacted, .. } => {
                    Some((thinking_signature.clone(), *redacted))
                }
                _ => None,
            })
            .collect()
    }

    fn thinking_off_entries(lines: &[BranchLine]) -> usize {
        lines
            .iter()
            .filter(|line| {
                matches!(
                    &line.entry,
                    Entry::Known(KnownEntry::ThinkingLevelChange { thinking_level, .. })
                        if thinking_level == "off"
                )
            })
            .count()
    }

    // ---- the model gate --------------------------------------------------------------------

    #[test]
    fn thinking_off_is_required_for_a_model_on_either_anthropic_axis() {
        let anthropic = catalog_entry(ANTHROPIC_QUALIFIED);
        assert_eq!(anthropic.provider.as_str(), "anthropic", "fixture precondition");
        assert!(
            forked_child_requires_thinking_off(Some(ANTHROPIC_QUALIFIED), None),
            "an `anthropic` provider model must force the sanitized branch to thinking-off"
        );

        // The SECOND axis, isolated: this model's provider is not `anthropic` at all, so only the
        // `api == anthropic-messages` test can be what carries it. Collapsing the gate to a
        // provider check alone would leave this one thinking-on and its inherited signed blocks
        // would be sent straight back to an Anthropic endpoint.
        let gateway = catalog_entry(ANTHROPIC_API_OTHER_PROVIDER);
        assert_ne!(gateway.provider.as_str(), "anthropic", "fixture precondition");
        assert_eq!(gateway.api.as_str(), "anthropic-messages", "fixture precondition");
        assert!(forked_child_requires_thinking_off(
            Some(ANTHROPIC_API_OTHER_PROVIDER),
            None
        ));
    }

    #[test]
    fn thinking_off_is_not_required_for_a_model_on_neither_anthropic_axis() {
        let entry = catalog_entry(NON_ANTHROPIC_QUALIFIED);
        assert_ne!(entry.provider.as_str(), "anthropic", "fixture precondition");
        assert_ne!(entry.api.as_str(), "anthropic-messages", "fixture precondition");
        assert!(
            !forked_child_requires_thinking_off(Some(NON_ANTHROPIC_QUALIFIED), None),
            "a model on neither Anthropic axis keeps its reasoning; forcing it off would cost \
             depth for no safety gain"
        );
    }

    /// pi `if (!model) return true` / `if (!info) return true` — the conservative arms. An
    /// unresolvable model is not assumed safe.
    #[test]
    fn thinking_off_is_required_when_the_model_cannot_be_resolved() {
        assert!(forked_child_requires_thinking_off(None, None), "absent model");
        assert!(forked_child_requires_thinking_off(Some(""), None), "empty model");
        assert!(
            forked_child_requires_thinking_off(Some("no-such-provider/no-such-model"), None),
            "a model absent from the catalog is unknown, and unknown is conservative"
        );
    }

    /// pi `matches.length === 1 ? matches[0] : undefined`: a bare id several providers offer is
    /// left UNRESOLVED rather than guessed at — which the conservative arm then turns into `true`.
    /// A `preferred_provider` breaks the tie and the real answer comes through.
    #[test]
    fn an_ambiguous_bare_id_stays_unresolved_until_a_preferred_provider_breaks_the_tie() {
        let providers: Vec<&str> = crate::extension::models::registry_models()
            .iter()
            .filter(|m| m.id.as_str() == AMBIGUOUS_BARE)
            .map(|m| m.provider.as_str())
            .collect();
        assert!(
            providers.len() > 1 && !providers.contains(&"anthropic"),
            "fixture precondition: {AMBIGUOUS_BARE} must be offered by several NON-anthropic \
             providers, got {providers:?}"
        );

        assert!(
            forked_child_requires_thinking_off(Some(AMBIGUOUS_BARE), None),
            "ambiguous -> unresolved -> conservative true"
        );
        assert!(
            !forked_child_requires_thinking_off(Some(AMBIGUOUS_BARE), Some(providers[0])),
            "the preferred provider resolves the id, and the resolved model is not Anthropic — \
             proof the `true` above came from ambiguity, not from a blanket default"
        );
    }

    /// pi resolves against `splitKnownThinkingSuffix(model).baseModel`, so a ladder entry that
    /// already carries `:high` still resolves. A colon that is NOT a known level is part of the id.
    #[test]
    fn the_gate_strips_a_known_thinking_suffix_before_resolving_but_leaves_other_colons() {
        assert!(
            forked_child_requires_thinking_off(
                Some(&format!("{ANTHROPIC_QUALIFIED}:high")),
                None
            ),
            "`:high` is a recognized level and must be stripped before the catalog lookup"
        );
        // `amazon.nova-pro-v1:0` ends in a colon segment that is NOT a thinking level. If the
        // split were unconditional the id would be truncated, resolve to nothing, and this would
        // come back conservatively `true`.
        assert!(!forked_child_requires_thinking_off(Some(NON_ANTHROPIC_QUALIFIED), None));
    }

    // ---- sanitization over a real fork -----------------------------------------------------

    /// The full live path, both gates armed: an Anthropic parent turn carrying BOTH unsafe shapes
    /// (a signed block and a redacted one) is branched, stripped, flagged `thinking_level_change:
    /// off`, and reports the override to the caller.
    #[tokio::test]
    async fn a_sanitized_fork_strips_unsafe_blocks_appends_thinking_off_and_reports_the_override() {
        let root = tempfile::tempdir().expect("tempdir");
        let cwd = PathBuf::from("/proj/sanitize-armed");
        let resolver = parent_with_assistant(
            root.path(),
            &cwd,
            assistant_from(
                "anthropic",
                "anthropic-messages",
                "anthropic/claude-opus-4-6",
                vec![
                    Content::text("visible answer"),
                    thinking(Some("sig-abc"), false),
                    thinking(None, true),
                ],
            ),
        )
        .await;

        let resolved = resolver
            .resolve(ContextMode::Fork, 0, true)
            .await
            .expect("fork resolves");
        let path = resolved.session_file_path.clone().expect("branch path");
        let (_, lines) = read_session_entries(&path).expect("branch parses");

        assert!(
            surviving_thinking(&lines).is_empty(),
            "both the signed and the redacted block must be gone; Anthropic rejects a request \
             carrying either once the signatures no longer match the request context"
        );
        assert!(
            lines.iter().any(|line| matches!(
                &line.entry,
                Entry::Known(KnownEntry::Message {
                    message: AgentMessage::Core(Message::Assistant(a)),
                    ..
                }) if a.content.iter().any(|b| matches!(b, Content::Text { text, .. } if text == "visible answer"))
            )),
            "only the thinking blocks go — the turn's actual answer must survive"
        );
        assert_eq!(
            thinking_off_entries(&lines),
            1,
            "the branch must record the level change, so a child resuming this session sees \
             thinking off from the transcript and not only from its argv"
        );
        assert!(
            matches!(
                lines.last(),
                Some(BranchLine { entry: Entry::Known(KnownEntry::ThinkingLevelChange { .. }), .. })
            ),
            "the level change is appended, not spliced mid-history"
        );
        assert_eq!(resolved.thinking_override.as_deref(), Some("off"));
    }

    /// The gate OFF: sanitization still happens (it is unconditional — the inherited blocks are
    /// unusable no matter who the child is), but nothing is flagged and no override is reported.
    /// This is the arm that would vanish if the two gates were collapsed into one.
    #[tokio::test]
    async fn a_fork_for_a_non_anthropic_child_is_still_sanitized_but_carries_no_override() {
        let root = tempfile::tempdir().expect("tempdir");
        let cwd = PathBuf::from("/proj/sanitize-gate-off");
        let resolver = parent_with_assistant(
            root.path(),
            &cwd,
            assistant_from(
                "anthropic",
                "anthropic-messages",
                "anthropic/claude-opus-4-6",
                vec![Content::text("answer"), thinking(Some("sig-abc"), false)],
            ),
        )
        .await;

        let resolved = resolver
            .resolve(ContextMode::Fork, 0, false)
            .await
            .expect("fork resolves");
        let path = resolved.session_file_path.clone().expect("branch path");
        let (_, lines) = read_session_entries(&path).expect("branch parses");

        assert!(surviving_thinking(&lines).is_empty(), "stripping is unconditional");
        assert_eq!(
            thinking_off_entries(&lines),
            0,
            "the model gate said no, so nothing may downgrade this child's reasoning"
        );
        assert_eq!(resolved.thinking_override, None);
    }

    /// The third outcome: a parent with nothing unsafe in it. No override, no level-change entry,
    /// and the branch carries exactly the parent's own entries — upstream keeps its `writeFileSync`
    /// INSIDE the `if (sanitized)` block, so a clean transcript is never rewritten at all.
    #[tokio::test]
    async fn a_fork_with_nothing_to_sanitize_is_left_alone() {
        let root = tempfile::tempdir().expect("tempdir");
        let cwd = PathBuf::from("/proj/sanitize-clean");
        let resolver = parent_with_assistant(
            root.path(),
            &cwd,
            assistant_from(
                "anthropic",
                "anthropic-messages",
                "anthropic/claude-opus-4-6",
                // An UNSIGNED, un-redacted thinking block is safe: there is no stale signature to
                // reject. Stripping it would discard reasoning the child can legitimately keep.
                vec![Content::text("answer"), thinking(None, false)],
            ),
        )
        .await;

        let resolved = resolver
            .resolve(ContextMode::Fork, 0, true)
            .await
            .expect("fork resolves");
        let path = resolved.session_file_path.clone().expect("branch path");
        let (_, lines) = read_session_entries(&path).expect("branch parses");

        assert_eq!(
            surviving_thinking(&lines),
            vec![(None, false)],
            "an unsigned, un-redacted thinking block is not unsafe and must survive"
        );
        assert_eq!(thinking_off_entries(&lines), 0);
        assert!(
            !std::fs::read_to_string(&path)
                .expect("branch readable")
                .contains("thinking_level_change"),
            "nothing may be appended to a branch that had nothing to sanitize"
        );
        assert_eq!(
            resolved.thinking_override, None,
            "the override is gated on something having been sanitized, not on the model alone"
        );
    }

    /// Per-block, per-message discrimination. `redacted` is upstream's `redacted_thinking` wire
    /// type, which it strips unconditionally; a mere signature is unsafe only on an Anthropic turn.
    #[tokio::test]
    async fn signature_stripping_is_anthropic_gated_while_redaction_is_not() {
        let root = tempfile::tempdir().expect("tempdir");
        let cwd = PathBuf::from("/proj/sanitize-per-provider");
        let resolver = parent_with_assistant(
            root.path(),
            &cwd,
            assistant_from(
                "openai",
                "openai-completions",
                "openai/gpt-5.4",
                vec![thinking(Some("sig-openai"), false), thinking(None, true)],
            ),
        )
        .await;

        let resolved = resolver
            .resolve(ContextMode::Fork, 0, true)
            .await
            .expect("fork resolves");
        let path = resolved.session_file_path.clone().expect("branch path");
        let (_, lines) = read_session_entries(&path).expect("branch parses");

        assert_eq!(
            surviving_thinking(&lines),
            vec![(Some("sig-openai".to_string()), false)],
            "a signed block on a NON-Anthropic turn is not Anthropic's problem and stays; the \
             redacted one goes regardless of provider, because that is the block pi carries as \
             the `redacted_thinking` wire type and strips unconditionally"
        );
        assert_eq!(resolved.thinking_override.as_deref(), Some("off"));
    }

    /// The resolution memo has to carry the override, not just the path: `resolve` is called
    /// repeatedly for the same index (eager batch validation, then the launch), and the second
    /// call takes the cached arm. Caching the path alone silently returned `None` from then on.
    #[tokio::test]
    async fn the_per_index_memo_replays_the_thinking_override_not_just_the_path() {
        let root = tempfile::tempdir().expect("tempdir");
        let cwd = PathBuf::from("/proj/sanitize-memo");
        let resolver = parent_with_assistant(
            root.path(),
            &cwd,
            assistant_from(
                "anthropic",
                "anthropic-messages",
                "anthropic/claude-opus-4-6",
                vec![thinking(Some("sig-abc"), false)],
            ),
        )
        .await;

        let first = resolver.resolve(ContextMode::Fork, 3, true).await.expect("first");
        let second = resolver.resolve(ContextMode::Fork, 3, true).await.expect("second");
        assert_eq!(first, second, "a cached resolution must replay in full");
        assert_eq!(second.thinking_override.as_deref(), Some("off"));
    }

    /// The rewrite is byte-faithful for every line it did not itself change. An entry cyrup does
    /// not model — a foreign `type`, or a known tag whose body does not fit — comes back exactly as
    /// its author wrote it, key order included, so a session annotated by a newer writer or an
    /// extension survives a sanitizing fork untouched.
    ///
    /// This is why [`BranchLine`] carries the original text: `Entry::Unknown` holds a
    /// [`serde_json::Value`], and this workspace's `serde_json` has no `preserve_order`, so
    /// re-encoding it would alphabetize the object's keys. The assistant entry alongside it is what
    /// makes the test load-bearing — it forces the `sanitized` branch, so the file really is
    /// rewritten rather than skipped.
    #[test]
    fn a_sanitizing_rewrite_leaves_every_untouched_line_byte_identical() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("branch.jsonl");

        let header_line =
            r#"{"type":"session","id":"s1","cwd":"/proj","version":2,"unknownHeaderKey":7}"#;
        // Deliberately NON-alphabetical keys at both levels: `payload` before `id`, `b` before `a`.
        // Re-encoding through `Value` would reorder every one of them.
        let foreign_line =
            r#"{"type":"future_entry_kind","payload":{"b":1,"a":2},"id":"e1","parentId":null}"#;
        let assistant_line = Entry::known(KnownEntry::Message {
            base: EntryBase {
                id: EntryId::from("e2"),
                parent_id: Some(EntryId::from("e1")),
                timestamp: "2026-01-01T00:00:00Z".to_string(),
                extra: serde_json::Map::new(),
            },
            message: AgentMessage::Core(assistant_from(
                "anthropic",
                "anthropic-messages",
                "anthropic/claude-opus-4-6",
                vec![Content::text("kept"), thinking(Some("sig"), false)],
            )),
        })
        .to_line()
        .expect("assistant entry serializes");

        std::fs::write(&path, format!("{header_line}\n{foreign_line}\n{assistant_line}\n"))
            .expect("seed");

        let (header, mut lines) = read_session_entries(&path).expect("read");
        assert!(
            matches!(lines.first(), Some(BranchLine { entry: Entry::Unknown(_), .. })),
            "an unmodelled entry must land in `Unknown`, not be dropped"
        );
        assert!(
            sanitize_unsafe_thinking_blocks(&mut lines),
            "the assistant entry's signed block must trigger the rewrite"
        );
        write_session_entries(&path, &header, &lines).expect("write");

        let rewritten = std::fs::read_to_string(&path).expect("reread");
        let mut written = rewritten.lines();
        assert_eq!(written.next(), Some(header_line), "header must be echoed verbatim");
        assert_eq!(
            written.next(),
            Some(foreign_line),
            "an untouched unmodelled entry must be echoed verbatim — re-encoding it through \
             `serde_json::Value` would alphabetize its keys"
        );
        assert!(
            !rewritten.contains("thinkingSignature"),
            "the entry that WAS touched must be re-encoded without its unsafe block"
        );
        assert!(rewritten.contains("kept"), "and with the rest of its content intact");
    }


    /// Repeated resolution for the SAME batch-step index returns the SAME branched session file
    /// (idempotent caching), rather than creating a second, divergent branch on every call.
    #[tokio::test]
    async fn resolve_is_idempotent_per_index() {
        let root = tempfile::tempdir().expect("tempdir");
        let cwd = PathBuf::from("/proj/idempotent-test");
        let lay = layout(root.path(), &cwd);

        let mut parent = SessionManager::create(&cwd, &lay, NewSessionOpts::default())
            .expect("create parent session");
        parent.append_message(user("hello")).expect("append user");
        parent
            .append_message(assistant("hi there"))
            .expect("append assistant");

        let manager = Arc::new(AsyncMutex::new(parent));
        let resolver = ForkContextResolver::new(Arc::clone(&manager), lay);

        let first = resolver
            .resolve(ContextMode::Fork, 7, true)
            .await
            .expect("first resolve");
        let second = resolver
            .resolve(ContextMode::Fork, 7, true)
            .await
            .expect("second resolve");
        assert_eq!(first.session_file_path, second.session_file_path);
    }
}

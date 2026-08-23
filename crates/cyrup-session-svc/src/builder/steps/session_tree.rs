//! Steps 2b, 3, 7 and the two session-tree tails of step 9 — everything that reads or writes the
//! [`SessionManager`] this session is anchored to: opening/creating/forking the tree, restoring the
//! model + thinking level from the resumed branch, seeding the agent transcript from it, appending
//! the model/thinking entries a future resume restores from, and resolving the session directory.

use std::path::PathBuf;

use cyrup_session::manager::{NewSessionOpts, SessionManager};
use cyrup_session::SessionLayout;
use cyrup_core::{ModelRef, ModelThinkingLevel, SessionId};
use cyrup_provider::Model;

use super::BuildCtx;
use crate::builder::model::resolve_model;
use crate::builder::thinking_level_to_str;
use crate::builder::SessionTarget;
use crate::error::SessionServiceError;
use crate::event::raw_message_to_agent;

/// The opened session tree plus the four projections of it every later step reads.
pub(in crate::builder) struct SessionTree {
    /// The resolved on-disk layout (an explicit `--session-dir` used literally, else the
    /// cwd-encoded default) — the fallback for [`session_dir_of`] on an in-memory session.
    pub(in crate::builder) layout: SessionLayout,
    pub(in crate::builder) manager: SessionManager,
    pub(in crate::builder) session_id: SessionId,
    /// The flattened context projection, read by model + thinking-level restore.
    pub(in crate::builder) existing: cyrup_session::context::SessionContext,
    /// The RAW projection taken beside `existing` so both see the same manager state (SEAM-112);
    /// what [`seed_transcript`] seeds the agent with.
    pub(in crate::builder) existing_raw: Vec<cyrup_session::agent_message::AgentMessage>,
    pub(in crate::builder) has_existing_session: bool,
    pub(in crate::builder) has_thinking_entry: bool,
}

/// What step 3 resolves: the catalog model and its address (both `None` on a modelless launch,
/// SEAM-075), the clamped thinking level, and pi's `modelFallbackMessage`.
pub(in crate::builder) struct ModelPick {
    pub(in crate::builder) resolved: Option<Model>,
    pub(in crate::builder) model_ref: Option<ModelRef>,
    pub(in crate::builder) thinking: ModelThinkingLevel,
    pub(in crate::builder) fallback_message: Option<String>,
}

/// Step 2b — open/create/continue/fork the session tree.
///
/// `prebuilt` is the builder's adopted manager (the runtime fork path), which short-circuits the
/// whole `cfg.target` match.
pub(in crate::builder) fn open_session_tree(
    ctx: &BuildCtx,
    prebuilt: Option<SessionManager>,
) -> Result<SessionTree, SessionServiceError> {
    let BuildCtx { cfg, cwd, .. } = ctx;
    // 2b. session tree (cyrup-session arch-04) — created BEFORE model resolution so the
    // model/thinking restore can read the resumed branch (Pi sdk.ts:178,187: the SessionManager
    // is constructed, then `buildSessionContext()` feeds `existingSession.model`/`thinkingLevel`).
    // Pi chooses the session directory per call: an explicit `sessionDir` (`--session-dir`) is
    // used LITERALLY, otherwise the cwd-encoded default `getDefaultSessionDir(cwd)` applies
    // (`sessionDir ? normalizePath(sessionDir) : getDefaultSessionDir(cwd)`,
    // session-manager.ts:1430,1457,1496). `cfg.session_dir` is `Some` only when `--session-dir`
    // (or its env) was explicitly supplied (the "was it explicit" signal ConfigDirs collapses one
    // layer too early is preserved as this `Option`); `None` ⇒ the encoded default. Using the
    // encoded [`SessionLayout::new`] on an explicit dir would nest one level too deep
    // (gap-analysis 05, Finding 3).
    let default_root = cfg.agent_dir.join("sessions");
    let layout = match &cfg.session_dir {
        Some(dir) => SessionLayout::literal(dir.clone(), cwd.clone()),
        None => SessionLayout::new(default_root.clone(), cwd.clone()),
    };
    let manager = match prebuilt {
        Some(m) => m,
        None => match &cfg.target {
            SessionTarget::New => {
                // Record `parentSession` on a freshly-created session (Pi `newSession`,
                // runtime.ts:238): the `New` target alone honors it — a resumed/continued
                // session keeps the parent it was created with.
                let opts = NewSessionOpts {
                    parent_session: cfg.parent_session.clone(),
                    ..NewSessionOpts::default()
                };
                if cfg.persist {
                    SessionManager::create(cwd, &layout, opts)?
                } else {
                    SessionManager::in_memory(cwd, opts)?
                }
            }
            // Rebind the resumed manager's cwd to the override when the runtime supplied one
            // (Pi `SessionManager.open(path, _, cwdOverride)`, runtime.ts:207); else derive from
            // the file header.
            SessionTarget::Resume(path) => {
                SessionManager::open_with_cwd(path, cfg.cwd_override.as_deref())?
            }
            // Pi `continueRecent` applies a cross-project cwd filter exactly when a custom
            // `sessionDir` is in play and it is not the cwd-default
            // (`filterCwd = sessionDir !== undefined && dir !== getDefaultSessionDirPath(cwd)`,
            // session-manager.ts:1458), so a shared `--session-dir` holding several projects'
            // sessions only resumes the current project's. The default (encoded) root already
            // isolates by cwd, so it never filters.
            SessionTarget::Continue => {
                let filter_cwd = match &cfg.session_dir {
                    Some(dir) => {
                        *dir != SessionLayout::new(default_root.clone(), cwd.clone()).dir()
                    }
                    None => false,
                };
                SessionManager::continue_recent_filtered(cwd, &layout, filter_cwd)?
            }
            // Fork the resolved source file into a fresh session at the build cwd (Pi
            // `forkSessionOrExit`/`SessionManager.forkFrom`, main.ts:251-258). The `--session-id`
            // (when given) becomes the forked session's id; otherwise one is minted.
            SessionTarget::Fork { source, id } => {
                let opts = NewSessionOpts {
                    id: id.clone().map(cyrup_core::SessionId::from),
                    ..NewSessionOpts::default()
                };
                SessionManager::fork_from(source, cwd, &layout, opts)?
            }
            // Create a fresh session with an explicit id (Pi `SessionManager.create(cwd, dir,
            // { id })`, main.ts:349). Persists like `New`; an ephemeral run goes in-memory.
            SessionTarget::CreateWithId(id) => {
                let opts = NewSessionOpts {
                    id: Some(cyrup_core::SessionId::from(id.clone())),
                    parent_session: cfg.parent_session.clone(),
                };
                if cfg.persist {
                    SessionManager::create(cwd, &layout, opts)?
                } else {
                    SessionManager::in_memory(cwd, opts)?
                }
            }
        },
    };
    let session_id = manager.session_id().clone();
    let existing = manager.build_context();
    // SEAM-112 — the RAW projection, taken here beside `existing` so BOTH reads see the same
    // manager state (pi calls `buildSessionContext()` ONCE, sdk.ts:190, and reuses the result
    // at :374). `existing` stays because `resolve_model` below restores the saved model +
    // thinking level from it (pi sdk.ts:191-242); `existing_raw` is what seeds the agent
    // transcript at step 7. See the comment there for why the flattened twin was wrong.
    let existing_raw = manager.build_context_raw();
    // Deliberately still the FLATTENED list. pi reads `existingSession.messages.length > 0`
    // off the raw projection (sdk.ts:191), and the two agree on every session cyrup writes:
    // `push_as_raw` (cyrup-session/src/context.rs:215-246) maps each context-visible entry to
    // exactly ONE raw message, and the single raw arm that can flatten to zero — a
    // `BashExecution` with `excludeFromContext` (`push_llm`, cyrup-session/src/agent_message.rs:
    // 191-197) — is never written by cyrup, whose `!!` executions persist as `custom_message`
    // entries ([`crate::AgentSession::record_bash_result`], session.rs:5550-5562) and so
    // survive the flattening as `custom`. Only a foreign file whose ONLY context entries are
    // `role:"bashExecution"` message entries could split the two, which is its own row.
    let has_existing_session = !existing.messages.is_empty();
    // Pi `hasThinkingEntry` (sdk.ts:189): does the resumed branch carry a thinking_level_change?
    let has_thinking_entry = manager
        .branch_path(None)
        .iter()
        .any(|e| matches!(e, cyrup_session::Entry::Known(
            cyrup_session::entry::KnownEntry::ThinkingLevelChange { .. })));

    Ok(SessionTree {
        layout,
        manager,
        session_id,
        existing,
        existing_raw,
        has_existing_session,
        has_thinking_entry,
    })
}

/// Step 3 — model resolution (cyrup-config + cyrup-provider).
///
/// Restores the model + thinking level from the resumed session, seeding a fallback message when
/// the saved model is no longer resolvable (Pi sdk.ts:191-242).
pub(in crate::builder) fn resolve_session_model(
    ctx: &BuildCtx,
    tree: &SessionTree,
) -> Result<ModelPick, SessionServiceError> {
    let (resolved, model_ref, thinking, fallback_message) = resolve_model(
        &*ctx.provider,
        &ctx.cfg,
        &ctx.settings,
        &tree.existing,
        tree.has_existing_session,
        tree.has_thinking_entry,
    )?;
    Ok(ModelPick { resolved, model_ref, thinking, fallback_message })
}

/// Step 7 — seed the agent transcript from the resumed branch (R-04-011). The manager was opened
/// at step 2b; `existing_raw` already holds its context.
pub(in crate::builder) fn seed_transcript(tree: &SessionTree) -> Vec<cyrup_agent::AgentMessage> {
    let existing_raw = &tree.existing_raw;
    // SEAM-112 (SESS-043 residual) — seeded from the RAW projection, roles intact. pi's build
    // path is `const existingSession = sessionManager.buildSessionContext();` (sdk.ts:190) then
    // `agent.state.messages = existingSession.messages;` (sdk.ts:374), and `buildSessionContext`
    // is `buildContextEntries(...).flatMap(sessionEntryToContextMessages)`
    // (session-manager.ts:461-469) — the projection BEFORE `convertToLlm`, whose
    // `sessionEntryToContextMessages` (`:383-407`) returns `custom` / `branchSummary` /
    // `compactionSummary` roles UNTOUCHED. `convertToLlm` is applied one layer out, at the
    // request boundary only (sdk.ts:301).
    //
    // Folding `build_context().messages` through `core_message_to_agent` produced a transcript
    // of a different LENGTH and different roles from pi's: `convertToLlm` DROPS every
    // `excludeFromContext` (`!!`) bash message and rewrites each summary into wrapper prose, so
    // pi's `messages.slice(0, -1)` arithmetic (`agent-session.ts:2008`, `:2188`, `:2703`) and
    // every agent-state token estimate ran over a different list. This was the LAST seed site
    // still on the flattened twin; the three re-seeds (`session.rs`'s `compact`,
    // `navigate_tree` and `run_auto_compaction`) already match pi's `agent-session.ts:1955`,
    // `:3206` and `:2280` respectively.
    existing_raw.iter().map(raw_message_to_agent).collect()
}

/// The step-9 tail that persists the run's model + thinking level so a future resume can restore
/// them, backfilling a thinking entry for a resumed session that lacks one (Pi sdk.ts:363-375).
pub(in crate::builder) fn seed_session_entries(
    tree: &mut SessionTree,
    model: &ModelPick,
) -> Result<(), SessionServiceError> {
    let has_existing_session = tree.has_existing_session;
    let has_thinking_entry = tree.has_thinking_entry;
    let manager = &mut tree.manager;
    let thinking = model.thinking;
    let resolved_model = &model.resolved;
    if has_existing_session {
        if !has_thinking_entry {
            manager.append_thinking_level_change(&thinking_level_to_str(thinking))?;
        }
    } else {
        // Pi sdk.ts:370-373 guards this on the model existing —
        // `if (model) { sessionManager.appendModelChange(model.provider, model.id); }` —
        // while the thinking-level entry is appended unconditionally. A modelless session must
        // therefore persist NO `model_change` entry, so a later resume has nothing bogus to
        // restore from.
        if let Some(m) = resolved_model.as_ref() {
            manager.append_model_change(m.provider.clone(), m.id.clone())?;
        }
        manager.append_thinking_level_change(&thinking_level_to_str(thinking))?;
    }
    Ok(())
}

/// The directory THIS session's files live in — Pi's `SessionManager.sessionDir`, exposed as
/// `getSessionDir()` (session-manager.ts:999-1001) and fixed once at construction. Pi resolves it
/// as `sessionDir ? normalizePath(sessionDir) : getDefaultSessionDir(cwd)` when a session is
/// created (`create`, :1519-1520) and as `sessionDir ?? resolve(path, "..")` — the OPEN FILE's own
/// parent — when one is resumed (`open`, :1547-1548). The interactive `/resume` picker lists
/// exactly this directory (`SessionManager.list(getCwd(), getSessionDir())`,
/// interactive-mode.ts:4867), so it is carried on the services instead of being re-derived from
/// the cwd-encoded default, which is wrong under `--session-dir` and after a resume from
/// elsewhere. An in-memory session has no file, so the resolved layout dir stands in.
pub(in crate::builder) fn session_dir_of(ctx: &BuildCtx, tree: &SessionTree) -> PathBuf {
    let cfg = &ctx.cfg;
    let manager = &tree.manager;
    let layout = &tree.layout;
    match &cfg.session_dir {
        Some(dir) => dir.clone(),
        None => manager
            .session_file()
            .and_then(std::path::Path::parent)
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| layout.dir()),
    }
}

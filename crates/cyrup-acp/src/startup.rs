//! The markdown startup prelude and the modelless banner that precedes it (`ACP-017`,
//! `ACP-066`, `ACP-068`, `ACP-081`).
//!
//! Port of pi-acp v0.0.33 `agent.ts`'s `buildStartupInfo` and `session.ts`'s
//! `sendStartupInfoIfPending` @v0.0.33 — the inventory a client is shown once, immediately after
//! `session/new`, so a user can see which context files, skills, prompt templates, extensions and
//! themes this session actually loaded.
//!
//! # [CYRUP-DELTA] — the discovery half is not ported, only the rendering
//!
//! **What differs.** Upstream re-derives the inventory from the filesystem at prelude time: an
//! `AGENTS.md` probe, a recursive `SKILL.md` walk under three roots, two `readdirSync`s, and a
//! re-read of both `settings.json` `packages` arrays, each call individually try/caught. Every one
//! of those answers a question the session has already answered — `services.context.snapshot()`,
//! `services.resources.{skills,prompts,themes}.all()` and `services.ext_host.loaded_ids()` are the
//! resolved, de-duplicated, conflict-arbitrated registries the session is actually running with.
//! [`StartupInventory::of`] reads those and nothing else, so there is no second scan and no way for
//! the prelude to disagree with the session.
//!
//! **What it costs, in three named behaviour changes.**
//!
//! 1. Items are **names**, not absolute paths — which is what pi's own `showLoadedResources` does,
//!    so this moves toward pi rather than away from it. A user who wants the path of a shadowed
//!    skill reads the conflict block instead.
//! 2. Duplicates are **resolved**, not double-listed: upstream's walk is unsorted and
//!    undeduplicated, so a skill reachable from two roots is printed twice. Here the registry has
//!    already picked a winner.
//! 3. The four **diagnostic blocks** are rendered, which pi-acp cannot do at all. pi shows them
//!    even under `quietStartup`, and this does too — see [`StartupInventory::of`].
//!
//! # `ACP-Q15`, decided — the inventory is projected here, and `cyrup-acp` does not depend on
//! `cyrup-tui`
//!
//! `cyrup::interactive::build_startup_report` builds the same shape as a
//! `cyrup_tui::StartupReport`, but it lives in the **bin** crate and its type lives in the TUI
//! crate, so reaching it means either moving two items across three crates or putting a `ratatui`
//! graph behind the ACP adapter for one struct of `Vec<String>`s.
//!
//! **What it costs.** The projection — which registry each block reads — exists twice, here and in
//! `cyrup::interactive::build_startup_report`, and the two can drift: a sixth block added there
//! would not appear here. The mitigation is that both read the same public accessors on
//! [`AgentSessionServices`](cyrup_session_svc::AgentSessionServices) and this doc names the
//! sibling, so a reader has one place to look. What is deliberately **not** duplicated is the
//! diagnostic *formatting*: `cyrup_tui::build_startup_lines`'s collision grouping is a terminal
//! rendering, and markdown gets its own, simpler one.
//!
//! # `ACP-068` — the ordering, and the guarantee it rests on
//!
//! **[CYRUP-DELTA] the `setTimeout(…, 0)` is cut; the ordering it bought is not.** Upstream needs
//! a timer because in TypeScript the response is emitted by *returning*, so a notification sent
//! before the return would reach the client first. Here `Responder::respond` is a synchronous
//! `send_fn` that serialises and enqueues on the connection's outgoing channel, and
//! `ConnectionTo::send_notification` enqueues on that same channel — so responding and then
//! notifying **from the same task** is deterministic with no timer and no race.
//!
//! **That guarantee is the whole reason this is correct**, and a refactor that answers
//! `session/new` from a different task than the one that sends its follow-ups breaks it silently.
//! The prelude therefore rides [`crate::HandlerOutcome::follow_up`], which is the type that makes
//! "respond, then notify, on one task" the only expressible shape; upstream's `startupInfoSent`
//! flag has no counterpart because a `follow_up` vector is consumed once by construction.
//!
//! `ACP-Q16`, decided: the prelude is carried **only** as a chunk, never also in
//! `_meta.piAcp.startupInfo`. Upstream carries both and accepted that a client rendering both
//! shows it twice; `ACP-065` already ruled that a second source of truth in `_meta` is a second
//! thing that can disagree, and the same reasoning applies here.

use agent_client_protocol::schema::v1::{ContentBlock, ContentChunk, SessionUpdate, TextContent};
use cyrup_session_svc::AgentSession;

/// One `## Heading` block of the prelude: a title and the lines under it.
///
/// A block with no items renders nothing at all — upstream's `if (items.length)` guard, kept
/// because an empty `## Skills` heading tells the user less than its absence does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartupBlock {
    /// The markdown heading text, without the `## `.
    pub title: String,
    /// One `- ` bullet each, in the order the registry returned them.
    pub items: Vec<String>,
}

impl StartupBlock {
    fn new(title: &str, items: Vec<String>) -> Self {
        Self {
            title: title.to_string(),
            items,
        }
    }
}

/// What this session loaded, as the prelude will render it (`ACP-066`).
///
/// Split from [`render_markdown`] so the renderer is pure and table-testable against a fixture —
/// which is what `ACP-066`'s *Verify* line asks for, and the only way to assert the `quiet_startup`
/// branch without building a session.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StartupInventory {
    /// `!quiet_startup`. When false the inventory blocks are suppressed and the diagnostics are
    /// not — pi's `showDiagnosticsWhenQuiet: true`. See [`StartupInventory::of`] for why the
    /// `verbose ||` half of `StartupReport::show_listing()` is not read here.
    pub show_listing: bool,
    /// Context, Skills, Prompts, Extensions, Themes — pi's order.
    pub listing: Vec<StartupBlock>,
    /// Skill conflicts, Prompt conflicts, Extension issues, Theme conflicts.
    pub diagnostics: Vec<StartupBlock>,
}

impl StartupInventory {
    /// Read the live registries. No filesystem access; see the module doc.
    #[must_use]
    pub fn of(session: &AgentSession) -> Self {
        use cyrup_resources::ResourceKind;

        let services = session.services();
        let snapshot = services.context.snapshot();

        // pi's order: Context, Skills, Prompts, Extensions, Themes
        // (`interactive-mode.ts:1550-1638`).
        //
        // Context files keep their load order — it is meaningful, and pi passes `{sort: false}`
        // for exactly this list. The other four are sorted, as `cyrup_tui::push_listing` sorts
        // them, so two runs of the same project render identically.
        let mut skills: Vec<String> = services
            .resources
            .skills
            .all()
            .iter()
            .map(|s| s.name.clone())
            .collect();
        skills.sort_unstable();
        let mut prompts: Vec<String> = services
            .resources
            .prompts
            .all()
            .iter()
            .map(|p| format!("/{}", p.name))
            .collect();
        prompts.sort_unstable();
        let mut extensions: Vec<String> = services
            .ext_host
            .loaded_ids()
            .iter()
            .map(ToString::to_string)
            .collect();
        extensions.sort_unstable();
        // `ACP-066`, decided: themes ARE included. They are available here, upstream excludes them
        // only because its own probe could not see them, and silently dropping a block the session
        // knows about is the worse default. Built-ins are filtered out, as pi filters on
        // `t.sourcePath`.
        let mut themes: Vec<String> = services
            .resources
            .themes
            .all()
            .iter()
            .filter(|t| t.origin_path.is_some())
            .map(|t| t.data.name.clone())
            .collect();
        themes.sort_unstable();

        let listing = vec![
            StartupBlock::new(
                "Context",
                snapshot
                    .context_files
                    .iter()
                    .map(|f| {
                        // The name, not the absolute path — delta (1) in the module doc. The file
                        // name alone is ambiguous between two `AGENTS.md`s, so the cwd-relative
                        // form is preferred and the full path is the fallback.
                        cyrup_tools::path::cwd_relative_path(&f.path, &services.cwd).map_or_else(
                            || f.path.display().to_string(),
                            |rel| rel.display().to_string(),
                        )
                    })
                    .collect(),
            ),
            StartupBlock::new("Skills", skills),
            StartupBlock::new("Prompts", prompts),
            StartupBlock::new("Extensions", extensions),
            StartupBlock::new("Themes", themes),
        ];

        let resource_block = |title: &str, kind: ResourceKind| {
            StartupBlock::new(
                title,
                services
                    .startup_diagnostics
                    .resources
                    .iter()
                    .filter(|d| d.resource_type == kind)
                    .map(|d| format!("{} ({})", d.message, d.path.display()))
                    .collect(),
            )
        };
        let diagnostics = vec![
            resource_block("Skill conflicts", ResourceKind::Skill),
            resource_block("Prompt conflicts", ResourceKind::Prompt),
            StartupBlock::new(
                "Extension issues",
                services
                    .startup_diagnostics
                    .extensions
                    .iter()
                    .map(|d| format!("{} ({})", d.error, d.path.display()))
                    .collect(),
            ),
            resource_block("Theme conflicts", ResourceKind::Theme),
        ];

        Self {
            // `StartupReport::show_listing()` is `verbose || !quiet_startup`. **[CYRUP-DELTA]**
            // the `verbose` half is dropped: `--verbose` is a CLI flag the TUI's own front-end
            // holds, not a setting, and nothing plumbs it to the ACP host. *What it costs*: a
            // user who sets `quietStartup` and then launches their editor cannot re-enable the
            // inventory for one session the way `cyrup --verbose` does in the terminal. The
            // diagnostics are unaffected — they are never suppressed.
            show_listing: !services.settings.effective().quiet_startup(),
            listing,
            diagnostics,
        }
    }
}

/// Render the prelude, or `None` when there is nothing to say (`ACP-066`, `ACP-081`).
///
/// # `ACP-081`, decided — an all-whitespace prelude is suppressed
///
/// **[CYRUP-DELTA]** upstream's `join('\n').trim() + '\n'` can never return the empty string: a
/// project with nothing to report yields the degenerate single-newline chunk, and pi-acp sends it.
/// *What it costs*: a client that renders every `agent_message_chunk` as a transcript entry shows
/// a blank one at the top of every session in a bare directory, and the parity it buys is a chunk
/// whose content is one whitespace character. `None` here means no notification at all.
#[must_use]
pub fn render_markdown(inventory: &StartupInventory) -> Option<String> {
    let mut sections: Vec<String> = Vec::new();

    if inventory.show_listing {
        sections.extend(inventory.listing.iter().filter_map(render_block));
    }
    // Diagnostics are rendered even under `quiet_startup` — pi's `showDiagnosticsWhenQuiet: true`
    // (`interactive-mode.ts:1769`), and the one behaviour pi-acp could not offer at all.
    sections.extend(inventory.diagnostics.iter().filter_map(render_block));

    if sections.is_empty() {
        return None;
    }
    Some(sections.join("\n\n"))
}

/// One block, or `None` when it has no items to show.
fn render_block(block: &StartupBlock) -> Option<String> {
    let items: Vec<&str> = block
        .items
        .iter()
        .map(|i| i.trim())
        .filter(|i| !i.is_empty())
        .collect();
    if items.is_empty() {
        return None;
    }
    let mut out = format!("## {}", block.title);
    for item in items {
        out.push_str("\n- ");
        out.push_str(item);
    }
    Some(out)
}

/// The `Warning: ` prefix the modelless banner carries (`ACP-017`).
///
/// The **same** string the sibling terminal front-end prefixes it with
/// (`crates/cyrup/src/interactive.rs`'s `push_warning(format!("Warning: {msg}"))`), spelled once
/// here so the two front-ends cannot drift in wording. It is deliberately *not* markdown-emphasised:
/// the prelude's other chunk is markdown, but this one is a sentence a user must be able to read in
/// a client that renders `agent_message_chunk` as plain text, and `**Warning:**` in such a client is
/// noise around the only instruction it carries.
pub const MODEL_FALLBACK_PREFIX: &str = "Warning: ";

/// One `agent_message_chunk` carrying `text`.
fn text_chunk(text: String) -> SessionUpdate {
    SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
        text,
    ))))
}

/// The updates `session/new` sends after its response, in wire order (`ACP-017`, `ACP-066`,
/// `ACP-068`, `ACP-081`).
///
/// Split from [`startup_prelude`] for the same reason [`render_markdown`] is: it is the ordering
/// rule, and it must be assertable without building an [`AgentSession`].
///
/// # `ACP-017` — the modelless session says so in-band, and says it **first**
///
/// `ACP-Q7` decided that a session with no resolvable model is **not** refused at `session/new`
/// (`crates/cyrup-acp/src/sessions.rs`'s `decorate_new_session`), where pi-acp's
/// `rawModelsCount === 0` branch cleans up and answers `auth-required`
/// (`src/acp/agent.ts:330-340` @v0.0.33). That answer is only half of a working behaviour: the
/// verify for it is two-part — **no `model` config option** *and* **a first `session/update`
/// carrying the fallback text** — and only the first half was written, so a credential-less first
/// run in Zed got a `session/new` that looked entirely successful and learned otherwise one round
/// trip later, from `session/prompt`'s `No model selected`.
///
/// `AgentSession::model_fallback_message()` is the message the session already built at
/// resolve time (`cyrup_session_svc`'s `resolve_model`): with an empty catalog it is
/// `format_no_models_available_message()` — *"No models available. Use /login to log into a
/// provider…"* — and on a resumed session whose saved model has gone it is
/// *"Could not restore model p/m. Using x/y"*. Both are worth saying; the terminal front-end says
/// both, from the same accessor, and this is the ACP host reaching the same value.
///
/// **It is emitted ahead of the inventory** because it is the actionable half: a client that
/// renders only the first chunk of a burst must render the instruction, not the skill list.
///
/// # [CYRUP-DELTA] — `quiet_startup` does not suppress it
///
/// **What differs.** `quiet_startup` is [`StartupInventory::show_listing`]'s gate and it is
/// upstream's `getQuietStartup` (`src/acp/pi-settings.ts` @v0.0.33), which gates the *whole* pi-acp
/// prelude. Here it gates the inventory only — the diagnostics already ignore it (pi's
/// `showDiagnosticsWhenQuiet: true`) and this banner ignores it too, because a session that cannot
/// run is not startup verbosity.
///
/// **What it costs.** A user who set `quietStartup` still gets one line on a credential-less
/// launch. That is the line that tells them what to do about it, and suppressing it would leave
/// `--terminal-login` (`ACP-010`) as the only clue that anything is wrong.
#[must_use]
pub fn startup_updates(
    model_fallback: Option<&str>,
    inventory: &StartupInventory,
) -> Vec<SessionUpdate> {
    let mut out = Vec::with_capacity(2);
    // `ACP-017` — first, and before the `ACP-081` suppression can empty the burst entirely.
    out.extend(
        model_fallback
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map(|m| text_chunk(format!("{MODEL_FALLBACK_PREFIX}{m}"))),
    );
    // `ACP-081` — a project with nothing to report contributes no chunk at all.
    out.extend(render_markdown(inventory).map(text_chunk));
    out
}

/// The `session/new` follow-up chunks, ready for its `follow_up` (`ACP-017`, `ACP-066`,
/// `ACP-068`, `ACP-081`).
///
/// Empty when there is nothing to say: no model warning *and* nothing to report (`ACP-081`).
/// The caller `extend`s its `follow_up` with this, so an empty vector is "no notification at all"
/// with no branch at the call site.
#[must_use]
pub fn startup_prelude(session: &AgentSession) -> Vec<SessionUpdate> {
    startup_updates(
        session.model_fallback_message(),
        &StartupInventory::of(session),
    )
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

    fn block(title: &str, items: &[&str]) -> StartupBlock {
        StartupBlock::new(title, items.iter().map(|s| (*s).to_string()).collect())
    }

    fn fixture() -> StartupInventory {
        StartupInventory {
            show_listing: true,
            listing: vec![
                block("Context", &["AGENTS.md"]),
                block("Skills", &["pdf", "xlsx"]),
                block("Prompts", &[]),
                block("Extensions", &["intercom"]),
                block("Themes", &[]),
            ],
            diagnostics: vec![
                block(
                    "Skill conflicts",
                    &["shadowed by ~/.cyrup/skills/pdf (/proj/.cyrup/skills/pdf)"],
                ),
                block("Prompt conflicts", &[]),
                block("Extension issues", &[]),
                block("Theme conflicts", &[]),
            ],
        }
    }

    /// **ACP-066** — the exact structure, and the empty-section rule.
    #[test]
    fn the_prelude_is_headings_and_bullets_and_an_empty_section_emits_nothing() {
        let rendered = render_markdown(&fixture()).expect("something to say");
        assert_eq!(
            rendered,
            "## Context\n\
             - AGENTS.md\n\
             \n\
             ## Skills\n\
             - pdf\n\
             - xlsx\n\
             \n\
             ## Extensions\n\
             - intercom\n\
             \n\
             ## Skill conflicts\n\
             - shadowed by ~/.cyrup/skills/pdf (/proj/.cyrup/skills/pdf)"
        );
        assert!(
            !rendered.contains("## Prompts") && !rendered.contains("## Themes"),
            "an empty section emits NOTHING, not a bare heading: {rendered}"
        );
    }

    /// **ACP-066** — `quiet_startup` suppresses the inventory and never the diagnostics.
    ///
    /// pi's `showDiagnosticsWhenQuiet: true`. This is the half pi-acp cannot express at all: its
    /// only surviving `quietStartup` output was the npm update notice, which cyrup cut.
    #[test]
    fn quiet_startup_keeps_the_diagnostics_and_drops_the_listing() {
        let mut inventory = fixture();
        inventory.show_listing = false;
        let rendered = render_markdown(&inventory).expect("the diagnostics survive");
        assert_eq!(
            rendered,
            "## Skill conflicts\n- shadowed by ~/.cyrup/skills/pdf (/proj/.cyrup/skills/pdf)"
        );
    }

    /// **ACP-066** — a report with only diagnostics still renders them, listing or not.
    #[test]
    fn diagnostics_alone_are_enough_to_produce_a_prelude() {
        let inventory = StartupInventory {
            show_listing: true,
            listing: vec![block("Context", &[]), block("Skills", &[])],
            diagnostics: vec![block(
                "Extension issues",
                &["world version mismatch (/x.wasm)"],
            )],
        };
        assert_eq!(
            render_markdown(&inventory).as_deref(),
            Some("## Extension issues\n- world version mismatch (/x.wasm)")
        );
    }

    /// **ACP-081** — a project with nothing to report emits NO chunk, where upstream emits one
    /// containing a single newline.
    #[test]
    fn a_bare_project_produces_no_prelude_at_all() {
        assert_eq!(render_markdown(&StartupInventory::default()), None);
        let all_empty = StartupInventory {
            show_listing: true,
            listing: vec![block("Context", &[]), block("Skills", &["", "   "])],
            diagnostics: vec![block("Theme conflicts", &[])],
        };
        assert_eq!(
            render_markdown(&all_empty),
            None,
            "a block whose only items are whitespace is an empty block"
        );

        // …and under quiet startup, an inventory with a listing but no diagnostics is likewise
        // nothing at all: the listing is suppressed and there is nothing left.
        let quiet = StartupInventory {
            show_listing: false,
            listing: vec![block("Skills", &["pdf"])],
            diagnostics: vec![block("Skill conflicts", &[])],
        };
        assert_eq!(render_markdown(&quiet), None);
    }

    // ---- ACP-017 -------------------------------------------------------------------------------

    /// The text of an `agent_message_chunk`, or a panic naming what came instead.
    fn chunk_text(update: &SessionUpdate) -> &str {
        match update {
            SessionUpdate::AgentMessageChunk(chunk) => match &chunk.content {
                ContentBlock::Text(text) => text.text.as_str(),
                other => panic!("the prelude is text, got {other:?}"),
            },
            other => panic!("the prelude is an agent_message_chunk, got {other:?}"),
        }
    }

    /// **ACP-017** — the credential-less `session/new` says so in-band, and says it FIRST.
    ///
    /// The exact string is `AgentSession::model_fallback_message()`'s, which for an empty catalog
    /// is `cyrup_session_svc::auth_guidance::format_no_models_available_message()`. This asserts
    /// the two things `ACP-017`'s verify names that the wire can check: the banner is present, and
    /// it precedes the inventory.
    #[test]
    fn a_modelless_session_leads_with_the_model_fallback_warning() {
        let fallback = "No models available. Use /login to log into a provider via OAuth or API \
                        key. See:\n  docs/providers.md\n  docs/models.md";
        let updates = startup_updates(Some(fallback), &fixture());
        assert_eq!(
            updates.len(),
            2,
            "the banner AND the inventory: {updates:?}"
        );
        assert_eq!(chunk_text(&updates[0]), format!("Warning: {fallback}"));
        assert!(
            chunk_text(&updates[0]).contains("/login"),
            "ACP-017: the banner's whole job is to name the remedy"
        );
        assert!(
            chunk_text(&updates[1]).starts_with("## Context"),
            "ACP-017: the actionable half is FIRST, the inventory second: {updates:?}"
        );
        assert_eq!(MODEL_FALLBACK_PREFIX, "Warning: ");
    }

    /// **ACP-017** — a session whose model resolved cleanly emits the inventory and nothing else.
    ///
    /// This is the assertion that stops the banner becoming an unconditional chunk: the accessor
    /// is `None` for every session that has a model, which is the normal case.
    #[test]
    fn a_session_with_a_model_emits_no_banner() {
        let updates = startup_updates(None, &fixture());
        assert_eq!(updates.len(), 1, "the inventory alone: {updates:?}");
        assert!(chunk_text(&updates[0]).starts_with("## Context"));
    }

    /// **ACP-017** / **ACP-081** — the banner survives `quiet_startup` and an empty inventory, and
    /// a whitespace-only message is not a banner.
    ///
    /// `quiet_startup` gates the inventory (`StartupInventory::show_listing`), not this: see
    /// [`startup_updates`]'s CYRUP-DELTA. A modelless session in a bare project therefore still
    /// gets exactly one chunk, where `ACP-081` alone would have sent none.
    #[test]
    fn the_banner_is_not_startup_verbosity_and_is_not_whitespace() {
        let quiet_and_bare = StartupInventory {
            show_listing: false,
            listing: vec![block("Skills", &["pdf"])],
            diagnostics: vec![],
        };
        assert_eq!(render_markdown(&quiet_and_bare), None);
        let updates = startup_updates(
            Some("Could not restore model a/b. Using c/d"),
            &quiet_and_bare,
        );
        assert_eq!(
            updates.len(),
            1,
            "ACP-017: quiet_startup suppresses the listing, never the banner: {updates:?}"
        );
        assert_eq!(
            chunk_text(&updates[0]),
            "Warning: Could not restore model a/b. Using c/d",
            "the resumed-session fallback is carried too, not just the modelless one"
        );

        // A message that is only whitespace is no message: `ACP-081`'s rule, applied to the banner
        // so an empty accessor value cannot produce a blank transcript entry.
        assert!(startup_updates(Some("   \n"), &StartupInventory::default()).is_empty());
        assert!(startup_updates(None, &StartupInventory::default()).is_empty());
    }
}

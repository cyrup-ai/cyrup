use super::*;

impl TranscriptView {
    /// Record a status / notification line.
    pub fn push_status(&mut self, text: impl Into<String>) {
        self.pending.push(Entry::Status(text.into()));
    }

    /// Record pi's accent swap receipt (`handleClearCommand`, `interactive-mode.ts:6316-6329`) —
    /// `/new`'s `✓ New session started`. Distinct from [`push_status`](Self::push_status): the
    /// accent colour and the trailing blank ([`Entry::Receipt`]).
    pub fn push_receipt(&mut self, text: impl Into<String>) {
        self.pending.push(Entry::Receipt(text.into()));
    }

    /// Push the startup loaded-resources / diagnostics panel (Pi `showLoadedResources`,
    /// interactive-mode.ts:1480-1690). No-op when there is nothing to show — a `quietStartup` boot
    /// with no problems prints nothing at all, exactly like Pi.
    pub fn push_loaded_resources(&mut self, lines: Vec<crate::startup::StartupLine>) {
        if lines.is_empty() {
            return;
        }
        self.pending.push(Entry::LoadedResources(lines));
    }

    /// Record an `error`-styled notice line — the incomplete/failed-turn footer Pi appends to an
    /// assistant message (`assistant-message.ts:177-201`). Distinct from
    /// [`push_status`](Self::push_status), which is dim and bulleted.
    pub fn push_error(&mut self, text: impl Into<String>) {
        self.pending.push(Entry::Error(text.into()));
    }

    /// Record a `warning`-styled notice line (Pi `showWarning`, `interactive-mode.ts:3956-3960`).
    pub fn push_warning(&mut self, text: impl Into<String>) {
        self.pending.push(Entry::Warning(text.into()));
    }

    /// Push a bordered info block (`/hotkeys`, `/changelog`, `/session`, `/debug`).
    pub fn push_block(&mut self, title: impl Into<String>, markdown: impl Into<String>) {
        self.pending.push(Entry::Block { title: title.into(), markdown: markdown.into() });
    }

    /// The startup "packages are out of date" notice — Pi `showPackageUpdateNotification`
    /// (`interactive-mode.ts:3920-3936`), pushed when the detached package-update check settles with
    /// a non-empty list (`:850-856`).
    ///
    /// Upstream's block is a `DynamicBorder`, a bold title, the instruction, `Packages:` and one
    /// `- name` line per package, then a closing border — structurally [`Entry::Block`], which is the
    /// same border/title/body sandwich (interactive-mode.ts:5502-5507). `[CYRUP-DELTA]`: upstream
    /// tints THIS block's border and title `warning` where the generic block is `accent`; cyrup
    /// reuses the generic block rather than forking the entry type for a colour.
    ///
    /// The action names cyrup's own command, `cyrup update --extensions` (`subcommands.rs`), which is
    /// upstream's `${APP_NAME} update --extensions` after the rebrand. A no-op on an empty list, so
    /// the caller never has to guard.
    pub fn push_package_updates(&mut self, packages: &[String]) {
        if packages.is_empty() {
            return;
        }
        let list = packages
            .iter()
            .map(|p| format!("- {p}"))
            .collect::<Vec<_>>()
            .join("\n");
        self.push_block(
            "Package Updates Available",
            format!(
                "Package updates are available. Run {} update --extensions\nPackages:\n{list}",
                crate::resume_hint::APP_NAME
            ),
        );
    }

    /// Push a skill-invocation message (`skill-invocation-message.ts`): a `[skill]` label + the skill
    /// name header, with the skill block content rendered as markdown.
    ///
    /// Upstream only ever builds this component inside `case "user"` (`interactive-mode.ts:3506`),
    /// so it takes the same `:3500` leading-spacer gate [`push_user`](Self::push_user) applies.
    pub fn push_skill_invocation(&mut self, name: impl Into<String>, content: impl Into<String>) {
        let lead_spacer = self.chat_has_children();
        self.pending.push(Entry::SkillInvocation {
            name: name.into(),
            content: content.into(),
            lead_spacer,
        });
    }

    /// Push a custom (extension) message (`custom-message.ts`): a bracketed type `label` + a markdown
    /// `body`.
    pub fn push_custom_message(&mut self, label: impl Into<String>, body: impl Into<String>) {
        self.pending.push(Entry::Custom {
            label: label.into(),
            body: body.into(),
            rendered: Rendered::None,
        });
    }

    /// [`Self::push_custom_message`] with the text an extension's registered message renderer
    /// produced for this custom type (EXT-006; Pi resolves the renderer at
    /// `interactive-mode.ts:3326` — `extensionRunner.getMessageRenderer(message.customType)` — and
    /// hands it to `CustomMessageComponent` INSTEAD of the default framing). When `rendered` is
    /// [`Rendered::Text`], the extension's lines are emitted verbatim: no `[label]` bracket, no
    /// markdown re-wrap, because the renderer already decided how the block looks;
    /// [`Rendered::Failed`] draws Pi's renderer-failure box (X15).
    pub fn push_custom_message_rendered(
        &mut self,
        label: impl Into<String>,
        body: impl Into<String>,
        rendered: Rendered,
    ) {
        self.pending.push(Entry::Custom {
            label: label.into(),
            body: body.into(),
            rendered,
        });
    }

    /// Push a branch-summary message (`branch-summary-message.ts`): the `**Branch Summary**` body
    /// produced when navigating away from / abandoning a branch.
    ///
    /// X14 — the collapsed/expanded choice is `component.setExpanded(this.toolOutputExpanded)`
    /// (`interactive-mode.ts:3493`) and is re-broadcast to every child on every toggle
    /// (`setToolsExpanded`, `:4032-4046`), so it is resolved at RENDER time from
    /// [`ImageOpts::tools_expanded`], never captured here.
    pub fn push_branch_summary(&mut self, summary: impl Into<String>) {
        self.pending.push(Entry::BranchSummary { summary: summary.into() });
    }

    /// Push a compaction-summary message (`compaction-summary-message.ts`): the pre-compaction token
    /// count + the `**Compacted from N tokens**` summary body.
    pub fn push_compaction_summary(&mut self, tokens_before: u64, summary: impl Into<String>) {
        // X14 — `interactive-mode.ts:3486`'s `setExpanded(this.toolOutputExpanded)`; like the branch
        // summary above, resolved at render time from the LIVE flag.
        self.pending.push(Entry::CompactionSummary { tokens_before, summary: summary.into() });
    }
}

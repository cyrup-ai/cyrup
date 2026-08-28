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

    /// pi `addCacheMissNotice` (`interactive-mode.ts:3828-3842` @v0.83.0) — the in-transcript
    /// warning that a turn silently re-billed a prompt prefix that should have been a cache read.
    ///
    /// ```ts
    /// if (miss.missedTokens < 20_000 && miss.missedCost < 0.1) return;
    /// const cost = miss.missedCost >= 0.01 ? ` (~$${miss.missedCost.toFixed(2)})` : "";
    /// let label = "Cache miss";
    /// if (miss.modelChanged) label = "Cache miss after model switch";
    /// else if (miss.idleMs >= CACHE_TTL_MS) label = `Cache miss after ${Math.round(miss.idleMs / 60_000)}m idle`;
    /// this.chatContainer.addChild(new Spacer(1));
    /// this.chatContainer.addChild(new Text(theme.fg("warning", `${label}: ${reBilled}`), 1, 0));
    /// ```
    ///
    /// The suppression floor is `&&`, so the notice shows when EITHER threshold is met.
    ///
    /// **Do not substitute [`cyrup_provider::cache_stats::CacheMiss::exceeded_ttl`]
    /// (`cache_stats.rs:70-72`) for the idle test.** That helper is `idle_ms > CACHE_TTL_MS` and
    /// belongs to a different question ("did the cache certainly expire"); upstream's label branch
    /// is `>=` (`interactive-mode.ts:3836`), and at exactly the TTL the two disagree.
    ///
    /// `Entry::Warning` already renders as `Spacer(1)` + one warning-coloured row
    /// (`transcript/render.rs`'s `Entry::Warning` arm), which is pi's two `addChild` calls — so
    /// there is no separate blank to push, and unlike `showWarning` this path carries no
    /// `Warning: ` prefix, exactly as upstream's raw `Text` does not.
    pub fn push_cache_miss_notice(&mut self, miss: &cyrup_provider::cache_stats::CacheMiss) {
        // `:3829` — below BOTH thresholds is breakpoint noise, not a story worth telling.
        if miss.missed_tokens < 20_000 && miss.missed_cost < 0.1 {
            return;
        }
        let cost = cost_suffix(miss.missed_cost);
        let label = if miss.model_changed {
            // `:3833-3834`
            "Cache miss after model switch".to_string()
        } else if miss.idle_ms >= cyrup_provider::cache_stats::CACHE_TTL_MS {
            // `:3835-3836` — `Math.round`, which for a non-negative `idle_ms` (the detector clamps
            // it at `cache_stats.rs:176-179`) is `f64::round`. The `as i64` is a saturating cast,
            // so it renders an integer count of minutes without a fallible conversion.
            format!("Cache miss after {}m idle", (miss.idle_ms as f64 / 60_000.0).round() as i64)
        } else {
            "Cache miss".to_string()
        };
        self.push_warning(format!(
            "{label}: {} tokens re-billed{cost}",
            crate::status::format_tokens(miss.missed_tokens)
        ));
    }

    /// pi `addCompactionCostNotice` (`interactive-mode.ts:3802-3814` @v0.83.0) — what a compaction
    /// or a branch summarization cost, attributed at the point it happened instead of only landing
    /// unlabelled in the footer's cumulative `$`.
    ///
    /// ```ts
    /// const tokens = usage.input + usage.output + usage.cacheRead + usage.cacheWrite;
    /// const cost = usage.cost.total >= 0.01 ? ` (~$${usage.cost.total.toFixed(2)})` : "";
    /// const label = notice.kind === "compaction" ? "Compaction" : "Branch summary";
    /// `${label}: ${formatTokens(tokens)} tokens billed${cost}`
    /// ```
    ///
    /// The sum is `saturating_add`ed rather than `+`ed: `Usage`'s four counters are `u64` read off
    /// a provider response, and a debug overflow panic here would be a crash in a cost notice.
    pub fn push_compaction_cost_notice(
        &mut self,
        kind: crate::transcript::CompactionCostKind,
        usage: &cyrup_core::Usage,
    ) {
        let tokens = usage
            .input
            .saturating_add(usage.output)
            .saturating_add(usage.cache_read)
            .saturating_add(usage.cache_write);
        let cost = cost_suffix(usage.cost.total);
        self.push_warning(format!(
            "{}: {} tokens billed{cost}",
            kind.label(),
            crate::status::format_tokens(tokens)
        ));
    }
}

/// The ` (~$x.xx)` tail both notices append, and the `>= 0.01` gate that suppresses it — pi writes
/// the same expression twice (`interactive-mode.ts:3806` and `:3831`), once per notice.
fn cost_suffix(dollars: f64) -> String {
    if dollars >= 0.01 {
        format!(" (~${dollars:.2})")
    } else {
        String::new()
    }
}

use super::*;

pub(crate) fn entry_lines(
    entry: &Entry,
    theme: &UiTheme,
    width: usize,
    output_pad: usize,
    images: ImageOpts<'_>,
) -> Vec<Line<'static>> {
    match entry {
        Entry::User { text, lead_spacer } => {
            // `UserMessageComponent` (`user-message.ts:38-58`) is exactly one child: a
            // `new Box(this.outputPad, 1, (c) => theme.bg("userMessageBg", c))` wrapping
            // `new Markdown(text, 0, 0, …, { color: userMessageText })`. The body is the block's
            // only content — there is **no** role label anywhere in the component (X1), and the
            // `userMessageBg` fill plus the `Box`'s `paddingY = 1` tinted blank above and below are
            // what identify a user turn (L1). The OSC-133 shell-zone markers at `:66-67` are
            // terminal shell-integration escapes the ratatui cell grid cannot carry.
            //
            // Content width is the `Box`'s `contentWidth = width - outputPad * 2` (`box.ts:79`),
            // which the inner `Markdown(…, 0, 0)` then passes through unchanged (`markdown.ts:284`
            // with `paddingX = 0`) — not `width - 5` (M9: that 5 was the width of the deleted
            // `"you: "` label).
            let role = theme.user_message_bg_style();
            // `user-message.ts:53` wraps the body in
            // `createMarkdownTransform("user", false, this.markdownTransformers)` — messageType
            // `"user"`, `isStreaming` hard-coded `false` (a user turn is never streamed).
            let md = crate::markdown::render_message(
                text,
                width.saturating_sub(output_pad * 2).max(1),
                theme,
                role.fg,
                crate::markdown::MermaidContext::new(
                    images.mermaid,
                    crate::markdown::MessageType::User,
                    false,
                ),
            );
            // `applyBackgroundToLine` paints the BACKGROUND only (`box.ts:132-134`).
            let fill = match role.bg {
                Some(bg) => Style::default().bg(bg),
                None => Style::default(),
            };
            let mut out = box_lines(md, width, output_pad, 1, fill);
            // `chatContainer.addChild(new Spacer(1))` before the component — but `:3500` GATES it on
            // `this.chatContainer.children.length > 0`, so the very first thing in a fresh session's
            // chat gets no leading blank (`lead_spacer`, frozen at push time by
            // [`TranscriptView::chat_has_children`]). `:3514`, the user message that trails a skill
            // block, is UNgated and always passes `true`.
            //
            // The blank is also skipped when the component itself rendered nothing: `box_lines`
            // returns `[]` for an empty child set (`box.ts:75-77`/`:91-93`), and upstream never
            // reaches either the spacer or the component in that case because `:3499`'s
            // `if (textContent)` has already skipped the whole `case "user"`.
            if *lead_spacer && !out.is_empty() {
                out.insert(0, Line::default());
            }
            out
        }
        Entry::Assistant(text) => {
            // L3 — `assistant-message.ts:107` renders a text block only when `content.text.trim()`
            // is truthy, and `:96-98` counts it toward `hasVisibleContent` by the same trimmed test.
            // A whitespace-only message is therefore neither body nor blank.
            if text.trim().is_empty() {
                return Vec::new();
            }
            // `assistant-message.ts:104-114`: the body is `new Markdown(content.text.trim(),
            // this.outputPad, 0, …)` and nothing else — no role label (X1). `contentWidth =
            // width - outputPad * 2` (`markdown.ts:284`), not `width - 11` (M9: the 11 was
            // `"assistant: "`).
            // `assistant-message.ts:112` passes `createMarkdownTransform("assistant",
            // this.isStreaming, …)`. A COMMITTED entry is by definition not streaming — the turn
            // has ended — so this is the `final`-mode leg; the live partial is the one
            // `is_streaming: true` site (`transcript/cache.rs`).
            let mut md = crate::markdown::render_message(
                text,
                width.saturating_sub(output_pad * 2).max(1),
                theme,
                None,
                crate::markdown::MermaidContext::new(
                    images.mermaid,
                    crate::markdown::MessageType::Assistant,
                    false,
                ),
            );
            if md.is_empty() {
                md.push(Line::default());
            }
            // The `outputPad` horizontal padding (Pi `Markdown(content, outputPad, 0)`).
            pad_lines(&mut md, output_pad);
            // L3 — `assistant-message.ts:100-102`:
            // `if (hasVisibleContent) { this.contentContainer.addChild(new Spacer(1)); }`.
            // Gated above by the same trimmed predicate `hasVisibleContent` uses (`:96-98`), so the
            // blank cannot outlive its content. [`TranscriptView::commit_assistant`] refuses to
            // commit a whitespace-only turn for the same reason; the check is repeated here because
            // `Entry::Assistant` is public and reachable without going through it.
            md.insert(0, Line::default());
            md
        }
        Entry::Thinking { text, hidden } => {
            // The reasoning section (`assistant-message.ts:139-165`), padded like every other
            // assistant-side block. `hidden` was frozen at commit time (see [`Entry::Thinking`]).
            let mut out = thinking_lines(
                text,
                *hidden,
                width.saturating_sub(output_pad * 2),
                theme,
                images.hidden_thinking_label.unwrap_or(HIDDEN_THINKING_LABEL),
            );
            if out.is_empty() {
                return out;
            }
            pad_lines(&mut out, output_pad);
            // The same single `Spacer(1)` as the assistant arm (`:100-102`). Upstream renders one
            // component per assistant message, so the spacer lands on whichever visible block comes
            // first; the reasoning run always precedes the answer text in the content walk, and the
            // blank BETWEEN them is upstream's `hasVisibleContentAfter` spacer (`:166-168`). Two
            // cyrup entries each carrying one leading blank therefore reproduce upstream's
            // `[blank] thinking [blank] text` exactly.
            out.insert(0, Line::default());
            out
        }
        Entry::Tool(run) => {
            // X14 — a committed tool renders at the LIVE `this.toolOutputExpanded`, exactly like a
            // live one. Upstream has ONE `ToolExecutionComponent` per call and never swaps its
            // expansion when it scrolls: the component is seeded `setExpanded(this.toolOutputExpanded)`
            // at every construction site (`interactive-mode.ts:3165`, `:3239`, `:3437`, `:3486`,
            // `:3602`) and re-broadcast on every toggle (`setToolsExpanded`, `:4032-4046`), with
            // `toolOutputExpanded` defaulting to **false** (`:442`).
            //
            // This used to pass a hardcoded `true`, "so finalized scrollback keeps the complete
            // record". That is the GREEN-SLAB defect: a collapsed `read`'s `renderResult` returns
            // `""` upstream (`read.ts:178-180`) and `bash`/`grep`/`ls` cap at 10 rows, so upstream's
            // committed block is a 3-row header. Forcing `true` dumped the WHOLE file — every line
            // of it — inside the full-width `toolSuccessBg` box, so one `read` of a 500-line file
            // painted 500 rows of solid tool tint (Indexed(22), a vivid `#005f00`, once a
            // 256-colour terminal quantises `#283228`) straight over the conversation.
            tool_lines(run, images.tools_expanded, width, theme, images)
        }
        Entry::Bash(b) => {
            // Same rule for the `!`/`!!` block: `BashExecutionComponent` is `isExpandable` and takes
            // the same broadcast (`setToolsExpanded`, `:4032-4046`), so a committed one renders at
            // the live flag rather than force-expanded.
            let mut full = b.clone();
            full.set_expanded(images.tools_expanded);
            full.render_lines(width, theme, None, None)
        }
        Entry::SkillInvocation { name, content, lead_spacer } => {
            // `[skill]` label + bold name header, full content as markdown (the committed/expanded
            // form — `skill-invocation-message.ts` expanded branch). The leading spacer is the gated
            // `interactive-mode.ts:3500` one (see [`Entry::SkillInvocation`]).
            labeled_message_lines(
                "skill",
                &format!("**{name}**"),
                content,
                false,
                *lead_spacer,
                theme,
                width,
            )
        }
        Entry::Custom { label, body, rendered } => match rendered {
            // X15 — the renderer THREW. Pi does not silently drop the entry: `CustomEntryComponent`
            // catches and draws a failure box in its place (`components/custom-entry.ts:47-52`):
            //
            // ```ts
            // } catch (error) {
            //     const message = error instanceof Error ? error.message : String(error);
            //     const box = new Box(1, 1, (text) => theme.bg("customMessageBg", text));
            //     box.addChild(new Text(theme.fg("error", `[${this.entry.customType}] renderer failed: ${message}`), 0, 0));
            //     component = box;
            // }
            // ```
            //
            // — a `customMessageBg` box holding ONE `error`-coloured line, and then `:59-60`'s
            // `Spacer(1)` + the box, the same leading blank the success arm gets.
            Rendered::Failed(message) => {
                let block = theme.custom_message_bg_style();
                let fill = match block.bg {
                    Some(bg) => Style::default().bg(bg),
                    None => Style::default(),
                };
                let text = format!("[{label}] renderer failed: {message}");
                // `new Text(…, 0, 0)` inside a `Box(1, 1)`: paddingX 0, so the row wraps at the
                // box's own content width (`box.ts:79`) with no further margin.
                let children = text_lines(&text, width.saturating_sub(2).max(1), 0, theme.error_style());
                let mut out = box_lines(children, width, 1, 1, fill);
                if !out.is_empty() {
                    out.insert(0, Line::default());
                }
                out
            }
            // EXT-006: an extension registered a renderer for this custom type, so ITS output is
            // the block (Pi hands the resolved renderer to `CustomMessageComponent` in place of the
            // default framing, interactive-mode.ts:3324-3336). Emitted verbatim — the renderer
            // already owns the presentation, so no `[label]` bracket is added.
            Rendered::Text(text) => {
                // `CustomMessageComponent` adds its `Spacer(1)` in the CONSTRUCTOR
                // (`custom-message.ts:33`), before `rebuild()` chooses between the custom renderer
                // (`:79`) and the default box (`:88`), so both arms carry the leading blank.
                let mut out = vec![Line::default()];
                // X11 — `custom-message.ts:76-81` is `this.customComponent = component;
                // this.addChild(component); return;`: the component is added AS-IS and the host
                // applies no colour of its own. cyrup re-styled every row `dim`, which overrode
                // whatever the extension had chosen — the one thing `renderShell: "self"`/a custom
                // renderer exists to prevent. Rows go out unstyled so the terminal default (and any
                // styling the renderer expressed) survives.
                out.extend(text.split('\n').map(|l| Line::raw(l.to_string())));
                out
            }
            // The component is re-rendered on EVERY frame at the LIVE width, theme and expansion —
            // the same X14 rule `Entry::BranchSummary` and `Entry::Tool` follow. Upstream re-invokes
            // `component.render(width)` per paint, which is what makes a resize re-wrap and the
            // expand toggle open a card that was pushed collapsed.
            Rendered::Live(component) => {
                let roles = crate::theme::UiThemeRoles::new(theme);
                let ctx = cyrup_ext::RenderCtx {
                    width: width.saturating_sub(output_pad * 2).max(1),
                    expanded: images.tools_expanded,
                    theme: &roles,
                };
                let rows = component.render(&ctx);
                // `custom-message.ts:33` — the constructor `Spacer(1)`, on every arm.
                let mut out = vec![Line::default()];
                out.extend(rows.iter().map(|r| crate::ansi::sgr_line(r)));
                pad_lines(&mut out, output_pad);
                out
            }
            // A bracketed extension-type label + the markdown body (`custom-message.ts`).
            // `custom-message.ts:33`'s constructor `Spacer(1)` — unconditional.
            Rendered::None => labeled_message_lines(label, "", body, true, true, theme, width),
        },
        Entry::BranchSummary { summary } => {
            // X14 — `BranchSummaryMessageComponent` is a `Box(1, 1, customMessageBg)` whose body
            // depends on `expanded`, which `interactive-mode.ts:3493` seeds from
            // `this.toolOutputExpanded` and `setToolsExpanded` re-broadcasts on every toggle
            // (`:4032-4046` walks `chatContainer.children` calling `setExpanded`), so the LIVE flag
            // is read here (`branch-summary-message.ts:11,22-25,32-56`). COLLAPSED it is one
            // row, not the whole summary:
            //
            // ```ts
            // theme.fg("customMessageText", "Branch summary (") +
            //     theme.fg("dim", keyText("app.tools.expand")) +
            //     theme.fg("customMessageText", " to expand)")
            // ```
            //
            // Note the two-tone split is `customMessageText`/`dim`, NOT `keyHint`'s `muted`/`dim`.
            // `interactive-mode.ts:3491` is UNgated — the branch summary always gets its blank.
            if images.tools_expanded {
                labeled_message_lines(
                    "branch",
                    "**Branch Summary**",
                    summary,
                    true,
                    true,
                    theme,
                    width,
                )
            } else {
                collapsed_summary_lines("branch", "Branch summary (", images.expand_key, theme, width)
            }
        }
        Entry::CompactionSummary { tokens_before, summary } => {
            // X14 — the same collapsed form (and the same LIVE `toolOutputExpanded` read), with the
            // token count in the lead (`compaction-summary-message.ts:48-56`):
            // `fg("customMessageText", `Compacted from ${tokenStr} tokens (`) + fg("dim", keyText(…)) + fg("customMessageText", " to expand)")`.
            if !images.tools_expanded {
                let lead =
                    format!("Compacted from {} tokens (", group_thousands(*tokens_before));
                return collapsed_summary_lines(
                    "compaction",
                    &lead,
                    images.expand_key,
                    theme,
                    width,
                );
            }
            let header = format!("**Compacted from {} tokens**", group_thousands(*tokens_before));
            // `interactive-mode.ts:3484` is UNgated too.
            labeled_message_lines("compaction", &header, summary, true, true, theme, width)
        }
        Entry::Status(text) => {
            // X18 — `showStatus` (`interactive-mode.ts:3411-3429`) is two chat children and nothing
            // more:
            //
            // ```ts
            // const spacer = new Spacer(1);
            // const text = new Text(theme.fg("dim", message), 1, 0);
            // this.chatContainer.addChild(spacer);
            // this.chatContainer.addChild(text);
            // ```
            //
            // so a status row is a leading blank plus a `dim` `Text` at **paddingX 1** — the same
            // one-column inset every other chat child sits at (`text.ts:64`, `:70-76`). There is no
            // bullet: `git grep "•" v0.84.1 -- packages/coding-agent/src/modes/interactive` finds no
            // status glyph, and `showStatus` interpolates nothing before `message`. The `• ` prefix
            // and the flush-left placement were both cyrup inventions.
            //
            // The spacer is UNgated here — unlike `:3500`, `:3424` has no
            // `chatContainer.children.length` test.
            let mut out = vec![Line::default()];
            out.extend(text_lines(text, width, 1, theme.dim_style()));
            out
        }
        Entry::Receipt(text) => {
            // pi's `handleClearCommand` (`interactive-mode.ts:6316-6329`): `Spacer(1)` +
            // `Text(theme.fg("accent", message), paddingX=1, paddingY=1)`. `Text.render` emits
            // `paddingY` blanks ABOVE **and** below (`packages/tui/src/components/text.ts:90-98`),
            // so the rows are `["", "", " ✓ New session started", ""]` — the leading `Spacer(1)`
            // plus the `Text`'s own leading blank, the accent-styled line, then the trailing blank
            // — distinct from [`Entry::Status`]'s `["", " msg"]`.
            let mut out = vec![Line::default(), Line::default()];
            out.extend(text_lines(text, width, 1, theme.accent_style()));
            out.push(Line::default());
            out
        }
        Entry::Warning(text) => {
            // Pi `showWarning` (`interactive-mode.ts:3884-3888` @v0.83.0): `Spacer(1)` then
            // `Text(theme.fg("warning", …), 1, 0)` — the `Error` shape in the warning colour.
            //
            // TUI-062(a) — the cite used to read `:3956-3960`, which is `getAllQueuedMessages` /
            // `clearAllQueues` at that tag, not `showWarning`. Re-read at v0.83.0: `showError` is
            // `:3878-3882` and `showWarning` immediately follows at `:3884-3888`. **The backlog's
            // own proposed correction (`:3885-3889`) is also off by one** — `:3885` is the `Spacer`,
            // i.e. the first line of the BODY, and `:3889` is the blank line after the closing
            // brace.
            //
            // TUI-062(b), the design half, is unchanged and deliberate: pi builds
            // `Warning: ${warningMessage}` INSIDE `showWarning` (`:3886`), while this arm renders
            // `text` verbatim, so the prefix stays a per-caller obligation. Two callers that are
            // ports of `showWarning` supply it (`app.rs:3626`, `crates/cyrup/src/main.rs`'s
            // `modelFallbackMessage` push); the project-trust banner (`app.rs`'s
            // `render_project_trust_warning_if_needed`) correctly does NOT, because pi's banner is a
            // raw warning-coloured `Text` (`:3505`) and never goes through `showWarning`. Moving the
            // prefix in here would therefore have to be conditional, which is why it has not been.
            let mut out = vec![Line::default()];
            out.extend(text_lines(text, width, output_pad, theme.warning_style()));
            out
        }
        Entry::Error(text) => {
            // Pi: `Spacer(1)` then `Text(theme.fg("error", text), outputPad, 0)`
            // (assistant-message.ts:180, :189, :193). A `Text` WRAPS at
            // `contentWidth = width - paddingX * 2` (`text.ts:64`) and margins each produced row
            // (`:70-76`) — it does not hand one long logical line to an outer reflow. cyrup did the
            // latter, so a long error printed row 0 at column `outputPad` and every continuation row
            // at column 0, the same L2 defect the markdown body had.
            let mut out = vec![Line::default()];
            out.extend(text_lines(text, width, output_pad, theme.error_style()));
            out
        }
        Entry::Block { title, markdown } => {
            // Both upstream instances of this stack are identical — `/changelog`
            // (interactive-mode.ts:6067-6072) and `/hotkeys` (:6197-6203); `git grep -n
            // "new DynamicBorder()" v0.84.1 -- .../interactive-mode.ts` finds exactly six sites and
            // four of them are those two pairs. Each is:
            //
            //   Spacer(1) / DynamicBorder() / Text(bold(accent(title)), 1, 0) / Spacer(1) /
            //   Markdown(body, 1, 1, theme) / DynamicBorder()
            //
            // The last two constructor arguments are `(paddingX, paddingY)` — **not**
            // `(paddingX, leftMargin)`: `markdown.ts:250-260` binds the third parameter to
            // `this.paddingY`, and the left margin is derived from paddingX alone
            // (`markdown.ts:329` `leftMargin = " ".repeat(this.paddingX)`). So the body is inset by
            // ONE column on both sides (content width `width - 2`, `markdown.ts:284`) and carries one
            // blank row above AND below it (`markdown.ts:352-361`), and the title is inset by one
            // column too (`Text`'s own `paddingX`, `text.ts:60-87`). Only the two `─` rules run
            // edge to edge.
            let w = width.max(1);
            let rule = "─".repeat(w);
            let bold = theme.accent_style().add_modifier(ratatui::style::Modifier::BOLD);
            let mut out: Vec<Line<'static>> = vec![
                Line::default(),
                Line::styled(rule.clone(), theme.border_style()),
            ];
            out.extend(text_lines_of(&Line::styled(title.clone(), bold), w, 1));
            out.push(Line::default());
            // `markdown.ts:288-296` returns EARLY on blank text, before the paddingY block, so an
            // empty body contributes no rows at all — not two blanks.
            if !markdown.trim().is_empty() {
                let mut md = crate::markdown::render(markdown, w.saturating_sub(2).max(1), theme);
                pad_lines(&mut md, 1);
                out.push(Line::default());
                out.extend(md);
                out.push(Line::default());
            }
            out.push(Line::styled(rule, theme.border_style()));
            out
        }
        Entry::LoadedResources(lines) => {
            crate::startup::startup_lines(lines, theme, width.max(1), output_pad)
        }
    }
}

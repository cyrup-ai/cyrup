use super::*;

/// The shared list-selector engine (spec/tui/05 §3.2 `ListView<T>`): a [`SelectList`] body wrapped in
/// the top/bottom `DynamicBorder` chrome, with a parallel `values` vector returned on confirm and an
/// optional live-preview hook.
pub struct ListSelector {
    list: SelectList,
    /// Confirm value per row, parallel to the list items (`SelectItem.value`, e.g.
    /// `thinking-selector.ts:35` `value: level`).
    values: Vec<String>,
    /// Whether a selection move emits [`SelectorOutcome::Preview`] (theme live preview only).
    preview: bool,
    /// An optional bold title rendered between the top rule and the list (`*-selector.ts` headers).
    title: Option<String>,
    /// The live selector bindings, so the hint row names the keys the user actually has bound
    /// (`keyHint` → `keyText` → `getKeybindings().getKeys(...)`, `keybinding-hints.ts:34-44`).
    /// Defaults to the stock table; [`ListSelector::with_hints`] adopts the app's merged one, and
    /// [`Selector::handle`] refreshes it from whatever keymap actually routed the key.
    keymap: SelectKeymap,
    /// Whether to draw the keyboard-hint row — OPT-IN, see [`SelectorKind::draws_hint_row`].
    hints: bool,
    /// Whether to inset the body one column — OPT-IN, see [`SelectorKind::insets_rows`].
    inset: bool,
    /// Whether to draw the envelope's `Spacer(1)` rows — OPT-IN, see
    /// [`SelectorKind::envelope_spacers`].
    spacers: bool,
}

impl ListSelector {
    /// Build from `(value, label, description)` rows, the max visible window, and whether the selector
    /// previews on navigation. The selection preselects `selected`. Column layout is Pi's selector
    /// default `{min:12,max:32}` (`THINKING_SELECT_LIST_LAYOUT` etc.).
    fn new(
        rows: Vec<(String, String, Option<String>)>,
        max_visible: u16,
        selected: usize,
        preview: bool,
    ) -> Self {
        let mut values = Vec::with_capacity(rows.len());
        let mut items = Vec::with_capacity(rows.len());
        for (value, label, desc) in rows {
            values.push(value);
            items.push(SelectItem::new(label, desc));
        }
        let mut list = SelectList::new(items, ColumnLayout::SLASH);
        list.set_max_visible(max_visible);
        list.set_selected(selected);
        ListSelector {
            list,
            values,
            preview,
            title: None,
            keymap: SelectKeymap::default(),
            hints: false,
            inset: false,
            spacers: false,
        }
    }

    /// A data-bound selector (`model`/`session`/`tree`/… — `*-selector.ts`): build the windowed list
    /// from `(value, label, description)` rows sourced from an L5 service (model catalog, session list,
    /// branch tree), with a bold `title` header and a `no_match` empty-state line. Confirming yields the
    /// row's `value` for the run loop to apply (set model, switch branch, login…). `maxVisible = 10`
    /// matches the data selectors (`model-selector.ts:244`, `session-selector.ts`).
    pub fn data(
        kind: SelectorKind,
        rows: Vec<(String, String, Option<String>)>,
        selected: usize,
    ) -> Self {
        let empty = format!("No {} available", kind.title().to_lowercase());
        let mut values = Vec::with_capacity(rows.len());
        let mut items = Vec::with_capacity(rows.len());
        for (value, label, desc) in rows {
            values.push(value);
            items.push(SelectItem::new(label, desc));
        }
        let mut list = SelectList::new(items, ColumnLayout::SLASH).with_no_match(empty);
        list.set_max_visible(10);
        list.set_selected(selected);
        ListSelector {
            list,
            values,
            preview: false,
            title: Some(kind.title().to_string()),
            keymap: SelectKeymap::default(),
            hints: false,
            inset: false,
            spacers: false,
        }
    }

    /// A generic titled prompt (Pi `showStartupSelector`, startup-ui.ts:134-163): the pre-launch
    /// Continue/Cancel-style selector the bin mounts before the agent runtime is built (e.g. the
    /// missing-session-cwd prompt). Rows are `(value, label, description)`; confirming yields the
    /// highlighted row's value. `selected` preselects a row; `maxVisible` is the row count.
    pub fn prompt(title: String, rows: Vec<(String, String, Option<String>)>, selected: usize) -> Self {
        let count = rows.len().clamp(1, u16::MAX as usize) as u16;
        let mut selector = ListSelector::new(rows, count, selected, false);
        selector.title = Some(title);
        selector
    }

    /// **Opt in** to the keyboard-hint row, binding it to the app's live `tui.select.*` table so it
    /// names the keys the user has actually bound rather than the stock defaults (`keyHint`
    /// resolves through `keyText` on every render upstream, `keybinding-hints.ts:34-44`).
    ///
    /// Only the kinds whose pi component builds such a row may call this — see
    /// [`SelectorKind::draws_hint_row`] for the enumeration and the source lines behind it.
    /// [`Self::with_upstream_chrome`] applies it per-kind and is what callers normally want.
    #[must_use]
    pub fn with_hints(mut self, keymap: &SelectKeymap) -> Self {
        self.keymap = keymap.clone();
        self.hints = true;
        self
    }

    /// **Opt in** to the one-column row inset — see [`SelectorKind::insets_rows`].
    #[must_use]
    pub fn with_inset(mut self) -> Self {
        self.inset = true;
        self
    }

    /// **Opt in** to the envelope's `Spacer(1)` rows — see [`SelectorKind::envelope_spacers`].
    #[must_use]
    pub fn with_spacers(mut self) -> Self {
        self.spacers = true;
        self
    }

    /// The number of `Spacer(1)` rows this selector's envelope adds when it is drawing them at all:
    /// **four** with a hint row (`extension-selector.ts:45,49,62,74`), **three** without (the
    /// `oauth-selector.ts:69,74,93` subset that does not sit under a search `Input`; its fourth,
    /// `:87`, belongs to the `Input` cyrup has not ported).
    fn spacer_rows(&self) -> u16 {
        if !self.spacers {
            0
        } else if self.hints {
            4
        } else {
            3
        }
    }

    /// Apply exactly the chrome the pi component behind `kind` draws: the hint row iff
    /// [`SelectorKind::draws_hint_row`], the one-column inset iff [`SelectorKind::insets_rows`].
    ///
    /// This is the single place the per-kind decision is made. It exists because the previous batch
    /// made both a property of the shared [`ListSelector`] engine, which gave every dialog chrome
    /// that upstream draws on four of them (hint row) and six (inset) — `ThinkingSelectorComponent`
    /// is 75 lines of `DynamicBorder` + `SelectList` + `DynamicBorder` and has neither.
    #[must_use]
    pub fn with_upstream_chrome(mut self, kind: SelectorKind, keymap: &SelectKeymap) -> Self {
        if kind.draws_hint_row() {
            self = self.with_hints(keymap);
        }
        if kind.insets_rows() {
            self = self.with_inset();
        }
        if kind.envelope_spacers() {
            self = self.with_spacers();
        }
        self
    }

    /// The keyboard-hint row Pi's `ExtensionSelectorComponent` puts above the bottom border
    /// (`extension-selector.ts:63-73`): `rawKeyHint("↑↓","navigate") + "  " +
    /// keyHint("tui.select.confirm","select") + "  " + keyHint("tui.select.cancel","cancel")`,
    /// rendered as `new Text(..., 1, 0)` so it is inset one column.
    ///
    /// Each pair is two-tone — `dim` key, `muted` description (`keybinding-hints.ts:42-44`) — via
    /// [`crate::chrome::key_hint_spans`]. Keys come from [`SelectKeymap::keys_label`], which joins
    /// **all** bound keys with `/` exactly as upstream's `keyText` does, so the stock cancel hint
    /// reads `escape/ctrl+c cancel`, not just the first key.
    ///
    /// The `Spacer(1)` rows upstream places either side of this row are L4/SYS-3 and land with the
    /// rest of the dialog-envelope work; this adds the hint row itself.
    fn hint_line(&self, theme: &UiTheme) -> Line<'static> {
        let mut spans = vec![Span::raw(" ")];
        spans.extend(crate::chrome::key_hint_spans("↑↓", "navigate", theme));
        if let Some(keys) = self.keymap.keys_label(SelectAction::Confirm) {
            spans.push(Span::raw("  "));
            spans.extend(crate::chrome::key_hint_spans(&keys, "select", theme));
        }
        if let Some(keys) = self.keymap.keys_label(SelectAction::Cancel) {
            spans.push(Span::raw("  "));
            spans.extend(crate::chrome::key_hint_spans(&keys, "cancel", theme));
        }
        Line::from(spans)
    }

    /// The value of the currently-highlighted row (empty string if the list is empty — never panics).
    fn current_value(&self) -> String {
        self.values.get(self.list.selected()).cloned().unwrap_or_default()
    }

    /// Read-only access to the inner list (tests / chrome inspection).
    pub fn list(&self) -> &SelectList {
        &self.list
    }

    // ---- Pi selector constructors -----------------------------------------------------------

    /// Thinking-level picker (`thinking-selector.ts:11-55`): one row per available level with its
    /// token-estimate description, `maxVisible = levels.len()`, preselecting `current`.
    pub fn thinking(current: &str) -> Self {
        // `LEVEL_DESCRIPTIONS` (`thinking-selector.ts:11-19`), in Pi's order. Pi's `max` commit
        // (fbdd4638) renamed the `xhigh` copy from "Maximum" to "Extra-high" and gave "Maximum
        // reasoning" to the new top rung.
        const LEVELS: [(&str, &str); 7] = [
            ("off", "No reasoning"),
            ("minimal", "Very brief reasoning (~1k tokens)"),
            ("low", "Light reasoning (~2k tokens)"),
            ("medium", "Moderate reasoning (~8k tokens)"),
            ("high", "Deep reasoning (~16k tokens)"),
            ("xhigh", "Extra-high reasoning (~32k tokens)"),
            ("max", "Maximum reasoning"),
        ];
        let rows: Vec<_> = LEVELS
            .iter()
            .map(|(level, desc)| ((*level).to_string(), (*level).to_string(), Some((*desc).to_string())))
            .collect();
        let selected = LEVELS.iter().position(|(l, _)| *l == current).unwrap_or(0);
        ListSelector::new(rows, LEVELS.len().min(u16::MAX as usize) as u16, selected, false)
    }

    /// Inline-images yes/no (`show-images-selector.ts:19-31`): `maxVisible = 5`, preselecting
    /// `Yes` when currently on, else `No`.
    pub fn show_images(current: bool) -> Self {
        let rows = vec![
            ("yes".to_string(), "Yes".to_string(), Some("Show images inline in terminal".to_string())),
            ("no".to_string(), "No".to_string(), Some("Show text placeholder instead".to_string())),
        ];
        let selected = if current { 0 } else { 1 };
        ListSelector::new(rows, 5, selected, false)
    }

    /// Theme picker with live preview (`theme-selector.ts:27-56`): one row per available theme,
    /// `maxVisible = 10`, the current theme marked `(current)`, preselecting it. Navigation emits
    /// [`SelectorOutcome::Preview`] so the whole UI re-themes as the highlight moves.
    pub fn theme(current: &str) -> Self {
        let mut rows = Vec::new();
        let mut selected = 0usize;
        for (i, theme) in builtin_themes().iter().enumerate() {
            let key = theme.key.as_str().to_string();
            let is_current = key == current;
            if is_current {
                selected = i;
            }
            let desc = is_current.then(|| "(current)".to_string());
            rows.push((key.clone(), key, desc));
        }
        ListSelector::new(rows, 10, selected, true)
    }
}

impl Selector for ListSelector {
    fn desired_height(&self, width: u16) -> u16 {
        // Top `DynamicBorder` + optional (now auto-sizing, wrapped) title + list body + the hint row
        // **when this kind draws one** + bottom `DynamicBorder` (spec/tui/05 §3;
        // `extension-selector.ts:44-75`).
        let title_h = self.title.as_deref().map_or(0, |t| title_wrapped_height(t, width));
        let hint_h = u16::from(self.hints);
        self.list
            .rendered_height()
            .saturating_add(2)
            .saturating_add(hint_h)
            .saturating_add(title_h)
            .saturating_add(self.spacer_rows())
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        let title_h = self.title.as_deref().map_or(0, |t| title_wrapped_height(t, area.width));
        let hint_h = u16::from(self.hints);
        // L4/SYS-3. The envelope row order is `ExtensionSelectorComponent`'s, counted from its
        // constructor (`extension-selector.ts:44-75`): `DynamicBorder`(:44) · `Spacer`(:45) ·
        // title(:47) · `Spacer`(:49) · list(:61) · `Spacer`(:62) · hint(:63-73) · `Spacer`(:74) ·
        // `DynamicBorder`(:75). `OAuthSelectorComponent` (`oauth-selector.ts:68-96`) is the same
        // order minus the hint row (it has none) — its `:87` spacer sits under a search `Input`
        // cyrup has not ported, so `sp_after_hint` collapses to 0 there and the count is three.
        // `spacers` is per-kind (`SelectorKind::envelope_spacers`); thinking/show-images/theme are
        // border/list/border upstream and keep a zero-spacer envelope.
        //
        // Every height below is the NATURAL one — `sp` does not depend on `area.height`, and the
        // body gets the list's own rendered height rather than "whatever is left". `stack_rows`
        // then fills the regions from the TOP and starves the trailing ones, which is what pi's
        // layout engine does; see its doc. The previous
        // `area.height - fixed` body made `fixed` count the hint unconditionally, so a three-row
        // slot spent its last row on the HINT and showed no options at all — the list starved
        // before the trailing chrome did, the exact inversion of upstream's order.
        let sp = u16::from(self.spacers);
        let sp_after_hint = sp.min(hint_h);
        let body_h = self.list.rendered_height();
        let [top, _, title_area, _, body, _, hint, _, bottom] = stack_rows(
            area,
            [1, sp, title_h, sp, body_h, sp, hint_h, sp_after_hint, 1],
        );
        frame.render_widget(border_rule(top.width, theme), top);
        if let Some(title) = &self.title {
            let style = theme.accent_style().add_modifier(ratatui::style::Modifier::BOLD);
            frame.render_widget(
                Paragraph::new(title_lines(title))
                    .style(style)
                    .wrap(ratatui::widgets::Wrap { trim: false }),
                title_area,
            );
        }
        // S28: where the pi component wraps its rows in `new Text(text, 1, 0)`
        // (`extension-selector.ts:87`) / `new TruncatedText(line, 1, 0)` (`oauth-selector.ts:144`),
        // the row gets a one-column left margin and a matching right one, and the list is laid out
        // in `contentWidth = max(1, width - paddingX * 2)` (`text.ts:64,70-76`) — hence `-2` here
        // and a single leading space. That reduced width is also what the two-column gate
        // (`select-list.ts:149` `width > 40`) then sees, which is correct for these kinds and
        // WRONG for the others: thinking / show-images / theme add the `SelectList` straight to the
        // container (`thinking-selector.ts:66`), so it is laid out at the full container width and
        // its rows start at column 0. Applying the inset unconditionally moved that gate by two
        // columns on every dialog.
        let lines = if self.inset {
            self.list
                .lines(body.width.saturating_sub(2), theme)
                .into_iter()
                .map(|line| {
                    let mut spans = vec![Span::raw(" ")];
                    spans.extend(line.spans);
                    Line::from(spans)
                })
                .collect()
        } else {
            self.list.lines(body.width, theme)
        };
        frame.render_widget(Paragraph::new(lines).style(theme.base_style()), body);
        if self.hints {
            frame.render_widget(
                Paragraph::new(vec![self.hint_line(theme)]).style(theme.base_style()),
                hint,
            );
        }
        frame.render_widget(border_rule(bottom.width, theme), bottom);
    }

    fn handle(&mut self, key: &KeyEvent, keymap: &SelectKeymap) -> SelectorOutcome {
        // Keep the hint row honest even for a selector constructed without `with_keymap`: adopt
        // whatever table actually routed this key.
        self.keymap = keymap.clone();
        match keymap.action_for(key) {
            Some(SelectAction::Up) | Some(SelectAction::PageUp) => {
                self.list.select_up();
                self.moved()
            }
            Some(SelectAction::Down) | Some(SelectAction::PageDown) => {
                self.list.select_down();
                self.moved()
            }
            Some(SelectAction::Confirm) => SelectorOutcome::Confirm(self.current_value()),
            Some(SelectAction::Cancel) => SelectorOutcome::Cancel,
            None => SelectorOutcome::Ignored,
        }
    }

    fn set_title(&mut self, title: String) {
        self.title = Some(title);
    }
}

impl ListSelector {
    /// The outcome of a navigation move: a live-preview emit for previewing selectors, else a redraw
    /// (`select-list.ts:103-108` `notifySelectionChange` → `onSelectionChange`).
    fn moved(&self) -> SelectorOutcome {
        if self.preview {
            SelectorOutcome::Preview(self.current_value())
        } else {
            SelectorOutcome::Redraw
        }
    }
}

use super::*;

impl InputEditor {
    /// The command-token highlight + argument-hint ghost for the current buffer (CMDHINT_01).
    ///
    /// Pure: a function of `lines[0]` and the registry alone, recomputed per render. NOT cached
    /// alongside `self.autocomplete` — the popup's lifetime is the very thing this outlives, and 26
    /// state-replacing paths never call `update_autocomplete()` at all: `set_text_internal`
    /// (`:486-495` ← `:1469`, `:1500`, `:1504` history recall **and `:480`**, deliberately, per
    /// TUI-061), its public caller `set_text` (`:472-481`, 8 app call sites — `app/tree_nav.rs:199`,
    /// `app/session_bind.rs:327`, `app/channels.rs:149`, `app/extension_ui.rs:320`,
    /// `app/selectors.rs:395`, `app/crossterm.rs:148`, `extension_editor.rs:58`, `:292`), and
    /// `set_registry` (`:326-328`), which fires on session rebind/`/reload` (`app/run.rs:400`),
    /// session swap (`app/run_arms.rs:33`) and the `enableSkillCommands` settings toggle
    /// (`app/execute_misc.rs:217`). A cached copy would describe a buffer or a registry that no longer
    /// exists — and `close_selector` (`app/selectors.rs:390-397`) can replace both around one frame.
    ///
    /// Scope is **logical line 0** only, and the token boundary is `split_command`'s
    /// (`cyrup-resources/src/prompt.rs:178-190`, pi `prompt-templates.ts:271`): the leading run of
    /// non-whitespace chars after `/`, where the implicit `\n` ending line 0 IS whitespace.
    pub fn command_highlight(&self) -> Option<CommandHighlight> {
        let line0 = self.lines.first()?;
        if line0.first() != Some(&'/') {
            return None;
        }
        // The token boundary: first whitespace CHAR index in line 0, or — when line 0 has none but the
        // buffer continues — line 0's end, because `split_command` finds that `\n`. `None` means the
        // name is still being typed with nothing after it.
        let boundary = line0
            .iter()
            .position(|c| c.is_whitespace())
            .or_else(|| (self.lines.len() > 1).then_some(line0.len()));
        let head: String = line0.iter().take(boundary.unwrap_or(line0.len())).skip(1).collect();
        match boundary {
            // Still typing the name: highlight while it is an honest prefix. No ghost — the popup is
            // open and already showing the full hint + description.
            None => crate::autocomplete::is_command_prefix(&self.registry, &head)
                .then_some(CommandHighlight { token: 0..line0.len(), ghost: None }),
            // Whitespace follows: the highlight FREEZES on `/name` iff that name is an EXACT registered
            // command — `registry.get`, NOT `match_command`/`dispatch_names`, which hold builtins plus
            // `HIDDEN_COMMANDS` only (`commands.rs:189-197`, `:297-312`, `:429-434`) and would drop
            // every prompt template, extension command and skill. Those still run, expanded
            // server-side by `expand_input_text` (`cyrup-session-svc/src/session.rs:1255-1258`). An
            // unknown or partial name (`/flux `, `/bogus `) gets nothing — honest, and identical to
            // today's silent fallback to a literal prompt (`commands.rs:277`).
            Some(i) => {
                let cmd = self.registry.get(&head)?;
                // The argument zone is "empty" when it holds no NON-whitespace char anywhere in the
                // buffer — line 0 past the token, and every later (soft-newline) line. Whitespace-only
                // is still empty, so `/model  ` with two spaces keeps its ghost.
                let zone_empty = line0.iter().skip(i).all(|c| c.is_whitespace())
                    && self.lines.iter().skip(1).all(|l| l.iter().all(|c| c.is_whitespace()));
                let ghost =
                    zone_empty.then_some(cmd.argument_hint.as_deref()).flatten().map(str::to_string);
                Some(CommandHighlight { token: 0..i, ghost })
            }
        }
    }
}

impl InputEditor {
    // ---- autocomplete ----------------------------------------------------------------------

    /// The buffer lines as `String`s, for the autocomplete engine.
    fn lines_as_strings(&self) -> Vec<String> {
        self.lines.iter().map(|l| l.iter().collect()).collect()
    }

    /// Recompute the popup after an edit: auto-open for slash **and** `@`-mention context, otherwise
    /// update an already-open popup or close it (spec/tui/04 §5 — bare path does not auto-pop without
    /// Tab; `@`-mention auto-pops on `@`, `autocomplete.ts:101`).
    pub(super) fn update_autocomplete(&mut self) {
        let was_open = self.autocomplete.is_some();
        // `@`-mention search auto-pops the moment an `@` token forms (whole-tree fuzzy file search).
        if let Some(ac) = self.compute_mention() {
            self.open_popup(ac);
            return;
        }
        let computed = Autocomplete::compute(
            &self.registry,
            &self.lines_as_strings(),
            self.row,
            self.col,
            false,
            &self.cwd,
        );
        match computed {
            Some(ac) if ac.context == CompletionContext::Slash || was_open => self.open_popup(ac),
            _ => self.autocomplete = None,
        }
    }

    /// The text left of the cursor on the current line (the autocomplete context window).
    fn before_cursor(&self) -> String {
        self.lines.get(self.row).map_or(String::new(), |line| line.iter().take(self.col).collect())
    }

    /// Compute the `@`-mention popup for the current cursor, lazily enumerating the tree on first use
    /// (`autocomplete.ts:719-772`). `None` when the trailing token is not an `@`-mention or nothing
    /// matches.
    fn compute_mention(&mut self) -> Option<Autocomplete> {
        let before = self.before_cursor();
        crate::autocomplete::mention_query(&before)?;
        if self.mention_files.is_none() {
            self.mention_files = Some(crate::autocomplete::list_files(&self.cwd, 2000));
        }
        let files = self.mention_files.as_deref().unwrap_or(&[]);
        crate::autocomplete::mention_autocomplete(&before, files)
    }

    /// Trigger completion explicitly (Tab with no popup): force path completion, or slash completion
    /// while typing a `/name` (spec/tui/04 §5). A single forced match auto-applies (§3.7 item 10).
    pub(super) fn trigger_completion(&mut self) -> EditorOutcome {
        // `@`-mention takes precedence on an explicit Tab too (whole-tree fuzzy file search).
        if let Some(ac) = self.compute_mention() {
            self.open_popup(ac);
            return EditorOutcome::Edited;
        }
        let computed = Autocomplete::compute(
            &self.registry,
            &self.lines_as_strings(),
            self.row,
            self.col,
            true,
            &self.cwd,
        );
        match computed {
            Some(ac) if ac.list.len() == 1 && ac.context == CompletionContext::Path => {
                self.open_popup(ac);
                self.accept_completion();
                EditorOutcome::Edited
            }
            Some(ac) => {
                self.open_popup(ac);
                EditorOutcome::Edited
            }
            None => EditorOutcome::Ignored,
        }
    }

    /// Apply the selected popup item to the buffer (Tab/Enter accept). Leaves the popup state to the
    /// caller (Tab keeps editing + recomputes; Enter on a slash item submits).
    pub(super) fn accept_completion(&mut self) {
        let Some(ac) = self.autocomplete.as_ref() else { return };
        if let Some(applied) = ac.apply(&self.lines_as_strings(), self.row, self.col) {
            self.lines = applied.lines.iter().map(|s| s.chars().collect()).collect();
            if self.lines.is_empty() {
                self.lines.push(Vec::new());
            }
            self.row = applied.cursor_line.min(self.lines.len().saturating_sub(1));
            self.col = applied.cursor_col.min(self.cur_len());
        }
    }
}

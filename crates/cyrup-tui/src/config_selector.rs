//! The `cyrup config` interactive resource-config selector (Pi `ConfigSelectorComponent` +
//! `selectConfig`, `modes/interactive/components/config-selector.ts` + `cli/config-selector.ts`): a
//! full-screen, grouped, per-resource **enable/disable** editor for the top-level auto-discovered
//! skills/prompts/themes.
//!
//! Pi's `pi config` (`handleConfigCommand`, package-manager-cli.ts:543-572) resolves the settings +
//! trust, calls `packageManager.resolve()` for the full `ResolvedPaths` (every resource tagged with
//! its current `enabled` flag + `PathMetadata`), then mounts this component. Space (or Enter) toggles
//! the highlighted resource; toggling writes a `+pattern`/`-pattern` override entry into the SAME
//! `skills`/`prompts`/`themes` settings arrays the rest of the override machinery reads
//! (`toggleTopLevelResource`, config-selector.ts:457-503) — cyrup's `global_overrides`/
//! `project_overrides` discovery wiring is the 1:1 equivalent (gap-07 §3 piece 1). Esc closes.
//!
//! This component is UI-only: it flips the checkbox in place and emits the persisted decision as a
//! [`SelectorOutcome::Apply`] payload the chrome (bin `run_config`) writes to settings — the exact
//! split the pre-launch selectors (`SessionSelector`/`TrustSelector`) already use with
//! [`crate::run_startup_selector`]'s `on_apply`. Package-tier resource toggling (Pi's
//! `togglePackageResource`, config-selector.ts:505-562) is out of this crate's scope — it needs the
//! installed-package → live-session wiring (gap-07 §1) and `PackageManager::set_enabled`, both in
//! `cyrup-resources`/`cyrup-session-svc`.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::keymap::{SelectAction, SelectKeymap};
use crate::selector::{Selector, SelectorOutcome};
use crate::theme::UiTheme;

/// The unit-separator that delimits the fields of a [`ConfigToggle`] `Apply` payload.
const UNIT_SEP: char = '\u{1f}';

/// The scope a resource belongs to — the settings scope its enable/disable pattern persists to and
/// the group label it renders under (Pi `PathMetadata.scope`, config-selector.ts:53).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ConfigScope {
    /// User/global scope (`<agent_dir>/skills`, `~/.agents/skills`) — persists to global settings.
    User,
    /// Project scope (`.cyrup/skills`, trust-gated) — persists to project settings.
    Project,
}

impl ConfigScope {
    fn label(self) -> &'static str {
        match self {
            ConfigScope::User => "User",
            ConfigScope::Project => "Project",
        }
    }

    /// The scope's sort rank — user groups render before project (Pi's group sort,
    /// config-selector.ts:157-159).
    fn order(self) -> u8 {
        match self {
            ConfigScope::User => 0,
            ConfigScope::Project => 1,
        }
    }

    fn payload(self) -> &'static str {
        match self {
            ConfigScope::User => "user",
            ConfigScope::Project => "project",
        }
    }

    fn from_payload(s: &str) -> Option<ConfigScope> {
        match s {
            "user" => Some(ConfigScope::User),
            "project" => Some(ConfigScope::Project),
            _ => None,
        }
    }
}

/// A manageable resource kind — the `skills`/`prompts`/`themes` settings-array key its `+`/`-` pattern
/// lands in (Pi `ResourceType`/`arrayKey`, config-selector.ts:25,462).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ConfigKind {
    Skills,
    Prompts,
    Themes,
}

impl ConfigKind {
    /// The settings-array key the pattern is written into (Pi `arrayKey`, config-selector.ts:462).
    pub fn key(self) -> &'static str {
        match self {
            ConfigKind::Skills => "skills",
            ConfigKind::Prompts => "prompts",
            ConfigKind::Themes => "themes",
        }
    }

    /// The subgroup header label (Pi `RESOURCE_TYPE_LABELS`, config-selector.ts:27-32).
    fn label(self) -> &'static str {
        match self {
            ConfigKind::Skills => "Skills",
            ConfigKind::Prompts => "Prompts",
            ConfigKind::Themes => "Themes",
        }
    }

    /// The subgroup sort rank (Pi `typeOrder`, config-selector.ts:164).
    fn order(self) -> u8 {
        match self {
            ConfigKind::Skills => 0,
            ConfigKind::Prompts => 1,
            ConfigKind::Themes => 2,
        }
    }

    fn from_key(s: &str) -> Option<ConfigKind> {
        match s {
            "skills" => Some(ConfigKind::Skills),
            "prompts" => Some(ConfigKind::Prompts),
            "themes" => Some(ConfigKind::Themes),
            _ => None,
        }
    }
}

/// One toggleable resource in the config editor (Pi `ResourceItem`, config-selector.ts:34-42).
#[derive(Clone, Debug)]
pub struct ConfigRow {
    /// Which settings scope this resource's toggle persists to.
    pub scope: ConfigScope,
    /// The resource kind (skills/prompts/themes).
    pub kind: ConfigKind,
    /// The display name (skill directory name, prompt/theme file name) — Pi `displayName`,
    /// config-selector.ts:124-133.
    pub display_name: String,
    /// The settings-relative pattern (`skills/foo/SKILL.md`) written as `+pattern`/`-pattern` on
    /// toggle (Pi `getResourcePattern` = `relative(baseDir, item.path)`, config-selector.ts:568-572).
    /// It MUST round-trip through cyrup-resources' `is_enabled_by_overrides` (the base-relative posix
    /// path), so the bin computes it as `path` relative to the resource root's parent.
    pub pattern: String,
    /// The base directory the group renders under (the resource root's parent, for the group label).
    pub base_dir: String,
    /// Whether the resource is currently enabled (the checkbox state).
    pub enabled: bool,
}

/// A persisted toggle decision the chrome applies to settings. Encoded as the
/// [`SelectorOutcome::Apply`] payload (`scope␟kind␟pattern␟enabled`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigToggle {
    pub scope: ConfigScope,
    pub kind: ConfigKind,
    pub pattern: String,
    pub enabled: bool,
}

impl ConfigToggle {
    /// Encode as the `Apply` payload the chrome persists.
    pub fn to_payload(&self) -> String {
        format!(
            "{}{UNIT_SEP}{}{UNIT_SEP}{}{UNIT_SEP}{}",
            self.scope.payload(),
            self.kind.key(),
            self.pattern,
            u8::from(self.enabled),
        )
    }

    /// Decode an `Apply` payload emitted by a [`ConfigSelector`] toggle. Returns `None` for a
    /// malformed payload (the chrome then ignores it rather than corrupting settings).
    pub fn from_payload(payload: &str) -> Option<ConfigToggle> {
        let mut parts = payload.split(UNIT_SEP);
        let scope = ConfigScope::from_payload(parts.next()?)?;
        let kind = ConfigKind::from_key(parts.next()?)?;
        let pattern = parts.next()?.to_string();
        let enabled = parts.next()? == "1";
        if parts.next().is_some() {
            return None;
        }
        Some(ConfigToggle { scope, kind, pattern, enabled })
    }
}

/// One row of the flattened, grouped display list (Pi `FlatEntry`, config-selector.ts:175-178).
#[derive(Clone, Debug)]
enum Flat {
    /// A scope group header (`User (…)` / `Project (…)`).
    Group(String),
    /// A resource-kind subgroup header (`Skills` / `Prompts` / `Themes`).
    Subgroup(String),
    /// A toggleable resource, carrying its index into [`ConfigSelector::rows`].
    Item(usize),
}

/// The `pi config` resource-config selector. Renders the grouped resource list with per-resource
/// checkboxes, toggles the highlighted one on space/enter (emitting the decision as an `Apply`
/// payload), filters as the user types, and closes on Esc.
pub struct ConfigSelector {
    /// Canonical resource list; the `enabled` flag is flipped in place on toggle.
    rows: Vec<ConfigRow>,
    /// Row indices in stable group/subgroup/name order (Pi's group + subgroup + item sort).
    order: Vec<usize>,
    /// The live type-to-filter query (Pi's `searchInput`, config-selector.ts:203).
    query: String,
    /// The current (query-filtered) flattened display list.
    flat: Vec<Flat>,
    /// The selected index into `flat` — always kept on an `Item` when any item is visible.
    selected: usize,
}

impl ConfigSelector {
    /// Build from the resolved resource rows (produced by the bin from a discovery pass).
    pub fn new(rows: Vec<ConfigRow>) -> ConfigSelector {
        // Stable group/subgroup/name order (Pi's group + subgroup + item sort): user before project,
        // then base dir, then kind order, then case-insensitive name.
        let mut order: Vec<usize> = (0..rows.len()).collect();
        order.sort_by_key(|&i| {
            rows.get(i).map(|r| {
                (r.scope.order(), r.base_dir.clone(), r.kind.order(), r.display_name.to_lowercase())
            })
        });
        let mut sel = ConfigSelector { rows, order, query: String::new(), flat: Vec::new(), selected: 0 };
        sel.flat = sel.build_flat();
        sel.selected = sel.first_item().unwrap_or(0);
        sel
    }

    /// Whether a resource passes the current filter (Pi `filterItems`, config-selector.ts:268-317):
    /// a case-insensitive substring match against its display name, kind key, or pattern.
    fn matches_query(row: &ConfigRow, query: &str) -> bool {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return true;
        }
        row.display_name.to_lowercase().contains(&q)
            || row.kind.key().contains(&q)
            || row.pattern.to_lowercase().contains(&q)
    }

    /// Rebuild the flattened display list from `order` + the current query, inserting group /
    /// subgroup headers only above the (filtered) items they contain (Pi `buildFlatList` +
    /// `filterItems`).
    fn build_flat(&self) -> Vec<Flat> {
        let mut flat = Vec::new();
        let mut cur_group: Option<(ConfigScope, String)> = None;
        let mut cur_kind: Option<ConfigKind> = None;
        for &i in &self.order {
            let Some(row) = self.rows.get(i) else { continue };
            if !Self::matches_query(row, &self.query) {
                continue;
            }
            let gkey = (row.scope, row.base_dir.clone());
            if cur_group.as_ref() != Some(&gkey) {
                flat.push(Flat::Group(format!("{} ({})", row.scope.label(), row.base_dir)));
                cur_group = Some(gkey);
                cur_kind = None;
            }
            if cur_kind != Some(row.kind) {
                flat.push(Flat::Subgroup(row.kind.label().to_string()));
                cur_kind = Some(row.kind);
            }
            flat.push(Flat::Item(i));
        }
        flat
    }

    /// The index of the first `Item` entry in the current flat list, if any.
    fn first_item(&self) -> Option<usize> {
        self.flat.iter().position(|e| matches!(e, Flat::Item(_)))
    }

    /// The next `Item` entry from `from` in `dir` (`+1` down / `-1` up), skipping headers; stays put
    /// if there is no further item (Pi `findNextItem`, config-selector.ts:257-266).
    fn find_item(&self, from: usize, dir: isize) -> usize {
        let mut i = from as isize + dir;
        while i >= 0 && (i as usize) < self.flat.len() {
            if matches!(self.flat.get(i as usize), Some(Flat::Item(_))) {
                return i as usize;
            }
            i += dir;
        }
        from
    }

    /// The row index the highlight currently points at, if it is on an item.
    fn selected_row(&self) -> Option<usize> {
        match self.flat.get(self.selected) {
            Some(Flat::Item(i)) => Some(*i),
            _ => None,
        }
    }

    /// Toggle the highlighted resource in place and emit the persisted decision (Pi's `toggleResource`
    /// → `onToggle`, config-selector.ts:433-447). Toggling never changes the flat structure (the item
    /// still matches its query and still lives in the same group), so the highlight is preserved.
    fn toggle_selected(&mut self) -> SelectorOutcome {
        let Some(i) = self.selected_row() else {
            return SelectorOutcome::Redraw;
        };
        let Some(row) = self.rows.get_mut(i) else {
            return SelectorOutcome::Redraw;
        };
        let enabled = !row.enabled;
        row.enabled = enabled;
        let toggle =
            ConfigToggle { scope: row.scope, kind: row.kind, pattern: row.pattern.clone(), enabled };
        self.flat = self.build_flat();
        SelectorOutcome::Apply(toggle.to_payload())
    }

    /// Apply a filter change: rebuild the flat list and reselect the first visible item (Pi
    /// `selectFirstItem`, config-selector.ts:319-322).
    fn on_query_changed(&mut self) {
        self.flat = self.build_flat();
        self.selected = self.first_item().unwrap_or(0);
    }

    /// Read-only access to the resource rows (tests / chrome inspection).
    pub fn rows(&self) -> &[ConfigRow] {
        &self.rows
    }
}

impl Selector for ConfigSelector {
    fn desired_height(&self, _width: u16) -> u16 {
        // Top rule + header + body (one line per flat entry) + bottom rule; the host clamps this to
        // the terminal height, and `render` windows within whatever slot it is given.
        let body = self.flat.len().max(1).min(u16::MAX as usize) as u16;
        body.saturating_add(3)
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        let [top, header, body, bottom] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(area);

        frame.render_widget(border_rule(top.width, theme), top);

        let hint = if self.query.is_empty() {
            " Resource Configuration    space toggle · esc close".to_string()
        } else {
            format!(" Resource Configuration    filter: {}", self.query)
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(hint, theme.accent_style().add_modifier(Modifier::BOLD)))),
            header,
        );

        let visible = body.height as usize;
        let total = self.flat.len();
        let mut lines: Vec<Line> = Vec::new();
        if total == 0 {
            lines.push(Line::from(Span::styled("  No resources found", theme.dim_style())));
        } else {
            // Center the highlighted row in the window (Pi's `startIndex` math, config-selector.ts:353).
            let start = if total <= visible {
                0
            } else {
                let half = visible / 2;
                self.selected.saturating_sub(half).min(total - visible)
            };
            let end = (start + visible).min(total);
            for (offset, entry) in self.flat.get(start..end).unwrap_or(&[]).iter().enumerate() {
                let i = start + offset;
                match entry {
                    Flat::Group(label) => {
                        lines.push(Line::from(Span::styled(
                            format!("  {label}"),
                            theme.accent_style().add_modifier(Modifier::BOLD),
                        )));
                    }
                    Flat::Subgroup(label) => {
                        lines.push(Line::from(Span::styled(format!("    {label}"), theme.muted_style())));
                    }
                    Flat::Item(ri) => {
                        let Some(row) = self.rows.get(*ri) else { continue };
                        let is_sel = i == self.selected;
                        let cursor = if is_sel { "> " } else { "  " };
                        let (checkbox, cb_style) = if row.enabled {
                            ("[x]", theme.success_style())
                        } else {
                            ("[ ]", theme.dim_style())
                        };
                        let name_style = if is_sel {
                            theme.base_style().add_modifier(Modifier::BOLD)
                        } else {
                            theme.base_style()
                        };
                        lines.push(Line::from(vec![
                            Span::styled(format!("{cursor}    "), theme.base_style()),
                            Span::styled(format!("{checkbox} "), cb_style),
                            Span::styled(row.display_name.clone(), name_style),
                        ]));
                    }
                }
            }
        }
        frame.render_widget(Paragraph::new(lines).style(theme.base_style()), body);
        frame.render_widget(border_rule(bottom.width, theme), bottom);
    }

    fn handle(&mut self, key: &KeyEvent, keymap: &SelectKeymap) -> SelectorOutcome {
        // Navigation + confirm/cancel come from the shared `tui.select.*` map first.
        if let Some(action) = keymap.action_for(key) {
            return match action {
                SelectAction::Up | SelectAction::PageUp => {
                    self.selected = self.find_item(self.selected, -1);
                    SelectorOutcome::Redraw
                }
                SelectAction::Down | SelectAction::PageDown => {
                    self.selected = self.find_item(self.selected, 1);
                    SelectorOutcome::Redraw
                }
                // Enter TOGGLES (it does not confirm-and-close) — Pi `tui.select.confirm` toggles the
                // resource (config-selector.ts:433). Esc/Ctrl+C close.
                SelectAction::Confirm => self.toggle_selected(),
                SelectAction::Cancel => SelectorOutcome::Cancel,
            };
        }
        // Space toggles the highlighted resource (Pi `data === " "`, config-selector.ts:433). A bare
        // Backspace / printable char drives the type-to-filter (config-selector.ts:445-446).
        if key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER) {
            return SelectorOutcome::Ignored;
        }
        match key.code {
            KeyCode::Char(' ') => self.toggle_selected(),
            KeyCode::Char(c) => {
                self.query.push(c);
                self.on_query_changed();
                SelectorOutcome::Redraw
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.on_query_changed();
                SelectorOutcome::Redraw
            }
            _ => SelectorOutcome::Ignored,
        }
    }
}

/// A full-width `─` rule styled `border`, matching Pi's `DynamicBorder` (`dynamic-border.ts:23`).
fn border_rule(width: u16, theme: &UiTheme) -> Paragraph<'static> {
    let rule = "─".repeat(width.max(1) as usize);
    Paragraph::new(Line::from(Span::styled(rule, theme.border_style())))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyEventKind, KeyEventState};

    fn row(scope: ConfigScope, kind: ConfigKind, name: &str, pattern: &str, enabled: bool) -> ConfigRow {
        ConfigRow {
            scope,
            kind,
            display_name: name.to_string(),
            pattern: pattern.to_string(),
            base_dir: match scope {
                ConfigScope::User => "~/.cyrup/agent/".to_string(),
                ConfigScope::Project => ".cyrup/".to_string(),
            },
            enabled,
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn sample() -> ConfigSelector {
        ConfigSelector::new(vec![
            row(ConfigScope::User, ConfigKind::Skills, "greeter", "skills/greeter/SKILL.md", true),
            row(ConfigScope::User, ConfigKind::Skills, "farewell", "skills/farewell/SKILL.md", true),
            row(ConfigScope::Project, ConfigKind::Prompts, "plan.md", "prompts/plan.md", true),
        ])
    }

    #[test]
    fn payload_round_trips() {
        let t = ConfigToggle {
            scope: ConfigScope::Project,
            kind: ConfigKind::Skills,
            pattern: "skills/foo/SKILL.md".to_string(),
            enabled: false,
        };
        assert_eq!(ConfigToggle::from_payload(&t.to_payload()), Some(t));
        assert_eq!(ConfigToggle::from_payload("garbage"), None);
    }

    #[test]
    fn first_selection_is_an_item_not_a_header() {
        let sel = sample();
        assert!(matches!(sel.flat[sel.selected], Flat::Item(_)));
        // The first flat entry is the User group header.
        assert!(matches!(sel.flat[0], Flat::Group(_)));
    }

    #[test]
    fn navigation_skips_headers_across_groups() {
        let mut sel = sample();
        let keymap = SelectKeymap::default();
        // Walk down through every item; each landing must be an Item (never a header).
        let mut items_seen = 0;
        for _ in 0..6 {
            assert!(matches!(sel.flat[sel.selected], Flat::Item(_)));
            items_seen += 1;
            let before = sel.selected;
            sel.handle(&key(KeyCode::Down), &keymap);
            if sel.selected == before {
                break;
            }
        }
        assert_eq!(items_seen, 3, "should visit exactly the three items");
    }

    #[test]
    fn space_toggles_selected_and_emits_apply_with_flipped_state() {
        let mut sel = sample();
        let keymap = SelectKeymap::default();
        let i = sel.selected_row().unwrap();
        let was = sel.rows[i].enabled;
        let out = sel.handle(&key(KeyCode::Char(' ')), &keymap);
        assert_eq!(sel.rows[i].enabled, !was, "checkbox flips in place");
        let SelectorOutcome::Apply(payload) = out else {
            panic!("space must emit Apply, got {out:?}");
        };
        let toggle = ConfigToggle::from_payload(&payload).unwrap();
        assert_eq!(toggle.enabled, !was);
        assert_eq!(toggle.pattern, sel.rows[i].pattern);
    }

    #[test]
    fn enter_toggles_and_esc_cancels() {
        let mut sel = sample();
        let keymap = SelectKeymap::default();
        assert!(matches!(sel.handle(&key(KeyCode::Enter), &keymap), SelectorOutcome::Apply(_)));
        assert_eq!(sel.handle(&key(KeyCode::Esc), &keymap), SelectorOutcome::Cancel);
    }

    #[test]
    fn typing_filters_and_reselects_first_match() {
        let mut sel = sample();
        let keymap = SelectKeymap::default();
        for c in "farewell".chars() {
            sel.handle(&key(KeyCode::Char(c)), &keymap);
        }
        // Only the farewell skill (and its User/Skills headers) survive the filter.
        let item_count = sel.flat.iter().filter(|e| matches!(e, Flat::Item(_))).count();
        assert_eq!(item_count, 1);
        assert_eq!(sel.selected_row().map(|i| sel.rows[i].display_name.clone()), Some("farewell".to_string()));
        // Backspacing restores the rest.
        for _ in 0..8 {
            sel.handle(&key(KeyCode::Backspace), &keymap);
        }
        assert_eq!(sel.flat.iter().filter(|e| matches!(e, Flat::Item(_))).count(), 3);
    }
}

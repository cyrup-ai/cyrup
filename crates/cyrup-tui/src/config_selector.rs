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
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::chrome::key_hint_spans;
use crate::keymap::{SelectAction, SelectKeymap};
use crate::selector::{border_rule, centered_window, input_line_spans, Selector, SelectorOutcome};
use crate::text_input::{Input, InputOutcome};
use crate::text_width::{str_width, truncate_line_to_width, truncate_to_width};
use crate::theme::UiTheme;

/// The unit-separator that delimits the fields of a [`ConfigToggle`] `Apply` payload.
const UNIT_SEP: char = '\u{1f}';

/// Cyrup's `CONFIG_DIR_NAME` (`pi/packages/coding-agent/src/config.ts:491`, `.pi` → `.cyrup` per
/// the rebrand) — the literal the header's scope row names the settings file with.
const CONFIG_DIR_NAME: &str = ".cyrup";

/// Which settings file a toggle is written to, and therefore which of the two header/row
/// presentations the dialog is in — Pi `ConfigWriteScope` (`config-selector.ts:27`).
///
/// `Global` writes `~/.cyrup/agent/settings.json` and shows a plain enable/disable checkbox.
/// `Project` writes `<cwd>/.cyrup/settings.json`, cycles each resource through
/// inherit → `+` → `-`, and **dims** everything inherited from the global scope so the user can
/// see which rows are local (S18).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ConfigWriteScope {
    #[default]
    Global,
    Project,
}

/// A resource's project-scope override, as recorded in the project settings arrays — Pi
/// `ProjectOverrideState` (`config-selector.ts:29`). Only meaningful under
/// [`ConfigWriteScope::Project`]; `getProjectOverrideState` returns `Inherit` unconditionally in
/// global scope (`:740`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ProjectOverrideState {
    /// No `+`/`-` entry — the resource follows the global decision (`[x]`/`[ ]`, dim).
    #[default]
    Inherit,
    /// A `+pattern` entry — force-loaded in this project (`[+]`, success, `  project load`).
    Load,
    /// A `-pattern` entry — force-unloaded in this project (`[-]`, warning, `  project unload`).
    Unload,
}

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
    /// The live type-to-filter query (Pi's `searchInput`, config-selector.ts:227,396) — the shared
    /// single-line editing surface, caret and all.
    input: Input,
    /// The current (query-filtered) flattened display list.
    flat: Vec<Flat>,
    /// The selected index into `flat` — always kept on an `Item` when any item is visible.
    selected: usize,
    /// How many body rows the window shows at once (Pi `ResourceList.maxVisible`,
    /// `config-selector.ts:228,266`). See [`ConfigSelector::max_visible_for`].
    max_visible: u16,
    /// Which settings file toggles land in, driving the header title, the checkbox glyphs and the
    /// inherited-global dimming (Pi `this.writeScope`, `config-selector.ts:232`).
    write_scope: ConfigWriteScope,
    /// Whether `Tab` can switch to project scope — Pi's `projectModeAvailable`
    /// (`config-selector.ts:189,890`), which gates the `tab switch mode` hint (`:205`) and the
    /// `onSwitchMode` wiring (`:920-925`). Defaults **false** because nothing yet feeds this
    /// selector a project-scope resource set; the chrome opts in once it does.
    project_mode_available: bool,
    /// Per-row project override state, parallel to `rows` — Pi's `getProjectOverrideState(item)`
    /// (`config-selector.ts:739-757`), which upstream derives from the project settings arrays.
    /// Held as data here so the render stays pure and this crate keeps no `SettingsManager`
    /// dependency; the chrome fills it via [`ConfigSelector::set_override_state`].
    override_states: Vec<ProjectOverrideState>,
    /// `inheritedEnabledByKey`'s key set (`config-selector.ts:233,262`) — see
    /// [`ConfigSelector::set_inherited_global_keys`].
    inherited_global_keys: std::collections::HashSet<String>,
}

/// The non-body rows of the `cyrup config` envelope: `Spacer`(`config-selector.ts:901`),
/// `DynamicBorder`(:902), `Spacer`(:903), header — **two** lines (:905, rendered at :215-218),
/// `Spacer`(:906), \[body], `Spacer`(:929), `DynamicBorder`(:930).
///
/// **Eight**, which is now upstream's own constant verbatim (`config-selector.ts:264-265`: "8 lines
/// of chrome: top spacer + top border + spacer + header (2 lines) + spacer + bottom spacer + bottom
/// border"). It was 7 while `ConfigSelectorHeader`'s second row was missing (S17).
///
/// Note the 8 does **not** count the search `Input` and the blank under it that `ResourceList.render`
/// itself pushes (`:396-397`) — those belong to the body, and pi's dialog consequently overshoots
/// the terminal by two rows. Reproduced rather than "fixed": the window size has to match upstream's
/// or the visible row set diverges.
const CHROME_ROWS: u16 = 8;

/// The window floor, straight from `Math.max(5, …)` (`config-selector.ts:266`).
const MIN_MAX_VISIBLE: u16 = 5;

/// Pi's `terminalHeight ?? 24` default (`config-selector.ts:266`), applied before any host has
/// called [`Selector::set_terminal_height`].
const DEFAULT_TERMINAL_ROWS: u16 = 24;

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
        let override_states = vec![ProjectOverrideState::default(); rows.len()];
        let mut sel = ConfigSelector {
            rows,
            order,
            input: Input::new(),
            flat: Vec::new(),
            selected: 0,
            max_visible: Self::max_visible_for(DEFAULT_TERMINAL_ROWS),
            write_scope: ConfigWriteScope::Global,
            project_mode_available: false,
            override_states,
            inherited_global_keys: std::collections::HashSet::new(),
        };
        sel.flat = sel.build_flat();
        sel.selected = sel.first_item().unwrap_or(0);
        sel
    }

    /// Which settings file toggles land in (Pi `this.writeScope`).
    pub fn write_scope(&self) -> ConfigWriteScope {
        self.write_scope
    }

    /// Set the write scope, i.e. switch between the global and project-local presentations
    /// (Pi `ConfigSelectorComponent.switchWriteScope`, `config-selector.ts:933-937`).
    pub fn set_write_scope(&mut self, scope: ConfigWriteScope) {
        self.write_scope = scope;
    }

    /// Whether `Tab` offers the project-scope mode (Pi `projectModeAvailable`,
    /// `config-selector.ts:890`). Off by default; turning it on both shows the `tab switch mode`
    /// hint and arms the `Tab` binding, exactly as upstream's single flag does (`:205`, `:920`).
    pub fn set_project_mode_available(&mut self, available: bool) {
        self.project_mode_available = available;
    }

    /// Record a resource's project override (Pi `getProjectOverrideState`,
    /// `config-selector.ts:739-757`). `index` indexes [`ConfigSelector::rows`]; out-of-range is a
    /// no-op.
    pub fn set_override_state(&mut self, index: usize, state: ProjectOverrideState) {
        if let Some(slot) = self.override_states.get_mut(index) {
            *slot = state;
        }
    }

    /// A resource's recorded project override (`Inherit` when unset).
    pub fn override_state(&self, index: usize) -> ProjectOverrideState {
        self.override_states.get(index).copied().unwrap_or_default()
    }

    /// The resource's identity in the inherited-global map — `getResourceItemKey`
    /// (`config-selector.ts:842-844`), `` `${item.resourceType}:${canonicalizePath(item.path)}` ``.
    /// cyrup's `ConfigRow` carries the path split into `base_dir` + `pattern` (the pattern is
    /// `relative(baseDir, item.path)`, `:854-858`), so rejoining them reconstitutes it.
    pub fn resource_key(row: &ConfigRow) -> String {
        format!("{}:{}{}", row.kind.key(), row.base_dir, row.pattern)
    }

    /// The keys present in the **global** resolve — `inheritedEnabledByKey`
    /// (`config-selector.ts:262`, built by `buildInheritedEnabledMap(groupsByScope.global)` at
    /// `:281-291`). Upstream gets a whole second `PackageManager.resolve()` for this, run against a
    /// settings manager with `projectTrusted: false` (`package-manager-cli.ts:655-660`); the chrome
    /// hands cyrup the same set as [`ConfigSelector::resource_key`] strings so this crate keeps no
    /// `PackageManager` dependency.
    ///
    /// Only the KEYS matter here: `isInheritedGlobalItem` calls `.has()` (`:782`). The map's boolean
    /// values feed `getInheritedEnabled` (`:774-778`), which only
    /// `cycleProjectOverrideState` (`:730-737`) reads, and cyrup's project rows are set as data.
    pub fn set_inherited_global_keys(&mut self, keys: impl IntoIterator<Item = String>) {
        self.inherited_global_keys = keys.into_iter().collect();
    }

    /// `isInheritedGlobalItem` (`config-selector.ts:781-783`), whole:
    /// `getItemScope(item) === "user" || this.inheritedEnabledByKey.has(this.getResourceItemKey(item))`.
    ///
    /// The second arm is not redundant with the first. `getItemScope` reports the scope of the
    /// directory the resource was DISCOVERED in (`:846-848`), while `inheritedEnabledByKey` is
    /// keyed by path over the global resolve — so a **project**-scope row whose file the global
    /// resolve also reaches (a global `skills: ["../foo"]` entry, a package installed in both
    /// tiers) is inherited too, and must get the ` inherited global` suffix (`:654`) and the dim
    /// state (`:657-663`). cyrup had reduced the predicate to the scope test alone, which silently
    /// dropped every such row back to "local".
    fn is_inherited_global(&self, row: &ConfigRow) -> bool {
        row.scope == ConfigScope::User
            || self.inherited_global_keys.contains(&Self::resource_key(row))
    }

    /// `isDimmedItem` (`config-selector.ts:657-663`): project scope **and** inherited from global
    /// **and** not overridden locally.
    fn is_dimmed(&self, index: usize, row: &ConfigRow) -> bool {
        self.write_scope == ConfigWriteScope::Project
            && self.is_inherited_global(row)
            && self.override_state(index) == ProjectOverrideState::Inherit
    }

    /// `Math.max(5, (terminalHeight ?? 24) - chrome)` (Pi `config-selector.ts:264-266`) with
    /// cyrup's exact [`CHROME_ROWS`].
    ///
    /// This is what makes the four envelope blanks reachable. Without it `desired_height` was
    /// `flat.len() + 7` with no cap, so on any real resource set the dialog was taller than the
    /// terminal and the host clamped the slot — costing the trailing `Spacer`/`DynamicBorder` on
    /// every frame. Upstream never has that problem: its body is windowed to the terminal, so the
    /// whole envelope fits whenever the terminal has at least `5 + chrome` rows.
    fn max_visible_for(terminal_rows: u16) -> u16 {
        terminal_rows.saturating_sub(CHROME_ROWS).max(MIN_MAX_VISIBLE)
    }

    /// The current body-window size (tests / inspection).
    pub fn max_visible(&self) -> u16 {
        self.max_visible
    }

    /// The body's natural row count — measured off the real [`Self::body_lines`] so the height can
    /// never disagree with what renders. Two rows of search chrome (`config-selector.ts:396-397`)
    /// plus the flat list windowed at [`Self::max_visible`], plus the scroll readout when the
    /// window does not cover the list (`:444-449`), or the single `"No resources found"` row
    /// (`:399-402`).
    fn body_rows(&self, width: u16) -> u16 {
        self.body_lines(width, UiTheme::default_ref())
            .len()
            .min(u16::MAX as usize) as u16
    }

    /// The visible window `[start, end)` — `config-selector.ts:405-409`. `Math.min(a, len - max)`
    /// goes negative when the list is shorter than the window and the outer `Math.max(0, …)`
    /// catches it.
    fn window(&self) -> (usize, usize) {
        centered_window(self.selected, self.flat.len(), usize::from(self.max_visible))
    }

    /// `renderCheckbox` (`config-selector.ts:639-647`) — the glyph **and** its colour.
    ///
    /// S19: in project scope the checkbox reports the *override*, not the resolved enable — a
    /// `success` `[+]` for a forced load, a `warning` `[-]` for a forced unload, and a **dim**
    /// `[x]`/`[ ]` for a row that is merely inheriting. Global scope keeps `success` `[x]` / dim
    /// `[ ]`. cyrup drew `success`/dim `[x]`/`[ ]` in both, so project mode was pixel-identical to
    /// global.
    fn checkbox(&self, index: usize, row: &ConfigRow, theme: &UiTheme) -> Span<'static> {
        if self.write_scope == ConfigWriteScope::Project {
            return match self.override_state(index) {
                ProjectOverrideState::Load => Span::styled("[+]", theme.success_style()),
                ProjectOverrideState::Unload => Span::styled("[-]", theme.warning_style()),
                ProjectOverrideState::Inherit => {
                    Span::styled(if row.enabled { "[x]" } else { "[ ]" }, theme.dim_style())
                }
            };
        }
        if row.enabled {
            Span::styled("[x]", theme.success_style())
        } else {
            Span::styled("[ ]", theme.dim_style())
        }
    }

    /// `getItemSuffix` (`config-selector.ts:649-655`) — the trailing state word. Empty outside
    /// project scope (S19).
    fn item_suffix(&self, index: usize, row: &ConfigRow, theme: &UiTheme) -> Option<Span<'static>> {
        if self.write_scope != ConfigWriteScope::Project {
            return None;
        }
        match self.override_state(index) {
            ProjectOverrideState::Load => Some(Span::styled("  project load", theme.muted_style())),
            ProjectOverrideState::Unload => {
                Some(Span::styled("  project unload", theme.muted_style()))
            }
            ProjectOverrideState::Inherit if self.is_inherited_global(row) => {
                Some(Span::styled("  inherited global", theme.dim_style()))
            }
            ProjectOverrideState::Inherit => None,
        }
    }

    /// `ConfigSelectorHeader.render` (`config-selector.ts:202-218`) — **two** lines, S17.
    ///
    /// Row 1 is `theme.bold(title)` — bold and *uncoloured*, not the accent shout cyrup drew —
    /// then `Math.max(1, width - titleWidth - hintWidth)` spaces, then a RIGHT-ALIGNED two-tone
    /// hint run: `keyHint("tui.input.tab","switch mode")` (only when project mode is available),
    /// `rawKeyHint("space", …)` and `rawKeyHint("esc","close")`, joined by a muted `" · "`. Each
    /// `keyHint` is a dim key + a muted ` description` (`keybinding-hints.ts:40-47`), which is
    /// exactly [`crate::chrome::key_hint_spans`]. `tui.input.tab`'s default key is the literal
    /// `tab` (`packages/tui/src/keybindings.ts:139`).
    ///
    /// Row 2 names the settings file being written, muted — the row cyrup omitted entirely, and the
    /// only place the user is told *which* scope the dialog is editing, which is what makes S18's
    /// dimming legible in the first place.
    ///
    /// Neither row carries a leading space: upstream returns the raw
    /// `truncateToWidth(..., width, "")` strings.
    fn header_lines(&self, width: u16, theme: &UiTheme) -> Vec<Line<'static>> {
        let w = usize::from(width);
        let project = self.write_scope == ConfigWriteScope::Project;
        let title = if project { "Project Local Resources" } else { "Global Resources" };
        let sep = || Span::styled(" · ", theme.muted_style());

        let mut hint: Vec<Span<'static>> = Vec::new();
        if self.project_mode_available {
            hint.extend(key_hint_spans("tab", "switch mode", theme));
            hint.push(sep());
        }
        let action = if project { "cycle inherit/+/-" } else { "toggle" };
        hint.extend(key_hint_spans("space", action, theme));
        hint.push(sep());
        hint.extend(key_hint_spans("esc", "close", theme));

        let hint_w: usize = hint.iter().map(|s| s.width()).sum();
        let spacing = w.saturating_sub(str_width(title)).saturating_sub(hint_w).max(1);
        let mut row1: Vec<Span<'static>> =
            vec![Span::styled(title, theme.base_style().add_modifier(Modifier::BOLD))];
        row1.push(Span::styled(" ".repeat(spacing), theme.base_style()));
        row1.extend(hint);

        let scope_hint = if project {
            format!("{CONFIG_DIR_NAME}/settings.json · inherited global resources are dimmed")
        } else {
            format!("~/{CONFIG_DIR_NAME}/agent/settings.json")
        };
        vec![
            truncate_line_to_width(Line::from(row1), w, ""),
            Line::from(Span::styled(truncate_to_width(&scope_hint, w, ""), theme.muted_style())),
        ]
    }

    /// `ResourceList.render` (`config-selector.ts:392-451`), line for line.
    fn body_lines(&self, width: u16, theme: &UiTheme) -> Vec<Line<'static>> {
        let w = usize::from(width);
        let mut lines: Vec<Line<'static>> = Vec::new();

        // `:396-397` — the search `Input` and the blank under it, via the shared `Input.render`
        // port (S31): a bare, unstyled `"> "` at column 0. cyrup used to hang the query off the
        // header as `filter: …`, a row upstream does not have.
        lines.push(Line::from(input_line_spans(
            self.input.value(),
            self.input.cursor(),
            width,
            theme,
        )));
        lines.push(Line::from(""));

        // `:399-402` — `theme.fg("muted", …)`, not dim (S17 fix #43).
        if self.flat.is_empty() {
            lines.push(Line::from(Span::styled("  No resources found", theme.muted_style())));
            return lines;
        }

        let (start, end) = self.window();
        for (offset, entry) in self.flat.get(start..end).unwrap_or(&[]).iter().enumerate() {
            let i = start + offset;
            match entry {
                // `:415-420` — the group header. S18: in project scope a user-scope group is
                // `inherited`, gains a ` · inherited global` tail INSIDE the bold, and is dim
                // instead of accent. Bold either way.
                Flat::Group(label) => {
                    let inherited = self.write_scope == ConfigWriteScope::Project
                        && label.starts_with(ConfigScope::User.label());
                    let text = if inherited {
                        format!("  {label} · inherited global")
                    } else {
                        format!("  {label}")
                    };
                    let style = if inherited { theme.dim_style() } else { theme.accent_style() };
                    lines.push(truncate_line_to_width(
                        Line::from(Span::styled(text, style.add_modifier(Modifier::BOLD))),
                        w,
                        "",
                    ));
                }
                // `:421-425` — the subgroup header: dim under an inherited group, muted otherwise.
                Flat::Subgroup(label) => {
                    let inherited = self.write_scope == ConfigWriteScope::Project
                        && self.group_scope_at(i) == Some(ConfigScope::User);
                    let style = if inherited { theme.dim_style() } else { theme.muted_style() };
                    lines.push(truncate_line_to_width(
                        Line::from(Span::styled(format!("    {label}"), style)),
                        w,
                        "",
                    ));
                }
                // `:426-440` — the resource row, truncated with a REAL ellipsis (`"..."`, `:437`).
                // cyrup made no truncation call at all, so long names hard-clipped at the frame.
                Flat::Item(ri) => {
                    let Some(row) = self.rows.get(*ri) else { continue };
                    let is_sel = i == self.selected;
                    let dimmed = self.is_dimmed(*ri, row);
                    let cursor = if is_sel { "> " } else { "  " };
                    // `:431-432` — bold only when selected AND not dimmed; the dim colour wins
                    // over the bold entirely.
                    let name_style = if dimmed {
                        theme.dim_style()
                    } else if is_sel {
                        theme.base_style().add_modifier(Modifier::BOLD)
                    } else {
                        theme.base_style()
                    };
                    let mut spans = vec![
                        Span::styled(format!("{cursor}    "), theme.base_style()),
                        self.checkbox(*ri, row, theme),
                        Span::styled(" ", theme.base_style()),
                        Span::styled(row.display_name.clone(), name_style),
                    ];
                    if let Some(suffix) = self.item_suffix(*ri, row, theme) {
                        spans.push(suffix);
                    }
                    lines.push(truncate_line_to_width(Line::from(spans), w, "..."));
                }
            }
        }

        // `:443-449` — the scroll readout. Both counters walk the **items only**: the denominator
        // is the number of `type === "item"` entries in the whole filtered list and the numerator
        // is how many precede the highlight, +1. Counting flat entries instead would report the
        // group/subgroup headers as resources.
        if start > 0 || end < self.flat.len() {
            let item_count = self.flat.iter().filter(|e| matches!(e, Flat::Item(_))).count();
            let current = self
                .flat
                .get(..self.selected)
                .unwrap_or(&[])
                .iter()
                .filter(|e| matches!(e, Flat::Item(_)))
                .count()
                .saturating_add(1);
            lines.push(Line::from(Span::styled(
                format!("  ({current}/{item_count})"),
                theme.dim_style(),
            )));
        }

        lines
    }

    /// The [`ConfigScope`] of the group header governing flat index `i` — the `entry.group.scope`
    /// upstream reads straight off the subgroup entry (`config-selector.ts:423`), recovered here by
    /// walking back to the nearest `Group`.
    fn group_scope_at(&self, i: usize) -> Option<ConfigScope> {
        for j in (0..=i).rev() {
            match self.flat.get(j) {
                Some(Flat::Group(label)) => {
                    return Some(if label.starts_with(ConfigScope::User.label()) {
                        ConfigScope::User
                    } else {
                        ConfigScope::Project
                    });
                }
                Some(Flat::Item(ri)) => {
                    if let Some(row) = self.rows.get(*ri) {
                        return Some(row.scope);
                    }
                }
                _ => {}
            }
        }
        None
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
            if !Self::matches_query(row, self.input.value()) {
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
    fn set_terminal_height(&mut self, rows: u16) {
        self.max_visible = Self::max_visible_for(rows);
    }

    fn desired_height(&self, width: u16) -> u16 {
        // Blank + top rule + blank + header (TWO rows) + blank + body + blank + bottom rule
        // (L4/SYS-3 — see `render`). The body term is WINDOWED at `max_visible`, exactly as
        // upstream's is (`config-selector.ts:266` → the `startIndex`/`endIndex` slice at
        // `:405-409`); an unbounded `flat.len()` here is what made the envelope unreachable on any
        // realistic resource list.
        let body = self.body_rows(width);
        body.saturating_add(CHROME_ROWS)
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        // L4/SYS-3. `ConfigSelectorComponent`'s child list (`config-selector.ts:901-930`):
        //   `Spacer`(:901) · `DynamicBorder`(:902) · `Spacer`(:903) · header(:905, two rows) ·
        //   `Spacer`(:906) · resourceList(:926) · `Spacer`(:929) · `DynamicBorder`(:930).
        // **Four** spacers, and — like `session-selector.ts:737` — the first sits *above* the top
        // rule.
        // Natural heights only — the blanks are unconditional, because upstream's `Spacer` children
        // are, and the body is windowed at `max_visible` rather than sized from the leftover rows.
        // `stack_rows` fills the regions from the TOP and starves the trailing ones, so the visible
        // rows are a prefix of the natural render; see its doc. On a slot
        // too short for the whole envelope the FIRST row is the `Spacer`(:901), not the rule —
        // upstream's is too.
        let header = self.header_lines(area.width, theme);
        let header_h = header.len().min(u16::MAX as usize) as u16;
        let lines = self.body_lines(area.width, theme);
        let body_h = lines.len().min(u16::MAX as usize) as u16;
        let [_, top, _, header_area, _, body, _, bottom] =
            crate::selector::stack_rows(area, [1, 1, 1, header_h, 1, body_h, 1, 1]);

        frame.render_widget(border_rule(top.width, theme), top);
        frame.render_widget(Paragraph::new(header).style(theme.base_style()), header_area);
        // The window is `maxVisible` rows and NOTHING else — `startIndex`/`endIndex`
        // (`config-selector.ts:405-409`) are computed from `this.maxVisible`, never from a box
        // height, because upstream's `ResourceList` has no box height. Deriving it from
        // `body.height` instead would re-centre the window as the slot shrank, so a one-row resize
        // would scroll the list; the `Paragraph` below already clips to `body`, which is what pi's
        // layout does one level up.
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
        // Space toggles the highlighted resource (Pi `data === " "`, config-selector.ts:497).
        //
        // There is deliberately NO `if key.modifiers.intersects(CONTROL|ALT|SUPER) { Ignored }`
        // guard here any more: upstream hands EVERY unclaimed key to the search `Input`
        // (`config-selector.ts:509-510`), and the `Input` performs the control-character rejection
        // itself (`input.ts:202-210`) — which is exactly where it now lives in cyrup too
        // ([`Input::handle_key`]'s `None` arm). The guard made Ctrl+W / Ctrl+U / Ctrl+K / Alt+B /
        // Alt+F / Alt+D unreachable in this dialog.
        match key.code {
            KeyCode::Char(' ') if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                => self.toggle_selected(),
            // `tui.input.tab` flips the write scope (`config-selector.ts:495-498` →
            // `switchWriteScope`, `:933-937`) — and only when project mode is available, because
            // upstream leaves `onSwitchMode` unset otherwise (`:920-925`).
            KeyCode::Tab if self.project_mode_available => {
                self.write_scope = match self.write_scope {
                    ConfigWriteScope::Global => ConfigWriteScope::Project,
                    ConfigWriteScope::Project => ConfigWriteScope::Global,
                };
                SelectorOutcome::Redraw
            }
            // "Pass to search input" (`:509-510`), then `filterItems(searchInput.getValue())`.
            _ => match self.input.handle_key(key) {
                InputOutcome::Edited => {
                    self.on_query_changed();
                    SelectorOutcome::Redraw
                }
                InputOutcome::Moved => SelectorOutcome::Redraw,
                InputOutcome::Ignored => SelectorOutcome::Ignored,
            },
        }
    }

    fn set_editor_keymap(&mut self, keymap: &crate::keymap::EditorKeymap) {
        self.input.set_editor_keymap(keymap);
    }

    fn handle_paste(&mut self, text: &str) -> SelectorOutcome {
        self.input.paste(text);
        self.on_query_changed();
        SelectorOutcome::Redraw
    }
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

//! Standalone-HTML session export — pi `core/export-html/index.ts` (`generateHtml`,
//! `exportSessionToHtml`, `exportFromFile`) @v0.84.4 (unchanged through v1.1.0), reached from
//! pi's `agent-session.ts:3427 exportToHtml`.
//!
//! pi renders the transcript into a **templated document**, not a text dump: `generateHtml`
//! (`index.ts:143-175`) base64-encodes a `SessionData{header, entries, leafId, systemPrompt, tools,
//! renderedTools}` payload into `<script id="session-data" type="application/json">`
//! (`template.html:42`) and substitutes five placeholders — `{{CSS}}`, `{{JS}}`,
//! `{{SESSION_DATA}}`, `{{MARKED_JS}}`, `{{HIGHLIGHT_JS}}` — while `template.css`'s own four
//! (`{{THEME_VARS}}`, `{{BODY_BG}}`, `{{CONTAINER_BG}}`, `{{INFO_BG}}`, `template.css:2-5`) carry
//! the ACTIVE theme's colours. The shipped `template.js` then rebuilds the entry **tree** from
//! `parentId`, walks it from `leafId`, renders markdown through `marked`, highlights code through
//! `highlight.js`, and offers a sidebar with search, five filters and per-branch navigation.
//!
//! cyrup used to emit a 131-line text dump instead: one `<pre>` per string found under a `"text"`
//! key, under a constant `#1e1e2e` stylesheet. That silently lost tool-call **arguments** (bash's
//! `command`, edit's diff — they sit under `arguments`), the tool name / call id / `isError` /
//! `details` of every result, all images, the whole branch structure (abandoned branches were
//! interleaved with the active path because `parentId`/`leafId` were ignored), markdown rendering,
//! syntax highlighting and the user's theme. `/export`, `/share`, `cyrup --export` and RPC
//! `export_html` all published that document. DRIFT-041.
//!
//! This module is the L5 seam every front-end shares (R-11-023): `cyrup-modes` RPC `export_html`
//! (through [`crate::AgentSession::export_to_html`]), `cyrup-tui`'s `/export` and `/share`, and
//! `cyrup --export`.
//!
//! **Assets.** `assets/{template.html,template.css,template.js}` and
//! `assets/vendor/{marked.min.js,highlight.min.js}` are byte-identical copies of pi v1.1.0's
//! `packages/coding-agent/src/core/export-html/` (`template.html` and `vendor/` are unchanged since
//! v0.84.4), `include_str!`-ed rather than read from a template directory at run time (pi's
//! `getExportTemplateDir()`, `index.ts:144`) because cyrup ships a single binary with no sibling
//! asset tree. `src/tests/export_html.rs` pins each file's
//! SHA-256 so a local edit cannot silently fork them from upstream. See `assets/vendor/README.md`
//! for the vendored libraries' provenance and licences.
//!
//! **Residual.** `ExportOptions.toolRenderer` / `preRenderCustomTools` (`index.ts:15-33`,
//! `:177-230`) — the `renderedTools` map that pre-renders EXTENSION tool calls and results through
//! their TUI renderers and converts the resulting ANSI to HTML (`export-html/tool-renderer.ts`,
//! `export-html/ansi-to-html.ts`) — is not ported. `template.js:1046` @v1.1.0 reads
//! `renderedTools?.[…]` and falls back to its own built-in rendering when the key is absent, so the
//! document is complete for every built-in tool and degrades only for a custom-rendered extension
//! tool. A port must target `tool-renderer.ts` @v1.1.0, which since v1.0.0 resolves renderers
//! through `getToolRenderers` (the `registerToolRenderer` chain, so an MCP tool renders before its
//! server connects) and passes `durationMs` / `outputPad` in its render context.
//!
//! That degradation is a LIVE-path gap, not a shape upstream also has. pi's `exportFromFile`
//! (`index.ts:288-316`) does pass no renderer — but pi's LIVE path always does:
//! `AgentSession.exportToHtml` builds `createToolHtmlRenderer(…)` unconditionally
//! (`agent-session.ts:3433-3437`) and hands it to `exportSessionToHtml` (`:3439-3443`), which
//! pre-renders the map into the payload (`index.ts:254-261`, `:269`). cyrup's `/export`, `/share`
//! and RPC `export_html` are all live paths, so citing the file entry point here would be excusing
//! a live gap with an unrelated call site — which is exactly the defence DRIFT-054 was filed
//! against. `cyrup --export`, the file path, matches upstream exactly.

mod color;

use std::collections::BTreeMap;
use std::sync::LazyLock;

use base64::Engine as _;
use cyrup_resources::color::Rgb;
use cyrup_resources::system_theme::{
    SystemThemeColors, SystemThemeInput, concrete_colors, generate_system_theme_colors,
};
use cyrup_resources::{ColorSpec, Theme};
use serde_json::{Map, Value};

pub use color::{CssColor, ExportBackdrops, ParseColorError, derive_export_colors};

const TEMPLATE_HTML: &str = include_str!("assets/template.html");
const TEMPLATE_CSS: &str = include_str!("assets/template.css");
const TEMPLATE_JS: &str = include_str!("assets/template.js");
const MARKED_JS: &str = include_str!("assets/vendor/marked.min.js");
const HIGHLIGHT_JS: &str = include_str!("assets/vendor/highlight.min.js");

/// pi's five `withThemeColorFallbacks` aliases (`modes/interactive/theme/theme.ts:164-178`
/// @v1.0.0): `(alias, source)` — the alias takes the source's value when the theme document does
/// not define it. Applied before the colours become CSS custom properties, because
/// `getResolvedThemeColors` runs `resolveThemeColors(withThemeColorFallbacks(colors), vars)`
/// (`theme.ts:1068`).
const COLOR_FALLBACKS: [(&str, &str); 5] = [
    ("scrollbarTrack", "muted"),
    ("scrollbarThumb", "text"),
    ("thinkingMax", "thinkingXhigh"),
    ("searchMatchBg", "selectedBg"),
    ("searchMatchText", "text"),
];

/// pi's `GUESSED_DEFAULT_COLORS` (`theme.ts:215-218` @v1.0.0): the terminal's default colours
/// when it reported none, by the appearance the theme is designed for. A token a theme sets to
/// `""` has no colour of its own — it is drawn in the terminal's default foreground (or, for a
/// background token, its default background) — and a browser needs a concrete value.
const GUESSED_DARK: (CssColor, CssColor) = (
    CssColor::from_rgb(0xe5, 0xe5, 0xe7),
    CssColor::from_rgb(0x00, 0x00, 0x00),
);
const GUESSED_LIGHT: (CssColor, CssColor) = (
    CssColor::from_rgb(0x00, 0x00, 0x00),
    CssColor::from_rgb(0xff, 0xff, 0xff),
);

/// pi's `BACKGROUND_TOKENS` (`theme.ts:573-581` @v1.0.0): the tokens a theme paints as a
/// background. A `""` among them is the terminal's default BACKGROUND; everywhere else it is the
/// default foreground.
const BACKGROUND_TOKENS: [&str; 7] = [
    "selectedBg",
    "searchMatchBg",
    "userMessageBg",
    "customMessageBg",
    "toolPendingBg",
    "toolSuccessBg",
    "toolErrorBg",
];

/// What the terminal said its default colours are, and whether it is light or dark — the inputs to
/// pi's `Theme.colors` getter (`theme.ts:321-338` @v1.0.0). An unreported colour is guessed from
/// the theme's appearance ([`GUESSED_DARK`], [`GUESSED_LIGHT`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalDefaults {
    /// OSC 10, if the terminal answered.
    pub foreground: Option<CssColor>,
    /// OSC 11, if the terminal answered.
    pub background: Option<CssColor>,
    /// Pi `getTerminalTheme()`: the appearance of a theme that declares none of its own.
    pub appearance: cyrup_resources::Appearance,
}

impl Default for TerminalDefaults {
    /// A terminal that reported nothing and is assumed dark, which is what a headless export has.
    fn default() -> Self {
        Self {
            foreground: None,
            background: None,
            appearance: cyrup_resources::Appearance::Dark,
        }
    }
}

impl TerminalDefaults {
    /// `(foreground, background)` for a theme of `appearance`: the reported colours, else the guess.
    fn resolve(&self, appearance: cyrup_resources::Appearance) -> (CssColor, CssColor) {
        let (guess_fg, guess_bg) = match appearance {
            cyrup_resources::Appearance::Dark => GUESSED_DARK,
            cyrup_resources::Appearance::Light => GUESSED_LIGHT,
        };
        (
            self.foreground.unwrap_or(guess_fg),
            self.background.unwrap_or(guess_bg),
        )
    }
}
/// `colors.userMessageBg || "#343541"` (`export-html/index.ts:120`, `:154` @v0.84.4).
const FALLBACK_USER_MESSAGE_BG: CssColor = CssColor::from_rgb(0x34, 0x35, 0x41);

/// The resolved palette one export is rendered with: pi's `generateThemeVars(themeName)` output
/// plus the three backdrop colours (`export-html/index.ts:111-128`, `:151-157` @v0.84.4).
///
/// Built in the *shell* — [`crate::AgentSession::export_theme`] reads the live theme name and the
/// session's discovered themes — and consumed by the pure renderer, so
/// [`session_jsonl_to_html_with_theme`] stays a total function of its two arguments with no
/// resource, settings or terminal access of its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportTheme {
    /// Every `colors` role, resolved through `vars`, keyed by pi's camelCase token name. Emitted as
    /// `--<token>: <color>;` into `{{THEME_VARS}}`.
    roles: BTreeMap<String, CssColor>,
    backdrops: ExportBackdrops,
}

/// The palette of an export with no TUI attached (headless `print`/`json`/`rpc` modes and
/// `cyrup --export`, which runs before any session exists): pi's `themeName ?? currentThemeName ??
/// SYSTEM_THEME_NAME` chain (`theme.ts:903-906` @v1.0.0) bottoming out in the generated `system`
/// theme. With no terminal behind it nothing was reported, so the generator runs at its third tier
/// (ANSI palette indices and the terminal's default colours) and `Theme.colors` resolves those
/// against the colours it guesses for the theme's appearance ([`ExportTheme::headless`]).
///
/// The appearance is the one pi's `getTerminalTheme()` falls back to without a terminal report:
/// `COLORFGBG`, then dark (`detectTerminalTheme`, `theme.ts:702-710`), read once at first use as pi
/// reads `process.env`.
static DEFAULT_EXPORT_THEME: LazyLock<ExportTheme> =
    LazyLock::new(|| ExportTheme::headless(ExportTheme::environment_appearance()));

impl Default for ExportTheme {
    fn default() -> Self {
        DEFAULT_EXPORT_THEME.clone()
    }
}

impl ExportTheme {
    /// The appearance pi assumes for a process with no terminal report: `COLORFGBG`'s background
    /// index, else dark (`detectColorFgBgTheme(env) ?? "dark"`, `theme.ts:689-710`).
    #[must_use]
    pub fn environment_appearance() -> cyrup_resources::Appearance {
        std::env::var("COLORFGBG")
            .ok()
            .as_deref()
            .and_then(cyrup_resources::Appearance::from_colorfgbg)
            .unwrap_or(cyrup_resources::Appearance::Dark)
    }

    /// The `system` theme of a terminal that reported nothing, which is an export with no TUI
    /// attached: pi's `generateSystemThemeColors({ saturation: 1, appearanceHint })` at its third
    /// tier, folded through `Theme.colors` (`theme.ts:321-338` @v1.0.0). `appearance_hint` is what
    /// `getTerminalTheme()` answers without a report ([`Self::environment_appearance`]).
    #[must_use]
    pub fn headless(appearance_hint: cyrup_resources::Appearance) -> Self {
        let generated = generate_system_theme_colors(&SystemThemeInput {
            appearance_hint: Some(appearance_hint),
            ..SystemThemeInput::default()
        });
        Self::from_system(&generated, None, None, appearance_hint)
    }

    /// A generated `system` theme as an export palette: every token resolved to the concrete colour
    /// pi's `Theme.colors` getter gives it ([`concrete_colors`]) against the terminal's reported
    /// default colours, else the guess for the theme's appearance. `terminal_appearance` is what a
    /// theme with no appearance of its own takes. The backdrops derive from `userMessageBg`
    /// ([`Self::from_resolved_roles`]).
    ///
    /// The one resolution both an interactive export of `system` and a headless export go through,
    /// so the two cannot disagree about what a token means.
    #[must_use]
    pub fn from_system(
        generated: &SystemThemeColors,
        foreground: Option<Rgb>,
        background: Option<Rgb>,
        terminal_appearance: cyrup_resources::Appearance,
    ) -> Self {
        let roles = concrete_colors(generated, foreground, background, terminal_appearance)
            .into_iter()
            .map(|(token, rgb)| (token.to_string(), CssColor::from_rgb(rgb.r, rgb.g, rgb.b)))
            .collect();
        Self::from_resolved_roles(roles)
    }

    /// pi `getResolvedThemeColors` + `getThemeExportColors` + `deriveExportColors`
    /// (`theme.ts:1064-1085`, `:1099-1125`; `export-html/index.ts:111-128`, `:151-157` @v0.84.4),
    /// against one already-loaded theme document.
    #[must_use]
    pub fn from_theme(theme: &Theme) -> Self {
        Self::from_theme_with(theme, &TerminalDefaults::default())
    }

    /// [`Self::from_theme`] for a terminal whose default colours are known: pi's `Theme.colors`
    /// getter fills every token set to `""` from them (`theme.ts:321-338` @v1.0.0), and
    /// `getResolvedThemeColors` exports that getter's output (`theme.ts:903-906`).
    #[must_use]
    pub fn from_theme_with(theme: &Theme, terminal: &TerminalDefaults) -> Self {
        // The theme's own appearance — declared, else detected from its colours — and only if it has
        // neither the terminal's (`Theme.appearance`, `theme.ts:313-315`).
        let appearance = theme.appearance().unwrap_or(terminal.appearance);
        let (default_fg, default_bg) = terminal.resolve(appearance);

        // An index exports as the standard xterm palette's RGB for it (`colorToHex(indexedColor(n))`,
        // `theme.ts:936`). `""` is the terminal's default foreground, or its default background for
        // a background token (`Theme.colors`, `theme.ts:326-332`).
        let to_css = |token: &str, spec: ColorSpec| match spec.to_rgb() {
            Some((r, g, b)) => CssColor::from_rgb(r, g, b),
            None if BACKGROUND_TOKENS.contains(&token) => default_bg,
            None => default_fg,
        };

        let mut roles: BTreeMap<String, CssColor> = theme
            .resolve()
            .roles // pi `resolveThemeColors(…, vars)` (`theme.ts:321-331`)
            .into_iter()
            .map(|(k, v)| {
                let css = to_css(&k, v);
                (k, css)
            })
            .collect();
        for (alias, source) in COLOR_FALLBACKS {
            if !roles.contains_key(alias)
                && let Some(v) = roles.get(source).copied()
            {
                roles.insert(alias.to_string(), v);
            }
        }

        let explicit = theme.resolve_export();
        let base = roles
            .get("userMessageBg")
            .copied()
            .unwrap_or(FALLBACK_USER_MESSAGE_BG);
        let derived = derive_export_colors(Some(base));
        // `themeExport.pageBg ?? derivedColors.pageBg` (`export-html/index.ts:123-125`, `:155-157`).
        // `resolve_export` maps an absent OR empty key to `Inherit`, which is pi's `undefined`
        // (`theme.ts:1120-1122` returns `undefined` for `""` too).
        let pick = |spec: ColorSpec, fallback: CssColor| match spec.to_rgb() {
            Some((r, g, b)) => CssColor::from_rgb(r, g, b),
            None => fallback,
        };
        let backdrops = ExportBackdrops {
            page_bg: pick(explicit.page_bg, derived.page_bg),
            card_bg: pick(explicit.card_bg, derived.card_bg),
            info_bg: pick(explicit.info_bg, derived.info_bg),
        };

        Self { roles, backdrops }
    }

    /// A palette whose roles were already resolved to concrete colours by the caller — the generated
    /// `system` theme, which exists only in the interactive TUI (`getResolvedThemeColors("system")`
    /// reads `createSystemTheme().colors`, `theme.ts:903-906`). The backdrops are derived from
    /// `userMessageBg`, because a generated theme has no `export` block
    /// (`getThemeExportColors`, `theme.ts:924-925`: `if (name === SYSTEM_THEME_NAME) return {}`).
    #[must_use]
    pub fn from_resolved_roles(roles: BTreeMap<String, CssColor>) -> Self {
        let base = roles
            .get("userMessageBg")
            .copied()
            .unwrap_or(FALLBACK_USER_MESSAGE_BG);
        Self {
            roles,
            backdrops: derive_export_colors(Some(base)),
        }
    }

    /// The three backdrop colours this palette resolved to.
    #[must_use]
    pub fn backdrops(&self) -> ExportBackdrops {
        self.backdrops
    }

    /// One resolved role, by pi's camelCase token name.
    ///
    /// Deliberately `pub` with no production caller: the whole palette leaves this type as CSS text
    /// through [`Self::theme_vars`], so the only way to assert that a NAMED role resolved the way
    /// pi's `generateThemeVars` resolves it is to read it back (`tests/export_html.rs`'s
    /// `optional_role_aliases_fall_back_the_way_pi_does`). Kept on the public surface rather than
    /// `cfg(test)` because it is also the accessor any future non-HTML export would want.
    #[must_use]
    pub fn role(&self, token: &str) -> Option<CssColor> {
        self.roles.get(token).copied()
    }

    /// pi `generateThemeVars` (`export-html/index.ts:111-128` @v0.84.4): one
    /// `--<token>: <color>;` per role plus the three `--export*` properties, joined by upstream's
    /// six-space continuation indent so the emitted `:root` block stays readable.
    ///
    /// [CYRUP-DELTA] pi emits the roles in the theme document's key order; cyrup emits them sorted,
    /// because `cyrup_resources::ResolvedTheme::roles` is a `BTreeMap`. CSS custom properties are
    /// order-independent unless a name repeats, and a JSON object cannot repeat a key.
    fn theme_vars(&self) -> String {
        let mut lines: Vec<String> = self
            .roles
            .iter()
            .map(|(k, v)| format!("--{k}: {v};"))
            .collect();
        lines.push(format!("--exportPageBg: {};", self.backdrops.page_bg));
        lines.push(format!("--exportCardBg: {};", self.backdrops.card_bg));
        lines.push(format!("--exportInfoBg: {};", self.backdrops.info_bg));
        lines.join("\n      ")
    }
}

/// One entry of the payload's `tools` array — pi's
/// `Pick<ToolDefinition, "name" | "description" | "parameters">` (`export-html/index.ts:135`
/// @v0.84.4), built from `state.tools.map((t) => ({ name, description, parameters }))` (`:268`).
///
/// `template.js:1448-1475` @v1.1.0 renders one row per element, expanding `parameters.properties`
/// with each property's `type`, its `required`/`optional` label and its description.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportTool {
    /// `ToolDefinition.name`.
    pub name: String,
    /// `ToolDefinition.description` — the text shown after the tool name.
    pub description: String,
    /// `ToolDefinition.parameters` — a JSON Schema object; the row expands it only when
    /// `parameters.properties` is a non-empty object (`template.js:1453`).
    pub parameters: Value,
}

/// The three `SessionData` keys the session JSONL itself cannot supply: pi reads them off the LIVE
/// `SessionManager` and `AgentState` (`export-html/index.ts:263-269` @v0.84.4), not off the file.
///
/// DRIFT-054. This type exists because a per-key positional parameter is exactly how the two agent
/// keys came to be missing: `systemPrompt` and `tools` are BOTH gated on pi's single `state?`
/// (`:267-268`), so they are supplied together or not at all, and holding them behind
/// [`Self::live`] with private fields makes "passed the leaf, forgot the prompt" unrepresentable.
///
/// The two shapes are pi's two call sites, and each is a NAMED constructor so that neither can be
/// reached by accident:
///
/// * [`Self::from_file`] is pi's `exportFromFile` shape (`:288-305`): `systemPrompt: undefined`,
///   `tools: undefined`, and the leaf derived from the file. That is `cyrup --export`'s path — a
///   file is all it has.
/// * [`Self::live`] is pi's `exportSessionToHtml` shape (`:263-270`), which is the ONLY entry point
///   `/export` (`interactive-mode.ts:6023`), `/share` (`session-share.ts:72`) and RPC `export_html`
///   (`rpc-mode.ts:601`) have, because `AgentSession.exportToHtml` always passes `this.state`
///   (`agent-session.ts:3439`).
///
/// WHAT THIS DOES NOT MAKE IMPOSSIBLE, stated plainly because the first version of this doc
/// overstated it: nothing at the TYPE level stops a live caller from handing the renderer a
/// [`Self::from_file`] value and losing both keys again — that is the original DRIFT-054 defect,
/// and it stays representable. What the type buys is that the two keys cannot be split from each
/// other, and that the lossy shape now has to be asked for by name (there is deliberately no
/// `Default` impl, which is the unnamed bypass the review found). What actually holds the live
/// paths on [`Self::live`] is the source-grep test in `src/tests/export_html.rs`
/// (`export_to_html_passes_the_live_session_state_to_the_renderer`) and its negative counterpart
/// (`the_file_only_export_omits_both_agent_keys_the_way_pi_does`), not the compiler.
///
/// Built in the imperative shell — [`crate::AgentSession::export_state`] — for the same reason
/// [`ExportTheme`] is, so the renderer stays a pure function of its arguments. `export_state` is
/// the only PRODUCER of a live value in this workspace, but [`Self::live`] is `pub`: the tests
/// build one directly, and so could a future caller.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportState {
    /// pi `sm.getLeafId()` (`index.ts:266` and `:301`).
    leaf_id: Option<String>,
    /// pi `state?.systemPrompt` (`:267`).
    system_prompt: Option<String>,
    /// pi `state?.tools?.map(...)` (`:268`). `None` is `undefined` — the key is omitted, as
    /// `JSON.stringify` omits it; `Some(vec![])` is an empty array, which `template.js:1448`'s
    /// `tools && tools.length > 0` guard renders identically but which is a different payload.
    tools: Option<Vec<ExportTool>>,
}

impl ExportState {
    /// The file-only shape: pi's `exportFromFile`, which opens a `SessionManager` over the file and
    /// passes NO `AgentState` at all — `systemPrompt: undefined, tools: undefined`
    /// (`export-html/index.ts:288-305` @v0.84.4). The leaf is left `None` here and derived from the
    /// file by [`session_jsonl_to_html`], which is what `SessionManager.open` would have seeded.
    ///
    /// Named rather than a `Default` impl on purpose: this shape LOSES both agent keys, so a caller
    /// that has a live session must not be able to reach it by writing `..Default::default()` or
    /// letting an inferred default fill a field in. `cyrup --export` is its only production caller.
    #[must_use]
    pub fn from_file() -> Self {
        Self {
            leaf_id: None,
            system_prompt: None,
            tools: None,
        }
    }

    /// The live-session shape: the manager's leaf plus the agent's system prompt and ACTIVE tool
    /// set (pi `exportSessionToHtml`, `export-html/index.ts:263-270` @v0.84.4).
    ///
    /// `leaf_id` stays an `Option` because `SessionManager::leaf_id` is one — a session with no
    /// entries yet has no leaf, and pi's `getLeafId()` is `string | null` for the same reason. The
    /// prompt and the tools are not optional here: a live session always has both.
    #[must_use]
    pub fn live(leaf_id: Option<String>, system_prompt: String, tools: Vec<ExportTool>) -> Self {
        Self {
            leaf_id,
            system_prompt: Some(system_prompt),
            tools: Some(tools),
        }
    }

    /// The leaf this export walks from, if the shell knew one.
    #[must_use]
    pub fn leaf_id(&self) -> Option<&str> {
        self.leaf_id.as_deref()
    }
}

/// The payload `template.js` decodes out of `<script id="session-data">` (pi `SessionData`,
/// `export-html/index.ts:130-138` @v0.84.4).
///
/// `header`, `entries` and `leafId` come from the transcript and the shell; `systemPrompt` and
/// `tools` come from [`ExportState`] and are present for every LIVE export, absent for the
/// file-only one — pi's two call sites exactly (`:263-270` vs `:298-304`). `renderedTools` is
/// never set; `template.js:15` destructures all three and every reader is `?.`-guarded.
///
/// `leaf_id` is pi's `sm.getLeafId()` (`index.ts:266` and `:301`), supplied by the shell. `None`
/// falls back to the last non-`session` line, which is `_buildIndex`'s own seeding rule
/// (`session-manager.ts:959-977`) and therefore exactly what pi's `exportFromFile` gets from a
/// `SessionManager.open`ed file. It is NOT a safe substitute for a live manager's leaf:
/// `SessionManager::branch` moves the leaf without appending and `reset_leaf` clears it, so after a
/// `/tree` branch switch with no new message the last file entry belongs to the ABANDONED branch —
/// pi's own `branch` / `branchWithSummary` reassign `this.leafId` for the same reason
/// (`session-manager.ts:1361-1365`, `:1393`), and `resetLeaf` nulls it (`:1373-1374`). Every caller that holds a manager passes `Some`.
fn session_data(jsonl: &str, state: &ExportState) -> Value {
    let mut lines = jsonl.lines().filter(|l| !l.trim().is_empty());
    // pi `sm.getHeader()` — `fileEntries[0]`, the `type: "session"` line.
    let header: Value = lines
        .next()
        .and_then(|l| serde_json::from_str(l).ok())
        .unwrap_or(Value::Null);

    // pi `sm.getEntries()` — every file entry EXCEPT the header (`session-manager.ts:1301`), and
    // `_buildIndex` (`:960-968`) skips `type === "session"` when it tracks the leaf.
    //
    // Unparseable lines are dropped rather than aborting the export: pi's own reader is
    // `parseSessionEntryLine`, a bare `JSON.parse` behind a blank/malformed → `null` guard
    // (`session-manager.ts:503-511`), so a corrupt line costs that entry and nothing else.
    let entries: Vec<Value> = lines
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v.get("type").and_then(Value::as_str) != Some("session"))
        .collect();

    let leaf = match state.leaf_id() {
        Some(id) => Value::String(id.to_string()),
        None => entries
            .last()
            .and_then(|e| e.get("id"))
            .cloned()
            .unwrap_or(Value::Null),
    };

    let mut map = Map::new();
    map.insert("header".to_string(), header);
    map.insert("entries".to_string(), Value::Array(entries));
    map.insert("leafId".to_string(), leaf);
    // `systemPrompt: state?.systemPrompt` (`index.ts:267`). Absent — not `null` — when the shell
    // has no agent state, because `JSON.stringify` drops an `undefined` value and `template.js`
    // reads it as `if (systemPrompt)`.
    if let Some(prompt) = &state.system_prompt {
        map.insert("systemPrompt".to_string(), Value::String(prompt.clone()));
    }
    // `tools: state?.tools?.map((t) => ({ name: t.name, description: t.description, parameters:
    // t.parameters }))` (`index.ts:268`) — the three fields upstream picks, and no others.
    if let Some(tools) = &state.tools {
        let rendered: Vec<Value> = tools
            .iter()
            .map(|t| {
                let mut entry = Map::new();
                entry.insert("name".to_string(), Value::String(t.name.clone()));
                entry.insert(
                    "description".to_string(),
                    Value::String(t.description.clone()),
                );
                entry.insert("parameters".to_string(), t.parameters.clone());
                Value::Object(entry)
            })
            .collect();
        map.insert("tools".to_string(), Value::Array(rendered));
    }
    Value::Object(map)
}

/// Render the session JSONL (`SessionManager::export_jsonl` output: header line + one entry per
/// line) into a standalone HTML document, using the compiled-in `dark` palette and deriving the
/// leaf from the file.
///
/// This is pi's `exportFromFile` shape exactly (`export-html/index.ts:288-305` @v0.84.4): a file is
/// all there is, so the leaf is whatever `_buildIndex` would seed from it and `systemPrompt` /
/// `tools` are `undefined`. Callers that hold a live session must use
/// [`session_jsonl_to_html_with_theme`] with [`crate::AgentSession::export_state`]; see
/// [`ExportState`].
#[must_use]
pub fn session_jsonl_to_html(jsonl: &str) -> String {
    session_jsonl_to_html_with_theme(jsonl, &ExportTheme::default(), &ExportState::from_file())
}

/// pi `generateHtml(sessionData, themeName)` (`export-html/index.ts:143-175` @v0.84.4).
///
/// Pure: the same `(jsonl, theme, leaf_id)` always yields the same document, and nothing here
/// touches the filesystem, the clock or the resource registry. Never panics — a malformed or empty
/// transcript still produces a valid document (the template renders an empty session).
///
/// `state` is the shell's, not the renderer's, exactly as pi's is: `generateHtml` is handed a
/// `SessionData` already carrying `sm.getLeafId()`, `state?.systemPrompt` and `state?.tools`
/// (`index.ts:263-270`). See [`ExportState`] for why the leaf must not be re-derived here, and why
/// the two agent keys travel with it rather than as separate parameters.
#[must_use]
pub fn session_jsonl_to_html_with_theme(
    jsonl: &str,
    theme: &ExportTheme,
    state: &ExportState,
) -> String {
    let data = session_data(jsonl, state);
    // `Buffer.from(JSON.stringify(sessionData)).toString("base64")` (`index.ts:160`). Base64 is
    // what makes the payload injection-proof: no transcript byte can close the `<script>` element,
    // which is why nothing on this path is HTML-escaped.
    let encoded = base64::engine::general_purpose::STANDARD.encode(data.to_string());

    // `index.ts:163-167`. `String.prototype.replace(string, string)` substitutes the FIRST match
    // only, which `replacen(_, _, 1)` mirrors exactly.
    let backdrops = theme.backdrops();
    let css = TEMPLATE_CSS
        .replacen("{{THEME_VARS}}", &theme.theme_vars(), 1)
        .replacen("{{BODY_BG}}", &backdrops.page_bg.to_string(), 1)
        .replacen("{{CONTAINER_BG}}", &backdrops.card_bg.to_string(), 1)
        .replacen("{{INFO_BG}}", &backdrops.info_bg.to_string(), 1);

    // `index.ts:169-174`, in upstream's order (the substituted CSS is in the document before
    // `{{JS}}` is looked for, so a `{{JS}}` literal inside the stylesheet would be consumed there
    // — preserved rather than "fixed", since no cyrup behaviour may differ from pi's here).
    TEMPLATE_HTML
        .replacen("{{CSS}}", &css, 1)
        .replacen("{{JS}}", TEMPLATE_JS, 1)
        .replacen("{{SESSION_DATA}}", &encoded, 1)
        .replacen("{{MARKED_JS}}", MARKED_JS, 1)
        .replacen("{{HIGHLIGHT_JS}}", HIGHLIGHT_JS, 1)
}

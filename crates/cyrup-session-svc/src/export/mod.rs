//! Standalone-HTML session export — pi `core/export-html/index.ts` (`generateHtml`,
//! `exportSessionToHtml`, `exportFromFile`) @v0.84.4, reached from pi's
//! `agent-session.ts:3022 exportToHtml`.
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
//! `assets/vendor/{marked.min.js,highlight.min.js}` are byte-identical copies of pi v0.84.4's
//! `packages/coding-agent/src/core/export-html/`, `include_str!`-ed rather than read from a
//! template directory at run time (pi's `getExportTemplateDir()`, `index.ts:144`) because cyrup
//! ships a single binary with no sibling asset tree. `src/tests/export_html.rs` pins each file's
//! SHA-256 so a local edit cannot silently fork them from upstream. See `assets/vendor/README.md`
//! for the vendored libraries' provenance and licences.
//!
//! **Residual.** `ExportOptions.toolRenderer` / `preRenderCustomTools` (`index.ts:15-33`,
//! `:177-230`) — the `renderedTools` map that pre-renders EXTENSION tool calls and results through
//! their TUI renderers and converts the resulting ANSI to HTML (`export-html/tool-renderer.ts`,
//! `export-html/ansi-to-html.ts`) — is not ported. `template.js:1026` reads `renderedTools?.[…]`
//! and falls back to its own built-in rendering when the key is absent, which is exactly the shape
//! pi's own `exportFromFile` (`index.ts:288-316`) produces: it passes no renderer either. So the
//! document is complete for every built-in tool and degrades for a custom-rendered one precisely
//! the way upstream's file-mode export does.

mod color;

use std::collections::BTreeMap;
use std::sync::LazyLock;

use base64::Engine as _;
use cyrup_resources::{ColorSpec, Theme};
use serde_json::{Map, Value};

pub use color::{CssColor, ExportBackdrops, ParseColorError, derive_export_colors};

const TEMPLATE_HTML: &str = include_str!("assets/template.html");
const TEMPLATE_CSS: &str = include_str!("assets/template.css");
const TEMPLATE_JS: &str = include_str!("assets/template.js");
const MARKED_JS: &str = include_str!("assets/vendor/marked.min.js");
const HIGHLIGHT_JS: &str = include_str!("assets/vendor/highlight.min.js");

/// pi's four `withThemeColorFallbacks` aliases (`modes/interactive/theme/theme.ts:332-346`
/// @v0.84.4): `(alias, source)` — the alias takes the source's value when the theme document does
/// not define it. Applied before the colours become CSS custom properties, because
/// `getResolvedThemeColors` runs `resolveThemeColors(withThemeColorFallbacks(colors), vars)`
/// (`theme.ts:1068`).
const COLOR_FALLBACKS: [(&str, &str); 4] = [
    ("thinkingMax", "thinkingXhigh"),
    ("scrollbarThumb", "selectedBg"),
    ("searchMatchBg", "selectedBg"),
    ("searchMatchText", "text"),
];

/// pi's substitute for a role the theme leaves empty — "" means "the terminal's default
/// foreground", which has no meaning in a browser (`theme.ts:1071-1072` @v0.84.4).
const DEFAULT_TEXT_DARK: CssColor = CssColor::from_rgb(0xe5, 0xe5, 0xe7);
const DEFAULT_TEXT_LIGHT: CssColor = CssColor::from_rgb(0x00, 0x00, 0x00);
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

/// The compiled-in `dark` theme's palette — pi's `themeName ?? currentThemeName ?? getDefaultTheme()`
/// chain (`theme.ts:1065`) bottoming out when no live theme is attached (headless `print`/`json`/`rpc`
/// modes and `cyrup --export`, which runs before any session exists).
///
/// [CYRUP-DELTA] pi's `getDefaultTheme()` (`theme.ts:833-835`) probes the TERMINAL background and
/// can answer `light`; this seam has no terminal to probe (RPC mode and `--export` are not attached
/// to one), so the default is the `dark` built-in. An interactive export goes through
/// [`crate::AgentSession::export_theme`] and carries the user's real theme either way.
static DEFAULT_EXPORT_THEME: LazyLock<ExportTheme> = LazyLock::new(|| {
    match Theme::parse(
        cyrup_resources::BUILTIN_DARK_JSON,
        None,
        cyrup_resources::ResourceScope::Builtin,
        cyrup_resources::ResourceOrigin::Builtin,
    ) {
        Ok(theme) => ExportTheme::from_theme(&theme),
        // Unreachable — the built-in is validated by `cyrup-resources`' own tests — but the
        // no-panic policy (R-00-009) forbids resolving it with `expect`.
        Err(_) => ExportTheme {
            roles: BTreeMap::new(),
            backdrops: derive_export_colors(None),
        },
    }
});

impl Default for ExportTheme {
    fn default() -> Self {
        DEFAULT_EXPORT_THEME.clone()
    }
}

impl ExportTheme {
    /// pi `getResolvedThemeColors` + `getThemeExportColors` + `deriveExportColors`
    /// (`theme.ts:1064-1085`, `:1099-1125`; `export-html/index.ts:111-128`, `:151-157` @v0.84.4),
    /// against one already-loaded theme document.
    #[must_use]
    pub fn from_theme(theme: &Theme) -> Self {
        // `isLight` is upstream's NAME test, not a luminance test (`theme.ts:1067`, and
        // `isLightTheme`, `:1090-1093`: "Currently just check the name").
        let default_text = if theme.data.name == "light" || theme.key.as_str() == "light" {
            DEFAULT_TEXT_LIGHT
        } else {
            DEFAULT_TEXT_DARK
        };

        let spec_to_css = |spec: ColorSpec| match spec {
            ColorSpec::Rgb { r, g, b } => CssColor::from_rgb(r, g, b),
            // `value === "" → defaultText` (`theme.ts:1078-1080`). `cyrup-resources` also lands an
            // unresolvable var reference here, where pi would throw out of `loadThemeJson` and
            // `getThemeExportColors` would swallow it (`:1116`); degrading one role is strictly
            // closer to upstream's rendering than losing the whole palette.
            ColorSpec::Inherit => default_text,
        };

        let mut roles: BTreeMap<String, CssColor> = theme
            .resolve()
            .roles // pi `resolveThemeColors(…, vars)` (`theme.ts:321-331`)
            .into_iter()
            .map(|(k, v)| (k, spec_to_css(v)))
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
        let pick = |spec: ColorSpec, fallback: CssColor| match spec {
            ColorSpec::Rgb { r, g, b } => CssColor::from_rgb(r, g, b),
            ColorSpec::Inherit => fallback,
        };
        let backdrops = ExportBackdrops {
            page_bg: pick(explicit.page_bg, derived.page_bg),
            card_bg: pick(explicit.card_bg, derived.card_bg),
            info_bg: pick(explicit.info_bg, derived.info_bg),
        };

        Self { roles, backdrops }
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

/// The payload `template.js` decodes out of `<script id="session-data">` (pi `SessionData`,
/// `export-html/index.ts:130-138` @v0.84.4).
///
/// `systemPrompt`, `tools` and `renderedTools` are absent, which is what `JSON.stringify` produces
/// for pi's own `exportFromFile` (`index.ts:298-304` sets the first two `undefined` and never sets
/// the third); `template.js:15` destructures them and every reader is `?.`-guarded.
///
/// `leaf_id` is pi's `sm.getLeafId()` (`index.ts:266` and `:301`), supplied by the shell. `None`
/// falls back to the last non-`session` line, which is `_buildIndex`'s own seeding rule
/// (`session-manager.ts:959-977`) and therefore exactly what pi's `exportFromFile` gets from a
/// `SessionManager.open`ed file. It is NOT a safe substitute for a live manager's leaf:
/// `SessionManager::branch` moves the leaf without appending and `reset_leaf` clears it, so after a
/// `/tree` branch switch with no new message the last file entry belongs to the ABANDONED branch —
/// pi's own `branch` / `branchWithSummary` reassign `this.leafId` for the same reason
/// (`session-manager.ts:1361-1365`, `:1393`), and `resetLeaf` nulls it (`:1373-1374`). Every caller that holds a manager passes `Some`.
fn session_data(jsonl: &str, leaf_id: Option<&str>) -> Value {
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

    let leaf = match leaf_id {
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
    Value::Object(map)
}

/// Render the session JSONL (`SessionManager::export_jsonl` output: header line + one entry per
/// line) into a standalone HTML document, using the compiled-in `dark` palette and deriving the
/// leaf from the file.
///
/// This is pi's `exportFromFile` shape exactly (`export-html/index.ts:288-305` @v0.84.4): a file is
/// all there is, so the leaf is whatever `_buildIndex` would seed from it. Callers that hold a live
/// `SessionManager` must use [`session_jsonl_to_html_with_theme`] and pass its
/// `SessionManager::leaf_id`; see [`session_data`].
#[must_use]
pub fn session_jsonl_to_html(jsonl: &str) -> String {
    session_jsonl_to_html_with_theme(jsonl, &ExportTheme::default(), None)
}

/// pi `generateHtml(sessionData, themeName)` (`export-html/index.ts:143-175` @v0.84.4).
///
/// Pure: the same `(jsonl, theme, leaf_id)` always yields the same document, and nothing here
/// touches the filesystem, the clock or the resource registry. Never panics — a malformed or empty
/// transcript still produces a valid document (the template renders an empty session).
///
/// `leaf_id` is the shell's, not the renderer's, exactly as pi's is (`index.ts:266` passes
/// `sm.getLeafId()` into `generateHtml`) — see [`session_data`] for why deriving it here is wrong
/// for a live session.
#[must_use]
pub fn session_jsonl_to_html_with_theme(
    jsonl: &str,
    theme: &ExportTheme,
    leaf_id: Option<&str>,
) -> String {
    let data = session_data(jsonl, leaf_id);
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

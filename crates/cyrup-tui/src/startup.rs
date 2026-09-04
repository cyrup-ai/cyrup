//! The startup **loaded-resources / diagnostics** panel — Pi `showLoadedResources`
//! (`coding-agent/src/modes/interactive/interactive-mode.ts:1480-1690`) plus its
//! `formatDiagnostics` helper (`:1427-1478`).
//!
//! Pi prints two things at boot:
//!
//! * a **listing** of what loaded — `[Context]`, `[Skills]`, `[Prompts]`, `[Extensions]`,
//!   `[Themes]` — gated on `force || options.verbose || !getQuietStartup()` (`:1488`);
//! * **diagnostics** — `[Skill conflicts]`, `[Prompt conflicts]`, `[Extension issues]`,
//!   `[Theme conflicts]` — gated on `showListing || showDiagnosticsWhenQuiet` (`:1489`), and the
//!   boot call site passes `{force: false, showDiagnosticsWhenQuiet: true}` (`:1769`). So a
//!   `quietStartup` user still sees everything that went WRONG; only the inventory is suppressed.
//!
//! cyrup had neither: a shadowed skill, a prompt path that does not exist, or an extension that
//! failed to instantiate was completely invisible (TUI-006). `--verbose` even advertised itself as
//! overriding `quietStartup` (`cli.rs:818`) while only ever setting the log level.
//!
//! # Divergences from pi — UNPORTED WORK
//!
//! Earlier revisions headed this section "Deliberate divergences (ADR-0001)". No ADR document
//! exists in this workspace, so that citation asserted an authority nothing here can verify, and it
//! read as permission to stop. It was not. Everything below is work: the port's goal is behavioural
//! equivalence, and where the mechanism must differ (committed entries live in the terminal's own
//! scrollback and cannot be re-rendered) the BEHAVIOUR still has to be reached by another route.
//!
//! * Pi wraps each listing section in an `ExpandableText` bound to `getStartupExpansionState()`, so
//!   `Ctrl+O` swaps a comma-joined summary for a per-path breakdown. cyrup's committed entries are
//!   handed to `Terminal::insert_before` and live in the terminal's own scrollback, where they can
//!   never be re-rendered — so the form is decided ONCE here. We commit the **expanded** body
//!   (per-path/per-name lines), which is the strictly more informative of Pi's two, and there is no
//!   runtime toggle. Same concession `Entry::CompactionSummary` already makes.
//! * Pi's `formatDiagnostics` nests colours (`theme.fg("dim", "  ✓ " + …)` with the tick itself in
//!   `success`); the spans below carry that split faithfully.

use crate::theme::UiTheme;
use ratatui::text::{Line, Span};

/// The colour role a startup line's span is painted with (Pi `theme.fg(...)` role names).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartupRole {
    /// Section header for a listing block — `mdHeading` (`:1494`).
    Heading,
    /// Listing bodies — `dim` (`:1500`).
    Dim,
    /// Diagnostic headers and `warning`-type diagnostics (`:1645`, `:1470`).
    Warning,
    /// `error`-type diagnostics (`:1470` picks `error` over `warning` by `d.type`).
    Error,
    /// The collision winner's `✓` (`:1451`).
    Success,
}

/// One span of a startup line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartupSpan {
    pub text: String,
    pub role: StartupRole,
}

impl StartupSpan {
    fn new(text: impl Into<String>, role: StartupRole) -> Self {
        StartupSpan {
            text: text.into(),
            role,
        }
    }
}

/// One rendered row of the panel. An empty `spans` is Pi's `Spacer(1)` between sections.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct StartupLine {
    pub spans: Vec<StartupSpan>,
}

impl StartupLine {
    fn of(spans: Vec<StartupSpan>) -> Self {
        StartupLine { spans }
    }
    fn single(text: impl Into<String>, role: StartupRole) -> Self {
        StartupLine {
            spans: vec![StartupSpan::new(text, role)],
        }
    }
    fn blank() -> Self {
        StartupLine::default()
    }
    /// The row's text, ignoring colour (test/inspection).
    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }
}

/// Severity of one diagnostic — Pi `ResourceDiagnostic.type` (`resource-loader.ts:8`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Warning,
    Error,
    Collision,
}

/// The winner/loser pair of a same-name `collision` diagnostic (`skills.ts:415-424`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticCollision {
    pub name: String,
    pub winner: String,
    pub loser: String,
}

/// One diagnostic, in the shape `formatDiagnostics` consumes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartupDiagnostic {
    pub severity: DiagnosticSeverity,
    pub message: String,
    /// Display path; `None` renders Pi's path-less `  {message}` form (`:1474`).
    pub path: Option<String>,
    pub collision: Option<DiagnosticCollision>,
}

impl StartupDiagnostic {
    /// A non-collision diagnostic.
    pub fn plain(
        severity: DiagnosticSeverity,
        path: Option<String>,
        message: impl Into<String>,
    ) -> Self {
        StartupDiagnostic {
            severity,
            message: message.into(),
            path,
            collision: None,
        }
    }
}

/// Everything the panel needs, already resolved to display strings by the caller (the front-end owns
/// path shortening — Pi's `formatDisplayPath`/`getShortPath`).
#[derive(Clone, Debug, Default)]
pub struct StartupReport {
    /// `--verbose` (Pi `this.options.verbose`, `:1488`).
    pub verbose: bool,
    /// `settings.quietStartup` (Pi `getQuietStartup()`, `:1488`).
    pub quiet_startup: bool,
    /// System-prompt source + appended prompts + `AGENTS.md`/`CLAUDE.md` (`:1551-1555`). Order is
    /// meaningful, so this list is NOT sorted (`{sort: false}`, `:1563`).
    pub context_files: Vec<String>,
    /// Loaded skill names.
    pub skills: Vec<String>,
    /// Loaded prompt-template commands, already `/`-prefixed by the caller (Pi `/${template.name}`).
    pub prompts: Vec<String>,
    /// Loaded (non-hidden) extension labels.
    pub extensions: Vec<String>,
    /// Loaded CUSTOM themes only — Pi filters out the built-ins (`t.sourcePath`, `:1615`).
    pub themes: Vec<String>,
    pub skill_diagnostics: Vec<StartupDiagnostic>,
    pub prompt_diagnostics: Vec<StartupDiagnostic>,
    pub extension_diagnostics: Vec<StartupDiagnostic>,
    pub theme_diagnostics: Vec<StartupDiagnostic>,
}

impl StartupReport {
    /// Assemble the panel's input from a live session — pi reads the same collaborators inside
    /// `showLoadedResources` itself, so the report has to be derivable from a bare
    /// [`cyrup_session_svc::AgentSession`] and nothing else (TUI-N02). The listing halves come
    /// from the session's own resource snapshot / context store / extension host; the
    /// diagnostics half comes from
    /// `AgentSessionServices::startup_diagnostics`, which the builder retains instead of discarding
    /// (`showLoadedResources`, `interactive-mode.ts:1693-1907` @v0.84.4).
    ///
    /// This lived in the binary (`crates/cyrup/src/interactive.rs::build_startup_report`) while the
    /// panel had exactly one call site, the boot path. pi has two — boot
    /// (`bindCurrentSessionExtensions`, `:1982`) and `/reload` (`handleReloadCommand`, `:5991`) —
    /// and the second one is inside the run loop, which lives in this crate, so the builder moved
    /// here with it.
    ///
    /// `install_extension_shortcuts` must already have run for this session: EXT-039's
    /// reserved-key warnings are recorded by `resolve_shortcut_specs` and folded into
    /// `extension_diagnostics` below, exactly as upstream orders `setupExtensionShortcuts` ahead of
    /// `showLoadedResources` at both call sites (`:1985-1986`, `:5990-5991`).
    pub fn from_session(session: &cyrup_session_svc::AgentSession, verbose: bool) -> Self {
        use cyrup_resources::ResourceKind;
        let services = session.services();
        let home = Some(services.home.as_path());
        let snapshot = services.context.snapshot();
        Self {
            verbose,
            quiet_startup: services.settings.effective().quiet_startup(),
            // Pi's `[Context]` list is the system-prompt source + appended prompts + the `AGENTS.md`
            // chain, in load order (`:1551-1555`, `{sort: false}`).
            // `formatContextPath` (interactive-mode.ts:1334-1343) tries the CWD-RELATIVE form FIRST
            // and only falls back to display formatting:
            //
            //     const relativePath = getCwdRelativePath(absolutePath, cwd);
            //     if (relativePath !== undefined) { return relativePath; }
            //     return this.formatDisplayPath(absolutePath);
            //
            // Only the fallback leg was ported, so a project `AGENTS.md` INSIDE the cwd listed as
            // `~/…` or as a full absolute path where pi lists it `AGENTS.md`.
            // `cyrup_tools::path::cwd_relative_path` is the `getCwdRelativePath` port; it already
            // existed and simply was not reached from here.
            context_files: snapshot
                .context_files
                .iter()
                .map(|f| {
                    // Returned RAW, not posix-normalised: `formatContextPath` hands back
                    // `relativePath` as-is. The `.split(sep).join("/")` fold belongs to
                    // `formatPathRelativeToCwdOrAbsolute` (`utils/paths.ts:119-122`), a different
                    // function that this call site does not use.
                    cyrup_tools::path::cwd_relative_path(&f.path, &services.cwd).map_or_else(
                        || display_path(&f.path, home),
                        |rel| rel.display().to_string(),
                    )
                })
                .collect(),
            skills: services
                .resources
                .skills
                .all()
                .iter()
                .map(|s| s.name.clone())
                .collect(),
            // Prompt templates list as their slash command (Pi `/${template.name}`, `:1596`).
            prompts: services
                .resources
                .prompts
                .all()
                .iter()
                .map(|p| format!("/{}", p.name))
                .collect(),
            extensions: services
                .ext_host
                .loaded_ids()
                .iter()
                .map(|id| id.to_string())
                .collect(),
            // Built-ins are excluded — Pi lists only themes with a `sourcePath` (`:1615`).
            themes: services
                .resources
                .themes
                .all()
                .iter()
                .filter(|t| t.origin_path.is_some())
                .map(|t| t.data.name.clone())
                .collect(),
            skill_diagnostics: resource_diagnostics(
                &services.startup_diagnostics.resources,
                ResourceKind::Skill,
                home,
            ),
            prompt_diagnostics: resource_diagnostics(
                &services.startup_diagnostics.resources,
                ResourceKind::Prompt,
                home,
            ),
            // The whole extension vector, Pi-faithfully (`:1660-1665` maps every recorded error into
            // the block). In practice only the NON-fatal entries — the project-trust skips — are
            // reachable here: a genuine load failure is reported and exits 1 at
            // `report_runtime_diagnostics`, well before this panel is built, exactly as Pi's
            // `main.ts:843-849` precedes `InteractiveMode`.
            // EXT-039 — the shortcut-resolution warnings join the load failures in the ONE
            // `[Extension issues]` block, appended last, exactly as upstream folds
            // `getShortcutDiagnostics()` in after the command diagnostics
            // (`interactive-mode.ts:1884-1886` @v0.84.4).
            extension_diagnostics: {
                let mut diags =
                    extension_diagnostics(&services.startup_diagnostics.extensions, home);
                diags.extend(shortcut_diagnostics(
                    &services.ext_host.shortcut_diagnostics(),
                ));
                diags
            },
            theme_diagnostics: resource_diagnostics(
                &services.startup_diagnostics.resources,
                ResourceKind::Theme,
                home,
            ),
        }
    }

    /// Pi `showListing` (`:1702` @v0.84.4): `force || verbose || !quietStartup`. cyrup has no
    /// `force` and does not need one — `force` is dead upstream at this tag, both of pi's call
    /// sites passing `{force: false, showDiagnosticsWhenQuiet: true}` (`:1982`, `:5991-5994`) — so
    /// `--verbose` is the only override, which is exactly what `cli.rs`'s help text has been
    /// promising all along.
    pub fn show_listing(&self) -> bool {
        self.verbose || !self.quiet_startup
    }

    /// Whether there is any diagnostic at all.
    pub fn has_diagnostics(&self) -> bool {
        !self.skill_diagnostics.is_empty()
            || !self.prompt_diagnostics.is_empty()
            || !self.extension_diagnostics.is_empty()
            || !self.theme_diagnostics.is_empty()
    }
}

/// `shortenPath` (`render-utils.ts:10-17`): a leading `home` becomes `~`. Passed in rather than read
/// from `$HOME` so the mapping is testable.
pub fn display_path(path: &std::path::Path, home: Option<&std::path::Path>) -> String {
    if let Some(home) = home
        && !home.as_os_str().is_empty()
        && let Ok(rest) = path.strip_prefix(home)
    {
        let rest = rest.display().to_string();
        return if rest.is_empty() {
            "~".to_string()
        } else {
            format!("~/{rest}")
        };
    }
    path.display().to_string()
}

/// Project the discovery pass's [`cyrup_resources::ResourceDiagnostic`]s of ONE family onto the
/// panel's shape (Pi keeps them pre-split via `getSkills()`/`getPrompts()`/`getThemes()`).
pub fn resource_diagnostics(
    diagnostics: &[cyrup_resources::ResourceDiagnostic],
    kind: cyrup_resources::ResourceKind,
    home: Option<&std::path::Path>,
) -> Vec<StartupDiagnostic> {
    use cyrup_resources::DiagnosticType;
    diagnostics
        .iter()
        .filter(|d| d.resource_type == kind)
        .map(|d| StartupDiagnostic {
            severity: match d.diagnostic_type {
                DiagnosticType::Warning => DiagnosticSeverity::Warning,
                DiagnosticType::Error => DiagnosticSeverity::Error,
                DiagnosticType::Collision => DiagnosticSeverity::Collision,
            },
            message: d.message.clone(),
            path: Some(display_path(&d.path, home)),
            collision: d.collision.as_ref().map(|c| DiagnosticCollision {
                name: c.name.clone(),
                winner: display_path(&c.winner_path, home),
                loser: display_path(&c.loser_path, home),
            }),
        })
        .collect()
}

/// Project per-path extension load failures onto the panel's shape — Pi pushes each as
/// `{type: "error", message: error.error, path: error.path}` (interactive-mode.ts:1660-1665).
pub fn extension_diagnostics(
    errors: &[cyrup_session_svc::ExtensionLoadDiagnostic],
    home: Option<&std::path::Path>,
) -> Vec<StartupDiagnostic> {
    errors
        .iter()
        .map(|e| {
            StartupDiagnostic::plain(
                DiagnosticSeverity::Error,
                Some(display_path(&e.path, home)),
                e.error.clone(),
            )
        })
        .collect()
}

/// Project the extension-shortcut conflict warnings onto the panel's shape — EXT-039.
///
/// Upstream appends `extensionRunner.getShortcutDiagnostics()` to the SAME `extensionDiagnostics`
/// vector the per-path load errors go into, immediately after the command diagnostics and just
/// before the `[Extension issues]` block is rendered
/// (`modes/interactive/interactive-mode.ts:1884-1886` @v0.84.4). Each is
/// `{type: "warning", message, path: extensionPath}` (`extensions/runner.ts:549-553`), so they
/// carry [`DiagnosticSeverity::Warning`] here and the load failures keep their `Error`.
///
/// [`cyrup_ext::ExtensionConflict::path`] is the extension ID, not a filesystem path — cyrup's
/// stand-in for pi's `extensionPath` — so it is passed through verbatim rather than shortened.
pub fn shortcut_diagnostics(conflicts: &[cyrup_ext::ExtensionConflict]) -> Vec<StartupDiagnostic> {
    conflicts
        .iter()
        .map(|c| {
            StartupDiagnostic::plain(
                DiagnosticSeverity::Warning,
                Some(c.path.to_string()),
                c.message.clone(),
            )
        })
        .collect()
}

/// Build the panel (Pi `showLoadedResources`). Returns an empty vec when there is nothing to show —
/// a quiet startup with no problems prints nothing at all, exactly like Pi.
pub fn build_startup_lines(report: &StartupReport) -> Vec<StartupLine> {
    let mut out: Vec<StartupLine> = Vec::new();

    if report.show_listing() {
        // Pi's order: Context, Skills, Prompts, Extensions, Themes (`:1550-1638`).
        push_listing(&mut out, "Context", &report.context_files, false);
        push_listing(&mut out, "Skills", &report.skills, true);
        push_listing(&mut out, "Prompts", &report.prompts, true);
        push_listing(&mut out, "Extensions", &report.extensions, true);
        push_listing(&mut out, "Themes", &report.themes, true);
    }

    // Diagnostics are shown even under `quietStartup` (`showDiagnosticsWhenQuiet: true`, `:1769`).
    push_diagnostics(&mut out, "Skill conflicts", &report.skill_diagnostics);
    push_diagnostics(&mut out, "Prompt conflicts", &report.prompt_diagnostics);
    push_diagnostics(&mut out, "Extension issues", &report.extension_diagnostics);
    push_diagnostics(&mut out, "Theme conflicts", &report.theme_diagnostics);

    // Drop the trailing `Spacer(1)`; the transcript already separates entries.
    while out.last().is_some_and(|l| l.spans.is_empty()) {
        out.pop();
    }
    out
}

/// One `addLoadedSection` (`:1502-1516`): the `[Name]` header, one indented line per item, a spacer.
fn push_listing(out: &mut Vec<StartupLine>, name: &str, items: &[String], sort: bool) {
    let mut labels: Vec<&str> = items
        .iter()
        .map(|i| i.trim())
        .filter(|i| !i.is_empty())
        .collect();
    if labels.is_empty() {
        return;
    }
    if sort {
        labels.sort_unstable();
    }
    out.push(StartupLine::single(
        format!("[{name}]"),
        StartupRole::Heading,
    ));
    for label in labels {
        out.push(StartupLine::single(format!("  {label}"), StartupRole::Dim));
    }
    out.push(StartupLine::blank());
}

/// One diagnostic block (`:1641-1690`): a `warning`-styled `[Name]` header, the formatted
/// diagnostics, a spacer. Nothing at all when the list is empty.
fn push_diagnostics(out: &mut Vec<StartupLine>, name: &str, diagnostics: &[StartupDiagnostic]) {
    if diagnostics.is_empty() {
        return;
    }
    out.push(StartupLine::single(
        format!("[{name}]"),
        StartupRole::Warning,
    ));
    out.extend(format_diagnostics(diagnostics));
    out.push(StartupLine::blank());
}

/// Pi `formatDiagnostics` (`:1427-1478`): `collision` diagnostics are grouped by name and printed
/// as one winner (`✓`) plus every shadowed loser (`✗ … (skipped)`); everything else prints its path
/// then its message, coloured by severity.
fn format_diagnostics(diagnostics: &[StartupDiagnostic]) -> Vec<StartupLine> {
    let mut lines: Vec<StartupLine> = Vec::new();
    // Grouped, insertion-ordered (Pi uses a `Map`, which preserves insertion order).
    let mut order: Vec<String> = Vec::new();
    let mut groups: std::collections::HashMap<String, Vec<&DiagnosticCollision>> =
        std::collections::HashMap::new();
    let mut others: Vec<&StartupDiagnostic> = Vec::new();

    for d in diagnostics {
        match (&d.collision, d.severity) {
            (Some(c), DiagnosticSeverity::Collision) => {
                let entry = groups.entry(c.name.clone()).or_insert_with(|| {
                    order.push(c.name.clone());
                    Vec::new()
                });
                entry.push(c);
            }
            _ => others.push(d),
        }
    }

    for name in &order {
        let Some(group) = groups.get(name) else {
            continue;
        };
        let Some(first) = group.first() else { continue };
        lines.push(StartupLine::single(
            format!("  \"{name}\" collision:"),
            StartupRole::Warning,
        ));
        lines.push(StartupLine::of(vec![
            StartupSpan::new("    ", StartupRole::Dim),
            StartupSpan::new("✓", StartupRole::Success),
            StartupSpan::new(format!(" {}", first.winner), StartupRole::Dim),
        ]));
        for c in group {
            lines.push(StartupLine::of(vec![
                StartupSpan::new("    ", StartupRole::Dim),
                StartupSpan::new("✗", StartupRole::Warning),
                StartupSpan::new(format!(" {} (skipped)", c.loser), StartupRole::Dim),
            ]));
        }
    }

    for d in others {
        let role = match d.severity {
            DiagnosticSeverity::Error => StartupRole::Error,
            _ => StartupRole::Warning,
        };
        match &d.path {
            Some(path) => {
                lines.push(StartupLine::single(format!("  {path}"), role));
                lines.push(StartupLine::single(format!("    {}", d.message), role));
            }
            None => lines.push(StartupLine::single(format!("  {}", d.message), role)),
        }
    }
    lines
}

/// Project the panel onto styled ratatui rows, honouring `outputPad` like every other entry.
///
/// **Each row is a `Text`, and a `Text` WRAPS.** Every child upstream adds to
/// `loadedResourcesContainer` is a `new Text(…, 0, 0)` (`interactive-mode.ts:1766`, `:1775`,
/// `:1798`, `:1807`) or an `ExpandableText(…, 0, 0)` (`:1626-1632`, which `extends Text`), and
/// `Text.render` wraps its string at `contentWidth = max(1, width - paddingX * 2)` (`text.ts:64`)
/// BEFORE prefixing `leftMargin` to each produced row (`:70-76`).
///
/// This took no `width` at all: it inserted the margin into the LOGICAL row and handed the result to
/// the outer `Paragraph::wrap`, which reflowed it at the FULL frame width — the exact shape of the
/// L2/M10 defect the rest of this batch removed. A single extension-error diagnostic carrying an
/// absolute path drew 77 and 73 columns inside a 40-column frame, with every continuation row flush
/// at column 0. Building each row through [`crate::transcript::text_lines_of`] wraps first and
/// margins second, like every other block.
///
/// A structurally empty [`StartupLine`] stays a bare [`Line::default`] rather than a lone margin
/// space: it is the panel's `Spacer(1)` (`interactive-mode.ts:1637`), not a `Text` with no content.
pub(crate) fn startup_lines(
    lines: &[StartupLine],
    theme: &UiTheme,
    width: usize,
    output_pad: usize,
) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::with_capacity(lines.len());
    for line in lines {
        if line.spans.is_empty() {
            out.push(Line::default());
            continue;
        }
        let spans: Vec<Span<'static>> = line
            .spans
            .iter()
            .map(|s| {
                Span::styled(
                    s.text.clone(),
                    match s.role {
                        StartupRole::Heading => theme.md_heading_style(),
                        StartupRole::Dim => theme.muted_style(),
                        StartupRole::Warning => theme.warning_style(),
                        StartupRole::Error => theme.error_style(),
                        StartupRole::Success => theme.success_style(),
                    },
                )
            })
            .collect();
        out.extend(crate::transcript::text_lines_of(
            &Line::from(spans),
            width,
            output_pad,
        ));
    }
    out
}

/// Render the `/arminsayshi` XBM bitmap as half-block art (`armin.ts`: 31×36, LSB-first, `1` =
/// background, `0` = foreground; two vertical pixels packed per cell into `█`/`▀`/`▄`/space). A pure,
/// deterministic transcript block (the animation effects are omitted as non-testable chrome).
pub(crate) fn armin_art() -> String {
    const WIDTH: usize = 31;
    const HEIGHT: usize = 36;
    const BYTES_PER_ROW: usize = WIDTH.div_ceil(8);
    const BITS: [u8; 144] = [
        255, 255, 255, 127, 255, 240, 255, 127, 255, 237, 255, 127, 255, 219, 255, 127, 255, 183,
        255, 127, 255, 119, 254, 127, 63, 248, 254, 127, 223, 255, 254, 127, 223, 63, 252, 127,
        159, 195, 251, 127, 111, 252, 244, 127, 247, 15, 247, 127, 247, 255, 247, 127, 247, 255,
        227, 127, 247, 7, 232, 127, 239, 248, 103, 112, 15, 255, 187, 111, 241, 0, 208, 91, 253,
        63, 236, 83, 193, 255, 239, 87, 159, 253, 238, 95, 159, 252, 174, 95, 31, 120, 172, 95, 63,
        0, 80, 108, 127, 0, 220, 119, 255, 192, 63, 120, 255, 1, 248, 127, 255, 3, 156, 120, 255,
        7, 140, 124, 255, 15, 206, 120, 255, 255, 207, 127, 255, 255, 207, 120, 255, 255, 223, 120,
        255, 255, 223, 125, 255, 255, 63, 126, 255, 255, 255, 127,
    ];
    // `1` (background) → false; `0` (foreground) → true. Out-of-range rows are background.
    let pixel = |x: usize, y: usize| -> bool {
        if y >= HEIGHT {
            return false;
        }
        let byte_index = y * BYTES_PER_ROW + x / 8;
        match BITS.get(byte_index) {
            Some(byte) => ((byte >> (x % 8)) & 1) == 0,
            None => false,
        }
    };
    let mut out = String::new();
    let rows = HEIGHT.div_ceil(2);
    for row in 0..rows {
        for x in 0..WIDTH {
            let upper = pixel(x, row * 2);
            let lower = pixel(x, row * 2 + 1);
            out.push(match (upper, lower) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            });
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]
    use super::*;

    fn texts(lines: &[StartupLine]) -> Vec<String> {
        lines.iter().map(StartupLine::text).collect()
    }

    #[test]
    fn quiet_startup_suppresses_the_listing_but_never_the_diagnostics() {
        let report = StartupReport {
            quiet_startup: true,
            skills: vec!["review".into()],
            skill_diagnostics: vec![StartupDiagnostic::plain(
                DiagnosticSeverity::Warning,
                Some("~/.cyrup/skills/gone".into()),
                "skill path does not exist",
            )],
            ..Default::default()
        };
        let lines = texts(&build_startup_lines(&report));
        assert!(
            !lines.iter().any(|l| l.contains("[Skills]")),
            "listing is quiet: {lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("[Skill conflicts]")),
            "diagnostics: {lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("skill path does not exist"))
        );
    }

    #[test]
    fn verbose_overrides_quiet_startup() {
        let report = StartupReport {
            verbose: true,
            quiet_startup: true,
            skills: vec!["review".into()],
            ..Default::default()
        };
        assert!(
            build_startup_lines(&report)
                .iter()
                .any(|l| l.text().contains("[Skills]"))
        );
    }

    #[test]
    fn a_clean_quiet_startup_prints_nothing() {
        let report = StartupReport {
            quiet_startup: true,
            ..Default::default()
        };
        assert!(build_startup_lines(&report).is_empty());
    }

    #[test]
    fn context_keeps_its_order_while_the_rest_sort() {
        let report = StartupReport {
            context_files: vec![
                "AGENTS.md".into(),
                "CLAUDE.md".into(),
                "~/.cyrup/AGENTS.md".into(),
            ],
            skills: vec!["zebra".into(), "alpha".into()],
            ..Default::default()
        };
        let lines = texts(&build_startup_lines(&report));
        let idx = |needle: &str| lines.iter().position(|l| l.contains(needle)).unwrap();
        assert!(
            idx("AGENTS.md") < idx("CLAUDE.md"),
            "context order is preserved: {lines:?}"
        );
        assert!(idx("alpha") < idx("zebra"), "skills sort: {lines:?}");
    }

    #[test]
    fn collisions_group_by_name_with_one_winner_and_every_loser() {
        let collision = |loser: &str| StartupDiagnostic {
            severity: DiagnosticSeverity::Collision,
            message: "name \"review\" collision".into(),
            path: Some(loser.into()),
            collision: Some(DiagnosticCollision {
                name: "review".into(),
                winner: "project/review".into(),
                loser: loser.into(),
            }),
        };
        let report = StartupReport {
            quiet_startup: true,
            skill_diagnostics: vec![collision("global/review"), collision("package/review")],
            ..Default::default()
        };
        let lines = texts(&build_startup_lines(&report));
        assert!(
            lines.iter().any(|l| l == "  \"review\" collision:"),
            "{lines:?}"
        );
        assert_eq!(
            lines
                .iter()
                .filter(|l| l.contains("✓ project/review"))
                .count(),
            1,
            "exactly one winner line: {lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("✗ global/review (skipped)")),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("✗ package/review (skipped)")),
            "{lines:?}"
        );
    }
}

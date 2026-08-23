//! Pi-faithful CLI leniency, parse diagnostics, and provider login-guidance text.
//!
//! clap is strict: an unknown short, a bad `--mode`, or a bad `--thinking` each abort with a usage
//! error + exit 2. Pi's hand-rolled parser (`cli/args.ts`) is lenient in three specific ways
//! (args.ts:80-82,131-139,202-203):
//!
//! * `--mode <bad>` is **silently ignored** (mode stays the default `text`).
//! * `--thinking <bad>` **warns and continues** (the thinking level stays unset).
//! * an unknown single-dash option (`-x`) is an **error** (`Unknown option: -x`) that exits 1 — NOT
//!   a clap usage error / exit 2.
//!
//! [`apply_arg_leniency`] runs over the already-`normalize_short_aliases`-d argv BEFORE clap sees it:
//! it drops a bad `--mode`/`--thinking` value (so clap never rejects it), records the warning/error
//! diagnostics, and removes unknown single-dash tokens (recording the `Unknown option` error). The
//! diagnostics are reported by the bin exactly as Pi does (main.ts:504-512): warnings in yellow,
//! errors in red, and any error exits 1. Values of known value-taking long flags are passed through
//! verbatim (so `--model -5` keeps `-5` as the model value, matching Pi).
//!
//! It also hosts the provider login-guidance messages (`core/auth-guidance.ts`) surfaced by the
//! no-models-available guard + the `--list-models` empty case.

/// The severity of a parse diagnostic (Pi `{ type: "warning" | "error" }`, args.ts:54).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticLevel {
    Warning,
    Error,
}

/// A single parse diagnostic (Pi `Args.diagnostics[]`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub level: DiagnosticLevel,
    pub message: String,
}

impl Diagnostic {
    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            level: DiagnosticLevel::Warning,
            message: message.into(),
        }
    }
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            level: DiagnosticLevel::Error,
            message: message.into(),
        }
    }
}

/// Valid `--thinking` levels (Pi `VALID_THINKING_LEVELS`, args.ts:**57** — `max` added in fbdd4638;
/// `:59` is `isValidThinkingLevel`, the predicate over this array). SEAM-029: the citation was off
/// by two.
const VALID_THINKING_LEVELS: [&str; 7] =
    ["off", "minimal", "low", "medium", "high", "xhigh", "max"];
/// Valid `--mode` values (Pi args.ts:80).
const VALID_MODES: [&str; 3] = ["text", "json", "rpc"];
/// Valid `--tui-mode` values (pi args.ts:182 @v0.84.1 — `mode === "regular" || mode === "fullscreen"`).
const VALID_TUI_MODES: [&str; 2] = ["regular", "fullscreen"];
/// pi's `--tui-mode`-with-no-usable-value error (args.ts:186 @v0.84.1), verbatim.
const TUI_MODE_REQUIRES: &str = "--tui-mode requires regular or fullscreen";

/// The single-char short flags clap accepts (Pi's exact short set, args.ts). Any OTHER single-dash
/// token is an unknown option (Pi args.ts:202-203) rather than a clap exit-2 usage error.
const KNOWN_SHORT_FLAGS: [&str; 9] = ["-p", "-c", "-r", "-a", "-n", "-t", "-e", "-h", "-v"];

/// The long flags that consume a following value token in their space-separated form (so the value is
/// passed through verbatim, never inspected as a flag). Kept in lockstep with [`crate::cli::Cli`];
/// `--mode`/`--thinking` are handled specially (their value IS inspected for leniency) and so are
/// excluded here.
const VALUE_LONG_FLAGS: [&str; 14] = [
    "--provider",
    "--api-key",
    "--system-prompt",
    "--append-system-prompt",
    "--extension",
    "--skill",
    "--prompt-template",
    "--theme",
    "--session",
    "--session-id",
    "--fork",
    "--session-dir",
    "--name",
    "--export",
];

/// `--model` is value-taking but is intentionally NOT lumped with [`VALUE_LONG_FLAGS`] only because
/// it is the most common; it is handled with the same verbatim pass-through.
const MODEL_FLAG: &str = "--model";

/// The three value-taking flags pi **ASSIGNS** rather than accumulates — `result.models = …`
/// (args.ts:114), `result.tools = …` (`:121-124`) and `result.excludeTools = …` (`:125-129`) — with
/// every spelling cyrup accepts for each. SEAM-105.
///
/// clap declares all three as `Vec<String>` with `value_delimiter = ','` (cli.rs), so a REPEATED
/// flag appends: `--tools read --tools bash` resolved to `{read,bash}` where pi resolves `{bash}`.
/// Only the repeated form ever diverged — the comma form (`--tools read,bash`) is identical under
/// both — which is why every existing test passed. [`apply_arg_leniency`] therefore drops every
/// occurrence but the LAST before clap sees the argv; the surviving occurrence still comma-splits
/// exactly as it does today.
///
/// `-xt` never reaches here (`normalize_short_aliases` rewrites it to `--exclude-tools`); `-t` does,
/// and pi's arm is `(arg === "--tools" || arg === "-t")`, so it belongs to the same family and takes
/// its value the same way.
const ASSIGNING_FLAGS: [&[&str]; 3] = [&["--models"], &["--tools", "-t"], &["--exclude-tools"]];

/// The [`ASSIGNING_FLAGS`] family `arg` belongs to, if any. The `--tools=read` form is matched on the
/// name part: pi has no `=` form at all (it would land in `unknownFlags`, args.ts:190-192), but cyrup
/// accepts one through `KNOWN_LONG_FLAGS` (cli.rs), so last-occurrence-wins has to cover it too or
/// `--tools read --tools=bash` would keep both.
fn assigning_family(arg: &str) -> Option<usize> {
    let name = arg.split('=').next().unwrap_or(arg);
    ASSIGNING_FLAGS.iter().position(|names| names.contains(&name))
}

/// Apply Pi's lenient arg handling over `argv` (program name already stripped, short-aliases already
/// normalized), returning the cleaned argv clap should parse plus the collected diagnostics
/// (args.ts:80-82,131-139,202-203).
pub fn apply_arg_leniency(argv: &[String]) -> (Vec<String>, Vec<Diagnostic>) {
    let mut clean: Vec<String> = Vec::new();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    // SEAM-105: the `[start, end)` slice of `clean` each occurrence of an [`ASSIGNING_FLAGS`] family
    // wrote. Everything but the last entry per family is deleted once the walk finishes.
    let mut assign_spans: [Vec<(usize, usize)>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut i = 0usize;
    while let Some(arg) = argv.get(i) {
        // `--mode <value>` (space form): keep when valid, silently drop both when invalid.
        if arg == "--mode"
            && let Some(value) = argv.get(i + 1)
        {
            if VALID_MODES.contains(&value.as_str()) {
                clean.push(arg.clone());
                clean.push(value.clone());
            }
            // else: silently ignored (Pi args.ts:80-82) — drop the flag AND its value.
            i += 2;
            continue;
        }
        // `--thinking <value>` (space form): keep when valid, warn + drop both when invalid.
        if arg == "--thinking"
            && let Some(value) = argv.get(i + 1)
        {
            if VALID_THINKING_LEVELS.contains(&value.as_str()) {
                clean.push(arg.clone());
                clean.push(value.clone());
            } else {
                diagnostics.push(Diagnostic::warning(format!(
                    "Invalid thinking level \"{value}\". Valid values: {}",
                    VALID_THINKING_LEVELS.join(", ")
                )));
            }
            i += 2;
            continue;
        }
        // `--tui-mode <value>` — pi args.ts:180-192 @v0.84.1, branch for branch:
        //   * `regular`/`fullscreen`  → keep flag + value (`i++`), clap parses it into `Cli::tui_mode`
        //   * missing, or a `-`-prefixed next token → error `--tui-mode requires regular or
        //     fullscreen`, and the value token is NOT consumed (pi does not `i++` on this branch,
        //     so `--tui-mode --offline` still parses `--offline`)
        //   * anything else → consume the value (`i++`) and error `Invalid TUI mode "<v>". …`
        // Both error branches leave the flag out of the cleaned argv: an error exits 1 in
        // `main.rs`, and clap must never see a value it would reject with its own exit-2 text.
        // SEAM-051 / ADR-0005 §Decision A.1.
        //
        // CYRUP-DELTA: the `--tui-mode=<v>` form is handled here too. pi's parser matches only
        // `arg === "--tui-mode"`, so `--tui-mode=regular` falls into its `unknownFlags` map
        // (args.ts:204-207) — as does `--model=x` and every other `=` form, which cyrup has always
        // accepted through `KNOWN_LONG_FLAGS`'s `split('=')` (cli.rs:706). Given cyrup accepts the
        // `=` form, the same two messages must cover it or `--tui-mode=bogus` would reach clap and
        // die with a clap usage error (exit 2) instead of pi's text.
        if arg == "--tui-mode" || arg.starts_with("--tui-mode=") {
            let (value, eq_form) = match arg.strip_prefix("--tui-mode=") {
                Some(v) => (Some(v.to_string()), true),
                None => (argv.get(i + 1).cloned(), false),
            };
            match value.as_deref() {
                Some(v) if VALID_TUI_MODES.contains(&v) => {
                    clean.push(arg.clone());
                    if !eq_form {
                        clean.push(v.to_string());
                        i += 1;
                    }
                }
                // pi: `mode === undefined || mode.startsWith("-")` (args.ts:185) — no value consumed.
                None | Some("") => diagnostics.push(Diagnostic::error(TUI_MODE_REQUIRES)),
                Some(v) if !eq_form && v.starts_with('-') => {
                    diagnostics.push(Diagnostic::error(TUI_MODE_REQUIRES));
                }
                Some(v) => {
                    if !eq_form {
                        i += 1;
                    }
                    diagnostics.push(Diagnostic::error(format!(
                        "Invalid TUI mode \"{v}\". Valid values: {}",
                        VALID_TUI_MODES.join(", ")
                    )));
                }
            }
            i += 1;
            continue;
        }
        // `--models` / `--tools` / `-t` / `--exclude-tools` — pi ASSIGNS these (args.ts:114,121-129),
        // so a repeated flag REPLACES the earlier value. Record the span this occurrence occupies in
        // `clean`; the post-pass below keeps only the last one per family. The value token is passed
        // through verbatim, exactly as the generic value-flag arm below does. SEAM-105.
        if let Some(family) = assigning_family(arg) {
            let start = clean.len();
            clean.push(arg.clone());
            if !arg.contains('=')
                && let Some(value) = argv.get(i + 1)
            {
                clean.push(value.clone());
                i += 1;
            }
            // Indexing is bounded by `assigning_family`, which only ever answers `0..3`.
            if let Some(spans) = assign_spans.get_mut(family) {
                spans.push((start, clean.len()));
            }
            i += 1;
            continue;
        }
        // `--list-models [pattern]` — pi args.ts:171-177, both halves of its guard:
        //   `if (i + 1 < args.length && !args[i + 1].startsWith("-") && !args[i + 1].startsWith("@"))`
        // The `@` half is the one cyrup lacked: clap's `num_args = 0..=1` consumes ANY following
        // non-flag token, so `cyrup --list-models @notes.md` searched for the pattern `@notes.md`
        // (printing `No models matching "@notes.md"`) and lost the file attachment, where pi lists
        // the whole configured catalog and routes `@notes.md` to `fileArgs`. Emitting the `=` form
        // binds the empty value explicitly, so clap cannot reach past the flag for one. SEAM-103.
        if arg == "--list-models" {
            match argv.get(i + 1) {
                Some(next) if !next.starts_with('-') && !next.starts_with('@') => {
                    clean.push(arg.clone());
                    clean.push(next.clone());
                    i += 2;
                }
                _ => {
                    clean.push("--list-models=".to_string());
                    i += 1;
                }
            }
            continue;
        }
        // `--print` / `-p` followed by a `---`-prefixed token — pi args.ts:140-146:
        //   `if (next !== undefined && !next.startsWith("@") && (!next.startsWith("-") ||
        //    next.startsWith("---"))) { result.messages.push(next); i++; }`
        // Every other shape of that condition already matches: a bare word after `-p` is a clap
        // positional, and an `@file` or a `-`/`--` flag is left alone. Only `next.startsWith("---")`
        // — the escape hatch that lets a prompt legitimately begin with dashes — was unported, so
        // `cyrup -p ---weird` captured an extension flag named `-weird` (which then died on
        // `Unknown option: ---weird`) instead of sending `---weird` as the prompt. SEAM-107.
        //
        // The token is emitted with a leading NUL, which `Cli::restore_escaped_positionals` strips
        // after the parse. That keeps the message at its exact argv POSITION among the positionals —
        // a trailing `--` escape would move it to the end of `messages` — and the marker cannot
        // collide with a real argument: process arguments arrive as NUL-terminated C strings, so no
        // argv token can contain a NUL byte.
        if (arg == "--print" || arg == "-p")
            && let Some(next) = argv.get(i + 1)
            && next.starts_with("---")
        {
            clean.push(arg.clone());
            clean.push(format!("{ESCAPED_MESSAGE_PREFIX}{next}"));
            i += 2;
            continue;
        }
        // A known value-taking long flag (space form): pass the flag AND its next token through
        // verbatim, so a value that looks like a flag (`--model -5`) is not re-interpreted (Pi
        // consumes `args[++i]` unconditionally).
        if (arg == MODEL_FLAG || VALUE_LONG_FLAGS.contains(&arg.as_str()))
            && !arg.contains('=')
            && let Some(value) = argv.get(i + 1)
        {
            clean.push(arg.clone());
            clean.push(value.clone());
            i += 2;
            continue;
        }
        // An unknown single-dash option (`-x`, `-5`, `-tfoo`, `-`, …) — Pi `Unknown option` error
        // (args.ts:202-203). A bare `--`-prefixed token is left for the extension-flag capture / clap;
        // an `@file` and a plain positional are left untouched.
        //
        // SEAM-104: there is deliberately NO length guard. Pi's predicate is exactly
        // `arg.startsWith("-") && !arg.startsWith("--")` (args.ts:202), which the one-character
        // token `-` satisfies, and its final arm is `else if (!arg.startsWith("-"))` (`:204`), so a
        // bare `-` can never reach `result.messages`. cyrup previously required `arg.len() > 1`,
        // letting `-` fall through to the positionals and become the PROMPT — a bare `cyrup -`
        // started a real agent turn and issued a provider request where pi exits 1 without
        // contacting anything.
        if arg.starts_with('-')
            && !arg.starts_with("--")
            && !KNOWN_SHORT_FLAGS.contains(&arg.as_str())
        {
            diagnostics.push(Diagnostic::error(format!("Unknown option: {arg}")));
            i += 1;
            continue;
        }
        clean.push(arg.clone());
        i += 1;
    }
    // SEAM-105: pi assigns, so the LAST occurrence of each family stands alone. Drop every earlier
    // span; the surviving one still comma-splits under clap exactly as it does today.
    let dropped: Vec<(usize, usize)> = assign_spans
        .iter()
        .flat_map(|spans| spans.iter().rev().skip(1).copied())
        .collect();
    if !dropped.is_empty() {
        clean = clean
            .into_iter()
            .enumerate()
            .filter(|(idx, _)| !dropped.iter().any(|(start, end)| idx >= start && idx < end))
            .map(|(_, token)| token)
            .collect();
    }
    (clean, diagnostics)
}

/// The marker [`apply_arg_leniency`] puts in front of a `-p ---…` escape-hatch message so clap
/// accepts it as a positional rather than reading it as a long flag (SEAM-107).
///
/// A NUL is used because it is the one byte that CANNOT appear in a process argument: `execve`
/// takes NUL-terminated C strings, so the kernel truncates at the first NUL and `std::env::args()`
/// can never yield a token containing one. The marker therefore cannot collide with anything a user
/// types, and no escaping-the-escape rule is needed.
pub const ESCAPED_MESSAGE_PREFIX: char = '\0';

/// Provider login guidance (Pi `getProviderLoginHelp`, auth-guidance.ts:6-11). The doc paths are
/// shown relative to the package docs dir (the absolute prefix is environment-cosmetic).
pub fn get_provider_login_help() -> String {
    "Use /login to log into a provider via OAuth or API key. See:\n  docs/providers.md\n  docs/models.md"
        .to_string()
}

/// The no-models-available message (Pi `formatNoModelsAvailableMessage`, auth-guidance.ts:14-16).
pub fn format_no_models_available_message() -> String {
    format!("No models available. {}", get_provider_login_help())
}

/// The hint shown after an extension-load failure (Pi `EXTENSION_LOAD_FAILURE_HINT`, main.ts:52).
pub const EXTENSION_LOAD_FAILURE_HINT: &str = "Hint: Start without extensions using \"cyrup -ne\".";

/// Report parse/settings diagnostics to **stderr** (Pi `reportDiagnostics`, main.ts:87-93): warnings
/// prefixed `Warning:`, errors `Error:`. Colour is omitted (no colour dep at the bin boundary).
pub fn report(diagnostics: &[Diagnostic]) {
    for d in diagnostics {
        match d.level {
            DiagnosticLevel::Error => eprintln!("Error: {}", d.message),
            DiagnosticLevel::Warning => eprintln!("Warning: {}", d.message),
        }
    }
}

/// Pi's SECOND `reportDiagnostics` checkpoint — `reportDiagnostics(runtime.diagnostics)` +
/// `process.exit(1)` on any error (main.ts:843-848). Returns `true` when the caller must exit 1.
///
/// SEAM-S01: `AgentSessionRuntime::diagnostics()` had NO production consumer, which is why a
/// mistyped `--flag` (captured as an extension flag, then owned by no loaded extension) was
/// swallowed with no message and exit 0. Runs in every mode, exactly like Pi's single call site,
/// which sits after runtime creation and before the mode dispatch.
///
/// EXT-S01: extension LOAD failures ride this channel too. Containment (one built-in's failing
/// `init()` no longer aborts the whole build) is Pi's `loader.ts:537-540` `errors.push(...); continue`
/// — but Pi then LIFTS those errors onto `runtime.diagnostics` (`main.ts:735-738`) and exits 1 on
/// them, including Pi's `EXTENSION_LOAD_FAILURE_HINT` (`main.ts:61`, `:844-846`), reproduced below.
/// Routing them to the interactive-only `[Extension issues]` panel alone would leave print/json/rpc
/// silent at exit 0 — and cyrup's natives include the permission gate, so that would be fail-OPEN.
pub async fn report_runtime(runtime: &cyrup_session_svc::AgentSessionRuntime) -> bool {
    let diagnostics = runtime.diagnostics().await;
    let mut fatal = false;
    for d in &diagnostics {
        if d.severity == "error" {
            fatal = true;
            eprintln!("Error: {}", d.message);
        } else {
            eprintln!("Warning: {}", d.message);
        }
    }
    // Pi `main.ts:844-846`: matched on the message text, over ALL diagnostics, not just the errors.
    if fatal && diagnostics.iter().any(|d| d.message.contains(EXTENSION_LOAD_FAILURE_MARKER)) {
        eprintln!("{EXTENSION_LOAD_FAILURE_HINT}");
    }
    fatal
}

/// Pi `main.ts:844` — the substring that selects the extension-load hint.
pub const EXTENSION_LOAD_FAILURE_MARKER: &str = "Failed to load extension";

/// The non-interactive no-models-available exit — pi
/// `console.error(chalk.red(formatNoModelsAvailableMessage())); process.exit(1);`
/// (main.ts:853-854 @v0.83.0, inside the `appMode !== "interactive"` gate at :852-855). Prints the
/// provider login guidance to stderr; the caller supplies pi's exit code 1. Interactive never calls
/// this: it
/// launches modelless and shows the same text as a banner instead (SEAM-075).
pub fn no_models_available() {
    eprintln!("{}", format_no_models_available_message());
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn bad_mode_is_silently_dropped() {
        // Pi args.ts:80-82: an invalid `--mode` value is silently ignored (no diagnostic).
        let (clean, diags) = apply_arg_leniency(&v(&["--mode", "bogus", "hi"]));
        assert_eq!(clean, v(&["hi"]));
        assert!(diags.is_empty());
        // A valid mode is preserved.
        let (clean, diags) = apply_arg_leniency(&v(&["--mode", "json"]));
        assert_eq!(clean, v(&["--mode", "json"]));
        assert!(diags.is_empty());
    }

    #[test]
    fn bad_thinking_warns_and_continues() {
        // Pi args.ts:131-139: invalid `--thinking` warns and drops the flag, keeping the run going.
        let (clean, diags) = apply_arg_leniency(&v(&["--thinking", "ultra", "go"]));
        assert_eq!(clean, v(&["go"]));
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].level, DiagnosticLevel::Warning);
        assert!(
            diags[0]
                .message
                .contains("Invalid thinking level \"ultra\"")
        );
        // A valid level passes through.
        let (clean, diags) = apply_arg_leniency(&v(&["--thinking", "high"]));
        assert_eq!(clean, v(&["--thinking", "high"]));
        assert!(diags.is_empty());
        // PROV-002: `max` is valid (Pi `VALID_THINKING_LEVELS`, args.ts:59). Before the fix the
        // leniency pass warned "Invalid thinking level \"max\"" and DROPPED the flag, so the top
        // rung could not be reached from the command line at all.
        let (clean, diags) = apply_arg_leniency(&v(&["--thinking", "max", "go"]));
        assert_eq!(clean, v(&["--thinking", "max", "go"]));
        assert!(diags.is_empty(), "{diags:?}");
        // …and the advertised value list names it.
        let (_, diags) = apply_arg_leniency(&v(&["--thinking", "ultra"]));
        assert!(diags[0].message.ends_with("xhigh, max"), "{}", diags[0].message);
    }

    #[test]
    fn unknown_single_dash_option_is_an_error() {
        // Pi args.ts:202-203: an unknown short is an error (exit 1), not a clap exit-2 usage error.
        let (clean, diags) = apply_arg_leniency(&v(&["-x", "hello"]));
        assert_eq!(clean, v(&["hello"]));
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].level, DiagnosticLevel::Error);
        assert_eq!(diags[0].message, "Unknown option: -x");
        // An attached-value short (clap WOULD accept `-tfoo`) is also unknown in Pi.
        let (_clean, diags) = apply_arg_leniency(&v(&["-tfoo"]));
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].level, DiagnosticLevel::Error);
    }

    /// SEAM-104 — RED before the fix, which carried an `arg.len() > 1` guard that Pi does not have.
    /// `arg.startsWith("-") && !arg.startsWith("--")` (args.ts:202 @v0.83.0) matches the
    /// one-character token `-`, and Pi's message arm is `else if (!arg.startsWith("-"))` (`:204`),
    /// so a bare dash is an exit-1 error upstream and is NEVER carried through as a prompt.
    #[test]
    fn bare_single_dash_is_an_unknown_option_not_a_prompt() {
        let (clean, diags) = apply_arg_leniency(&v(&["-"]));
        assert!(clean.is_empty(), "a bare `-` must not survive as a message: {clean:?}");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].level, DiagnosticLevel::Error);
        assert_eq!(diags[0].message, "Unknown option: -");
        // `--` still belongs to clap / the extension-flag capture, not to this arm.
        let (clean, diags) = apply_arg_leniency(&v(&["--"]));
        assert_eq!(clean, v(&["--"]));
        assert!(diags.is_empty(), "{diags:?}");
    }

    /// SEAM-103 — RED before the fix. pi's guard is TWO-part (args.ts:171-177):
    /// `!args[i + 1].startsWith("-") && !args[i + 1].startsWith("@")`. clap's `num_args = 0..=1`
    /// only ever knew the `-` half, so an `@file` token following `--list-models` was eaten as the
    /// search pattern — `cyrup --list-models @notes.md` printed `No models matching "@notes.md"` and
    /// dropped the attachment, where pi lists the configured catalog and routes `@notes.md` to
    /// `fileArgs`.
    #[test]
    fn list_models_does_not_swallow_a_following_file_arg() {
        // Presence before absence: a real search pattern is still taken as the value.
        let (clean, diags) = apply_arg_leniency(&v(&["--list-models", "gpt"]));
        assert_eq!(clean, v(&["--list-models", "gpt"]));
        assert!(diags.is_empty(), "{diags:?}");

        // …and an `@file` is NOT. The `=` form binds the empty value so clap cannot reach past the
        // flag for one; `@foo` survives to the positionals, where `split_positionals` makes it a
        // file arg.
        let (clean, diags) = apply_arg_leniency(&v(&["--list-models", "@foo"]));
        assert_eq!(clean, v(&["--list-models=", "@foo"]));
        assert!(diags.is_empty(), "{diags:?}");

        // The `-` half of pi's guard, and the end-of-argv case, keep working.
        let (clean, _) = apply_arg_leniency(&v(&["--list-models", "--verbose"]));
        assert_eq!(clean, v(&["--list-models=", "--verbose"]));
        let (clean, _) = apply_arg_leniency(&v(&["--list-models"]));
        assert_eq!(clean, v(&["--list-models="]));
    }

    /// SEAM-105 — RED before the fix. pi ASSIGNS all three (args.ts:114, :121-124, :125-129), so the
    /// last occurrence replaces the earlier one; clap's `Vec<String>` appended across repeats.
    #[test]
    fn repeated_assigning_flags_keep_only_the_last_occurrence() {
        // Presence before absence: the comma form — identical under both, and the reason every
        // existing test passed — is untouched, and so is a single occurrence.
        let (clean, _) = apply_arg_leniency(&v(&["--tools", "read,bash"]));
        assert_eq!(clean, v(&["--tools", "read,bash"]));
        let (clean, _) = apply_arg_leniency(&v(&["--models", "a"]));
        assert_eq!(clean, v(&["--models", "a"]));

        // …and the repeated form now resolves to the LAST occurrence alone.
        let (clean, _) = apply_arg_leniency(&v(&["--tools", "read", "--tools", "bash"]));
        assert_eq!(clean, v(&["--tools", "bash"]));
        let (clean, _) = apply_arg_leniency(&v(&["--models", "a", "--models", "b"]));
        assert_eq!(clean, v(&["--models", "b"]));
        let (clean, _) =
            apply_arg_leniency(&v(&["--exclude-tools", "x", "--exclude-tools", "y"]));
        assert_eq!(clean, v(&["--exclude-tools", "y"]));

        // `-t` is pi's own alias for `--tools` in the SAME arm (`arg === "--tools" || arg === "-t"`),
        // so it belongs to the same family — including when the spellings are mixed. `-xt` never
        // reaches here (`normalize_short_aliases` rewrote it), and the `=` form cyrup additionally
        // accepts counts as an occurrence too.
        let (clean, _) = apply_arg_leniency(&v(&["-t", "read", "-t", "bash"]));
        assert_eq!(clean, v(&["-t", "bash"]));
        let (clean, _) = apply_arg_leniency(&v(&["--tools", "read", "-t", "bash"]));
        assert_eq!(clean, v(&["-t", "bash"]));
        let (clean, _) = apply_arg_leniency(&v(&["--tools", "read", "--tools=bash"]));
        assert_eq!(clean, v(&["--tools=bash"]));

        // Three families are independent, and the surrounding argv is preserved in order.
        let (clean, _) = apply_arg_leniency(&v(&[
            "--tools", "read", "--models", "a", "hello", "--tools", "bash", "--models", "b",
            "--verbose",
        ]));
        assert_eq!(
            clean,
            v(&["hello", "--tools", "bash", "--models", "b", "--verbose"])
        );
    }

    /// SEAM-107 — RED before the fix. pi's `-p` arm consumes the next token as a MESSAGE when
    /// `next !== undefined && !next.startsWith("@") && (!next.startsWith("-") ||
    /// next.startsWith("---"))` (args.ts:140-146). The `---` clause is the escape hatch that lets a
    /// prompt begin with dashes; unported, `cyrup -p ---weird` captured an extension flag named
    /// `-weird` and died on `Unknown option: ---weird` instead of sending the prompt.
    #[test]
    fn print_consumes_a_triple_dash_token_as_the_message() {
        for flag in ["-p", "--print"] {
            let (clean, diags) = apply_arg_leniency(&v(&[flag, "---weird"]));
            assert_eq!(
                clean,
                vec![flag.to_string(), format!("{ESCAPED_MESSAGE_PREFIX}---weird")],
                "the `---` token must be marked as a positional, not left to the flag capture"
            );
            assert!(diags.is_empty(), "{diags:?}");
        }
        // Presence before absence: every OTHER shape of pi's condition is unchanged — a bare word is
        // already a clap positional, an `@file` is left for `fileArgs`, and a one- or two-dash flag
        // stays a flag.
        let (clean, _) = apply_arg_leniency(&v(&["-p", "hello"]));
        assert_eq!(clean, v(&["-p", "hello"]));
        let (clean, _) = apply_arg_leniency(&v(&["-p", "@notes.md"]));
        assert_eq!(clean, v(&["-p", "@notes.md"]));
        let (clean, _) = apply_arg_leniency(&v(&["-p", "--verbose"]));
        assert_eq!(clean, v(&["-p", "--verbose"]));
        let (clean, _) = apply_arg_leniency(&v(&["-p"]));
        assert_eq!(clean, v(&["-p"]));
    }

    #[test]
    fn known_shorts_and_long_flag_values_pass_through() {
        // Known shorts are left for clap.
        let (clean, diags) = apply_arg_leniency(&v(&["-p", "-c", "-a"]));
        assert_eq!(clean, v(&["-p", "-c", "-a"]));
        assert!(diags.is_empty());
        // A value that looks like a short is preserved as the long flag's value (`--model -5`).
        let (clean, diags) = apply_arg_leniency(&v(&["--model", "-5"]));
        assert_eq!(clean, v(&["--model", "-5"]));
        assert!(diags.is_empty());
        // `--name -x` keeps `-x` as the name value (not an unknown option).
        let (clean, diags) = apply_arg_leniency(&v(&["--name", "-x"]));
        assert_eq!(clean, v(&["--name", "-x"]));
        assert!(diags.is_empty());
    }

    #[test]
    fn login_guidance_and_no_models_message() {
        let help = get_provider_login_help();
        assert!(help.contains("/login"));
        assert!(help.contains("providers.md"));
        let msg = format_no_models_available_message();
        assert!(msg.starts_with("No models available."));
        assert!(msg.contains("/login"));
        assert!(EXTENSION_LOAD_FAILURE_HINT.contains("-ne"));
    }
}

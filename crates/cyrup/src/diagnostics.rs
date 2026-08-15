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
const VALUE_LONG_FLAGS: [&str; 17] = [
    "--provider",
    "--api-key",
    "--models",
    "--system-prompt",
    "--append-system-prompt",
    "--tools",
    "--exclude-tools",
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

/// Apply Pi's lenient arg handling over `argv` (program name already stripped, short-aliases already
/// normalized), returning the cleaned argv clap should parse plus the collected diagnostics
/// (args.ts:80-82,131-139,202-203).
pub fn apply_arg_leniency(argv: &[String]) -> (Vec<String>, Vec<Diagnostic>) {
    let mut clean: Vec<String> = Vec::new();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
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
    (clean, diagnostics)
}

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

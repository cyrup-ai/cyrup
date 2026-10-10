/// A captured unknown CLI flag (Pi `unknownFlags` map entry, args.ts:52-53). `Bool(true)` is a bare
/// `--flag`; `Str` is `--flag=value` or `--flag value`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtFlagValue {
    Bool(bool),
    Str(String),
}

/// A captured unknown flag (its name without the leading `--`, plus its value).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtensionFlag {
    pub name: String,
    pub value: ExtFlagValue,
}

/// Rewrite Pi's multi-character short flags (`-nt`/`-nbt`/`-xt`/`-ne`/`-ns`/`-np`/`-nc`/`-na`) to
/// their long forms before clap parsing — clap's native shorts are single-character only, so these
/// Pi aliases (args.ts:116-183) are normalized here so `cyrup -nt` is accepted exactly as Pi accepts
/// it. Only exact whole-token matches are rewritten; longer combinations are left untouched, and
/// rewriting STOPS at a bare `--` so the end-of-options tail is passed through verbatim (SEAM-123).
pub fn normalize_short_aliases<I, S>(args: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut out: Vec<String> = Vec::new();
    // SEAM-123 — this pass runs over the WHOLE argv (`main.rs` feeds it `std::env::args()`), so it
    // is the first thing that can break pi's `--` contract. pi's `--` arm is the FIRST arm of
    // `parseArgs` (args.ts:82-91 @v0.87.1) and `break`s out of the loop; its alias arms
    // (`-nt`/`-nbt`/… at args.ts:141-183) sit *later* in the same `else if` chain and therefore
    // never see a post-`--` token. Without this latch `cyrup -p -- -nc` delivered the message
    // `--no-context-files`, i.e. silently different text, where pi delivers `-nc` verbatim.
    let mut end_of_options = false;
    // SEAM-152 — the token after one of pi's unconditional value flags is that flag's VALUE: pi's
    // arm takes it with `args[++i]` before any alias arm can see it (`cli/args.ts:115-131`,
    // `:134-176`, `:184-201` @f1b2e77f5), so `--name -nc` names the session `-nc` and `-xt -nt`
    // excludes a tool named `-nt`. Checked BEFORE the `--` latch: `--name --` takes `--` as the name.
    let mut value_next = false;
    let args: Vec<String> = args.into_iter().map(Into::into).collect();
    let last = args.len().saturating_sub(1);
    for (idx, arg) in args.into_iter().enumerate() {
        if end_of_options {
            out.push(arg);
            continue;
        }
        if value_next {
            value_next = false;
            out.push(arg);
            continue;
        }
        value_next = UNCONDITIONAL_VALUE_FLAGS.contains(&arg.as_str());
        // A trailing `-xt` has no value, so pi's `-xt` arm (guarded by `i + 1 < args.length`) does not
        // match and the token is `Unknown option: -xt` (`args.ts:263-264`). Left as typed, so
        // `apply_arg_leniency` reports it under that spelling.
        if arg == "-xt" && idx == last {
            out.push(arg);
            continue;
        }
        if arg == "--" {
            end_of_options = true;
            out.push(arg);
            continue;
        }
        out.push(match arg.as_str() {
            "-nt" => "--no-tools".to_string(),
            "-nbt" => "--no-builtin-tools".to_string(),
            "-xt" => "--exclude-tools".to_string(),
            "-ne" => "--no-extensions".to_string(),
            "-ns" => "--no-skills".to_string(),
            "-np" => "--no-prompt-templates".to_string(),
            "-nc" => "--no-context-files".to_string(),
            "-na" => "--no-approve".to_string(),
            _ => arg,
        });
    }
    out
}

/// The process-argv pre-pass `main` runs: [`normalize_short_aliases`], except when the first
/// argument after the program name is the `intercom` subcommand.
///
/// `cyrup intercom …` hands its tail to `cyrup_intercom::cli`, whose grammar is pi-intercom's
/// `cli.ts` — it has no multi-letter short flags, and its `--text`/`--to`/`--name` take VALUES that
/// may legitimately be `-nc` or `-na`. Rewriting those would deliver different text than was typed
/// (the same defect SEAM-123 fixed for the post-`--` tail), so the intercom argv is returned
/// verbatim. A subcommand is only recognised in that first position
/// ([`crate::subcommands::first_subcommand`]), so there is no pre-subcommand prefix to normalize.
pub fn normalize_process_argv<I, S>(args: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let raw: Vec<String> = args.into_iter().map(Into::into).collect();
    if raw.get(1).map(String::as_str) == Some("intercom") {
        return raw;
    }
    normalize_short_aliases(raw)
}

/// pi's value-taking flags whose arm consumes the next token whatever it looks like (`args[++i]`,
/// guarded only by `i + 1 < args.length`): `cli/args.ts:115-131` (`--provider`, `--model`,
/// `--api-key`, `--system-prompt`, `--append-system-prompt`, `--name`/`-n`), `:134-176` (`--session`,
/// `--session-id`, `--fork`, `--session-dir`, `--models`, `--tools`/`-t`, `--exclude-tools`/`-xt`,
/// `--thinking`) and `:184-201` (`--export`, `--extension`/`-e`, `--skill`, `--prompt-template`,
/// `--theme`) @f1b2e77f5. SEAM-152. The exceptions inspect the token and are NOT here: `--mode`
/// (`:96-110`), `--use-theme`, `--tui-mode` (`apply_arg_leniency`'s own arms), `--list-models` and
/// `-p` (which take it only when it does not start with `-`).
pub(crate) const UNCONDITIONAL_VALUE_FLAGS: &[&str] = &[
    "--provider",
    "--model",
    "--api-key",
    "--system-prompt",
    "--append-system-prompt",
    "--name",
    "-n",
    "--session",
    "--session-id",
    "--fork",
    "--session-dir",
    "--models",
    "--tools",
    "-t",
    "--exclude-tools",
    "-xt",
    "--thinking",
    "--export",
    "--extension",
    "-e",
    "--skill",
    "--prompt-template",
    "--theme",
];

/// The single-dash members of [`UNCONDITIONAL_VALUE_FLAGS`] clap knows (`-xt` is rewritten to its
/// long form before this point).
const SHORT_VALUE_FLAGS: &[&str] = &["-n", "-t", "-e"];

/// Partition `argv` (program name already stripped, short-aliases already normalized) into the args
/// clap should parse and the captured unknown `--flag[=val]` extension flags — a 1:1 port of Pi's
/// hand-rolled unknown-flag arm (args.ts:188-201). A `--flag=val` captures `(flag,val)`; a bare
/// `--flag` followed by a non-`-`/non-`@` token captures `(flag,next)` and consumes it, else captures
/// `(flag,true)`. Values of KNOWN value-taking long flags are passed through untouched (so `--model
/// --x` is not mis-captured). Single-dash unknowns are left for clap (it reports them like Pi's
/// "Unknown option" diagnostic).
pub fn partition_extension_flags(argv: &[String]) -> (Vec<String>, Vec<ExtensionFlag>) {
    let mut clean: Vec<String> = Vec::new();
    let mut flags: Vec<ExtensionFlag> = Vec::new();
    let mut i = 0usize;
    while let Some(arg) = argv.get(i) {
        // SEAM-123 — pi args.ts:82-91 @v0.87.1: `--` ENDS option parsing, and it is the first arm
        // of pi's own loop (`if (arg === "--") { … break; }`). It must be first here too: without
        // it, `arg.strip_prefix("--")` yields `""` for the bare token, `KNOWN_LONG_FLAGS` has no
        // `""`, and the unknown-long-flag arm below captured an extension flag NAMED `""` that also
        // swallowed the next token as its value. The tail is copied through verbatim — no `@`/`-`
        // inspection, no flag capture — and clap's native `--` handling routes it into
        // `Cli::positionals`, where `input.rs`'s `strip_prefix('@')` split reproduces pi's
        // `fileArgs`/`messages` partition unchanged.
        if arg == "--" {
            clean.extend(argv.iter().skip(i).cloned());
            break;
        }
        let name_part = arg.split('=').next().unwrap_or(arg);
        if let Some(stripped) = arg.strip_prefix("--") {
            if KNOWN_LONG_FLAGS.contains(&name_part) {
                clean.push(arg.clone());
                // A known value-taking flag in its space-separated form consumes the next token.
                if KNOWN_VALUE_LONG_FLAGS.contains(&name_part)
                    && !arg.contains('=')
                    && let Some(next) = argv.get(i + 1)
                {
                    clean.push(next.clone());
                    i += 2;
                    continue;
                }
                // SEAM-152 — one of pi's unconditional value flags with NOTHING after it: pi's own arm
                // is guarded by `i + 1 < args.length`, so the flag falls into `unknownFlags` as
                // `true` (`args.ts:250-262` @f1b2e77f5) and is reported at runtime as `Unknown
                // option: --<name>` unless an extension owns it (`agent-session-services.ts:120-123`).
                if !arg.contains('=')
                    && argv.get(i + 1).is_none()
                    && UNCONDITIONAL_VALUE_FLAGS.contains(&arg.as_str())
                {
                    clean.pop();
                    flags.push(ExtensionFlag {
                        name: stripped.to_string(),
                        value: ExtFlagValue::Bool(true),
                    });
                }
                i += 1;
                continue;
            }
            // Unknown long flag → capture as an extension flag (Pi args.ts:188-201).
            if let Some(eq) = stripped.find('=') {
                flags.push(ExtensionFlag {
                    name: stripped[..eq].to_string(),
                    value: ExtFlagValue::Str(stripped[eq + 1..].to_string()),
                });
                i += 1;
                continue;
            }
            match argv.get(i + 1) {
                Some(next) if !next.starts_with('-') && !next.starts_with('@') => {
                    flags.push(ExtensionFlag {
                        name: stripped.to_string(),
                        value: ExtFlagValue::Str(next.clone()),
                    });
                    i += 2;
                }
                _ => {
                    flags.push(ExtensionFlag {
                        name: stripped.to_string(),
                        value: ExtFlagValue::Bool(true),
                    });
                    i += 1;
                }
            }
            continue;
        }
        // SEAM-152 — a short value flag takes its next token verbatim (pi `args[++i]`), so a
        // `--`-leading value is not captured as an extension flag.
        if SHORT_VALUE_FLAGS.contains(&arg.as_str())
            && let Some(next) = argv.get(i + 1)
        {
            clean.push(arg.clone());
            clean.push(next.clone());
            i += 2;
            continue;
        }
        clean.push(arg.clone());
        i += 1;
    }
    (clean, flags)
}

/// Every long flag clap knows (used by [`partition_extension_flags`] to leave known flags + their
/// values for clap). Kept in lockstep with the [`super::args::Cli`] struct.
const KNOWN_LONG_FLAGS: &[&str] = &[
    "--version",
    "--help",
    "--mode",
    "--print",
    // ACP-002 — `--acp` is in the known set so it reaches clap instead of being captured as an
    // extension flag by `partition_extension_flags` (the CYRUP-DELTA on `Cli::acp`, SEAM-057).
    "--acp",
    "--provider",
    "--model",
    "--api-key",
    "--thinking",
    "--models",
    "--system-prompt",
    "--append-system-prompt",
    "--no-tools",
    "--no-builtin-tools",
    "--tools",
    "--exclude-tools",
    "--extension",
    "--no-extensions",
    "--skill",
    "--no-skills",
    "--prompt-template",
    "--no-prompt-templates",
    "--theme",
    "--use-theme",
    "--no-themes",
    "--no-context-files",
    "--approve",
    "--no-approve",
    "--continue",
    "--resume",
    "--session",
    "--session-id",
    "--fork",
    "--session-dir",
    "--no-session",
    "--name",
    "--export",
    "--list-models",
    "--tui-mode",
    "--offline",
    "--verbose",
];

/// The subset of [`KNOWN_LONG_FLAGS`] that take a value in their space-separated form (so the next
/// token must be passed through to clap, never captured as an extension flag). `--list-models` is
/// intentionally excluded — its value is optional and clap resolves it.
const KNOWN_VALUE_LONG_FLAGS: &[&str] = &[
    "--mode",
    "--provider",
    "--model",
    "--api-key",
    "--thinking",
    "--models",
    "--system-prompt",
    "--append-system-prompt",
    "--tools",
    "--exclude-tools",
    "--extension",
    "--skill",
    "--prompt-template",
    "--theme",
    "--use-theme",
    "--session",
    "--session-id",
    "--fork",
    "--session-dir",
    "--name",
    "--export",
    // pi consumes `args[i + 1]` for `--tui-mode` (args.ts:181 @v0.84.1), so the value token must
    // reach clap rather than being captured as an extension flag (SEAM-051).
    "--tui-mode",
];

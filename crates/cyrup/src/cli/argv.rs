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
/// it. Only exact whole-token matches are rewritten; longer combinations are left untouched.
pub fn normalize_short_aliases<I, S>(args: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    args.into_iter()
        .map(Into::into)
        .map(|a| match a.as_str() {
            "-nt" => "--no-tools".to_string(),
            "-nbt" => "--no-builtin-tools".to_string(),
            "-xt" => "--exclude-tools".to_string(),
            "-ne" => "--no-extensions".to_string(),
            "-ns" => "--no-skills".to_string(),
            "-np" => "--no-prompt-templates".to_string(),
            "-nc" => "--no-context-files".to_string(),
            "-na" => "--no-approve".to_string(),
            _ => a,
        })
        .collect()
}

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
    "--output-format",
    "--json",
    "--rpc",
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
    "--output-format",
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

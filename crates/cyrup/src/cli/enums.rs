use clap::ValueEnum;
use cyrup_sdk::core::ModelThinkingLevel;

/// Pi's primary output selector `--mode <text|json|rpc>` (args.ts:78-82).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lower")]
pub enum Mode {
    /// Human-oriented text (the default; interactive unless `--print`/non-TTY).
    Text,
    /// One `AgentSessionEvent` per line (JSONL).
    Json,
    /// The persistent stdin/stdout RPC line protocol.
    Rpc,
}

/// Output format for the non-interactive one-shot path (`--output-format`; a cyrup back-compat
/// alias — Pi expresses this through `--mode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lower")]
pub enum OutputFormat {
    /// Human-oriented final assistant text (PRINT mode).
    Text,
    /// One `AgentSessionEvent` per line (JSON mode / JSONL).
    Json,
}

/// `--thinking <level>` (args.ts:57,130).
///
/// Clap validates membership, but it never SEES an invalid value: pi's warn-and-continue path
/// (`args.ts:135`) is ported and runs **pre-clap**, in `diagnostics.rs`'s `apply_arg_leniency`
/// (called from `main.rs` before `Cli::parse_from`). That pass keeps a value in
/// `VALID_THINKING_LEVELS` (`diagnostics.rs`, the same seven entries as `args.ts:57`) and otherwise
/// DROPS both tokens with pi's `Invalid thinking level "{value}". Valid values: {joined}`, so clap
/// only ever receives a valid value or no flag at all.
///
/// SEAM-029 — this comment used to claim the leniency path was "unreachable here", which is the
/// opposite of the truth and is exactly what mis-set a previous edition of the gap analysis: a
/// reader concludes the path does not exist and files a false gap.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lower")]
pub enum ThinkingArg {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl ThinkingArg {
    /// Map onto the core [`ModelThinkingLevel`].
    pub fn to_level(self) -> ModelThinkingLevel {
        match self {
            ThinkingArg::Off => ModelThinkingLevel::Off,
            ThinkingArg::Minimal => ModelThinkingLevel::Minimal,
            ThinkingArg::Low => ModelThinkingLevel::Low,
            ThinkingArg::Medium => ModelThinkingLevel::Medium,
            ThinkingArg::High => ModelThinkingLevel::High,
            ThinkingArg::Xhigh => ModelThinkingLevel::Xhigh,
            ThinkingArg::Max => ModelThinkingLevel::Max,
        }
    }
}

/// `--tui-mode <regular|fullscreen>` (pi `args.ts:180-192` @v0.84.1; the `TuiMode` type is
/// `settings-manager.ts:36` @v0.84.1, re-exported from `pi-tui`). Upstream drift: the flag does not
/// exist at v0.83.0, the tag cyrup ported — see ADR-0005, which decided cyrup DOES build the
/// alternate-screen renderer, so the value is modelled in full here rather than being collapsed to a
/// bool. `regular` is pi's documented default and is a working no-op.
///
/// The ADR-0005 §A-2 interim that declined `fullscreen` at startup is GONE — deleted by work unit
/// B-13, which is what the grep for its wording was planted to catch. Both values are now accepted
/// in silence. (`crates/cyrup-it/tests/bin/tui_mode_flag.rs:135` still asserts that refusal text and
/// is therefore red against this file; it belongs to the test owners, not to this crate.)
///
/// # What a value parsed here still has to travel through
/// Nothing between [`crate::Cli`] and the TUI reads this field. The renderer is selected by
/// `cyrup_tui::App::switch_tui_mode(TuiRenderMode::Fullscreen, …)`
/// (`crates/cyrup-tui/src/app/mode_switch.rs`), and the `App` is constructed in
/// `crate::interactive::run_interactive` — so that function is the one place the merge can happen,
/// and it is where ADR-0005 §B-14 puts it:
///
/// 1. this flag, when given, wins; otherwise
/// 2. the persisted `tuiMode` key — `EffectiveSettings::tui_mode()`, ADR-0005 §A-3, already live in
///    `cyrup-config` and already offered by the `/settings` `TUI mode` row; otherwise
/// 3. `regular`, which is pi's default and a working no-op.
///
/// The two `TuiMode` enums that step 1-vs-2 has to reconcile — this clap `ValueEnum` and
/// `cyrup_config::settings::TuiMode` — carry the same two variants with the same lowercase
/// spellings, so the mapping is total in both directions. `main.rs` cannot do the merge itself:
/// `run_interactive` owns the `App` for its whole lifetime and takes no mode argument today.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lower")]
pub enum TuiMode {
    /// The inline (main-screen) renderer — pi's default (`settings-manager.ts:1129` @v0.84.1).
    Regular,
    /// The alternate-screen renderer (`tui-alt-screen.ts` @v0.84.1), built by ADR-0005 §Decision B
    /// in `crates/cyrup-tui/src/altscreen/`.
    Fullscreen,
}

/// Split a `--models` pattern into its base and optional `:level` thinking suffix (Pi
/// `resolveModelScope`, main.ts:685): `sonnet:high` ⇒ `("sonnet", Some(High))`. Only a trailing,
/// recognized level is treated as a suffix (so `provider/id` slashes are preserved).
pub fn split_model_level(pattern: &str) -> (String, Option<ThinkingArg>) {
    if let Some((base, level)) = pattern.rsplit_once(':')
        && let Ok(parsed) = ThinkingArg::from_str(level, true)
    {
        return (base.to_string(), Some(parsed));
    }
    (pattern.to_string(), None)
}

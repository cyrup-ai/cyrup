//! cyrup (lib) — the testable core of the CLI binary (arch-11 §2.4).
//!
//! The binary's real logic lives here so it can be exercised without a TTY: argument parsing
//! ([`cli`]) and its mapping onto a [`cyrup_session_svc::SessionConfig`], prompt-input assembly from
//! positionals / `@file` / piped stdin ([`input`], R-11-006/024/025), the provider-selection seam
//! ([`provider`], faux today / clear error for an unimplemented real provider), the non-interactive
//! mode dispatchers over injectable readers/writers ([`run`], R-11-005/007/011), and per-mode signal
//! handling ([`signals`], R-11-010/018).
//!
//! `main.rs` stays thin: it parses args, initialises tracing to stderr, wires real stdio /
//! `CrosstermBackend`, calls into this library, and maps results to process exit codes — the single
//! `anyhow` boundary (arch-00 §8).
#![forbid(unsafe_code)]

pub mod cli;
pub mod diagnostics;
pub mod input;
pub mod migrations;
pub mod provider;
pub mod run;
pub mod signals;
pub mod startup;
pub mod subcommands;
pub mod timings;

pub use cli::{
    normalize_short_aliases, partition_extension_flags, render_help, resolve_app_mode,
    should_take_over_stdout, Cli, ExtFlagValue, ExtensionFlag, Mode, OutputFormat, ThinkingArg,
};
pub use diagnostics::{
    apply_arg_leniency, format_no_models_available_message, get_provider_login_help, Diagnostic,
    DiagnosticLevel, EXTENSION_LOAD_FAILURE_HINT,
};
pub use input::{build_inputs, compose_inputs, split_positionals, Inputs};
pub use provider::select_provider;
pub use run::{exit_code, initial_input, run_json_dispatch, run_print_dispatch, run_rpc_dispatch};
pub use signals::spawn_abort_on_signal;
pub use startup::{
    are_experimental_features_enabled, file_settings_store, is_official_distribution,
    should_run_first_time_setup,
};

// Re-export the runtime-mode enum the whole bin pivots on (arch-11 §6.1).
pub use cyrup_config::AppMode;

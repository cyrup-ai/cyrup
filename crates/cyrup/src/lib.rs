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
pub mod credential_print;
pub mod diagnostics;
pub mod input;
pub mod intercom_broker_cmd;
pub mod migrations;
pub mod output_guard;
pub mod provider;
pub mod run;
pub mod session_resolve;
pub mod signals;
pub mod startup;
pub mod startup_ui;
pub mod subagent_config;
pub mod subagent_runner_cmd;
pub mod subcommands;
pub mod timings;
pub mod update_check;

pub use cli::{
    Cli, ExtFlagValue, ExtensionFlag, Mode, OutputFormat, ThinkingArg, normalize_short_aliases,
    partition_extension_flags, render_help, resolve_app_mode, should_take_over_stdout,
};
pub use credential_print::{
    CredentialPrintCommand, CredentialPrintError, CredentialPrintKind, credential_print_help,
    is_credential_print_help, parse_credential_print_command, resolve_credential_for_print,
    validate_credential_print_args,
};
pub use diagnostics::{
    Diagnostic, DiagnosticLevel, EXTENSION_LOAD_FAILURE_HINT, apply_arg_leniency,
    format_no_models_available_message, get_provider_login_help,
};
pub use input::{Inputs, build_inputs, compose_inputs, split_positionals};
pub use output_guard::{
    emit_stray, emit_stray_line, is_stdout_taken_over, restore_stdout, take_over_stdout,
};
pub use provider::{select_provider, unknown_model_warning};
pub use run::{
    dispose_session, exit_code, initial_input, run_json_dispatch, run_print_dispatch,
    run_rpc_dispatch,
};
pub use session_resolve::{
    MissingSessionCwd, Outcome, Resolution, SessionFlags, SessionLookup, SessionRef,
    format_missing_session_cwd_prompt, match_session_arg, missing_session_cwd_error,
    resolve_session_target, session_cwd_is_missing,
};
pub use signals::spawn_abort_on_signal;
pub use startup::{
    apply_settings_session_dir, are_experimental_features_enabled, file_settings_store,
    is_official_distribution, should_run_first_time_setup,
};
pub use startup_ui::{
    MissingCwdChoice, ResumeChoice, TrustChoice, has_trust_requiring_project_resources,
    run_missing_cwd_prompt, run_resume_picker, run_trust_prompt, session_rows, trust_needs_prompt,
};

// Re-export the runtime-mode enum the whole bin pivots on (arch-11 §6.1).
pub use cyrup_config::AppMode;

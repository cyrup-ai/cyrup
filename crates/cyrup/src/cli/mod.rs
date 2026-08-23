//! The clap argument surface and its mapping onto a [`cyrup_session_svc::SessionConfig`] + the
//! runtime [`cyrup_config::AppMode`] (arch-11 §3.7/§6.1; R-11-001/024). A 1:1 port of Pi
//! `cli/args.ts`: every flag Pi's hand-rolled parser accepts is present here, with Pi's short
//! aliases (incl. the multi-char `-nt`/`-nbt`/`-xt`/`-ne`/`-ns`/`-np`/`-nc`/`-na` forms, which clap
//! cannot express as native shorts — they are rewritten to their long forms by
//! [`normalize_short_aliases`] before parsing). The mapping is otherwise free of I/O so it stays
//! unit-testable; the ONE exception is [`resolve_prompt_input`], Pi's `resolvePromptInput`
//! (resource-loader.ts:53-68), which must stat and read the `--system-prompt`/
//! `--append-system-prompt` token to decide path-vs-literal. It takes its `cwd` explicitly so a
//! test can point it at a tempdir.
//!
//! Split by concern across this directory: [`args`] (the clap struct itself), [`enums`] (its
//! value-enum types), [`argv`] (pre-clap argv preprocessing: short aliases + unknown-flag capture),
//! [`session_target`] (which session file this run resolves to), [`config_map`] (mapping the
//! parsed CLI onto a `SessionConfig`), [`runtime_mode`] (`AppMode` resolution + stdout takeover),
//! [`help`] (the `--help` body).

mod args;
mod argv;
mod config_map;
mod enums;
mod help;
mod runtime_mode;
mod session_target;

#[cfg(test)]
mod tests;

pub use args::Cli;
pub use argv::{ExtFlagValue, ExtensionFlag, normalize_short_aliases, partition_extension_flags};
pub use config_map::{is_local_path, resolve_cli_paths, resolve_prompt_input};
pub use enums::{Mode, OutputFormat, ThinkingArg, TuiMode, split_model_level};
pub use help::render_help;
pub use runtime_mode::{
    is_plain_runtime_metadata_command, resolve_app_mode, should_take_over_stdout,
};
pub use session_target::assert_valid_session_id;

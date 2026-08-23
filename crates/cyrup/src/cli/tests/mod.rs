#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use clap::Parser;
use cyrup_config::ConfigDirs;

use super::*;

mod args;
mod argv;
mod config_map;
mod enums;
mod help;
mod runtime_mode;
mod session_target;

fn parse(args: &[&str]) -> Cli {
    let mut full = vec!["cyrup".to_string()];
    full.extend(normalize_short_aliases(args.iter().map(|s| s.to_string())));
    Cli::try_parse_from(full).expect("parse")
}

/// The whole pre-clap pipeline `main.rs` runs, in `main.rs`'s order: short-alias normalization →
/// [`crate::diagnostics::apply_arg_leniency`] → `partition_extension_flags` → clap →
/// `Cli::normalize_list_flags` → `Cli::restore_escaped_positionals`. The plain [`parse`]
/// helper above skips the first two, which is exactly where SEAM-103/105/107 live.
fn parse_like_main(args: &[&str]) -> Cli {
    let raw = normalize_short_aliases(args.iter().map(|s| s.to_string()));
    let (lenient, _) = crate::diagnostics::apply_arg_leniency(&raw);
    let (clean, extension_flags) = partition_extension_flags(&lenient);
    let mut full = vec!["cyrup".to_string()];
    full.extend(clean);
    let mut cli = Cli::try_parse_from(full).expect("parse");
    cli.extension_flags = extension_flags;
    cli.normalize_list_flags();
    cli.restore_escaped_positionals();
    cli
}

fn dirs() -> ConfigDirs {
    ConfigDirs {
        agent_dir: "/agent".into(),
        session_dir: "/agent/sessions".into(),
        session_dir_explicit: false,
        package_dir: "/agent/packages".into(),
        cwd: "/work".into(),
        home: "/home/user".into(),
    }
}

/// Build a `ConfigDirs` whose `cwd` is a real tempdir, so relative-path resolution can be
/// exercised exactly the way Pi's `existsSync(input)` resolves against `process.cwd()`.
fn dirs_at(cwd: &std::path::Path) -> ConfigDirs {
    ConfigDirs {
        cwd: cwd.to_path_buf(),
        ..dirs()
    }
}

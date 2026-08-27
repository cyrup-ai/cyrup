//! `cargo run -p xtask -- feature-matrix` — the non-default feature gate.
//!
//! `cargo check --workspace --all-targets` (README "Build") builds ONE point in the feature space.
//! Nine crates declare `[features]`, and the combinations that are NOT that point are where
//! compilation errors hide in this workspace — the `#[cfg(not(feature = "wasm-host"))]` arms of
//! `cyrup-ext` / `cyrup-session-svc` are never compiled by the everyday gate, and neither is any
//! build with `ratatui/scrolling-regions` off. A `cyrup-tui` integration test once carried an
//! ungated `impl Backend::scroll_region_{up,down}` for exactly as long as nothing in the repo
//! ever compiled it with that feature off — this matrix is what found it.
//!
//! There is no CI here (README "Build": *"There is no CI in this repository, so nothing runs these
//! for you"*), so this is a command, not a workflow. It sits beside `cargo clippy` in the README's
//! Build block and is run the same way: by hand, before merging.
//!
//! # Why a curated list and not `cargo hack --feature-powerset`
//!
//! `cargo-hack` is not installed here, and a powerset is the wrong shape for this workspace anyway.
//! Three things it cannot express, each load-bearing:
//!
//! * **`--all-features` must never select `cyrup-it`.** It sets `it`, which un-no-ops
//!   `crates/cyrup-it/build.rs` (`build.rs:95`) into a nested build of five workspace binaries plus
//!   a `wasm32-wasip2` guest component, and re-arms every seam test. `docs/TEST-ARCHITECTURE.md`
//!   §9.3 G3 exists for exactly this. `--exclude cyrup-it` is encoded in the row's own `args`, not
//!   left to whoever types the command.
//! * **A row can be worth running without proving what it looks like it proves.** The
//!   `cyrup-session-svc --no-default-features` row compiles the native arms; it does NOT remove
//!   wasmtime (EXT-026). Its `why` says so, so a green run cannot be over-read.
//! * **A row can be worth keeping while being a no-op *today*.** `--workspace
//!   --no-default-features` is one — see that row's `why` for the measurement and why it stays.
//!
//! Every row carries the obligation it discharges, and the obligation is printed when the row fails.

use std::path::PathBuf;
use std::process::Command;

/// One row: the cargo verb, everything after it, and the obligation it discharges.
struct Combo {
    /// `check` for every row but one — MCP-037a's verify line requires `cyrup-ext`'s tests to RUN
    /// on both arms of `wasm-host`, not merely to type-check
    /// (`docs/gap-analysis/13a-mcp-activation.md:1895-1899`).
    verb: &'static str,
    args: &'static [&'static str],
    /// Printed when the row fails. Name the obligation, not the command.
    why: &'static str,
    /// Rows that cost minutes: excluded by `--fast`.
    slow: bool,
}

impl Combo {
    fn label(&self) -> String {
        format!("cargo {} {}", self.verb, self.args.join(" "))
    }
}

const MATRIX: &[Combo] = &[
    Combo {
        verb: "check",
        args: &["--workspace", "--all-targets"],
        why: "the default point — README \"Build\". Every other row is a departure from it.",
        slow: false,
    },
    Combo {
        verb: "check",
        args: &["-p", "cyrup-ext", "--no-default-features", "--all-targets"],
        why: "the `#[cfg(not(feature = \"wasm-host\"))]` arms of facade.rs compile in NO other row: \
              every in-workspace edge into `cyrup-ext` asks for its default features, so only a \
              `-p cyrup-ext` selection can take `wasm-host` away. EXT-026 found a hard build error \
              here once already.",
        slow: false,
    },
    Combo {
        verb: "nextest",
        args: &["run", "-p", "cyrup-ext", "--no-default-features"],
        why: "MCP-037a's verify line: `refresh_tools` must report a late NATIVE registration on \
              BOTH arms of `wasm-host`. The two tests (src/tests/seam_liveness.rs:242, :265) are \
              deliberately feature-agnostic; this is the run that exercises the arm the everyday \
              gate skips. A type-check would not catch a wrong answer, only a wrong type.",
        slow: false,
    },
    Combo {
        verb: "check",
        args: &["-p", "cyrup-session-svc", "--no-default-features", "--all-targets"],
        why: "compiles the native arms (builder.rs:936, :1158, :2056; session.rs:1180), which no \
              other row compiles. It does NOT remove wasmtime: `src/lib.rs:30` declares \
              `mod host_services;` ungated and that module names `cyrup_ext::{caps, host}::*`, so \
              the crate's `cyrup-ext` edge pins `features = [\"wasm-host\"]`. A wasmtime-free build \
              of this crate is EXT-026 (docs/gap-analysis/06-cyrup-ext.md:263), not this row.",
        slow: false,
    },
    Combo {
        verb: "check",
        args: &["-p", "cyrup-tools", "--no-default-features", "--all-targets"],
        why: "`inline-images` off: read.rs:330's fallback arm and the \
              `#[cfg(not(feature = \"inline-images\"))]` test at src/tests/tools.rs:210.",
        slow: false,
    },
    Combo {
        verb: "check",
        args: &["-p", "cyrup-tui", "--no-default-features", "--all-targets"],
        why: "`scrolling-regions` off removes two REQUIRED methods from ratatui's `Backend` \
              (ratatui-core-0.1.2/src/backend.rs:362, :387); every `impl Backend` reachable from \
              this selection must gate them to match.",
        slow: false,
    },
    Combo {
        verb: "check",
        args: &[
            "-p",
            "cyrup-tui",
            "--no-default-features",
            "--features",
            "scrollback-accumulator",
            "--all-targets",
        ],
        why: "The row that was red. A `cyrup-tui` integration test self-gated on \
              `scrollback-accumulator` while implementing `scroll_region_{up,down}` UNGATED, so this \
              was the only combination in which the file compiled but the trait methods did not \
              exist (E0407 x2). That test has since been gated, then deleted as the throwaway it \
              declared itself to be, so this row is now a regression guard rather than a live \
              failure: nothing else reaches this corner, because cyrup-it only ever adds the \
              accumulator ON TOP of defaults (crates/cyrup-it/Cargo.toml:99).",
        slow: false,
    },
    Combo {
        verb: "check",
        args: &["-p", "cyrup-tui", "--features", "scrollback-accumulator", "--all-targets"],
        why: "defaults + the accumulator — the shape cyrup-it's dev edge creates \
              (crates/cyrup-it/Cargo.toml:99), and the only one in which the perf probe's \
              `scroll_region_*` delegations are actually compiled.",
        slow: false,
    },
    Combo {
        verb: "check",
        args: &["-p", "cyrup-provider", "--features", "faux", "--all-targets"],
        why: "the scripted double compiles standalone, not only via cyrup-test-support's edge.",
        slow: false,
    },
    Combo {
        verb: "check",
        args: &["-p", "cyrup", "--features", "faux", "--all-targets"],
        why: "src/provider.rs:525's `Some(\"faux\")` arm — the spawn-the-binary tests in cyrup-it \
              depend on this compiling.",
        slow: false,
    },
    Combo {
        verb: "check",
        args: &["-p", "cyrup-ext-subagents", "--features", "test-fixtures", "--all-targets"],
        why: "the two `required-features` fixture bins (Cargo.toml:92-112); cyrup-it's build.rs \
              builds them BY NAME and fails if a target stops existing.",
        slow: false,
    },
    Combo {
        verb: "check",
        args: &["-p", "cyrup-intercom", "--features", "test-fixtures", "--all-targets"],
        why: "same, for cyrup-intercom-child-fixture (Cargo.toml:62-72).",
        slow: false,
    },
    Combo {
        verb: "check",
        args: &["--workspace", "--no-default-features", "--all-targets"],
        why: "TRIPWIRE, and today a no-op — do not read a green here as proof that anything is \
              optional. Measured with `cargo tree --workspace --no-default-features -e \
              normal,build,dev --format '{p}|{f}'`, this resolves identically to row 1 except that \
              cyrup-it's `default` (which is `[]`) goes off: every in-workspace edge requests its \
              dependency's default features, so `cyrup-ext/wasm-host`, `cyrup-tools/inline-images` \
              and `cyrup-tui/scrolling-regions` all come straight back. It is kept, and it is cheap \
              (identical units, so cargo serves it from cache) because the day any member adds \
              `default-features = false` to a `cyrup-*` edge, this is the row that starts \
              compiling a graph nothing else compiles.",
        slow: false,
    },
    Combo {
        verb: "check",
        // `--exclude cyrup-it` is NOT optional and NOT the caller's business: `--all-features`
        // sets `it`, which un-no-ops crates/cyrup-it/build.rs into a nested build of five binaries
        // plus a wasm guest and re-arms every seam test (docs/TEST-ARCHITECTURE.md §9.3 G3).
        args: &["--workspace", "--exclude", "cyrup-it", "--all-features", "--all-targets"],
        why: "every optional feature in the workspace on AT ONCE — two that are individually fine \
              and jointly contradictory fail HERE and nowhere else. `--exclude cyrup-it` is part \
              of the row's data; feature selection is per-package, so deselecting that one package \
              is what keeps `--all-features` from reaching `cyrup-it/it`.",
        slow: false,
    },
    Combo {
        verb: "check",
        args: &["-p", "cyrup-ext-sdk", "--target", "wasm32-wasip2"],
        why: "the guest SDK is excluded from default-members, so nothing else type-checks it for \
              the target it actually ships to. Needs `rustup target add wasm32-wasip2` \
              (setup.sh:18-24 installs it).",
        slow: false,
    },
    Combo {
        verb: "check",
        args: &["-p", "cyrup-it", "--features", "it,wasm-host", "--all-targets"],
        why: "the deliberate suite's own type-check — its [[test]] targets are `required-features \
              = [\"it\"]`, so the everyday gate never compiles a line of them. SLOW: build.rs runs \
              a nested cargo build of five binaries plus the wasm guest (build.rs:95-180). Set \
              CYRUP_IT_BIN_DIR and CYRUP_EXT_FIXTURE_COMPONENT to skip that, or pass --fast to \
              skip this row.",
        slow: true,
    },
];

fn cargo_bin() -> std::ffi::OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}

/// `feature-matrix [--fast]`. Runs every row, reports every failure — it does NOT stop at the
/// first, because the point of a matrix is to learn how many combinations are broken, not one.
pub fn run_matrix(flags: &[String], root: PathBuf) -> Result<(), String> {
    let mut fast = false;
    for flag in flags {
        match flag.as_str() {
            "--fast" => fast = true,
            other => {
                return Err(format!("unknown flag {other:?} — feature-matrix takes `--fast`"))
            }
        }
    }

    let mut failed: Vec<String> = Vec::new();
    let mut ran = 0usize;
    for combo in MATRIX {
        if fast && combo.slow {
            println!("SKIP  {} (--fast)", combo.label());
            continue;
        }
        ran += 1;
        println!("\n──── {}", combo.label());
        let status = Command::new(cargo_bin())
            .current_dir(&root)
            .arg(combo.verb)
            .args(combo.args)
            .status()
            .map_err(|e| format!("cannot run cargo: {e}"))?;
        if !status.success() {
            eprintln!("FAIL  {}\n      {}", combo.label(), combo.why);
            failed.push(combo.label());
        }
    }

    if failed.is_empty() {
        println!("\nfeature-matrix: {ran} combination(s) green");
        return Ok(());
    }
    Err(format!(
        "{} of {ran} combination(s) failed:\n  {}",
        failed.len(),
        failed.join("\n  ")
    ))
}

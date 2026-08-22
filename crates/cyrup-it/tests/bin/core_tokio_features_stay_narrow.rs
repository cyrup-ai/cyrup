//! `cyrup-core` must keep the **narrowed** tokio feature set it was given, and must not silently
//! drift back to `tokio = { workspace = true }`.
//!
//! ## What is being guarded
//!
//! `crates/cyrup-core/Cargo.toml` declares tokio directly —
//! `{ version = "1", default-features = false, features = ["sync"] }` — instead of inheriting the
//! workspace entry. The workspace entry (root `Cargo.toml`) turns on the union every member needs
//! (`rt-multi-thread`, `macros`, `fs`, `process`, `io-util`, `time`, `signal`), while this crate
//! does "no I/O, no tokio tasks of its own" (`crates/cyrup-core/src/lib.rs`): its only tokio path is
//! `tokio::sync::mpsc` in `event_stream.rs`. Inheritance cannot express the narrowing — cargo
//! hard-errors with ``default-features = false` cannot override workspace's `default-features``, so
//! the version is spelled out by hand, and a hand-spelled dependency is exactly the kind that gets
//! "tidied" back to `workspace = true` by the next reader who does not know why it is not.
//!
//! ## Why this cannot be a unit test in `cyrup-core`
//!
//! The invariant is a **Cargo feature-graph** property, resolved before any Rust is parsed, so no
//! `#[cfg]`-based test can observe it: whether `feature = "fs"` is on inside tokio is decided by the
//! resolver, and the resolver is what would regress. It has to shell out to `cargo tree`, which
//! makes it an integration test, and `docs/TEST-ARCHITECTURE.md` §0 keeps every crate on unit tests
//! only with integration tests living in this single crate. Hence: a `[[test]]` target here, in the
//! same binary as its sibling instrument `faux_not_in_normal_build.rs`, whose `cargo tree --offline
//! --locked` + [`SKIP_VAR`] conventions this file follows deliberately. Change one, look at the
//! other.
//!
//! ## SCOPE — stated exactly, so nobody maintains the wrong invariant
//!
//! **The bound is PER-CRATE.** The assertion below is about `cargo tree -p cyrup-core -e normal`,
//! i.e. the graph a `cargo build -p cyrup-core` (or `cargo check -p cyrup-core`) resolves. It is
//! *not* true of `--workspace`: Cargo features are additive and unified per package across
//! everything built in one invocation, so a `--workspace` build still hands `cyrup-core` a tokio
//! compiled with the union of every member's features, `fs` and `process` included. That is
//! expected, and it does not make the narrowing pointless — the narrowing is what lets `cyrup-core`
//! be built, checked and (eventually) published on its own without dragging tokio's I/O driver in,
//! and it is what documents that this crate's tokio surface is `sync` and nothing more.
//!
//! `-e normal` is the load-bearing flag: it excludes `[dev-dependencies]` and
//! `[build-dependencies]`, so the dev entry's extra `macros`/`rt` (needed for `#[tokio::test]`) are
//! correctly invisible here.
//!
//! ## RED → GREEN
//!
//! Demonstrated against the live manifest: with `tokio = { workspace = true }` restored in
//! `crates/cyrup-core/Cargo.toml` the guard is RED and names `fs, io-util, process, rt-multi-thread,
//! signal`; with the narrowed declaration back in place it is GREEN.

// Test target: the workspace no-panic lints (`unwrap_used`/`expect_used`/`panic`) are policy for
// production code. `assert!` is the point of this file, and a manifest path that cannot resolve its
// own workspace root is an environment bug the test should surface loudly.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::process::Command;

/// The **opt-in** escape hatch for an environment that genuinely cannot run cargo (an air-gapped
/// packager, a vendored source tarball with no registry index). Same variable, same contract and
/// same reasoning as `faux_not_in_normal_build.rs`: a guard that goes green when its instrument
/// breaks is not a guard, and the single likeliest cause of a non-zero `--offline --locked` here is
/// a manifest edit that outran the lockfile — precisely the class of change this file exists to
/// police.
const SKIP_VAR: &str = "CYRUP_SKIP_CARGO_GRAPH_TESTS";

/// The tokio features `cyrup-core` must NOT be resolving on a normal edge. Each one drags in
/// machinery this crate has no use for: `fs`/`process`/`signal`/`io-util` pull tokio's I/O driver
/// and (on unix) `mio` + `signal-hook-registry`; `rt-multi-thread` pulls the work-stealing
/// scheduler. `time` and `default` are deliberately absent from this list — they arrive through
/// `tokio-stream`/`tokio-util`, which `cyrup-core` genuinely depends on, and are not what the
/// narrowing was about.
const FORBIDDEN: &[&str] = &["fs", "process", "signal", "io-util", "rt-multi-thread"];

/// The opt-out predicate, split from the environment read so its contract is directly testable:
/// **absent, empty and `0` all mean "do not skip"**. Only an affirmative value opts out.
fn skip_from(raw: Option<&str>) -> bool {
    match raw {
        Some(v) => !v.is_empty() && v != "0",
        None => false,
    }
}

fn skip_requested() -> bool {
    skip_from(std::env::var(SKIP_VAR).ok().as_deref())
}

/// Locate the workspace root from this crate's manifest dir (`crates/cyrup-it`).
/// `CARGO_MANIFEST_DIR` is the *package's* dir, not the test file's, so the `tests/bin/` nesting is
/// invisible here.
fn workspace_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/cyrup-it is two levels below the workspace root")
        .to_path_buf()
}

/// Run `cargo tree -p cyrup-core -e normal -f "{p}|{f}"` in the workspace root. `Err` carries a
/// diagnostic good enough to act on: the exact argv, the exit status, and cargo's own stderr.
fn cargo_tree() -> Result<String, String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let args = [
        "tree",
        "-p",
        "cyrup-core",
        "-e",
        "normal",
        "--offline",
        "--locked",
        "-f",
        "{p}|{f}",
    ];
    let mut cmd = Command::new(&cargo);
    cmd.current_dir(workspace_root()).args(args);
    // `tree` only resolves, it does not build, so this takes no target-dir build lock — which is
    // why it needs no `--target-dir` of its own and cannot deadlock against the outer cargo that is
    // running this suite. (`--offline` additionally keeps it off the package-cache download lock.)
    // Like its sibling, this nested cargo is deliberately left in a test body rather than hoisted
    // into `build.rs`: resolving the graph IS the assertion, so it must run at test time, against
    // the live manifests, not once at build time.
    let out = cmd
        .output()
        .map_err(|e| format!("could not spawn `{cargo}`: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "`{cargo} {}` exited with {}\n--- cargo stderr ---\n{}",
            args.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim_end()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Resolve the feature graph, or **fail**. Returns `None` only when the skip was explicitly
/// requested via [`SKIP_VAR`]; every other cargo error panics, because an unavailable instrument
/// must not be reported as a satisfied invariant.
fn feature_graph_or_fail() -> Option<String> {
    match cargo_tree() {
        Ok(out) => Some(out),
        Err(why) if skip_requested() => {
            eprintln!(
                "{SKIP_VAR} is set — skipping the cyrup-core tokio-narrowing guard BY REQUEST. \
                 The invariant is therefore UNCHECKED in this run.\n{why}"
            );
            None
        }
        Err(why) => panic!(
            "TOKIO-NARROWING GUARD COULD NOT RUN — failing rather than passing silently, because a \
             green result here would be indistinguishable from a satisfied invariant.\n\n{why}\n\n\
             Most likely causes, in order: (1) `Cargo.lock` is stale relative to the manifests — \
             run `cargo check --workspace` (or `cargo update -w`) and re-run; (2) no vendored \
             registry for `--offline`; (3) no cargo on PATH and `CARGO` unset.\n\n\
             If this environment truly cannot run cargo, opt out EXPLICITLY with `{SKIP_VAR}=1` and \
             check the invariant by hand:\n  \
             cargo tree -p cyrup-core -e normal -f \"{{p}}|{{f}}\" | grep '^.*tokio v'   \
             # the `tokio` rows must list neither fs, process, signal, io-util nor rt-multi-thread"
        ),
    }
}

/// Pull the resolved feature list off every `tokio` row of a `-f "{p}|{f}"` tree.
///
/// Split out from the subprocess so its parsing contract is directly testable — the failure mode
/// this protects against is a format drift that makes the extractor match nothing, which would turn
/// the guard green forever. Rows look like:
///
/// ```text
/// ├── tokio v1.52.3|default,sync,time
/// │   └── tokio v1.52.3|default,sync,time (*)
/// ├── tokio-stream v0.1.18|default,time
/// ```
///
/// `tokio-stream` and `tokio-util` must NOT match: their feature lists are their own.
fn tokio_feature_rows(tree: &str) -> Vec<Vec<&str>> {
    tree.lines()
        .filter_map(|line| {
            let (pkg, feats) = line.split_once('|')?;
            // Strip cargo's box-drawing prefix; what remains starts at the package name.
            let name = pkg
                .trim_start_matches(|c: char| c.is_whitespace() || "│├└─".contains(c))
                .split_whitespace()
                .next()?;
            if name != "tokio" {
                return None;
            }
            // `(*)` marks a subtree cargo has already printed in full; it trails the features.
            let feats = feats.trim().trim_end_matches("(*)").trim();
            Some(
                feats
                    .split(',')
                    .map(str::trim)
                    .filter(|f| !f.is_empty())
                    .collect(),
            )
        })
        .collect()
}

/// The invariant. `-e normal` is the graph a per-crate `cargo build -p cyrup-core` resolves.
#[test]
fn cyrup_core_resolves_tokio_without_the_io_and_runtime_features() {
    let Some(tree) = feature_graph_or_fail() else {
        return;
    };

    let rows = tokio_feature_rows(&tree);

    // Instrument check FIRST. Zero rows means the extractor matched nothing — cargo's `{p}|{f}`
    // format drifted, or tokio left the graph entirely. Either way the assertion below would be
    // vacuously true, so fail here instead.
    assert!(
        !rows.is_empty(),
        "TOKIO-NARROWING GUARD FOUND NO `tokio` ROW in `cargo tree -p cyrup-core -e normal -f \
         \"{{p}}|{{f}}\"` — the instrument is broken, not the invariant satisfied. Either cargo's \
         tree format changed (the extractor keys on `<name> <version>|<features>`) or cyrup-core no \
         longer depends on tokio at all, in which case delete this guard deliberately rather than \
         letting it pass on nothing.\n\n--- tree ---\n{tree}"
    );

    let offenders: Vec<&str> = FORBIDDEN
        .iter()
        .copied()
        .filter(|bad| rows.iter().any(|feats| feats.contains(bad)))
        .collect();

    assert!(
        offenders.is_empty(),
        "TOKIO NARROWING REGRESSED — `cyrup-core` resolves tokio with {:?} on a NORMAL edge.\n\n\
         `crates/cyrup-core/Cargo.toml` must keep declaring tokio directly as\n  \
         tokio = {{ version = \"1\", default-features = false, features = [\"sync\"] }}\n\
         and must NOT be \"tidied\" back to `tokio = {{ workspace = true }}` — the workspace entry \
         turns on the union every member needs (rt-multi-thread, macros, fs, process, io-util, \
         time, signal), while this crate's only tokio path is `tokio::sync::mpsc` in \
         `event_stream.rs`. Inheritance cannot express the narrowing: cargo hard-errors with \
         \"`default-features = false` cannot override workspace's `default-features`\", which is \
         why the version is spelled out by hand there.\n\n\
         If a new `cyrup-core` module genuinely needs one of these features, widening is a \
         deliberate decision: add the feature to that manifest AND to `FORBIDDEN` here, with the \
         reason.\n\n--- resolved tokio rows ---\n{}",
        offenders,
        rows.iter()
            .map(|f| f.join(","))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// The other half: the narrowing must not have gone so far that `tokio::sync` itself is gone. A
/// guard that only asserts absences is satisfied by deleting the dependency outright, and
/// `cyrup_core::event_stream` would stop compiling long before anyone read this file.
#[test]
fn cyrup_core_still_resolves_tokio_sync() {
    let Some(tree) = feature_graph_or_fail() else {
        return;
    };
    let rows = tokio_feature_rows(&tree);
    assert!(
        rows.iter().any(|feats| feats.contains(&"sync")),
        "`cyrup-core` no longer resolves tokio's `sync` feature on a normal edge — \
         `event_stream.rs`'s `tokio::sync::mpsc` has lost its feature, or the dependency was \
         dropped. The narrowing is `features = [\"sync\"]`, not \"no tokio\".\n\n--- tree ---\n{tree}"
    );
}

/// The extractor's contract, asserted against fixed input rather than the live graph — the drift
/// this protects against would otherwise be invisible, since a broken extractor and a satisfied
/// invariant look identical from the assertion's side.
#[test]
fn the_extractor_reads_tokio_rows_and_only_tokio_rows() {
    let sample = "\
cyrup-core v0.0.0 (/w/crates/cyrup-core)|
├── tokio v1.52.3|default,sync,time
├── tokio-stream v0.1.18|default,time
│   └── tokio v1.52.3|default,sync,time (*)
└── tokio-util v0.7.18|default
";
    let rows = tokio_feature_rows(sample);
    assert_eq!(rows.len(), 2, "both tokio rows, and only those: {rows:?}");
    assert!(
        rows.iter().all(|f| f == &["default", "sync", "time"]),
        "features parsed verbatim, with the `(*)` back-reference marker stripped: {rows:?}"
    );
    assert!(
        tokio_feature_rows("├── tokio-stream v0.1.18|default,time,fs").is_empty(),
        "`tokio-stream` is a different package — its features must never be read as tokio's"
    );
    assert!(
        tokio_feature_rows("├── tokio v1.52.3|")
            .first()
            .is_some_and(|f| f.is_empty()),
        "a featureless row parses to an empty list, never to a phantom feature"
    );
}

/// The opt-out predicate's contract, asserted directly rather than against ambient state (a test
/// that merely read the live env would break the very escape hatch it documents). Widen `skip_from`
/// to "the var exists" and the guards above go quietly green forever; the load-bearing case is the
/// FIRST one — an unset var must never skip.
#[test]
fn the_skip_is_opt_in_only() {
    assert!(
        !skip_from(None),
        "unset must never skip — that is the defect this convention exists to prevent"
    );
    assert!(!skip_from(Some("")), "empty must never skip");
    assert!(!skip_from(Some("0")), "an explicit 0 must never skip");
    assert!(skip_from(Some("1")), "an affirmative value opts out");
    assert!(
        skip_from(Some("true")),
        "any non-empty non-zero value opts out"
    );
}

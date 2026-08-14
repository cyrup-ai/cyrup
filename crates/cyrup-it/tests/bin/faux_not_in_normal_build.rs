//! PROV-052 — the scripted `faux` test double must not be **selectable** from the shipped binary,
//! and must not even be **compiled** into the default-member build it is produced from.
//!
//! ## What actually gates this (read before "fixing" the manifests)
//!
//! The load-bearing gate is a Rust one: the only path from a CLI flag to the double is
//! `select_provider`'s `Some("faux")` arm in `crates/cyrup/src/provider.rs`, and that arm sits
//! behind `#[cfg(feature = "faux")]` keyed to the **`cyrup` package's own** `faux` feature, which
//! is enabled solely by cyrup's self-dev-dependency (`cyrup = { path = ".", features = ["faux"] }`)
//! — a dev edge that `cargo build`, `cargo build --release` and `cargo install` never resolve.
//!
//! This file pins the *second*, weaker property: the double is not even compiled into the graph the
//! shipped binary comes from. That is a **Cargo feature-graph** invariant, not a Rust one, so no
//! `#[cfg]`-based unit test can express it: whether `feature = "faux"` is on inside a given
//! compilation is decided by the resolver before any Rust is parsed, and the resolver is what
//! regressed. Cargo features are **additive and unified per package across everything built in one
//! invocation**, so a single `features = ["faux"]` edge in ANY crate's `[dependencies]` turns the
//! feature on for EVERY consumer of `cyrup-provider` in that build — which is exactly how the
//! shipped binary came to resolve a bare `cyrup -p hi` to a scripted test double and answer
//! `No more faux responses queued` (see `docs/gap-analysis/01-cyrup-core-and-provider.md`, PROV-052,
//! and `docs/gap-analysis/REPRO-LOG.md`).
//!
//! **Scope, stated exactly so nobody maintains the wrong invariant.** The assertion below is about
//! `-p cyrup`, the default-member graph. It is *not* true of `--workspace`: `cyrup-test-support`
//! enables `cyrup-provider/faux` on a NORMAL edge (its `src/` IS the scripted harness), so
//! `cargo tree -e features --workspace --edges normal` does report `cyrup-provider feature "faux"`
//! and `cargo build --workspace` compiles the module into the `cyrup-provider` rlib. That is
//! harmless and expected — present is not reachable — because of the `#[cfg]` gate above.
//!
//! The guard is the same instrument that found the defect. It was **RED before the fix**
//! (`cargo tree -p cyrup -e features --edges normal` printed `cyrup-provider feature "faux"`) and is
//! **GREEN after**. `--edges normal` is the load-bearing flag: it excludes `[dev-dependencies]` and
//! `[build-dependencies]`, so it reports precisely the graph a `cargo build`, `cargo build
//! --release` or `cargo install` produces.
//!
//! Upstream referent: pi's own scripted provider (`packages/ai/src/providers/faux.ts` @v0.83.0) is
//! exported from the `pi-ai` package for tests only. It is absent from
//! `packages/ai/src/providers/all.ts`, it is not a member of `KnownProvider`, and
//! `git grep faux v0.83.0 -- packages/coding-agent/src/` matches zero files — nothing a pi user can
//! type reaches it. cyrup must be at least as strict.

// Test target: the workspace no-panic lints (`unwrap_used`/`expect_used`/`panic`) are policy for
// production code. `assert!` is the point of this file, and a manifest path that cannot resolve its
// own workspace root is an environment bug the test should surface loudly.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::process::Command;

/// The **opt-in** escape hatch for an environment that genuinely cannot run cargo (an air-gapped
/// packager, a vendored source tarball with no registry index).
///
/// It is opt-in on purpose. The previous version of this file returned early — i.e. **PASSED** —
/// whenever `cargo tree` exited non-zero for ANY reason, a stale `Cargo.lock` included. A guard that
/// goes green when its instrument breaks is not a guard, and the single likeliest cause of a
/// non-zero `--offline --locked` here is a manifest edit that outran the lockfile: precisely the
/// class of change this file exists to police.
const SKIP_VAR: &str = "CYRUP_SKIP_CARGO_GRAPH_TESTS";

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
///
/// MIGRATION: unchanged arithmetic. `crates/cyrup-it` sits at exactly the same depth as the
/// `crates/cyrup-provider` this file was drained from, so `.nth(2)` still names the workspace root;
/// only the message moved. `CARGO_MANIFEST_DIR` is the *package's* dir, not the test file's, so the
/// extra `tests/bin/` nesting is invisible here.
fn workspace_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/cyrup-it is two levels below the workspace root")
        .to_path_buf()
}

/// Run `cargo tree …` in the workspace root. `Err` carries a diagnostic good enough to act on:
/// the exact argv, the exit status, and cargo's own stderr.
fn cargo_tree(extra: &[&str]) -> Result<String, String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let base = [
        "tree",
        "-p",
        "cyrup",
        "-e",
        "features",
        "--offline",
        "--locked",
    ];
    let mut cmd = Command::new(&cargo);
    cmd.current_dir(workspace_root()).args(base).args(extra);
    // `tree` only resolves, it does not build, so this takes no target-dir build lock — which is
    // why it needs no `--target-dir` of its own and cannot deadlock against the outer cargo that
    // is running this suite, unlike the nested `cargo build`s `build.rs` replaced. (`--offline`
    // additionally keeps it off the package-cache download lock.) It is the ONE nested cargo in
    // this crate that is deliberately left in a test body rather than hoisted into `build.rs`:
    // resolving the graph IS the assertion, so it must run at test time, against the live
    // manifests, not once at build time.
    let out = cmd
        .output()
        .map_err(|e| format!("could not spawn `{cargo}`: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "`{cargo} {} {}` exited with {}\n--- cargo stderr ---\n{}",
            base.join(" "),
            extra.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim_end()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Resolve the feature graph, or **fail**. Returns `None` only when the skip was explicitly
/// requested via [`SKIP_VAR`]; every other cargo error panics, because an unavailable instrument
/// must not be reported as a satisfied invariant.
fn feature_graph_or_fail(extra: &[&str]) -> Option<String> {
    match cargo_tree(extra) {
        Ok(out) => Some(out),
        Err(why) if skip_requested() => {
            eprintln!(
                "{SKIP_VAR} is set — skipping the PROV-052 Cargo-graph guard BY REQUEST. \
                 The invariant is therefore UNCHECKED in this run.\n{why}"
            );
            None
        }
        Err(why) => panic!(
            "PROV-052 GUARD COULD NOT RUN — failing rather than passing silently, because a green \
             result here would be indistinguishable from a satisfied invariant.\n\n{why}\n\n\
             Most likely causes, in order: (1) `Cargo.lock` is stale relative to the manifests — \
             run `cargo check --workspace` (or `cargo update -w`) and re-run; (2) no vendored \
             registry for `--offline`; (3) no cargo on PATH and `CARGO` unset.\n\n\
             If this environment truly cannot run cargo, opt out EXPLICITLY with `{SKIP_VAR}=1` and \
             check the invariant by hand:\n  \
             cargo tree -p cyrup -e features --edges normal | grep faux   # must print NOTHING"
        ),
    }
}

/// The invariant. `--edges normal` is the graph the shipped binary is built from.
#[test]
fn faux_is_absent_from_the_normal_dependency_graph_of_the_binary() {
    let Some(normal) = feature_graph_or_fail(&["--edges", "normal"]) else {
        return;
    };

    let offenders: Vec<&str> = normal
        .lines()
        .filter(|l| l.contains("feature \"faux\""))
        .collect();

    assert!(
        offenders.is_empty(),
        "PROV-052 REGRESSION — the scripted faux test double is enabled on a NORMAL dependency \
         edge of `-p cyrup`, so it is compiled into the shipped `cyrup` binary.\n\nOffending lines \
         from `cargo tree -p cyrup -e features --edges normal`:\n{}\n\nFix: move the \
         `features = [\"faux\"]` edge that causes this into a `[dev-dependencies]` section (Cargo \
         unifies features additively, so ONE such edge anywhere in the graph is enough), and keep \
         `crates/cyrup-test-support` out of the workspace `default-members`.",
        offenders.join("\n")
    );
}

/// The other half: the feature must still be reachable for tests, or the guard above could be
/// satisfied by deleting the double outright and silently dropping the suite's offline oracle.
/// Nine crates drive their tests through it.
#[test]
fn faux_is_still_enabled_on_a_dev_edge() {
    let Some(all) = feature_graph_or_fail(&[]) else {
        return;
    };
    assert!(
        all.contains("feature \"faux\""),
        "`cargo tree -p cyrup -e features` (dev edges included) no longer reports the faux \
         feature — the test double is unreachable and the offline suite has lost its oracle.\n{all}"
    );
}

/// The opt-out predicate's contract, asserted directly rather than against ambient state (a test
/// that merely read the live env would break the very escape hatch it documents).
///
/// This is the piece that could silently regress back into the original defect: widen `skip_from`
/// to "the var exists" or "always true" and both guards above go quietly green forever. The
/// load-bearing case is the FIRST one — an unset var must never skip.
#[test]
fn the_skip_is_opt_in_only() {
    assert!(
        !skip_from(None),
        "unset must never skip — this is the defect"
    );
    assert!(!skip_from(Some("")), "empty must never skip");
    assert!(!skip_from(Some("0")), "an explicit 0 must never skip");
    assert!(skip_from(Some("1")), "an affirmative value opts out");
    assert!(
        skip_from(Some("true")),
        "any non-empty non-zero value opts out"
    );
}

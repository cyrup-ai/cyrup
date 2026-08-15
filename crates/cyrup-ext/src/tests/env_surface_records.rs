//! The two env-var-surface records area 05 routes into this crate: `CFG-071` (`XDG_CACHE_HOME`)
//! and `CFG-075` (`CYRUP_EXT_ABI_FINGERPRINT`).
//!
//! Both rows propose **no behavioural work** — their whole content is a fact about the environment
//! surface that a later fidelity pass must not have to re-derive. So the deliverable is the
//! annotation, and the assertion is on the annotation. These are not vacuous: each one was RED
//! before this pass, because the doc text it requires did not exist.

#![allow(clippy::expect_used)]

/// CFG-071 — `XDG_CACHE_HOME` is a FALSE NAME-MATCH in both directions at once.
///
/// cyrup reads it in `ArtifactCache::default_location` to site the Tier-1 WASM build cache; pi
/// reads it at `packages/coding-agent/src/extensions/llama/huggingface.ts:53` (inside
/// `findHuggingFaceToken`, declared `:46`) to find `$XDG_CACHE_HOME/huggingface/token` — re-derived
/// at v0.83.0 and v0.84.1 this pass, byte-identical at both. A name-only parity diff therefore
/// scores the variable as PARITY while cyrup has both an unexplained extra read and a missing one.
///
/// RED before this pass: `default_location`'s entire doc was the one line "The default cache
/// location (`$XDG_CACHE_HOME` or `~/.cache`)." — no `CYRUP-DELTA` marker, no mention of pi's
/// unrelated use, and no pointer to the item that owns pi's half. Every assertion below failed.
///
/// The `EXT-027` assertion is the one that matters most: it is what stops a future sweep from
/// closing CFG-071 in one direction and reading the other as done.
#[test]
fn cfg071_the_build_cache_read_records_both_directions_of_the_name_match() {
    let src = include_str!("../build/cache.rs");
    let at = src
        .find("pub fn default_location()")
        .expect("`ArtifactCache::default_location` is where cyrup reads `XDG_CACHE_HOME`");
    let doc = &src[..at];
    let doc = &doc[doc
        .rfind("[CYRUP-DELTA")
        .expect("the cyrup-original read carries a delta annotation")..];

    for needle in [
        // cyrup's own read, and that it is an original with no upstream counterpart.
        "XDG_CACHE_HOME",
        // pi's unrelated read of the SAME name, cited by file and symbol.
        "huggingface.ts:53",
        "findHuggingFaceToken",
        // the item that owns pi's half, so neither direction closes the other.
        "EXT-027",
        // the adjacent grep trap DRIFT-032 warns about.
        "HF_TOKEN",
    ] {
        assert!(
            doc.contains(needle),
            "the CFG-071 record must name `{needle}`; got: {doc}"
        );
    }

    // Guard against the record being "fixed" by deleting the read it documents.
    assert!(
        src.contains(r#"std::env::var_os("XDG_CACHE_HOME")"#),
        "CFG-071 is a RECORD: the cyrup-original read stays, it is only documented"
    );
}

/// CFG-075 — `CYRUP_EXT_ABI_FINGERPRINT` is the env surface's ONLY build-time dependency.
///
/// It is an `env!` (compile-time substitution), not a `std::env::var` (runtime lookup with a
/// fallback), so a missing value is a compile error and there is no branch to reach. The row asks
/// for exactly one thing: that the build script supplying it be documented *next to the consumer*,
/// so the dependency is discoverable from `build/mod.rs` without grepping for a `build.rs`.
///
/// RED before this pass: `ABI_FINGERPRINT`'s doc was two lines ("BLAKE3 of every ABI source file,
/// computed by `build.rs` at HOST compile time (EXT-028). `unknown` only if the build script could
/// not resolve the workspace `crates/` directory.") — it named neither the `cargo:rustc-env` key
/// nor the emitting line, and said nothing about the compile-vs-runtime distinction that is the
/// entire content of the item. The `cargo:rustc-env`, `env!` and `--no-default-features`
/// assertions below all failed.
#[test]
fn cfg075_the_build_time_env_dependency_is_documented_at_its_consumer() {
    let src = include_str!("../build/mod.rs");
    let at = src
        .find("pub const ABI_FINGERPRINT")
        .expect("the `env!` consumer");
    let doc = &src[..at];
    let doc = &doc[doc
        .rfind("/// BLAKE3 of every ABI source file")
        .expect("the const's own doc block")..];

    for needle in [
        // the variable, and the fact that it is compile-time.
        "CYRUP_EXT_ABI_FINGERPRINT",
        "env!",
        "compile",
        // the supplier, by file and by the exact cargo directive it emits.
        "build.rs",
        "cargo:rustc-env",
        // the sentinel branch, so `unknown` is not read later as a bug.
        "unknown",
        // that neither feature arm removes the dependency.
        "--no-default-features",
    ] {
        assert!(
            doc.contains(needle),
            "the CFG-075 record must name `{needle}`; got: {doc}"
        );
    }
}

/// The supplier side of CFG-075, asserted against the build script itself so the two halves cannot
/// drift: the doc above claims `build.rs` emits the key, and this pins that it still does.
///
/// **Coverage, not proof of a fix.** This one could not go red before the pass — `build.rs` already
/// emitted both lines. It exists so that moving or renaming the emission breaks a test in the same
/// crate as the `env!` rather than only at some downstream build.
#[test]
fn cfg075_the_build_script_still_emits_the_key_and_its_sentinel() {
    let build_rs = include_str!("../../build.rs");
    assert!(
        build_rs.contains("cargo:rustc-env=CYRUP_EXT_ABI_FINGERPRINT={}"),
        "build.rs must still emit the fingerprint the `env!` consumes"
    );
    assert!(
        build_rs.contains("cargo:rustc-env=CYRUP_EXT_ABI_FINGERPRINT=unknown"),
        "the documented `unknown` sentinel is the no-manifest-dir branch, not an error path"
    );
    // The value actually reached the binary: `env!` guarantees non-empty at compile time, but the
    // sentinel would still be a silently degraded cache key, so name it explicitly.
    assert!(
        !crate::build::ABI_FINGERPRINT.is_empty(),
        "`env!` substituted an empty fingerprint"
    );
}

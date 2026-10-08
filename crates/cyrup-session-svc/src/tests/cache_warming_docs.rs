//! The user guide's cache-warming copy, pinned against the code it describes (SEAM-131).
//!
//! `docs/guide/guides/sessions.md` and `docs/guide/reference/settings.md` quote the strings a user
//! actually sees — the `Inactive (…)` reasons, the `Cache warmed: $…` transcript line, the
//! `cacheWarming` values and its default. None of that is reachable from a compiler error: rename
//! a stop reason or change the default and the guide goes quietly wrong, which is worse than no
//! documentation, because a reader trusts it. These tests are the forcing function.
//!
//! They are deliberately shaped the way `cyrup-ext`'s `wit_world_sync` is: read the real file from
//! `CARGO_MANIFEST_DIR`, and fail naming the drift.

// Tests may unwrap/expect/panic (workspace no-panic policy, `Cargo.toml:143-151`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;

use cyrup_config::CacheWarmingMode;

use crate::cache_warmer::{
    CacheWarmingAction, CacheWarmingDecision, CacheWarmingPhase, CacheWarmingState,
    CacheWarmingStatus, format_cache_warming_status, format_cache_warming_usage,
};

fn repo_doc(relative: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("the guide must be readable at {}: {e}", path.display()))
}

/// The warmer's own source, which is where every stop reason is written.
const WARMER_SOURCE: &str = include_str!("../cache_warmer.rs");

/// A status for a run that stopped before it ever had economics to report — the shape that makes
/// `format_cache_warming_status` take its `Inactive (reason)` branch.
fn stopped_without_a_decision(reason: &str) -> CacheWarmingStatus {
    CacheWarmingStatus {
        state: CacheWarmingState::Inactive,
        reason: Some(reason.to_string()),
        next_warm_in: None,
        decision: None,
        extension_override: false,
    }
}

/// The reason column of the guide's table: every leading `| \`…\` |` cell in the block that
/// follows the sentence introducing it.
fn documented_reasons(guide: &str) -> Vec<String> {
    let table = guide
        .split_once("| Reason | What happened |")
        .expect("the sessions guide must still document the warming reasons in a table")
        .1;
    table
        .lines()
        // `split_once` leaves the tail of the header line, so walk forward to the table body
        // before trusting `starts_with('|')` as the end-of-table test.
        .skip_while(|line| !line.starts_with('|'))
        .take_while(|line| line.starts_with('|'))
        .filter_map(|line| {
            let cell = line.split('|').nth(1)?.trim();
            let literal = cell.strip_prefix('`')?.strip_suffix('`')?;
            Some(literal.to_string())
        })
        .collect()
}

/// Every reason the guide prints is a reason the warmer really produces, spelled the same way.
///
/// The source check is the half that catches a rename: the render check alone would keep passing
/// for a reason no code path can reach any more.
#[test]
fn seam131_every_documented_stop_reason_is_one_the_warmer_really_produces() {
    let reasons = documented_reasons(&repo_doc("docs/guide/guides/sessions.md"));
    assert!(
        reasons.len() >= 10,
        "the guide's reason table lost rows: {reasons:?}"
    );
    for reason in &reasons {
        assert!(
            WARMER_SOURCE.contains(&format!("\"{reason}\"")),
            "the guide documents the reason {reason:?}, which no longer appears in \
             cache_warmer.rs — either the reason was renamed or the guide is now lying"
        );
        assert_eq!(
            format_cache_warming_status(&stopped_without_a_decision(reason)),
            format!("Inactive ({reason})"),
            "the guide shows this reason inside `Inactive (…)`"
        );
    }
}

/// The two reasons the guide calls out in prose rather than in the table, and which it says mean
/// different things: one is a warmer that has nothing to schedule against, the other is a session
/// with no warmer at all. The second is NOT a warmer string — it is the `/session` literal for a
/// `None` status — so it must NOT be in the warmer's source, or the distinction the guide draws
/// has collapsed.
#[test]
fn seam131_the_guide_keeps_the_no_lifetime_and_no_warmer_cases_apart() {
    let guide = repo_doc("docs/guide/guides/sessions.md");
    for quoted in [
        "Inactive (cache lifetime unavailable)",
        "Inactive (cache warming unavailable)",
    ] {
        assert!(
            guide.contains(quoted),
            "the guide must still name {quoted:?}"
        );
    }
    assert_eq!(
        format_cache_warming_status(&stopped_without_a_decision("cache lifetime unavailable")),
        "Inactive (cache lifetime unavailable)"
    );
    assert!(
        !WARMER_SOURCE.contains("\"cache warming unavailable\""),
        "`cache warming unavailable` is the /session literal for a session with NO warmer; if the \
         warmer itself starts producing it, the guide's distinction is wrong"
    );
}

/// The guide's worked `Stopped (…)` example: a run the economics turned down still reports the
/// arithmetic, not a reason — which is the sentence in the guide that this pins.
#[test]
fn seam131_the_guides_stopped_example_is_really_what_renders() {
    let turned_down = CacheWarmingStatus {
        state: CacheWarmingState::Inactive,
        // The reason the warmer passes alongside the decision at this stop. It is deliberately
        // NOT what renders, and the guide says so.
        reason: Some("expected savings below threshold".to_string()),
        next_warm_in: None,
        decision: Some(CacheWarmingDecision {
            phase: CacheWarmingPhase::Idle,
            warm_cost: 0.001,
            miss_cost: 0.03,
            continuation_probability: 0.15,
            expected_savings: 0.004,
            economics_available: true,
            action: CacheWarmingAction::Stop,
        }),
        extension_override: false,
    };
    let rendered = format_cache_warming_status(&turned_down);
    assert!(
        rendered.starts_with("Stopped (") && rendered.ends_with("-> stop)"),
        "unexpected shape: {rendered}"
    );
    let fragment = "expected savings $0.004 < $0.050 -> stop";
    assert!(
        rendered.contains(fragment),
        "the guide quotes {fragment:?}, got {rendered}"
    );
    assert!(
        repo_doc("docs/guide/guides/sessions.md").contains(fragment),
        "the guide no longer quotes the fragment this test pins"
    );
}

/// The transcript one-liner the guide shows, and the `/session` column header words it promises.
#[test]
fn seam131_the_guides_transcript_line_and_session_rows_are_the_real_ones() {
    let guide = repo_doc("docs/guide/guides/sessions.md");
    let warmed = format_cache_warming_usage(None, 0.000_9);
    assert_eq!(warmed, "Cache warmed: $0.0009");
    assert!(
        guide.contains(&warmed),
        "the guide must show the line the formatter really prints, got {warmed}"
    );
    for row in ["warming mode", "warming", "miss penalty", "refresh cost"] {
        assert!(
            guide.contains(row),
            "the guide must name the /session row {row:?}"
        );
    }
}

/// The settings reference's `cacheWarming` row: the three values and the default are the ones
/// `CacheWarmingMode` really has.
#[test]
fn cfg093_the_settings_reference_states_the_real_values_and_default() {
    let settings = repo_doc("docs/guide/reference/settings.md");
    let row = settings
        .lines()
        .find(|line| line.starts_with("| `cacheWarming` |"))
        .expect("settings.md must document the cacheWarming key");
    for mode in [
        CacheWarmingMode::Off,
        CacheWarmingMode::Streaming,
        CacheWarmingMode::Idle,
    ] {
        let spelling = mode.as_str();
        assert!(
            row.contains(&format!("`{spelling}`")),
            "the cacheWarming row omits the value {spelling:?}: {row}"
        );
        assert_eq!(
            CacheWarmingMode::parse(spelling),
            Some(mode),
            "the documented spelling must round-trip"
        );
    }
    assert!(
        row.contains(&format!("`\"{}\"`", CacheWarmingMode::Streaming.as_str())),
        "the row must state the real default, {:?}: {row}",
        CacheWarmingMode::Streaming.as_str()
    );
    assert!(
        row.contains("Global scope only"),
        "the row must keep saying the key is global-only: {row}"
    );
    assert!(
        settings.contains("## cacheWarming"),
        "the row's `See below` must still have a section to point at"
    );
}

/// The CONVERSE of [`seam131_every_documented_stop_reason_is_one_the_warmer_really_produces`]:
/// every reason the warmer can produce is either documented or explicitly accounted for here.
///
/// That test alone is one-directional, and the direction it does not cover is the one a user
/// feels. A reachable reason the guide does not carry renders as an `Inactive (…)` row nobody can
/// look up, and the existing test keeps passing throughout. That is not hypothetical: `"inactive"`
/// — upstream's session-TEARDOWN literal — used to leak out of
/// `ProviderSwap::maybe_start_warming` for every virtual-model selection and for any id missing
/// from the installed provider's catalog, printing the content-free `warming | Inactive
/// (inactive)`. The seam now answers `"cache lifetime unavailable"` there
/// (`provider_swap::tests::an_unwarmable_model_stops_with_a_documented_reason`), which is what
/// upstream's own `start` says for the same situation.
///
/// **How it reads the reasons.** Not by scanning `self.stop(` arguments: five of the fourteen are
/// passed as a BINDING (`mode_stop_reason`'s two arms, `validate_run`'s context clause,
/// `schedule`'s window `match`, `start`'s no-TTL `match`, `refresh`'s stop branch), so an
/// argument-shaped scan silently misses them — the first draft of this test did, and emptying the
/// economics allowlist left it green. It instead sweeps every REASON-SHAPED string literal in the
/// warmer outside comments (lowercase, letters/digits/spaces/hyphens only, so format strings,
/// serde attributes and `CamelCase` text drop out) and makes each one justify itself. The cost of
/// that breadth is [`NOT_A_REASON`]; the benefit is that a new reason cannot be added through any
/// call shape without landing here.
///
/// **Red-proved four ways**, each applied alone: (a) `"inactive"` removed from
/// `TEARDOWN_ONLY_REASONS` fails naming it — that is the pre-fix leak; (b) `WITH_ECONOMICS_REASONS`
/// emptied fails on all three; (c) a `self.stop("not a real reason", None, false)` added to the
/// warmer fails naming the string; (d) `documented_reasons` pointed at an empty table fails on
/// every row.
#[test]
fn seam131_every_reason_a_user_can_see_is_documented() {
    /// Reasons that stop a run while CARRYING a decision. `format_cache_warming_status` renders
    /// those as `Stopped (<economics>)` and never prints the reason at all
    /// (`cache-warmer.ts:441-447` @v1.0.4, pinned by
    /// `seam131_the_guides_stopped_example_is_really_what_renders`), so the guide documents the
    /// arithmetic in prose rather than listing these three in its reason table.
    const WITH_ECONOMICS_REASONS: &[&str] = &[
        "stopped by extension",
        "expected savings below threshold",
        "cache economics unavailable",
    ];
    /// `CacheWarmer::cancel`'s reason, and nothing else produces it. Its only non-test caller is
    /// the session teardown in `session/lifecycle.rs` (pi's `dispose()`,
    /// `agent-session.ts:1395-1397` @v1.0.4), so by the time it is set there is no `/session` left
    /// to render it. The assertions at the bottom are what keep that argument true.
    const TEARDOWN_ONLY_REASONS: &[&str] = &["inactive"];
    /// Reason-SHAPED literals that are not reasons. Each is here with its own justification, and
    /// the sweep below checks every one is still present — a stale exemption would hide the next
    /// real leak.
    const NOT_A_REASON: &[(&str, &str)] = &[
        (
            "unknown reason",
            "upstream's `status.reason ?? \"unknown reason\"` fallback (`cache-warmer.ts:437` \
             @v1.0.4), ported verbatim. Unreachable: every stop sets a reason and the \
             constructed state sets `waiting for first request`, so it is parity scaffolding, \
             not a reason the warmer produces.",
        ),
        (
            "extension override",
            "the NOTE on the persisted `usage` entry (`cache-warmer.ts:344`), which renders \
             inside the transcript line `Cache warmed (extension override): $…` — pinned by \
             `seam131_the_guides_transcript_line_and_session_rows_are_the_real_ones`.",
        ),
        (
            "warm",
            "`CacheWarmingAction::Warm`'s rendering in the `-> action` tail of a decision-bearing \
             status, not a stop reason.",
        ),
        ("stop", "`CacheWarmingAction::Stop`'s rendering, same tail."),
        (
            "lowercase",
            "a `#[serde(rename_all = …)]` attribute value on the action/phase enums.",
        ),
        (
            "cache-warm usage entry failed to serialize",
            "a `tracing` log message on the best-effort persist path; never rendered to a user.",
        ),
        (
            "cache-warm usage entry failed to append",
            "the same, for the append half.",
        ),
    ];

    let guide = repo_doc("docs/guide/guides/sessions.md");
    let documented = documented_reasons(&guide);
    assert!(
        documented.len() >= 10,
        "the guide's reason table lost rows: {documented:?}"
    );

    // Every reason-shaped string literal the warmer spells, outside comments.
    let mut produced: Vec<String> = Vec::new();
    for line in WARMER_SOURCE.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") {
            continue;
        }
        let mut rest = line;
        while let Some(open) = rest.find('"') {
            let after = &rest[open + 1..];
            let Some(close) = after.find('"') else { break };
            let literal = &after[..close];
            rest = &after[close + 1..];
            let shaped = !literal.is_empty()
                && literal
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == ' ' || c == '-')
                && literal.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit());
            if shaped && !produced.iter().any(|p| p == literal) {
                produced.push(literal.to_string());
            }
        }
    }
    // The sweep must be seeing the whole reason set, not a subset — the failure mode of the first
    // draft. Spot-check one reason from each call SHAPE: a literal argument, a `match` arm and a
    // binding passed through `refresh`.
    for anchor in [
        "cache warming disabled",
        "one-hour safety limit reached",
        "expected savings below threshold",
    ] {
        assert!(
            produced.iter().any(|p| p == anchor),
            "the sweep no longer sees {anchor:?}, so it has stopped measuring the call shape that \
             produces it: {produced:?}"
        );
    }

    let exempt: Vec<&str> = WITH_ECONOMICS_REASONS
        .iter()
        .copied()
        .chain(TEARDOWN_ONLY_REASONS.iter().copied())
        .chain(NOT_A_REASON.iter().map(|(literal, _)| *literal))
        .collect();
    for reason in &produced {
        assert!(
            documented.iter().any(|d| d == reason) || exempt.contains(&reason.as_str()),
            "the warmer spells the reason-shaped literal {reason:?}, which the guide's reason \
             table does not carry and which is on no allowlist. If a user can see \
             `Inactive ({reason})`, document it in docs/guide/guides/sessions.md; if not, add it \
             to the allowlist it belongs on WITH the reason it is exempt."
        );
    }

    // The allowlists must stay honest in the other direction: an entry nothing produces any more
    // is a stale exemption.
    for reason in exempt {
        assert!(
            produced.iter().any(|p| p == reason),
            "{reason:?} is exempted from the guide but the warmer no longer spells it — drop the \
             exemption"
        );
    }

    // The specific regression. `"inactive"` is exempt only because `cancel()` is its single
    // producer and the session teardown is its single caller, so both halves are pinned.
    assert_eq!(
        WARMER_SOURCE.matches("self.stop(\"inactive\"").count(),
        1,
        "`\"inactive\"` is exempted ONLY because `cancel()` is its single producer; a second \
         producer breaks that argument"
    );
    let swap_source = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/provider_swap.rs"),
    )
    .expect("provider_swap.rs must be readable");
    // Scoped to the seam's BODY, not the file: the test beside it names the regression in its own
    // doc comment, so a whole-file grep would match that.
    let seam = swap_source
        .split_once("fn maybe_start_warming")
        .expect("the request seam must still exist")
        .1;
    let seam_body = seam.split_once("\n    /// ").map_or(seam, |(body, _)| body);
    assert!(
        !seam_body.contains("warmer.cancel()"),
        "the request seam must not reach for `cancel()` — that is how `Inactive (inactive)` \
         became user-visible for every virtual-model selection"
    );
    assert!(
        seam_body.contains("warmer.stop_unwarmable_model()"),
        "the seam must stop an unresolvable model with upstream's own reason for it"
    );
}

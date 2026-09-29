//! `xtask` — the repo's own tooling. Three commands, all run by hand (there is no CI here; see
//! README "Build"):
//!
//! * `gen-catalogs` — regenerate `crates/cyrup-provider/src/providers/catalog/*.json` and
//!   `catalog_manifest.json` from pi's published catalogs, plus one file from a pinned pi revision
//!   (PROV-018 / PROV-060 / PROV-071). Documented below.
//! * `feature-matrix` — type-check the feature combinations `cargo check --workspace
//!   --all-targets` does not reach, and RUN the two suites the everyday gate skips (`cyrup-ext`
//!   with `wasm-host` off, and the gated `cyrup-it` seam suite). See [`features`] for the matrix
//!   and why each row is in it.
//! * `it` — run the gated `cyrup-it` seam suite alone, in a hermetic harness environment
//!   (`env_clear` + [`IT_ENV_ALLOWLIST`]): the supported invocation on a machine whose shell
//!   exports real provider credentials, guaranteed to spend zero tokens. See [`run_it`].
//!
//! # `gen-catalogs`
//!
//! # Where the rows come from, and why that changed twice
//!
//! PROV-018's original Fix said to run pi's `npm run generate-models`, because "the tree can no
//! longer simply be read". PROV-060 refuted that: pi gitignores `packages/ai/src/providers/data/`
//! (`pi/.gitignore:11`) only from `a9f6a3159` (`feat(ai): separate generated model data (#6765)`)
//! onward, and at its direct parent `b0c2a90e` every `packages/ai/src/providers/<p>.models.ts` is
//! still a full data literal. So the whole catalog was recoverable with `git show` plus [`tsdata`],
//! with no `npm install`, no generator run and no network, and this binary became that recipe.
//!
//! **PROV-071 is the second correction, and it is the one that matters now.** `b0c2a90e` was never
//! a chosen pin: it is the LAST revision that answers at all, and it has been the floor for every
//! provider since 2026-07-17. Measured at `v0.87.1`, 41 of 41 `*.models.ts` are re-exports and
//! `git ls-tree -r v0.87.1 packages/ai/src/providers/data` is empty, so `git show` recovers nothing
//! newer for ANY provider at ANY revision. XAI_1 unfroze one catalog by fetching
//! `https://pi.dev/api/models/providers/<id>` — the same endpoint the RUNTIME overlay reads — and
//! DRIFT-009 four more. [`LIVE_CATALOGS`] now carries all 38 provider catalogs; [`CATALOGS`] holds
//! the one file that is not a provider module (`openrouter-images`, PROV-065), for which no live
//! endpoint exists.
//!
//! The refresh that closed PROV-071 moved 536 rows in, 204 out and 845 field values, across 27
//! catalogs — two months of upstream data that the pinned path could not have reached.
//!
//! # The floor stays COMMITTED
//!
//! Live-fetched is not runtime-fetched. `providers/catalog/*.json` is `include_str!`-ed into the
//! binary and reviewed as a diff; `gen-catalogs` is run by hand by a maintainer (there is no CI
//! here, see README "Build"). The refresh-time path already exists and is
//! `cyrup-provider/src/remote_catalog.rs`, hitting the same endpoint per configured provider. A
//! second one here would duplicate it and still leave the committed floor frozen — and the floor is
//! exactly what an offline or first-run user prices against, who by definition has no network.
//!
//! # What it is NOT allowed to do
//!
//! Regeneration must be **total and accounted for**: it rewrites every catalog from one source, it
//! refuses to run if a module is missing, the only rows it may diverge from upstream on are the
//! ones listed in [`DELTAS`], and every divergence upstream has since ADOPTED is re-checked through
//! [`CONVERGED`] instead of being dropped. A generator with a silent skip is how catalog data goes
//! missing without a diff — and a generalization that routed the live path around [`DELTAS`] is how
//! twelve signed-off decisions would go missing the same way.
//!
//! # Usage
//!
//! ```text
//! cargo run -p xtask -- gen-catalogs [--pi <path>] [--rev <rev>] [--out <dir>] [--check] [--diff]
//! cargo run -p xtask -- gen-catalogs --roster <rev> [--pi <path>]
//! cargo run -p xtask -- feature-matrix [--fast]
//! ```
//!
//! * `--check` — generate in memory and compare byte-for-byte with what is on disk; exit 1 on any
//!   difference. This is the drift check. It goes red when pi republishes a catalog, which is the
//!   point: the committed floor is then behind upstream and somebody has to look. Offline, set
//!   `CYRUP_XTASK_SKIP_LIVE` and it checks the pinned file alone rather than reporting 38 fake
//!   differences.
//! * `--diff` — print a **structural** (model-level and field-level) diff of on-disk vs generated
//!   instead of writing anything. Whitespace-insensitive, so it reports only real data movement.
//!   Its comparison is a port of pi's own `scripts/diff-model-catalog.mjs` (see [`canonicalize`]),
//!   not an invention: key order alone is not a difference, and rows are walked over the sorted
//!   union of both sides' model ids.
//! * `--roster <rev>` — audit the provider **set** rather than the catalog **content**: list every
//!   `packages/ai/src/providers/*.models.ts` pi ships at `<rev>` and require each one to be either
//!   generated ([`CATALOGS`]) or ledgered as unported ([`UNPORTED`]). Generates and writes nothing.
//!   `--check` catches a row that moved inside a catalog cyrup already has; this catches a whole
//!   provider that appeared and has no catalog at all, which is the failure DRIFT-009 was filed for
//!   — `catalog_data.rs` claimed 31 catalogs while the tree shipped 35 and pi shipped 39, and no
//!   command could disagree with the prose. It also re-tests DRIFT-009's data block itself: if one
//!   of the four blocked modules is a data literal again at `<rev>`, the block has lifted and the
//!   audit fails saying so.

mod features;
mod live_catalog;
mod tsdata;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use tsdata::Val;

/// The pinned upstream revision. `b0c2a90e` is the LAST revision at which pi's `*.models.ts`
/// modules are data literals rather than two-line re-exports of gitignored JSON, and it is the
/// revision `catalog_manifest.json` names as the embedded catalogs' staleness floor (PROV-039).
const DEFAULT_REV: &str = "b0c2a90e";

/// `b0c2a90e`'s commit timestamp in UTC, which is the `generatedAt` the manifest must carry.
const DEFAULT_REV_TIMESTAMP: &str = "2026-07-17T09:00:03Z";

/// One embedded catalog and the upstream module its rows come from.
#[derive(Debug)]
struct CatalogSpec {
    /// `providers/catalog/<file>.json`.
    file: &'static str,
    /// Path under `packages/ai/src/` of the module that declares the rows.
    module: &'static str,
    /// `Some(provider)` when the module binds a `provider -> id -> Model` record and only one
    /// provider's sub-record belongs in this catalog (`image-models.generated.ts`, PROV-065).
    /// `None` when it binds a flat `id -> Model` record, which is every `<p>.models.ts`.
    images_provider: Option<&'static str>,
    /// `Some(rev)` when this catalog is read at its OWN pinned revision rather than `--rev`
    /// (PROV-089). Only a module that is still a data literal after `a9f6a3159` can carry one:
    /// `image-models.generated.ts` stayed tracked through `v0.87.1`, so its rows are recoverable
    /// from a revision 29 tags newer than [`DEFAULT_REV`], where every `*.models.ts` is not.
    rev: Option<&'static str>,
}

/// The revision `image-models.generated.ts` is read at (PROV-089): the ledger's pinned pi tag, and
/// the last one that tracks the file — post-tag `a328aa89a` deletes it and moves the rows into the
/// gitignored `providers/data/openrouter.json`, after which no revision can yield newer image rows.
const IMAGES_REV: &str = "v0.87.1";

/// The catalogs still recovered from a pinned revision with `git show` — **one**, and it is not
/// a provider module.
///
/// [`LIVE_CATALOGS`] below carries the other 38. PROV-071: pi's `*.models.ts` modules stopped
/// being data literals at `a9f6a3159` (`b0c2a90e`'s direct child) and every one of them has been a
/// re-export of gitignored, network-generated JSON ever since, so `git show` cannot recover
/// anything newer for ANY provider at ANY revision. Pinning to `b0c2a90e` was never a choice about
/// which revision to take; it was the last revision that answered at all, and it has been the
/// floor for every provider — not just the four DRIFT-009 named — since 2026-07-17.
///
/// `openrouter-images` is the exception because it is **not** a provider module: its rows are the
/// `openrouter` sub-record of `packages/ai/src/image-models.generated.ts`, which is still a data
/// literal in git, and `pi.dev/api/models/providers/openrouter-images` is a 404 — there is no live
/// endpoint to move it to (PROV-065). It is read at [`IMAGES_REV`] rather than `--rev` (PROV-089):
/// the newest revision that still tracks the file, so its rows are as fresh as git can make them.
const CATALOGS: &[CatalogSpec] = &[CatalogSpec {
    file: "openrouter-images",
    module: "image-models.generated.ts",
    images_provider: Some("openrouter"),
    rev: Some(IMAGES_REV),
}];

/// Every catalog whose rows are fetched LIVE, because the pinned-revision path cannot reach them.
///
/// # This is now the whole provider roster, and that is PROV-071's closure
///
/// [`CATALOGS`] above is `git show` against `DEFAULT_REV`. For a provider module that mechanism is
/// permanently dead, and it is dead for ALL of them for one reason: `<p>.models.ts` became a
/// two-line re-export of gitignored data at `a9f6a3159` — `b0c2a90e`'s direct child — so the
/// newest rows any revision can yield were already stale on the day the pin was taken. Measured at
/// `v0.87.1`: 41 of 41 `packages/ai/src/providers/*.models.ts` are re-exports and
/// `git ls-tree -r v0.87.1 packages/ai/src/providers/data` is empty.
///
/// XAI_1 unfroze one catalog this way and DRIFT-009 four more. PROV-071's whole content was that
/// the remaining 33 were frozen for exactly the same reason and were being tracked rather than
/// fixed; they are here now. pi publishes the shaped rows at the URL each spec derives — the same
/// endpoint the RUNTIME overlay reads (`cyrup-provider/src/remote_catalog.rs`) — so the embedded
/// floor and the runtime overlay come from one source and can no longer disagree.
///
/// # What this does NOT change
///
/// The floor is still a set of files committed to this repo, generated by a maintainer running
/// `gen-catalogs` and reviewed as a diff. It is not fetched at build time and not fetched at
/// runtime by this crate: `include_str!` needs a file, and the offline/first-run user this floor
/// exists for has no network by definition. What changed is only WHERE a refresh reads from —
/// `pi.dev` instead of a revision that cannot answer — and therefore that a refresh is now
/// possible at all. The refresh-time path already exists and is `remote_catalog.rs`; a second one
/// here would duplicate it and still leave the floor frozen.
///
/// # DELTAS and CONVERGED both run over these rows
///
/// A live catalog is not exempt from the signed-off divergences. [`generate_all`] applies
/// [`DELTAS`] and checks [`CONVERGED`] for live rows exactly as it does for pinned ones, which is
/// what turned the twelve `b0c2a90e`-era exceptions from silently-dropped into measured — see
/// [`CONVERGED`].
const LIVE_CATALOGS: &[live_catalog::LiveCatalogSpec] = &[
    // PROV-071 — the 33 provider modules that used to be `git show` against `b0c2a90e`.
    live("amazon-bedrock", "PROV-071"),
    live("ant-ling", "PROV-071"),
    live("anthropic", "PROV-071"),
    live("azure-openai-responses", "PROV-071"),
    live("cerebras", "PROV-071"),
    live("cloudflare-ai-gateway", "PROV-071"),
    live("cloudflare-workers-ai", "PROV-071"),
    live("deepseek", "PROV-071"),
    live("fireworks", "PROV-071"),
    live("github-copilot", "PROV-071"),
    live("google", "PROV-071"),
    live("google-vertex", "PROV-071"),
    live("groq", "PROV-071"),
    live("huggingface", "PROV-071"),
    live("kimi-coding", "PROV-071"),
    live("minimax", "PROV-071"),
    live("minimax-cn", "PROV-071"),
    live("mistral", "PROV-071"),
    live("moonshotai", "PROV-071"),
    live("moonshotai-cn", "PROV-071"),
    live("nvidia", "PROV-071"),
    live("openai", "PROV-071"),
    live("openai-codex", "PROV-071"),
    live("opencode", "PROV-071"),
    live("opencode-go", "PROV-071"),
    live("openrouter", "PROV-071"),
    live("vercel-ai-gateway", "PROV-071"),
    live("xiaomi", "PROV-071"),
    live("xiaomi-token-plan-ams", "PROV-071"),
    live("xiaomi-token-plan-cn", "PROV-071"),
    live("xiaomi-token-plan-sgp", "PROV-071"),
    live("zai", "PROV-071"),
    live("zai-coding-cn", "PROV-071"),
    // DRIFT-009 — the four whose rows are in git at NO revision, live since that row landed.
    live("baseten", "DRIFT-009"),
    live("qwen-token-plan", "DRIFT-009"),
    live("qwen-token-plan-cn", "DRIFT-009"),
    live("qwen-token-plan-individual", "DRIFT-009"),
    // XAI_1 — the first catalog to take this path, and the proof the other 37 could.
    live("xai", "XAI_1"),
];

/// One [`LIVE_CATALOGS`] entry. Everything but the ledger id is derived from the stem
/// (`live_catalog::LiveCatalogSpec`), so a thirty-eight-row table cannot hold a mismatched
/// endpoint.
const fn live(file: &'static str, item: &'static str) -> live_catalog::LiveCatalogSpec {
    live_catalog::LiveCatalogSpec { file, item }
}

impl CatalogSpec {
    /// Path under `packages/ai/src/` of this catalog's source module.
    ///
    /// Spelled explicitly on every entry now. It used to default to
    /// `providers/<file>.models.ts` for the 33 provider catalogs this table held; those are
    /// [`LIVE_CATALOGS`] entries (PROV-071) and derive their own module path from their stem, so
    /// the default branch here had no caller left and a default nothing exercises is a default
    /// nobody can trust.
    fn module_path(&self) -> String {
        self.module.to_string()
    }

    /// The revision this catalog's rows are read at: its own pin, else the run's `--rev`.
    fn rev<'a>(&'a self, run_rev: &'a str) -> &'a str {
        self.rev.unwrap_or(run_rev)
    }
}

/// One upstream `packages/ai/src/providers/<stem>.models.ts` module that this generator produces
/// **no** catalog for, with the reason and the ledger item that owns the absence.
///
/// [`CATALOGS`] is the positive half of the roster and this is the negative half. Together they
/// must account for every provider module pi ships at the revision under audit — that is what
/// `gen-catalogs --roster <rev>` checks, and it is the whole point of writing the shortfall down
/// as a table. DRIFT-009 exists because the shortfall was prose: `catalog_data.rs:5-8` said "31
/// embedded catalogs" long after the tree had 35, while pi shipped 39, and nothing in the repo
/// could contradict it.
#[derive(Debug)]
struct Unported {
    /// The module stem — `packages/ai/src/providers/<stem>.models.ts`.
    stem: &'static str,
    reason: UnportedReason,
    /// The gap-analysis item that owns this absence.
    item: &'static str,
    why: &'static str,
}

/// Why an upstream provider module has no embedded catalog.
///
/// # `DataNotInGit` is gone, and its removal is the substance of DRIFT-009
///
/// There used to be a second variant for data this workspace "cannot obtain at all — no extractor,
/// no revision and no amount of effort will produce it", held by DRIFT-009's four modules, and the
/// `--roster` audit re-tested it by re-parsing each module at the audited revision. It is deleted
/// rather than left empty because the category was WRONG, not merely unpopulated: the four modules
/// still do not parse as data literals at `v0.87.1` and `providers/data/` is still absent from git,
/// yet all four catalogs now generate — from `pi.dev/api/models/providers/<id>`, which serves the
/// very artifact the gitignored JSON is built into. A category whose members were obtainable all
/// along cannot be a category, and re-testing the git half of it would report "still blocked" about
/// four catalogs sitting in the tree. The escalation the variant recorded is preserved on the ledger
/// row beside its refutation; what replaces the audit here is [`LIVE_CATALOGS`] membership, which
/// `account_for_roster` already checks, and `rows_from_body`, which HARD-ERRORS on a body whose
/// shape it does not recognise.
#[derive(Debug, PartialEq, Eq)]
enum UnportedReason {
    /// cyrup carries the rows as Rust literals somewhere else, so this generator cannot own them.
    HandPorted,
    /// cyrup does not implement the provider at all, so there is nothing for a catalog to feed.
    /// The absence is a PROVIDER gap owned by its own ledger item, not a catalog one — filing it
    /// here is how the roster stays total without this generator pretending to fix it.
    NoProvider,
    /// cyrup implements the provider, but its catalog reaches it by a mechanism other than
    /// `providers/catalog/*.json`, and closing the difference is owned elsewhere.
    CatalogElsewhere,
}

/// The upstream provider modules with no `providers/catalog/*.json`, as of pi `v0.87.1`.
///
/// [`CATALOGS`] and [`LIVE_CATALOGS`] are the positive halves of the roster and this is the
/// negative half; `account_for_roster` requires the three together to account for every
/// `*.models.ts` pi ships at the audited revision.
///
/// DRIFT-009's four modules — `baseten` and the three `qwen-token-plan*` — used to be here under a
/// `DataNotInGit` reason. They are now [`LIVE_CATALOGS`] entries; see that table's doc comment for
/// why the block was a scope error rather than a data one, and the retired variant's doc above for
/// why the category itself is gone.
///
/// `meta` and `radius` are new upstream at v0.86.0/v0.87.1 and were UNACCOUNTED FOR until this
/// change: `gen-catalogs --roster v0.87.1` failed with "pi ships 2 provider module(s) this
/// generator has never heard of". That failure is the audit working — it is the exact shape of the
/// failure DRIFT-009 was filed for — and the fix is to name them and their owners, not to widen a
/// positive table over providers cyrup does not have.
const UNPORTED: &[Unported] = &[
    Unported {
        stem: "meta",
        reason: UnportedReason::NoProvider,
        item: "PROV-080",
        why: "cyrup has no provider id `meta` at all (`providers/meta.ts` @v0.87.1 — base URL \
              `https://api.meta.ai/v1`, `envApiKeyAuth(\"Meta Model API key\", [\"META_API_KEY\"])`, \
              a Muse-subscription lazy OAuth, registered `all.ts:108`), so a catalog would have \
              nothing to attach to; PROV-080 owns porting the provider and its catalog arrives in \
              that change",
    },
    Unported {
        stem: "radius",
        reason: UnportedReason::CatalogElsewhere,
        item: "PROV-014",
        why: "cyrup's `providers/radius.rs` learns its rows from the gateway's own \
              `GET /v1/config` and carries an EMPTY embedded catalog; pi gained a static \
              `baselineModels` floor from `RADIUS_MODELS` at v0.86.0 (`providers/radius.ts:25-28` \
              @v0.87.1, applied only when the gateway is the default one), which cyrup does not \
              seed — PROV-014's residual piece (1) owns that, and it is provider wiring rather \
              than a generator table",
    },
    Unported {
        stem: "together",
        reason: UnportedReason::HandPorted,
        item: "PROV-060",
        why: "cyrup hand-ports Together's rows as Rust literals in \
              `providers/together.rs::together_models()`, so there is no JSON catalog to generate",
    },
];

/// A row-level divergence from `b0c2a90e` that the regeneration must PRESERVE.
///
/// Every entry is a divergence somebody signed off on, with the ledger id and the upstream citation
/// that justify it. Without this table a refresh silently reverts an accepted decision, which is
/// exactly what PROV-064 warns about: "a regeneration from `b0c2a90e` will re-introduce the map and
/// turn the guard test red, so the regeneration must carry this exception explicitly or it will be
/// silently reverted."
///
/// **`b0c2a90e` is not the last word on every field, and that is the second reason this table
/// exists.** The catalogs pi *generates* are mostly models.dev data, which is in git at no revision
/// — for those, `b0c2a90e` is the only obtainable evidence. But a large part of
/// `packages/ai/scripts/generate-models.ts` is HARDCODED, and that script **is** in git at the
/// ported tag `v0.83.0`. Where the script hardcodes a value, `v0.83.0` is strictly better
/// provenance than `b0c2a90e`'s 13-day-older generated output, and [`Set`](DeltaAction::Set) pins
/// the `v0.83.0` value over it.
struct Delta {
    catalog: &'static str,
    model: &'static str,
    key: &'static str,
    action: DeltaAction,
    why: &'static str,
}

// Constructed only by [`apply_deltas_from`]'s own tests while [`DELTAS`] is empty — see that
// table's doc for why it is empty and why the machinery stays.
#[cfg_attr(not(test), allow(dead_code))]
enum DeltaAction {
    /// Delete the key upstream sets.
    Drop,
    /// Replace upstream's value with this JSON literal.
    Set(&'static str),
}

/// The GPT-5.6 long-context tier threshold, spelled once so the pinned cost literals below read
/// the way `withOpenAiLongContextPricing` (`ai/scripts/generate-models.ts:351-364`) writes them.
const GPT_56_LUNA_COST: &str = r#"{"input":0.2,"output":1.2,"cacheRead":0.02,"cacheWrite":0.25,
     "tiers":[{"inputTokensAbove":272000,"input":0.4,"output":1.8,"cacheRead":0.04,"cacheWrite":0.5}]}"#;
const GPT_56_TERRA_COST: &str = r#"{"input":2,"output":12,"cacheRead":0.2,"cacheWrite":2.5,
     "tiers":[{"inputTokensAbove":272000,"input":4,"output":18,"cacheRead":0.4,"cacheWrite":5}]}"#;
/// The Azure rows are a DERIVED clone of the `openai` rows that copies the four scalar rates and
/// drops `tiers` (`ai/scripts/generate-models.ts:2718-2723` @v0.84.1).
const GPT_56_LUNA_COST_NO_TIERS: &str =
    r#"{"input":0.2,"output":1.2,"cacheRead":0.02,"cacheWrite":0.25}"#;
const GPT_56_TERRA_COST_NO_TIERS: &str =
    r#"{"input":2,"output":12,"cacheRead":0.2,"cacheWrite":2.5}"#;

/// The GPT-5.6 price-cut rationale, shared by the six cost pins.
const WHY_GPT_56_PRICE_CUT: &str = "[CYRUP-DELTA] pi `OPENAI_GPT_56_STANDARD_COSTS` (v0.84.1 `ai/scripts/generate-models.ts:387-393`) \
     vs the inline `{1,6,0.1,1.25}` / `{2.5,15,0.25,3.125}` literals at v0.83.0 (`:2193`, `:2181`) \
     — the same literals b0c2a90e's generated data carries. OpenAI cut Luna and Terra prices on \
     2026-07-30 and cyrup adopted the post-cut table: a deliberate, documented v0.84.1 forward-port \
     pinned by three tests (`providers/openai.rs::gpt_5_6_luna_and_terra_use_the_post_cut_prices`, \
     `providers/azure_openai_responses.rs::the_gpt_5_6_clone_carries_the_post_cut_prices_and_no_tiers`, \
     `providers/openai_codex.rs::the_gpt_5_6_codex_rows_match_the_upstream_literals`). Reverting it \
     would bill users 5x (Luna) and 1.25x (Terra) over the real rate. PROV-059 lists these six as \
     defects because sweep 9 measured only against b0c2a90e; they are preserved, not fixed.";

/// **Empty, and the emptiness is a measurement.** Every one of the twelve entries this table
/// carried was a forward-port: a value pi's `scripts/generate-models.ts` hardcoded at the ported
/// tag, pinned over the older value `b0c2a90e`'s generated data still held. Once [`LIVE_CATALOGS`]
/// took over the provider roster (PROV-071) the rows arrive from `pi.dev` already current, so
/// every pin became either a no-op or a reference to a model upstream has retired — and
/// [`apply_deltas`]'s own guards said so, in twelve hard errors, rather than silently carrying
/// them. Each one moved to [`CONVERGED`], where it is now ASSERTED instead of imposed.
///
/// The machinery stays because the next signed-off divergence is a `Delta`, not a patch to this
/// comment; its unit tests construct their own entries and do not depend on this table being
/// populated.
const DELTAS: &[Delta] = &[];

/// A divergence that [`DELTAS`] used to IMPOSE and that upstream has since ADOPTED (or retired the
/// row for), checked on every run so the decision cannot regress unnoticed.
///
/// This table is the answer to the question a live-fetch generalization has to answer: what
/// happens to the twelve signed-off exceptions when the data stops coming from `b0c2a90e`? Dropping
/// them silently is how a deliberate decision is reverted without a diff — the exact failure
/// [`DELTAS`]'s doc comment warns about. Keeping them as pins is worse: a pin whose value upstream
/// already has is a no-op the generator refuses, and a pin that ever differs from live data would
/// override current pricing with a two-month-old literal.
///
/// So each one is inverted. Instead of "write this value over upstream's", it is now "upstream
/// must already carry this value" — the same guarantee, sourced rather than asserted, and it FAILS
/// if upstream ever moves back. [`Expect::Retired`] covers the three whose model pi no longer
/// ships: there is nothing left to pin, and the assertion becomes "still gone", so a
/// re-introduction forces the original decision to be re-taken rather than silently re-inherited.
struct Converged {
    catalog: &'static str,
    model: &'static str,
    expect: Expect,
    why: &'static str,
}

enum Expect {
    /// The row must exist and `key` must equal this JSON literal.
    Carries {
        key: &'static str,
        value: &'static str,
    },
    /// The row must NOT exist. `key` names what the retired exception acted on, so the failure
    /// message can say what has to be re-decided if the row comes back.
    Retired { key: &'static str },
}

const CONVERGED: &[Converged] = &[
    Converged {
        catalog: "groq",
        model: "qwen/qwen3-32b",
        expect: Expect::Retired {
            key: "thinkingLevelMap",
        },
        why: "PROV-064. The DELTA dropped `thinkingLevelMap` from this row because v0.84.1 \
              retargeted Groq's sole thinking-level override from `qwen/qwen3-32b` \
              (v0.83.0 `ai/scripts/generate-models.ts:837`) to `qwen/qwen3.6-27b` (v0.84.1 `:870`). \
              pi.dev no longer serves `qwen/qwen3-32b` at all, so the override cannot be \
              re-inherited; `providers/fleet.rs` \
              `groq_qwen3_32b_no_longer_carries_the_retargeted_thinking_level_map` still pins the \
              cyrup-side absence.",
    },
    Converged {
        catalog: "openai-codex",
        model: "gpt-5.6-luna",
        expect: Expect::Carries {
            key: "contextWindow",
            value: "272000",
        },
        why: WHY_CODEX_CONTEXT,
    },
    Converged {
        catalog: "openai-codex",
        model: "gpt-5.6-sol",
        expect: Expect::Carries {
            key: "contextWindow",
            value: "272000",
        },
        why: WHY_CODEX_CONTEXT,
    },
    Converged {
        catalog: "openai-codex",
        model: "gpt-5.6-terra",
        expect: Expect::Carries {
            key: "contextWindow",
            value: "272000",
        },
        why: WHY_CODEX_CONTEXT,
    },
    Converged {
        catalog: "openai-codex",
        model: "gpt-5.6-luna",
        expect: Expect::Carries {
            key: "cost",
            value: GPT_56_LUNA_COST,
        },
        why: WHY_GPT_56_PRICE_CUT,
    },
    Converged {
        catalog: "openai-codex",
        model: "gpt-5.6-terra",
        expect: Expect::Carries {
            key: "cost",
            value: GPT_56_TERRA_COST,
        },
        why: WHY_GPT_56_PRICE_CUT,
    },
    Converged {
        catalog: "openai",
        model: "gpt-5.6-luna",
        expect: Expect::Carries {
            key: "cost",
            value: GPT_56_LUNA_COST,
        },
        why: WHY_GPT_56_PRICE_CUT,
    },
    Converged {
        catalog: "openai",
        model: "gpt-5.6-terra",
        expect: Expect::Carries {
            key: "cost",
            value: GPT_56_TERRA_COST,
        },
        why: WHY_GPT_56_PRICE_CUT,
    },
    Converged {
        catalog: "azure-openai-responses",
        model: "gpt-5.6-luna",
        expect: Expect::Carries {
            key: "cost",
            value: GPT_56_LUNA_COST_NO_TIERS,
        },
        why: WHY_GPT_56_PRICE_CUT,
    },
    Converged {
        catalog: "azure-openai-responses",
        model: "gpt-5.6-terra",
        expect: Expect::Carries {
            key: "cost",
            value: GPT_56_TERRA_COST_NO_TIERS,
        },
        why: WHY_GPT_56_PRICE_CUT,
    },
    Converged {
        catalog: "fireworks",
        model: "accounts/fireworks/models/glm-5p2",
        expect: Expect::Retired { key: "compat" },
        why: WHY_FIREWORKS_GLM_COMPAT,
    },
    Converged {
        catalog: "fireworks",
        model: "accounts/fireworks/routers/glm-5p2-fast",
        expect: Expect::Retired { key: "compat" },
        why: WHY_FIREWORKS_GLM_COMPAT,
    },
];

/// Check every [`CONVERGED`] entry for one catalog against the rows about to be written.
///
/// Runs for pinned and live catalogs alike, from [`generate_all`], immediately after
/// [`apply_deltas`] — so an entry is checked against exactly the bytes that land on disk.
fn check_converged(catalog: &str, rows: &[Val]) -> Result<(), String> {
    check_converged_from(CONVERGED, catalog, rows)
}

/// [`check_converged`] over an explicit table, so the guard's own failure modes are testable
/// without depending on what upstream happens to serve today.
fn check_converged_from(
    converged: &[Converged],
    catalog: &str,
    rows: &[Val],
) -> Result<(), String> {
    for entry in converged.iter().filter(|c| c.catalog == catalog) {
        let row = rows
            .iter()
            .find(|r| r.get("id").and_then(Val::as_str) == Some(entry.model));
        match (&entry.expect, row) {
            (Expect::Retired { .. }, None) => {}
            (Expect::Retired { key }, Some(_)) => {
                return Err(format!(
                    "{catalog}: `{}` is back upstream. A signed-off divergence acted on its `{key}` \
                     and was retired because the row was gone; it must be re-decided now, not \
                     silently re-inherited. ({})",
                    entry.model, entry.why
                ));
            }
            (Expect::Carries { key, .. }, None) => {
                return Err(format!(
                    "{catalog}: CONVERGED expects `{}` to carry `{key}`, but upstream no longer \
                     ships the row — the convergence claim is stale and must be re-decided. ({})",
                    entry.model, entry.why
                ));
            }
            (Expect::Carries { key, value }, Some(row)) => {
                let expected = tsdata::parse_json(value).map_err(|e| {
                    format!(
                        "{catalog}: CONVERGED value for {}.{key} is not JSON: {e}",
                        entry.model
                    )
                })?;
                let actual = row.get(key).ok_or_else(|| {
                    format!(
                        "{catalog}: CONVERGED expects `{}` to carry `{key}`, but upstream does not \
                         set it at all. ({})",
                        entry.model, entry.why
                    )
                })?;
                if canonicalize(actual, "") != canonicalize(&expected, "") {
                    return Err(format!(
                        "{catalog}: `{}`.{key} REGRESSED — upstream now serves {}, the signed-off \
                         value is {}. ({})",
                        entry.model,
                        compact(actual),
                        compact(&expected),
                        entry.why
                    ));
                }
            }
        }
    }
    Ok(())
}

/// DRIFT-052's rationale, now attached to the two Fireworks GLM rows' CONVERGED entries.
const WHY_FIREWORKS_GLM_COMPAT: &str = "[CYRUP-DELTA] DRIFT-052. pi `b9497c8c1` (\"fix(ai): correct Fireworks GLM prompt caching, \
     closes #7676\", first tag **v0.84.0**, still current at v0.84.2) moved the Fireworks GLM rows \
     off the inline `candidate.compat = { supportsStore: false, supportsDeveloperRole: false }` \
     they carried at the ported tag v0.83.0 (`ai/scripts/generate-models.ts:2151-2155`) onto the \
     shared `openAICompat` constant in `processFireworksModels` (v0.84.2 `:1239-1244`), which adds \
     `sendSessionAffinityHeaders: true` and `supportsLongCacheRetention: false`. Both are \
     load-bearing and NEITHER is auto-detected: `api/compat.rs::detect_compat` hardcodes \
     `send_session_affinity_headers: false` and computes `supports_long_cache_retention` from a \
     provider list Fireworks is not on, so without the pin cyrup sends no affinity header (every \
     Fireworks prompt-cache lookup misses, since Fireworks routes cache by replica affinity) and \
     claims a 24h/1h retention Fireworks does not honour. This is the same class of signed-off \
     forward-port as the six GPT-5.6 cost pins above, and is pinned by \
     `providers/fireworks.rs::the_glm_5p2_rows_carry_pi_s_openai_compat`.";

const WHY_CODEX_CONTEXT: &str = "PROV-059(d) REFUTED. `CODEX_GPT_56_CONTEXT` is 272000 at the ported tag v0.83.0 \
     (`ai/scripts/generate-models.ts:2352`) AND at v0.84.1 (`:2541`); v0.83.0's comment at `:2349` \
     reads \"GPT-5.6 follows Codex's 272k catalog limit (formerly 372k)\". b0c2a90e's generated \
     data still holds the FORMER 372000, so taking it here would inflate the window 100k past the \
     real limit. The openai-codex rows are hardcoded in the script, not models.dev data, so the \
     script at the ported tag is the better source.";

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

struct Args {
    pi: PathBuf,
    rev: String,
    out: PathBuf,
    check: bool,
    diff: bool,
    /// `Some(rev)` selects the provider-set audit at `rev` instead of generating anything.
    roster: Option<String>,
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf()
}

fn parse_args() -> Result<Args, String> {
    let root = workspace_root();
    let mut args = Args {
        pi: root.parent().unwrap_or(Path::new("..")).join("pi"),
        rev: DEFAULT_REV.to_string(),
        out: root.join("crates/cyrup-provider/src/providers"),
        check: false,
        diff: false,
        roster: None,
    };
    let mut it = std::env::args().skip(1);
    let cmd = it.next().unwrap_or_default();
    if cmd != "gen-catalogs" {
        return Err(format!(
            "unknown command {cmd:?} — expected `gen-catalogs` \
             (see this file's module docs for flags)"
        ));
    }
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("flag {a} needs a value"));
        match a.as_str() {
            "--pi" => args.pi = PathBuf::from(value()?),
            "--rev" => args.rev = value()?,
            "--out" => args.out = PathBuf::from(value()?),
            "--check" => args.check = true,
            "--diff" => args.diff = true,
            "--roster" => args.roster = Some(value()?),
            other => return Err(format!("unknown flag {other:?}")),
        }
    }
    Ok(args)
}

/// Command dispatch. `xtask` takes no dependencies (see `xtask/Cargo.toml`), so this is a `match`
/// on `argv[1]` rather than a parser.
fn run() -> Result<(), String> {
    let mut argv = std::env::args().skip(1);
    let cmd = argv.next().unwrap_or_default();
    match cmd.as_str() {
        // `parse_args` re-reads `std::env::args()` and re-validates the command itself, so this arm
        // hands it nothing: `run_gen_catalogs` is a pure rename of the old `run` body.
        "gen-catalogs" => run_gen_catalogs(),
        "feature-matrix" => features::run_matrix(&argv.collect::<Vec<_>>(), workspace_root()),
        "it" => run_it(&argv.collect::<Vec<_>>()),
        other => Err(format!(
            "unknown command {other:?} — commands are `gen-catalogs`, `feature-matrix` and `it` \
             (see each one's module docs for flags)"
        )),
    }
}

/// Environment variables the hermetic integration-suite runner ([`run_it`]) reinstates after
/// `env_clear()`. Everything the toolchain needs to build and run, and NOTHING that can name,
/// carry or unlock a provider credential: no `*_API_KEY`/token vars, no `AWS_*`/`GOOGLE_*`
/// ambient-credential triggers, no proxies, no `CYRUP_HOME`/`CYRUP_*` gates. `HOME` itself is
/// safe to keep — the suite's in-process seams read credentials from env VARS (which are cleared
/// here) and its spawned children are re-homed onto per-test tempdirs by
/// `crates/cyrup-it/tests/support/env.rs::hermetic`.
pub(crate) const IT_ENV_ALLOWLIST: &[&str] = &[
    // process basics
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "TERM",
    "TMPDIR",
    "TZ",
    "LANG",
    "LC_ALL",
    // toolchain — the suite's build.rs runs nested `cargo build`s and a wasm32-wasip2 build
    "CARGO",
    "CARGO_HOME",
    "CARGO_TARGET_DIR",
    "CARGO_BUILD_JOBS",
    "CARGO_INCREMENTAL",
    "CARGO_TERM_COLOR",
    "RUSTUP_HOME",
    "RUSTUP_TOOLCHAIN",
    "RUSTC",
    "RUSTC_WRAPPER",
    "RUSTFLAGS",
    "RUST_BACKTRACE",
    // the suite's own documented overrides (crates/cyrup-it/build.rs) — deliberate, named opt-ins
    "CYRUP_IT_BIN_DIR",
    "CYRUP_EXT_FIXTURE_COMPONENT",
];

/// `cargo xtask it [extra cargo-test args…]` — run the armed integration suite
/// (`cargo test -p cyrup-it --features it`) in a **hermetic harness environment**: `env_clear()`
/// plus [`IT_ENV_ALLOWLIST`], the `env -i` shape of pi's own `test.sh`.
///
/// This is the supported way to run the suite on a machine whose shell exports real provider
/// credentials (`ANTHROPIC_API_KEY`, `AWS_PROFILE`, `CYRUP_HOME`, …). Run raw, the suite's §4 R5
/// layer-3 guards (`crates/cyrup-it/tests/support/env.rs`) red immediately and name the leaked
/// variables — by design, because in-process seams like `session_svc/model_registry.rs` read the
/// process environment through `provider_is_configured(.., env: None)` (1:1 Pi parity,
/// non-injectable), so ambient credentials would change what those tests observe. Under this
/// wrapper the harness process itself starts clean, the guards pass, and the run is provably
/// spend-free — spawned children were already hermetic by construction.
fn run_it(extra: &[String]) -> Result<(), String> {
    // Resolve the invoking cargo BEFORE clearing anything (cargo sets $CARGO for run targets).
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());

    // Prefer the documented runner (docs/TEST-ARCHITECTURE.md §2.4:
    // `cargo nextest run -p cyrup-it --features it,wasm-host`): process-per-test isolation is what
    // several seam tests assume (`tests/mcp/http_oauth.rs` shares a process-global auth-store
    // override and interferes with itself under plain `cargo test`'s in-process threading). Fall
    // back to `cargo test` when nextest is not installed, and say so.
    let has_nextest = Command::new(&cargo)
        .args(["nextest", "--version"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());

    let mut cmd = Command::new(&cargo);
    cmd.current_dir(workspace_root());
    if has_nextest {
        cmd.args([
            "nextest",
            "run",
            "-p",
            "cyrup-it",
            "--features",
            "it,wasm-host",
        ]);
    } else {
        println!(
            "xtask it: cargo-nextest not installed; falling back to `cargo test` \
             (docs/TEST-ARCHITECTURE.md §2.4 prefers nextest for process-per-test isolation)"
        );
        cmd.args(["test", "-p", "cyrup-it", "--features", "it"]);
    }
    cmd.args(extra);
    apply_hermetic_env(&mut cmd);
    let status = cmd
        .status()
        .map_err(|e| format!("cannot spawn cargo for the it suite: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("integration suite failed: {status}"))
    }
}

/// `env_clear()` + [`IT_ENV_ALLOWLIST`] on a child cargo invocation. Shared by [`run_it`] and by
/// every `feature-matrix` row (`features::run_matrix`), so the ONLY two documented ways to run
/// the armed seam suite both start it from a credential-free harness environment.
pub(crate) fn apply_hermetic_env(cmd: &mut Command) {
    cmd.env_clear();
    for key in IT_ENV_ALLOWLIST {
        if let Some(value) = std::env::var_os(key) {
            cmd.env(key, value);
        }
    }
}

fn run_gen_catalogs() -> Result<(), String> {
    let args = parse_args()?;
    if let Some(rev) = args.roster.clone() {
        return run_roster(&args, &rev);
    }

    // Live catalogs FIRST: the manifest carries their provenance, so it has to be built after them.
    let live = live_catalog::refresh(LIVE_CATALOGS, &live_catalog::fetch_with_curl)?;
    for (spec, outcome) in &live {
        if let live_catalog::LiveOutcome::Skipped { why } = outcome {
            // Non-fatal, exactly like the UNPORTED table's "declare and skip cleanly" precedent:
            // `<file>.json` and its manifest entry are left byte-for-byte as they are.
            println!(
                "gen-catalogs: {}.json left unchanged — live fetch skipped ({why})",
                spec.file
            );
        }
    }

    let generated = generate_all(&args, &live)?;

    if args.diff {
        return report_diff(&args, &generated);
    }

    let mut differing: Vec<String> = Vec::new();
    for (name, body) in &generated {
        let path = catalog_path(&args.out, name);
        let current = std::fs::read_to_string(&path).unwrap_or_default();
        if &current == body {
            continue;
        }
        differing.push(name.clone());
        if !args.check {
            std::fs::write(&path, body)
                .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        }
    }

    if args.check {
        if differing.is_empty() {
            // `generated` holds only what this run could produce, so a skipped live fetch is
            // reported as a smaller comparison rather than as a silent pass over 39 files.
            let fetched = generated.len().saturating_sub(CATALOGS.len() + 1);
            println!(
                "gen-catalogs --check: all {} file(s) compared reproduce — {fetched} live from \
                 {}<id>, {} from pi@{}{}{}",
                generated.len(),
                live_catalog::LIVE_CATALOG_ENDPOINT,
                CATALOGS.len(),
                args.rev,
                own_rev_summary(&args.rev),
                if fetched == LIVE_CATALOGS.len() {
                    String::new()
                } else {
                    format!(
                        " ({} live catalog(s) NOT checked — their fetch was skipped)",
                        LIVE_CATALOGS.len() - fetched
                    )
                }
            );
            return Ok(());
        }
        return Err(format!(
            "{} file(s) do not reproduce: {}\nrun `cargo run -p xtask -- gen-catalogs` to refresh, \
             and account for every change in docs/gap-analysis/01-cyrup-core-and-provider.md. A \
             live catalog differs whenever pi.dev has republished since the last refresh, which is \
             a real change to review, not noise.",
            differing.len(),
            differing.join(", ")
        ));
    }

    println!(
        "gen-catalogs: wrote {} of {} files — {} from pi@{}{}, {} live from {}<id>",
        differing.len(),
        generated.len(),
        CATALOGS.len(),
        args.rev,
        own_rev_summary(&args.rev),
        LIVE_CATALOGS.len(),
        live_catalog::LIVE_CATALOG_ENDPOINT
    );
    for name in &differing {
        println!("  updated {name}");
    }
    Ok(())
}

/// `" (openrouter-images at pi@v0.87.1)"` for every catalog read at its own pin, so a run's summary
/// never names one revision for rows that came from two.
fn own_rev_summary(run_rev: &str) -> String {
    let pinned: Vec<String> = CATALOGS
        .iter()
        .filter(|c| c.rev.is_some())
        .map(|c| format!("{} at pi@{}", c.file, c.rev(run_rev)))
        .collect();
    if pinned.is_empty() {
        String::new()
    } else {
        format!(" ({})", pinned.join(", "))
    }
}

// -------------------------------------------------------------------------- the provider roster --

/// How the two roster tables account for the provider modules pi ships at one revision.
#[derive(Debug)]
struct Roster {
    /// Modules [`CATALOGS`] generates a catalog from, in upstream order.
    embedded: Vec<&'static CatalogSpec>,
    /// Modules [`LIVE_CATALOGS`] fetches live instead (XAI_1) — the third roster bucket.
    live: Vec<&'static live_catalog::LiveCatalogSpec>,
    /// [`UNPORTED`] entries pi ships at this revision.
    unported_present: Vec<&'static Unported>,
    /// [`UNPORTED`] entries pi does **not** ship at this revision — the four DRIFT-009 modules
    /// against the `b0c2a90e` floor, which predates all of them. Reported, never an error: the
    /// table describes pi's tip and the floor is allowed to be older than it.
    unported_absent: Vec<&'static Unported>,
}

/// Account for every `*.models.ts` stem pi ships at one revision, or say which ones nobody has.
///
/// Pure so the accounting is testable without a pi checkout: the shell ([`run_roster`]) owns the
/// `git ls-tree` and this owns the decision. Two conditions are hard errors, and both are silent
/// today: a module pi ships that none of the three tables names (a provider appeared and no
/// catalog was filed — DRIFT-009's original failure), and a [`CATALOGS`] or [`LIVE_CATALOGS`]
/// entry pi no longer ships (cyrup would keep embedding — or fetching — a retired provider, and a
/// plain `gen-catalogs` run would fail far less legibly, on a missing `git show` or a 404 from the
/// live endpoint).
fn account_for_roster(upstream_stems: &[String]) -> Result<Roster, String> {
    let mut embedded = Vec::new();
    let mut live = Vec::new();
    let mut unported_present = Vec::new();
    let mut unaccounted = Vec::new();

    for stem in upstream_stems {
        if let Some(spec) = CATALOGS
            .iter()
            .find(|c| c.images_provider.is_none() && c.file == stem)
        {
            embedded.push(spec);
        } else if let Some(l) = LIVE_CATALOGS.iter().find(|l| l.file == stem) {
            live.push(l);
        } else if let Some(u) = UNPORTED.iter().find(|u| u.stem == stem) {
            unported_present.push(u);
        } else {
            unaccounted.push(stem.as_str());
        }
    }
    if !unaccounted.is_empty() {
        return Err(format!(
            "pi ships {} provider module(s) this generator has never heard of: {}. Each one is a \
             catalog cyrup does not embed. File it, then add it to CATALOGS (if its rows are \
             extractable from a pinned revision), to LIVE_CATALOGS (if the rows must instead be \
             fetched live), or to UNPORTED with the ledger item that owns the absence — a roster \
             that cannot disagree with the tree is how DRIFT-009 went four catalogs stale.",
            unaccounted.len(),
            unaccounted.join(", ")
        ));
    }

    let retired: Vec<&str> = CATALOGS
        .iter()
        .filter(|c| c.images_provider.is_none())
        .map(|c| c.file)
        .chain(LIVE_CATALOGS.iter().map(|l| l.file))
        .filter(|file| !upstream_stems.iter().any(|s| s == file))
        .collect();
    if !retired.is_empty() {
        return Err(format!(
            "CATALOGS/LIVE_CATALOGS generate {} catalog(s) pi does not ship at this revision: {}. \
             Either the revision is older than the port (expected for --rev b0c2a90e, not for a \
             tag) or upstream retired the provider and the embedded (or live) catalog must be \
             retired with it.",
            retired.len(),
            retired.join(", ")
        ));
    }

    let unported_absent = UNPORTED
        .iter()
        .filter(|u| !upstream_stems.iter().any(|s| s == u.stem))
        .collect();
    Ok(Roster {
        embedded,
        live,
        unported_present,
        unported_absent,
    })
}

/// The `*.models.ts` stems pi ships at `rev`, in `git ls-tree` (byte) order.
fn upstream_provider_stems(pi: &Path, rev: &str) -> Result<Vec<String>, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(pi)
        .args(["ls-tree", "--name-only", rev, "packages/ai/src/providers/"])
        .output()
        .map_err(|e| format!("cannot run git in {}: {e}", pi.display()))?;
    if !out.status.success() {
        return Err(format!(
            "git ls-tree {rev} packages/ai/src/providers/ failed in {}: {}",
            pi.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| line.rsplit('/').next())
        .filter_map(|name| name.strip_suffix(".models.ts"))
        .map(str::to_string)
        .collect())
}

/// `gen-catalogs --roster <rev>`: audit the provider SET at `rev`, writing nothing.
///
/// It used to also re-parse every `DataNotInGit` module at `rev` to see whether DRIFT-009's block
/// had lifted. That re-test went with the variant (DRIFT-009): the four modules are live-fetched
/// now, so "does this still fail to parse as a data literal?" has no consequence — the answer is
/// yes at `v0.87.1` and the catalogs generate regardless.
fn run_roster(args: &Args, rev: &str) -> Result<(), String> {
    let stems = upstream_provider_stems(&args.pi, rev)?;
    if stems.is_empty() {
        return Err(format!(
            "no packages/ai/src/providers/*.models.ts at {rev} in {} — wrong --pi path or wrong \
             revision",
            args.pi.display()
        ));
    }
    let roster = account_for_roster(&stems)?;

    println!(
        "gen-catalogs --roster {rev}: {} provider module(s) upstream — {} embedded, {} \
         live-fetched, {} accounted for as unported",
        stems.len(),
        roster.embedded.len(),
        roster.live.len(),
        roster.unported_present.len()
    );
    for l in &roster.live {
        println!("  live-fetched {} — {} — {}", l.file, l.item, l.url());
    }
    for u in &roster.unported_present {
        let reason = match u.reason {
            UnportedReason::HandPorted => "hand-ported",
            UnportedReason::NoProvider => "no such provider in cyrup",
            UnportedReason::CatalogElsewhere => "catalog supplied elsewhere",
        };
        println!("  unported {} — {reason}, {} — {}", u.stem, u.item, u.why);
    }
    for u in &roster.unported_absent {
        println!(
            "  (not shipped at {rev}) {} — {}, listed because pi's tip ships it",
            u.stem, u.item
        );
    }
    Ok(())
}

fn catalog_path(out: &Path, name: &str) -> PathBuf {
    if name == "catalog_manifest" {
        out.join("catalog_manifest.json")
    } else {
        out.join("catalog").join(format!("{name}.json"))
    }
}

/// Generate every catalog body plus the manifest, in a stable order.
fn generate_all(
    args: &Args,
    live: &[(
        &'static live_catalog::LiveCatalogSpec,
        live_catalog::LiveOutcome,
    )],
) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for spec in CATALOGS {
        let src = git_show(
            &args.pi,
            spec.rev(&args.rev),
            &format!("packages/ai/src/{}", spec.module_path()),
        )?;
        let rows = extract_rows(spec, &src)?;
        let rows = apply_deltas(spec.file, rows)?;
        check_converged(spec.file, &rows)?;
        out.push((spec.file.to_string(), live_catalog::render(rows)));
    }
    // The live half goes through the SAME two guards. Skipping them here is how a live-fetch
    // generalization loses every signed-off decision without a diff (PROV-071).
    for (spec, outcome) in live {
        if let live_catalog::LiveOutcome::Fetched { rows, .. } = outcome {
            let rows = apply_deltas(spec.file, rows.clone())?;
            check_converged(spec.file, &rows)?;
            out.push((spec.file.to_string(), live_catalog::render(rows)));
        }
    }
    out.push(("catalog_manifest".to_string(), manifest_json(args, live)?));
    Ok(out)
}

/// The rows of one catalog, in upstream declaration order.
fn extract_rows(spec: &CatalogSpec, src: &str) -> Result<Vec<Val>, String> {
    match spec.images_provider {
        None => {
            tsdata::parse_models_module(src).map_err(|e| format!("{}: {e}", spec.module_path()))
        }
        Some(provider) => {
            let whole = tsdata::parse_module_object(src)
                .map_err(|e| format!("{}: {e}", spec.module_path()))?;
            let sub = whole.get(provider).ok_or_else(|| {
                format!(
                    "{}: no `{provider}` sub-record — image-models.generated.ts binds \
                     provider -> id -> ImagesModel (PROV-065)",
                    spec.module_path()
                )
            })?;
            tsdata::object_values(sub).map_err(|e| format!("{}: {e}", spec.module_path()))
        }
    }
}

/// Apply the signed-off divergences for this catalog, refusing to run if one no longer applies.
///
/// A stale exception is as dangerous as a missing one: it means somebody is holding a divergence
/// open against a row upstream has already changed, and nobody would ever be told.
fn apply_deltas(catalog: &str, rows: Vec<Val>) -> Result<Vec<Val>, String> {
    apply_deltas_from(DELTAS, catalog, rows)
}

/// [`apply_deltas`] over an explicit table. [`DELTAS`] is empty today (every entry converged), so
/// its guards would be untestable against the const — and an untestable guard is how the next
/// signed-off divergence gets carried wrongly.
fn apply_deltas_from(
    deltas: &[Delta],
    catalog: &str,
    mut rows: Vec<Val>,
) -> Result<Vec<Val>, String> {
    for delta in deltas.iter().filter(|d| d.catalog == catalog) {
        let row = rows
            .iter_mut()
            .find(|r| r.get("id").and_then(Val::as_str) == Some(delta.model))
            .ok_or_else(|| {
                format!(
                    "{catalog}: DELTAS names model `{}`, which pi@this revision no longer ships \
                     — the exception is stale and must be re-decided, not carried. ({})",
                    delta.model, delta.why
                )
            })?;
        match delta.action {
            DeltaAction::Drop => {
                if !row.remove(delta.key) {
                    return Err(format!(
                        "{catalog}: DELTAS drops `{}` from `{}`, but upstream no longer sets it — \
                         the exception is a no-op and must be deleted. ({})",
                        delta.key, delta.model, delta.why
                    ));
                }
            }
            DeltaAction::Set(json) => {
                let pinned = tsdata::parse_json(json).map_err(|e| {
                    format!(
                        "{catalog}: DELTAS pin for {}.{} is not JSON: {e}",
                        delta.model, delta.key
                    )
                })?;
                let upstream = row.get(delta.key).ok_or_else(|| {
                    format!(
                        "{catalog}: DELTAS pins `{}` on `{}`, but upstream does not set that key \
                         at all — the pin would be an invention, not a divergence. ({})",
                        delta.key, delta.model, delta.why
                    )
                })?;
                if upstream == &pinned {
                    return Err(format!(
                        "{catalog}: DELTAS pins `{}` on `{}` to the value upstream already has — \
                         the exception is a no-op and must be deleted. ({})",
                        delta.key, delta.model, delta.why
                    ));
                }
                row.set(delta.key, pinned);
            }
        }
    }
    Ok(rows)
}

fn git_show(pi: &Path, rev: &str, path: &str) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(pi)
        .arg("show")
        .arg(format!("{rev}:{path}"))
        .output()
        .map_err(|e| format!("cannot run git in {}: {e}", pi.display()))?;
    if !out.status.success() {
        return Err(format!(
            "git show {rev}:{path} failed in {}: {}",
            pi.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    String::from_utf8(out.stdout).map_err(|e| format!("{path} is not UTF-8: {e}"))
}

/// The regenerated `catalog_manifest.json`.
///
/// PROV-060 asked for a **per-provider** revision map so that a provenance split can never again be
/// described by one value. Since XAI_1 there IS a split — 34 catalogs from one pinned revision, one
/// (`xai`) fetched live — and the map is what makes it machine-checkable instead of prose.
fn manifest_json(
    args: &Args,
    live: &[(
        &'static live_catalog::LiveCatalogSpec,
        live_catalog::LiveOutcome,
    )],
) -> Result<String, String> {
    let source = format!("pi@{}", args.rev);
    let endpoint = live_catalog::LIVE_CATALOG_ENDPOINT;
    let pinned_count = CATALOGS.len();
    let live_count = LIVE_CATALOGS.len();
    let catalog_count = pinned_count + live_count; // D5 — 39, one file per catalog on disk
    let generated_at = if args.rev == DEFAULT_REV {
        DEFAULT_REV_TIMESTAMP.to_string()
    } else {
        git_show_commit_date(&args.pi, &args.rev)
            .unwrap_or_else(|_| DEFAULT_REV_TIMESTAMP.to_string())
    };

    // Derived from DELTAS rather than written as prose: the previous note said "one signed-off row
    // divergence" while the table already carried ten, which is precisely the stale-by-hand failure
    // PROV-060 exists to prevent. A count that cannot disagree with the table cannot go stale.
    let delta_count = DELTAS.len();
    let delta_summary = {
        let mut per_catalog: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for delta in DELTAS {
            per_catalog
                .entry(delta.catalog)
                .or_default()
                .push(format!("{} {}", delta.model, delta.key));
        }
        per_catalog
            .into_iter()
            .map(|(catalog, mut rows)| {
                rows.sort();
                format!("{catalog}: {}", rows.join(", "))
            })
            .collect::<Vec<_>>()
            .join("; ")
    };
    let delta_summary = if delta_summary.is_empty() {
        "none; see CONVERGED".to_string()
    } else {
        delta_summary
    };

    let note = format!(
        "Machine-readable counterpart of the provenance prose in src/tests/catalog_data.rs. All \
         {catalog_count} embedded catalogs under providers/catalog/*.json are generated by \
         `cargo run -p xtask -- gen-catalogs`. {live_count} of them — every provider catalog — are \
         fetched LIVE from {endpoint}<id>, the same endpoint the runtime overlay reads \
         (cyrup-provider/src/remote_catalog.rs), and each carries its own fetchedAt/revision under \
         `catalogs` below. {pinned_count} (openrouter-images) is read at its OWN pinned revision, \
         pi@{IMAGES_REV}, named by its `catalogs` entry (PROV-089), and neither `generatedAt` nor \
         any `fetchedAt` is a floor for it because the pi.dev overlay never serves image rows. \
         Top-level `generatedAt` and `source` ({source}, {generated_at}) therefore describe no \
         embedded catalog's rows any more; they remain the global overlay floor described below. \
         PROV-071: the pinned path is DEAD for provider modules and always was — every \
         packages/ai/src/providers/<p>.models.ts has been a re-export of gitignored, \
         network-generated JSON since a9f6a3159 (b0c2a90e's direct child), so no revision can \
         yield newer rows for any of them; b0c2a90e was never a chosen pin, only the last \
         revision that answered. openrouter-images is not a provider module — its rows are the \
         openrouter sub-record of packages/ai/src/image-models.generated.ts, still a data literal \
         in git, and {endpoint}openrouter-images is a 404 (PROV-065). `catalogs` records the per-provider source so the split is machine-checkable \
         rather than prose (PROV-060). Per-provider `fetchedAt` is the staleness floor for that \
         provider's pi.dev overlay and takes precedence over the global `generatedAt`; the global \
         value remains the floor for any catalog without one and must not be moved to follow a \
         live fetch. `generatedAt` is the staleness floor for the pi.dev overlay (DRIFT-007): a \
         persisted remote catalog whose Last-Modified is not strictly newer than this is discarded \
         whole, so upgrading cyrup can never leave a pre-upgrade overlay shadowing freshly \
         refreshed embedded data. THIS IS STILL A COMMITTED FLOOR, NOT A RUNTIME FETCH: the files \
         are include_str!-ed, a maintainer runs gen-catalogs and reviews the diff, and the \
         offline or first-run user this floor exists for has no network by definition. \
         EXCEPTIONS: providers/together.rs hand-ports Together's rows as Rust literals and has no \
         file here (PROV-060); `meta` has no cyrup provider to attach a catalog to (PROV-080); \
         `radius` learns its rows from its gateway and has no embedded floor, where pi gained one \
         at v0.86.0 (PROV-014). SIGNED-OFF ROW DIVERGENCES: {delta_count} — {delta_summary}. Every \
         one of the twelve this generator used to impose against b0c2a90e has CONVERGED: upstream \
         now serves the pinned value itself, or retired the row, and the generator's CONVERGED \
         table asserts that on every run instead of overwriting the data — so a regression is a \
         hard error rather than a silent revert. The pre-cut GPT-5.6 prices, the 372k Codex \
         context windows and the pre-#7676 Fireworks GLM compat block that made those exceptions \
         necessary are gone with the b0c2a90e data that carried them, along with the \
         cloudflare-ai-gateway asymmetry the previous note recorded as known incompleteness: that \
         family's rows now arrive priced by upstream like the other three."
    );

    let mut catalogs: Vec<(String, Val)> = Vec::new();
    for spec in CATALOGS {
        catalogs.push((
            spec.file.to_string(),
            Val::Obj(vec![
                (
                    "source".to_string(),
                    Val::Str(format!("pi@{}", spec.rev(&args.rev))),
                ),
                (
                    "module".to_string(),
                    Val::Str(format!("packages/ai/src/{}", spec.module_path())),
                ),
            ]),
        ));
    }
    for (spec, outcome) in live {
        let (fetched_at, revision) = match outcome {
            live_catalog::LiveOutcome::Fetched {
                fetched_at,
                revision,
                ..
            } => (
                fetched_at.clone().map_or(Val::Null, Val::Str),
                revision.clone().map_or(Val::Null, Val::Str),
            ),
            // D6 — a failed fetch must not revert provenance to a revision the data does not sit
            // on: carry the previous manifest's entry forward instead.
            live_catalog::LiveOutcome::Skipped { .. } => carry_forward(&args.out, spec.file),
        };
        catalogs.push((
            spec.file.to_string(),
            Val::Obj(vec![
                ("source".to_string(), Val::Str(spec.url())),
                ("module".to_string(), Val::Str(spec.module())), // D7
                ("fetchedAt".to_string(), fetched_at),
                ("revision".to_string(), revision),
            ]),
        ));
    }

    let manifest = Val::Obj(vec![
        ("generatedAt".to_string(), Val::Str(generated_at)),
        ("source".to_string(), Val::Str(source)),
        ("note".to_string(), Val::Str(note)),
        ("catalogs".to_string(), Val::Obj(catalogs)),
    ]);
    let mut body = manifest.to_json();
    body.push('\n');
    Ok(body)
}

/// A live catalog's manifest provenance, carried forward from the manifest already on disk when
/// its fetch is skipped (D6) — NEVER reverted to `pi@b0c2a90e`, which is a revision the live data
/// never sat on. Never fails the run: a missing file, unreadable JSON, or no entry for `file` all
/// degrade to `(Val::Null, Val::Null)`, exactly the shape a first-ever run (no manifest yet on
/// disk) would produce.
fn carry_forward(out_dir: &Path, file: &str) -> (Val, Val) {
    let previous = std::fs::read_to_string(catalog_path(out_dir, "catalog_manifest"))
        .ok()
        .and_then(|src| tsdata::parse_json(&src).ok());
    let entry = previous
        .as_ref()
        .and_then(|doc| doc.get("catalogs"))
        .and_then(|catalogs| catalogs.get(file));
    let fetched_at = entry
        .and_then(|e| e.get("fetchedAt"))
        .cloned()
        .unwrap_or(Val::Null);
    let revision = entry
        .and_then(|e| e.get("revision"))
        .cloned()
        .unwrap_or(Val::Null);
    (fetched_at, revision)
}

fn git_show_commit_date(pi: &Path, rev: &str) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(pi)
        .args([
            "show",
            "-s",
            "--format=%cd",
            "--date=format-local:%Y-%m-%dT%H:%M:%SZ",
            rev,
        ])
        .env("TZ", "UTC")
        .output()
        .map_err(|e| format!("cannot run git in {}: {e}", pi.display()))?;
    if !out.status.success() {
        return Err(format!("git show -s {rev} failed"));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

// ------------------------------------------------------------------------------- structural diff --

/// Print a model-level and field-level diff of what is on disk against what the pinned revision
/// says, ignoring formatting entirely. This is the accounting view: every line it prints is a
/// datum that a plain `gen-catalogs` run would move.
fn report_diff(args: &Args, generated: &[(String, String)]) -> Result<(), String> {
    let mut added = 0usize;
    let mut removed = 0usize;
    let mut changed = 0usize;
    for (name, body) in generated {
        if name == "catalog_manifest" {
            continue;
        }
        let path = catalog_path(&args.out, name);
        let current_src = std::fs::read_to_string(&path).unwrap_or_default();
        let current = index_rows(
            &tsdata::parse_json(&current_src).map_err(|e| format!("{}: {e}", path.display()))?,
        )?;
        let next = index_rows(&tsdata::parse_json(body).map_err(|e| format!("{name}: {e}"))?)?;

        for row in diff_catalog(&current, &next) {
            match row {
                RowDiff::Added(id) => {
                    println!("{name}: + {id}  (absent in cyrup, present upstream)");
                    added += 1;
                }
                RowDiff::Removed(id) => {
                    println!("{name}: - {id}  (shipped by cyrup, retired upstream)");
                    removed += 1;
                }
                RowDiff::Changed(id, lines) => {
                    for line in lines {
                        println!("{name}: ~ {id}  {line}");
                        changed += 1;
                    }
                }
            }
        }
    }
    println!("\ntotals: {added} missing rows, {removed} retired rows, {changed} field differences");
    Ok(())
}

/// What happened to one model id between the on-disk catalog and the freshly extracted one.
///
/// Upstream's differ reports the same three outcomes off one walk of the sorted union of both
/// sides' ids (`v0.84.4 scripts/diff-model-catalog.mjs:191-198`, where `beforeModel` and
/// `afterModel` are each `undefined` when that side lacks the row). Naming them keeps the decision
/// — which row moved, and how — separable from the printing, and is what lets the union walk be
/// tested without capturing stdout.
enum RowDiff {
    /// Absent in cyrup, present upstream.
    Added(String),
    /// Shipped by cyrup, retired upstream.
    Removed(String),
    /// On both sides, with at least one field difference.
    Changed(String, Vec<String>),
}

/// Classify one catalog's rows, walking the **sorted union** of both sides' model ids.
///
/// Port of `diff-model-catalog.mjs:191-195`: `modelIds` is the deduplicated, sorted union, and a
/// row counts as unchanged when the two sides are equal **after canonicalization**, so an added
/// row and a changed row that sort next to each other are reported next to each other rather than
/// in two separate passes.
fn diff_catalog(current: &BTreeMap<String, Val>, next: &BTreeMap<String, Val>) -> Vec<RowDiff> {
    let mut ids: Vec<&String> = current.keys().chain(next.keys()).collect();
    ids.sort();
    ids.dedup();

    let mut out = Vec::new();
    for id in ids {
        match (current.get(id), next.get(id)) {
            (None, Some(_)) => out.push(RowDiff::Added(id.clone())),
            (Some(_), None) => out.push(RowDiff::Removed(id.clone())),
            (Some(cur), Some(row)) => {
                if canonicalize(cur, "") == canonicalize(row, "") {
                    continue;
                }
                out.push(RowDiff::Changed(id.clone(), field_diff(cur, row)));
            }
            (None, None) => {}
        }
    }
    out
}

/// pi's thinking-level ladder, in the order its differ canonicalizes `thinkingLevelMap` keys into
/// (`v0.84.4 scripts/diff-model-catalog.mjs:93`).
const THINKING_LEVEL_ORDER: &[&str] = &["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// A key's position in [`THINKING_LEVEL_ORDER`], or "after everything" for a key that is not a
/// thinking level — `THINKING_LEVEL_RANKS.get(left) ?? Number.POSITIVE_INFINITY`
/// (`v0.84.4 scripts/diff-model-catalog.mjs:99-100`).
fn thinking_level_rank(key: &str) -> usize {
    THINKING_LEVEL_ORDER
        .iter()
        .position(|level| *level == key)
        .unwrap_or(usize::MAX)
}

/// Canonicalize a catalog value for comparison, the way upstream's own differ does.
///
/// Port of `canonicalizeJson` / `sortJsonKeys` (`v0.84.4 scripts/diff-model-catalog.mjs:96-114`):
/// object keys are sorted, except under a `thinkingLevelMap` or `values` parent, where they are
/// ordered by the thinking ladder first and lexically only to break ties. **Array entries are
/// canonicalized with no parent key** (`:106` calls `canonicalizeJson(entry)` with one argument),
/// so a map nested inside an array does not inherit the ladder — ported literally rather than
/// tidied, because the point of the port is that the two sides agree.
///
/// This is why the emitted catalogs keep upstream's declaration order on disk (`Val::Obj` is an
/// ordered vector, and `--check` is a byte comparison) yet `--diff` reports no difference when only
/// that order moves: reordering a `compat` block is not a data change, and a differ that says it is
/// buries the rows that are.
///
/// The one deliberate approximation: JS breaks ties with `localeCompare`, this uses byte order.
/// Every key pi emits is ASCII (`tsdata`'s module docs record that no `*.models.ts` at `b0c2a90e`
/// contains a non-ASCII string), where the two agree.
fn canonicalize(v: &Val, parent_key: &str) -> Val {
    match v {
        Val::Arr(items) => Val::Arr(items.iter().map(|i| canonicalize(i, "")).collect()),
        Val::Obj(entries) => {
            let ladder = parent_key == "thinkingLevelMap" || parent_key == "values";
            let mut sorted: Vec<&(String, Val)> = entries.iter().collect();
            sorted.sort_by(|(a, _), (b, _)| {
                if ladder {
                    thinking_level_rank(a)
                        .cmp(&thinking_level_rank(b))
                        .then_with(|| a.cmp(b))
                } else {
                    a.cmp(b)
                }
            });
            Val::Obj(
                sorted
                    .into_iter()
                    .map(|(k, v)| (k.clone(), canonicalize(v, k)))
                    .collect(),
            )
        }
        other => other.clone(),
    }
}

fn index_rows(v: &Val) -> Result<BTreeMap<String, Val>, String> {
    let Val::Arr(rows) = v else {
        return Err("catalog is not a JSON array".to_string());
    };
    let mut out = BTreeMap::new();
    for row in rows {
        let id = row
            .get("id")
            .and_then(Val::as_str)
            .ok_or_else(|| "catalog row has no `id`".to_string())?;
        out.insert(id.to_string(), row.clone());
    }
    Ok(out)
}

/// Field-by-field differences between two rows, as human-readable lines.
///
/// Both sides are [`canonicalize`]d first, so this reports only differences upstream's own differ
/// would report: a key that moved is not a difference, and the lines come out in the same
/// canonical order on every run.
fn field_diff(cur: &Val, next: &Val) -> Vec<String> {
    let (cur, next) = (canonicalize(cur, ""), canonicalize(next, ""));
    let (Val::Obj(a), Val::Obj(b)) = (&cur, &next) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (k, bv) in b {
        match a.iter().find(|(ak, _)| ak == k) {
            None => out.push(format!("{k}: (absent) -> {}", compact(bv))),
            Some((_, av)) if av != bv => {
                out.push(format!("{k}: {} -> {}", compact(av), compact(bv)));
            }
            Some(_) => {}
        }
    }
    for (k, av) in a {
        if !b.iter().any(|(bk, _)| bk == k) {
            out.push(format!("{k}: {} -> (absent)", compact(av)));
        }
    }
    out
}

fn compact(v: &Val) -> String {
    v.to_json()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace("{ ", "{")
        .replace(" }", "}")
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;

    /// The roster must stay pinned to the file set it claims to own. `include_str!` cannot glob and
    /// this table cannot walk the tree at compile time, so the count is the guard that a new pi
    /// provider (or a new cyrup catalog) forces somebody to look here.
    ///
    /// DRIFT-009 widened the SPLIT without widening the total: the four ex-`UNPORTED` modules gained
    /// catalog files (so the total went 35 -> 39) while `CATALOGS` stayed at 34, because all four
    /// arrived through [`LIVE_CATALOGS`] rather than through `git show`. PROV-071 then moved the
    /// remaining 33 provider catalogs across the same way, leaving `CATALOGS` with the ONE file
    /// that is not a provider module. Both halves are asserted because the interesting failure is a
    /// file moving between the tables, which a total alone cannot see.
    #[test]
    fn the_catalog_roster_is_the_39_embedded_files() {
        assert_eq!(
            CATALOGS.len(),
            1,
            "PROV-071 moved every provider catalog to LIVE_CATALOGS; `openrouter-images` is the \
             only file left on the pinned path, because it is not a provider module"
        );
        assert_eq!(
            LIVE_CATALOGS.len(),
            38,
            "every `packages/ai/src/providers/<p>.models.ts` pi ships except `together` (hand-\
             ported), `meta` (no cyrup provider) and `radius` (catalog from its gateway)"
        );
        assert_eq!(
            CATALOGS.len() + LIVE_CATALOGS.len(),
            39,
            "the roster is 39 embedded catalog files across the two tables"
        );
        // Every live spec derives its endpoint and module from its stem, so a mismatched pair is
        // unspellable rather than merely detected — the reason `LiveCatalogSpec` collapsed three
        // hand-written fields into one.
        for l in LIVE_CATALOGS {
            assert_eq!(l.provider(), l.file);
            assert_eq!(
                l.url(),
                format!("https://pi.dev/api/models/providers/{}", l.file)
            );
            assert_eq!(
                l.module(),
                format!("packages/ai/src/providers/{}.models.ts", l.file)
            );
            assert!(!l.item.is_empty(), "{}: no ledger item", l.file);
        }
        // Every name is a real file on disk. This is what catches a table entry whose catalog was
        // never generated — the failure mode DRIFT-009's four spent nine sweeps in.
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../crates/cyrup-provider/src/providers/catalog");
        for name in CATALOGS
            .iter()
            .map(|c| c.file)
            .chain(LIVE_CATALOGS.iter().map(|l| l.file))
        {
            let path = dir.join(format!("{name}.json"));
            let body = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert!(
                body.trim_start().starts_with('['),
                "{name}.json must be a JSON array of rows"
            );
            assert!(
                body.len() > 2,
                "{name}.json is an empty catalog — the table names a file nothing generated"
            );
        }
        let images: Vec<&str> = CATALOGS
            .iter()
            .filter(|c| c.images_provider.is_some())
            .map(|c| c.file)
            .collect();
        assert_eq!(images, vec!["openrouter-images"]);
    }

    /// The pinned path's one remaining entry names a module that is NOT a provider module, and
    /// that is the whole reason it is still on the pinned path (PROV-071).
    #[test]
    fn the_pinned_catalog_is_the_images_record() {
        let img = CATALOGS
            .iter()
            .find(|c| c.file == "openrouter-images")
            .unwrap();
        assert_eq!(img.module_path(), "image-models.generated.ts");
        assert_eq!(img.images_provider, Some("openrouter"));
        // And nothing else is on it. A provider catalog that reappeared here would be silently
        // frozen at `b0c2a90e` again, which is the state PROV-071 exists to have ended.
        assert!(
            CATALOGS.iter().all(|c| c.images_provider.is_some()),
            "a provider module is back on the pinned path, where no revision can yield newer rows"
        );
        // PROV-089: the images module is read at its own pin, whatever `--rev` says, and it is
        // the only catalog that carries one.
        assert_eq!(img.rev("b0c2a90e"), "v0.87.1");
        assert_eq!(img.rev("some-other-rev"), "v0.87.1");
        let own: Vec<&str> = CATALOGS
            .iter()
            .filter(|c| c.rev.is_some())
            .map(|c| c.file)
            .collect();
        assert_eq!(own, vec!["openrouter-images"]);
    }

    /// The `Drop` and `Set` fixtures these guard tests run against.
    ///
    /// [`DELTAS`] is empty — every entry it carried has converged, see that table — so the guards
    /// are exercised against a table declared here instead of against the const. That is not a
    /// weaker test: `apply_deltas_from` is the function the generator calls with `DELTAS`, and the
    /// next signed-off divergence will be a `Delta` of exactly this shape. A guard that could only
    /// run while the table happened to be populated would have quietly stopped running the day it
    /// emptied.
    const FIXTURE_DELTAS: &[Delta] = &[
        Delta {
            catalog: "groq",
            model: "qwen/qwen3-32b",
            key: "thinkingLevelMap",
            action: DeltaAction::Drop,
            why: "fixture",
        },
        Delta {
            catalog: "openai-codex",
            model: "gpt-5.6-sol",
            key: "contextWindow",
            action: DeltaAction::Set("272000"),
            why: "fixture",
        },
    ];

    fn deltas(catalog: &str, rows: Vec<Val>) -> Result<Vec<Val>, String> {
        apply_deltas_from(FIXTURE_DELTAS, catalog, rows)
    }

    /// A signed-off divergence that upstream has already dropped is a no-op nobody would be told
    /// about, so the generator must refuse rather than carry it.
    #[test]
    fn a_stale_delta_is_a_hard_error() {
        let rows = vec![Val::Obj(vec![(
            "id".into(),
            Val::Str("qwen/qwen3-32b".into()),
        )])];
        let err = deltas("groq", rows).unwrap_err();
        assert!(err.contains("must be deleted"), "{err}");

        let err = deltas("groq", Vec::new()).unwrap_err();
        assert!(err.contains("stale and must be re-decided"), "{err}");
    }

    #[test]
    fn a_live_delta_removes_exactly_its_key() {
        let rows = vec![Val::Obj(vec![
            ("id".into(), Val::Str("qwen/qwen3-32b".into())),
            ("thinkingLevelMap".into(), Val::Obj(vec![])),
            ("reasoning".into(), Val::Bool(true)),
        ])];
        let out = deltas("groq", rows).unwrap();
        assert_eq!(out.len(), 1);
        assert!(out[0].get("thinkingLevelMap").is_none());
        assert_eq!(out[0].get("reasoning"), Some(&Val::Bool(true)));
    }

    /// The `openai-codex` fixture every `Set`-pin test needs. Every value is `b0c2a90e`'s, so the
    /// pin is a real replacement rather than a no-op.
    fn codex_rows() -> Vec<Val> {
        vec![Val::Obj(vec![
            ("id".into(), Val::Str("gpt-5.6-sol".into())),
            ("contextWindow".into(), Val::Num("372000".into())),
            ("maxTokens".into(), Val::Num("128000".into())),
        ])]
    }

    /// The index of `gpt-5.6-sol` in [`codex_rows`] — the row these tests assert on.
    const SOL: usize = 0;

    /// A `Set` pin replaces the value in place and keeps its declaration position, so the emitted
    /// row still diffs against upstream's key order.
    #[test]
    fn a_set_pin_replaces_in_place() {
        let out = deltas("openai-codex", codex_rows()).unwrap();
        let Val::Obj(entries) = &out[SOL] else {
            panic!("object")
        };
        let keys: Vec<&str> = entries.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["id", "contextWindow", "maxTokens"]);
        assert_eq!(
            out[SOL].get("contextWindow"),
            Some(&Val::Num("272000".into()))
        );
    }

    /// A pin whose value upstream has since adopted is a no-op, and a pin on a key upstream does
    /// not set at all is an invention. Both must stop the generator rather than pass silently.
    #[test]
    fn a_no_op_or_inventing_pin_is_a_hard_error() {
        // Upstream has ADOPTED the pinned value: the exception is now a no-op. This is the guard
        // that emptied `DELTAS` — nine of its twelve entries hit exactly this error the first time
        // the generator ran against live rows.
        let mut already = codex_rows();
        already[SOL].set("contextWindow", Val::Num("272000".into()));
        let err = deltas("openai-codex", already).unwrap_err();
        assert!(err.contains("value upstream already has"), "{err}");

        // Upstream does not set the key at all: pinning it would invent data rather than diverge.
        let missing = vec![Val::Obj(vec![(
            "id".into(),
            Val::Str("gpt-5.6-sol".into()),
        )])];
        let err = deltas("openai-codex", missing).unwrap_err();
        assert!(err.contains("would be an invention"), "{err}");
    }

    /// A delta naming a model the revision no longer ships must stop the generator — proved on the
    /// `Set` family too, not just the `Drop` one. This is the OTHER error that emptied `DELTAS`:
    /// the three remaining entries named rows upstream had retired.
    #[test]
    fn a_set_pin_naming_a_dropped_model_is_a_hard_error() {
        let err = deltas("openai-codex", Vec::new()).unwrap_err();
        assert!(err.contains("stale and must be re-decided"), "{err}");
        assert!(err.contains("gpt-5.6-sol"), "{err}");
    }

    /// PROV-071 — the twelve signed-off divergences did not vanish, they INVERTED: each is now a
    /// [`Converged`] assertion over the rows the generator is about to write, and every one of them
    /// runs against the catalogs actually on disk.
    ///
    /// This is the test that would have caught the failure the first attempt at this change made.
    /// Moving the roster to `LIVE_CATALOGS` without routing `DELTAS`/`CONVERGED` through the live
    /// path drops all twelve silently — no diff, no error, no red test — and the GPT-5.6 price
    /// cut, the 272k Codex window and DRIFT-052's Fireworks compat block go back to being
    /// unguarded.
    #[test]
    fn every_converged_divergence_still_holds_in_the_shipped_catalogs() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../crates/cyrup-provider/src/providers/catalog");
        let mut catalogs: BTreeMap<&str, Vec<Val>> = BTreeMap::new();
        for entry in CONVERGED {
            if catalogs.contains_key(entry.catalog) {
                continue;
            }
            let path = dir.join(format!("{}.json", entry.catalog));
            let body = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let Val::Arr(rows) = tsdata::parse_json(&body).expect("catalog parses") else {
                panic!("{}: not an array", path.display())
            };
            catalogs.insert(entry.catalog, rows);
        }
        assert_eq!(
            catalogs.len(),
            5,
            "the twelve converged divergences span azure-openai-responses, fireworks, groq, \
             openai and openai-codex"
        );
        for (catalog, rows) in &catalogs {
            check_converged(catalog, rows)
                .unwrap_or_else(|e| panic!("a signed-off divergence regressed: {e}"));
        }

        // And the table is TOTAL over what it claims: every entry names a catalog that exists, and
        // the two kinds are both represented, so a future edit cannot quietly reduce it to one.
        assert_eq!(CONVERGED.len(), 12);
        assert_eq!(
            CONVERGED
                .iter()
                .filter(|c| matches!(c.expect, Expect::Retired { .. }))
                .count(),
            3,
            "groq's qwen/qwen3-32b and the two Fireworks glm-5p2 rows are retired upstream"
        );
        assert!(
            CONVERGED.iter().all(|c| !c.why.is_empty()),
            "every converged divergence must carry the reasoning that authorised it"
        );
    }

    /// Every `packages/ai/src/providers/*.models.ts` pi ships at `v0.87.1`, from
    /// `git -C tmp/pi ls-tree v0.87.1 --name-only packages/ai/src/providers/`. Hard-coded rather
    /// than shelled out for so the accounting is testable with no pi checkout — the checkout is
    /// `--roster`'s job, this is the decision's.
    ///
    /// It was `v0.84.4`'s 39 stems until PROV-071. The tag moved with the audit because two of the
    /// three UNPORTED entries — `meta` and `radius` — do not EXIST at v0.84.4, so an accounting
    /// pinned there could not exercise the rows that made the live `--roster v0.87.1` run fail.
    fn upstream_stems_at_v0_87_1() -> Vec<String> {
        [
            "amazon-bedrock",
            "ant-ling",
            "anthropic",
            "azure-openai-responses",
            "baseten",
            "cerebras",
            "cloudflare-ai-gateway",
            "cloudflare-workers-ai",
            "deepseek",
            "fireworks",
            "github-copilot",
            "google-vertex",
            "google",
            "groq",
            "huggingface",
            "kimi-coding",
            "meta",
            "minimax-cn",
            "minimax",
            "mistral",
            "moonshotai-cn",
            "moonshotai",
            "nvidia",
            "openai-codex",
            "openai",
            "opencode-go",
            "opencode",
            "openrouter",
            "qwen-token-plan-cn",
            "qwen-token-plan-individual",
            "qwen-token-plan",
            "radius",
            "together",
            "vercel-ai-gateway",
            "xai",
            "xiaomi-token-plan-ams",
            "xiaomi-token-plan-cn",
            "xiaomi-token-plan-sgp",
            "xiaomi",
            "zai-coding-cn",
            "zai",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect()
    }

    /// PROV-071's shortfall, stated as an assertion instead of prose: pi ships 41 provider modules
    /// at `v0.87.1`, and every one is accounted for by exactly one of the three tables.
    ///
    /// The shape of the accounting is what changed. DRIFT-009 made it 33 embedded + 5 live + 1
    /// unported. It is now **0 embedded + 38 live + 3 unported**: every provider module is
    /// live-fetched, because `git show` can recover none of them at any revision, and the three
    /// left over are the ones cyrup does not embed a catalog for at all — `together` (rows are Rust
    /// literals), `meta` (no cyrup provider) and `radius` (rows come from its gateway).
    ///
    /// `meta` and `radius` are the substance here, not bookkeeping: before this change
    /// `gen-catalogs --roster v0.87.1` FAILED with "pi ships 2 provider module(s) this generator
    /// has never heard of". That is the audit doing its job, and it had been failing unnoticed
    /// because nobody had run it at a tag newer than the ones both providers post-date.
    #[test]
    fn the_v0_87_1_provider_roster_is_fully_accounted_for() {
        let stems = upstream_stems_at_v0_87_1();
        assert_eq!(stems.len(), 41);
        let roster = account_for_roster(&stems).unwrap();
        assert_eq!(
            roster.embedded.len(),
            0,
            "no provider module is on the pinned path any more (PROV-071)"
        );
        assert_eq!(roster.live.len(), 38);
        assert!(roster.unported_absent.is_empty());

        // Asserted as the whole unported set, in upstream stem order, with each one's owner: a
        // provider quietly moving in or out of this list is the failure the audit exists for.
        let unported: Vec<(&str, &str)> = roster
            .unported_present
            .iter()
            .map(|u| (u.stem, u.item))
            .collect();
        assert_eq!(
            unported,
            [
                ("meta", "PROV-080"),
                ("radius", "PROV-014"),
                ("together", "PROV-060"),
            ]
        );
        assert!(
            roster
                .unported_present
                .iter()
                .all(|u| !u.why.is_empty() && !u.item.is_empty())
        );
        // Each absence has its OWN reason; collapsing them onto one would hide that `meta` is a
        // missing provider while `radius` is a present provider with another catalog source.
        let reasons: Vec<&UnportedReason> =
            roster.unported_present.iter().map(|u| &u.reason).collect();
        assert_eq!(
            reasons,
            [
                &UnportedReason::NoProvider,
                &UnportedReason::CatalogElsewhere,
                &UnportedReason::HandPorted,
            ]
        );
    }

    /// DRIFT-009 — each of the four ex-`DataNotInGit` modules is a `LIVE_CATALOGS` entry pointing at
    /// its OWN pi.dev endpoint, still citing the upstream module it originates from, and no longer
    /// named anywhere in [`UNPORTED`].
    ///
    /// The url/provider/file agreement is the assertion that matters: `refresh` writes
    /// `<file>.json` from whatever `<url>` answers, and `rows_from_body` validates the rows against
    /// `provider` — so a copy-paste that left one spec pointing at a neighbour's endpoint would
    /// write one provider's rows into another's catalog file, and the fetch-time provider check
    /// would not notice because the tag it compares against came from the same wrong spec.
    #[test]
    fn drift_009s_four_modules_are_live_catalog_specs() {
        for stem in [
            "baseten",
            "qwen-token-plan",
            "qwen-token-plan-cn",
            "qwen-token-plan-individual",
        ] {
            let spec = LIVE_CATALOGS
                .iter()
                .find(|l| l.file == stem)
                .unwrap_or_else(|| panic!("{stem} must be a LIVE_CATALOGS entry (DRIFT-009)"));
            assert_eq!(spec.provider(), stem, "{stem}: file and provider id agree");
            assert_eq!(
                spec.url(),
                format!("https://pi.dev/api/models/providers/{stem}"),
                "{stem} must fetch its OWN endpoint"
            );
            assert_eq!(
                spec.module(),
                format!("packages/ai/src/providers/{stem}.models.ts"),
                "{stem} still originates from its upstream module, re-export or not"
            );
            assert_eq!(spec.item, "DRIFT-009");
            assert!(
                !UNPORTED.iter().any(|u| u.stem == stem),
                "{stem} is live-fetched now and must not also be declared unported"
            );
        }
        assert_eq!(
            LIVE_CATALOGS.len(),
            38,
            "the four, plus xai (XAI_1), plus the 33 PROV-071 moved across"
        );
        assert_eq!(
            LIVE_CATALOGS
                .iter()
                .filter(|l| l.item == "DRIFT-009")
                .count(),
            4,
            "the four keep the ledger id that unblocked them, not PROV-071's"
        );
    }

    /// `CYRUP_XTASK_SKIP_LIVE=1` must still degrade every live spec to `Skipped` — an offline
    /// maintainer regenerating the 34 pinned catalogs is not blocked by the five live ones, and
    /// DRIFT-009 multiplying those five by five did not change that.
    ///
    /// The `fetch` seam is passed a closure that PANICS, which is how "without invoking `fetch` at
    /// all" is proven rather than asserted. The skip decision goes in through
    /// [`live_catalog::refresh_with`] because the workspace lints deny `std::env::set_var`; the
    /// variable's own name is pinned separately below, so the two halves together cover what
    /// `refresh` does.
    #[test]
    fn skip_live_degrades_every_spec_without_fetching() {
        let outcomes = live_catalog::refresh_with(
            LIVE_CATALOGS,
            &|url| {
                panic!(
                    "{} must short-circuit before any fetch, but {url} was hit",
                    live_catalog::SKIP_LIVE_ENV
                )
            },
            true,
        )
        .expect("skipping is never an error");
        assert_eq!(outcomes.len(), LIVE_CATALOGS.len());
        for (spec, outcome) in &outcomes {
            match outcome {
                live_catalog::LiveOutcome::Skipped { why } => {
                    assert!(
                        why.contains(live_catalog::SKIP_LIVE_ENV),
                        "{}: the skip notice must name the variable that caused it, got {why}",
                        spec.file
                    );
                }
                live_catalog::LiveOutcome::Fetched { .. } => {
                    panic!("{} was fetched despite skip_all", spec.file)
                }
            }
        }

        // The other half: `skip_all` really is what the environment decides, under the name the
        // `gen-catalogs` usage block documents. Read-only, so no `set_var` is needed.
        assert_eq!(live_catalog::SKIP_LIVE_ENV, "CYRUP_XTASK_SKIP_LIVE");
        assert_eq!(
            live_catalog::skip_live_requested(),
            std::env::var_os("CYRUP_XTASK_SKIP_LIVE").is_some()
        );
    }

    /// The failure this whole table exists to catch: pi adds a provider and nobody files a catalog
    /// for it. Silence here is how DRIFT-009 stayed four catalogs stale across nine sweeps.
    #[test]
    fn an_unaccounted_upstream_module_is_a_hard_error() {
        let mut stems = upstream_stems_at_v0_87_1();
        stems.push("brand-new-provider".to_string());
        let err = account_for_roster(&stems).unwrap_err();
        assert!(err.contains("brand-new-provider"), "{err}");
        assert!(err.contains("never heard of"), "{err}");
    }

    /// The mirror failure: cyrup keeps embedding a catalog upstream has retired.
    #[test]
    fn an_embedded_catalog_pi_no_longer_ships_is_a_hard_error() {
        let stems: Vec<String> = upstream_stems_at_v0_87_1()
            .into_iter()
            .filter(|s| s != "zai")
            .collect();
        let err = account_for_roster(&stems).unwrap_err();
        assert!(err.contains("zai"), "{err}");
        assert!(err.contains("does not ship"), "{err}");
    }

    /// A stem in more than one table would make the accounting ambiguous and hide whichever
    /// branch lost. Three tables now that `LIVE_CATALOGS` exists (XAI_1), so the check is pairwise.
    ///
    /// DRIFT-009 added the other half: EXACTLY ONE table, not at most one. A pairwise-disjointness
    /// check passes for a stem that is in no table at all, which is precisely what a half-finished
    /// move looks like — deleted from `UNPORTED`, not yet added to `LIVE_CATALOGS` — and
    /// `account_for_roster` would then report it as a provider it has "never heard of" only when
    /// somebody remembered to run `--roster`.
    #[test]
    fn every_upstream_stem_is_in_exactly_one_roster_table() {
        for stem in upstream_stems_at_v0_87_1() {
            let n = usize::from(
                CATALOGS
                    .iter()
                    .any(|c| c.images_provider.is_none() && c.file == stem),
            ) + usize::from(LIVE_CATALOGS.iter().any(|l| l.file == stem))
                + usize::from(UNPORTED.iter().any(|u| u.stem == stem));
            assert_eq!(
                n, 1,
                "{stem} is in {n} roster tables; every upstream module must be in exactly one"
            );
        }
    }

    #[test]
    fn the_two_roster_tables_are_disjoint() {
        for u in UNPORTED {
            assert!(
                !CATALOGS
                    .iter()
                    .any(|c| c.images_provider.is_none() && c.file == u.stem),
                "{} is in both CATALOGS and UNPORTED",
                u.stem
            );
            assert!(
                !LIVE_CATALOGS.iter().any(|l| l.file == u.stem),
                "{} is in both LIVE_CATALOGS and UNPORTED",
                u.stem
            );
        }
        for l in LIVE_CATALOGS {
            assert!(
                !CATALOGS
                    .iter()
                    .any(|c| c.images_provider.is_none() && c.file == l.file),
                "{} is in both CATALOGS and LIVE_CATALOGS",
                l.file
            );
        }
    }

    /// Upstream's differ compares canonicalized JSON (`diff-model-catalog.mjs:195`), so moving a
    /// key is not a data change. Before this port, `field_diff` compared `Val`s whose object
    /// equality is order-sensitive and reported a reordered `compat` block as a difference on
    /// every model in the catalog.
    #[test]
    fn key_order_alone_is_not_a_field_difference() {
        let a = Val::Obj(vec![
            ("id".into(), Val::Str("m".into())),
            (
                "compat".into(),
                Val::Obj(vec![
                    ("supportsStore".into(), Val::Bool(false)),
                    ("maxTokensField".into(), Val::Str("max_tokens".into())),
                ]),
            ),
        ]);
        let b = Val::Obj(vec![
            (
                "compat".into(),
                Val::Obj(vec![
                    ("maxTokensField".into(), Val::Str("max_tokens".into())),
                    ("supportsStore".into(), Val::Bool(false)),
                ]),
            ),
            ("id".into(), Val::Str("m".into())),
        ]);
        assert_eq!(field_diff(&a, &b), Vec::<String>::new());
    }

    /// `sortJsonKeys` (`diff-model-catalog.mjs:96-103`) orders a `thinkingLevelMap` by the ladder,
    /// not alphabetically, and everything else alphabetically.
    #[test]
    fn thinking_level_maps_canonicalize_in_ladder_order() {
        let row = Val::Obj(vec![
            ("reasoning".into(), Val::Bool(true)),
            ("api".into(), Val::Str("openai-completions".into())),
            (
                "thinkingLevelMap".into(),
                Val::Obj(vec![
                    ("max".into(), Val::Str("max".into())),
                    ("off".into(), Val::Str("none".into())),
                    ("high".into(), Val::Str("high".into())),
                    ("zzz-not-a-level".into(), Val::Null),
                    ("low".into(), Val::Null),
                ]),
            ),
        ]);
        let Val::Obj(top) = canonicalize(&row, "") else {
            panic!("object")
        };
        let keys: Vec<&str> = top.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["api", "reasoning", "thinkingLevelMap"]);

        let Some((_, Val::Obj(map))) = top.iter().find(|(k, _)| k == "thinkingLevelMap") else {
            panic!("thinkingLevelMap")
        };
        let levels: Vec<&str> = map.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            levels,
            vec!["off", "low", "high", "max", "zzz-not-a-level"],
            "unranked keys sort after every ladder key"
        );
    }

    /// `canonicalizeJson(entry)` is called with ONE argument for array entries (`:106`), so a map
    /// nested in an array does not inherit the ladder. Ported literally; pinned so a later tidy-up
    /// cannot silently diverge from upstream's canonical form.
    #[test]
    fn array_entries_canonicalize_without_the_parent_key() {
        let v = Val::Obj(vec![(
            "thinkingLevelMap".into(),
            Val::Arr(vec![Val::Obj(vec![
                ("max".into(), Val::Null),
                ("off".into(), Val::Null),
            ])]),
        )]);
        let Val::Obj(top) = canonicalize(&v, "") else {
            panic!("object")
        };
        let Some((_, Val::Arr(items))) = top.first() else {
            panic!("array")
        };
        let Some(Val::Obj(entry)) = items.first() else {
            panic!("entry")
        };
        let keys: Vec<&str> = entry.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["max", "off"], "alphabetical, not ladder order");
    }

    /// The union walk (`diff-model-catalog.mjs:191`): one pass in sorted id order, so an added row
    /// and a changed row that sort next to each other are reported next to each other.
    #[test]
    fn diff_catalog_walks_the_sorted_union() {
        let row = |name: &str| Val::Obj(vec![("name".into(), Val::Str(name.into()))]);
        let current: BTreeMap<String, Val> = [
            ("a".to_string(), row("a")),
            ("b".to_string(), row("old")),
            ("d".to_string(), row("d")),
        ]
        .into_iter()
        .collect();
        let next: BTreeMap<String, Val> = [
            ("a".to_string(), row("a")),
            ("b".to_string(), row("new")),
            ("c".to_string(), row("c")),
        ]
        .into_iter()
        .collect();

        let seen: Vec<String> = diff_catalog(&current, &next)
            .into_iter()
            .map(|d| match d {
                RowDiff::Added(id) => format!("+{id}"),
                RowDiff::Removed(id) => format!("-{id}"),
                RowDiff::Changed(id, lines) => format!("~{id}:{}", lines.len()),
            })
            .collect();
        assert_eq!(
            seen,
            vec!["~b:1", "+c", "-d"],
            "`a` is unchanged and silent"
        );
    }

    #[test]
    fn field_diff_reports_both_directions() {
        let a = Val::Obj(vec![
            ("api".into(), Val::Str("openai-completions".into())),
            ("gone".into(), Val::Bool(true)),
        ]);
        let b = Val::Obj(vec![
            ("api".into(), Val::Str("openai-responses".into())),
            ("added".into(), Val::Num("1".into())),
        ]);
        let lines = field_diff(&a, &b);
        assert!(
            lines
                .iter()
                .any(|l| l.contains("api: \"openai-completions\" -> \"openai-responses\"")),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("added: (absent) -> 1")),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("gone: true -> (absent)")),
            "{lines:?}"
        );
    }
}

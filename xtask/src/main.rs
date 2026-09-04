//! `xtask` — the repo's own tooling. Two commands, both run by hand (there is no CI here; see
//! README "Build"):
//!
//! * `gen-catalogs` — regenerate `crates/cyrup-provider/src/providers/catalog/*.json` and
//!   `catalog_manifest.json` from a pinned pi revision (PROV-018 / PROV-060). Documented below.
//! * `feature-matrix` — type-check the feature combinations `cargo check --workspace
//!   --all-targets` does not reach. See [`features`] for the matrix and why each row is in it.
//!
//! # `gen-catalogs`
//!
//! # Why a `git show` extractor and not pi's own generator
//!
//! PROV-018's original Fix said to run pi's `npm run generate-models`, because "the tree can no
//! longer simply be read". That premise is FALSE and PROV-060 refutes it: pi gitignores
//! `packages/ai/src/providers/data/` (`pi/.gitignore:11`) only from `a9f6a3159`
//! (`feat(ai): separate generated model data (#6765)`) onward. At its direct parent `b0c2a90e` —
//! precisely the revision `catalog_manifest.json` already names as its provenance floor — every
//! `packages/ai/src/providers/<p>.models.ts` is still a full data literal. So the whole catalog is
//! recoverable with `git show` plus [`tsdata`], with no `npm install`, no generator run and no
//! network. This binary is that recipe, and `--check` is PROV-018's drift check.
//!
//! # What it is NOT allowed to do
//!
//! Regeneration must be **total and accounted for**: it rewrites every catalog from one revision,
//! it refuses to run if any module is missing, and the only rows it is permitted to diverge from
//! upstream on are the ones listed in [`DELTAS`] — each of which names the ledger item that
//! authorised it. A generator with a silent skip is how catalog data goes missing without a diff.
//!
//! # Usage
//!
//! ```text
//! cargo run -p xtask -- gen-catalogs [--pi <path>] [--rev <rev>] [--out <dir>] [--check] [--diff]
//! cargo run -p xtask -- feature-matrix [--fast]
//! ```
//!
//! * `--check` — generate in memory and compare byte-for-byte with what is on disk; exit 1 on any
//!   difference. This is the drift check: point it at a newer pi and it fails.
//! * `--diff` — print a **structural** (model-level and field-level) diff of on-disk vs generated
//!   instead of writing anything. Whitespace-insensitive, so it reports only real data movement.

mod features;
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
struct CatalogSpec {
    /// `providers/catalog/<file>.json`.
    file: &'static str,
    /// Path under `packages/ai/src/` of the module that declares the rows.
    module: &'static str,
    /// `Some(provider)` when the module binds a `provider -> id -> Model` record and only one
    /// provider's sub-record belongs in this catalog (`image-models.generated.ts`, PROV-065).
    /// `None` when it binds a flat `id -> Model` record, which is every `<p>.models.ts`.
    images_provider: Option<&'static str>,
}

/// The 35 embedded catalogs, each bound to its upstream source module.
///
/// This is 34 of pi's 35 `*.models.ts` modules plus `openrouter-images.json`. The two asymmetries
/// are deliberate and both are ledgered:
///
/// * `together.models.ts` has **no** catalog file — cyrup hand-ports Together's 20 rows as Rust
///   literals in `providers/together.rs::together_models()`, so this generator cannot own them.
/// * `openrouter-images.json` has no `*.models.ts` counterpart (PROV-065); its rows are the
///   `openrouter` sub-record of `packages/ai/src/image-models.generated.ts`.
const CATALOGS: &[CatalogSpec] = &[
    spec("amazon-bedrock"),
    spec("ant-ling"),
    spec("anthropic"),
    spec("azure-openai-responses"),
    spec("cerebras"),
    spec("cloudflare-ai-gateway"),
    spec("cloudflare-workers-ai"),
    spec("deepseek"),
    spec("fireworks"),
    spec("github-copilot"),
    spec("google-vertex"),
    spec("google"),
    spec("groq"),
    spec("huggingface"),
    spec("kimi-coding"),
    spec("minimax-cn"),
    spec("minimax"),
    spec("mistral"),
    spec("moonshotai-cn"),
    spec("moonshotai"),
    spec("nvidia"),
    spec("openai-codex"),
    spec("openai"),
    spec("opencode-go"),
    spec("opencode"),
    CatalogSpec {
        file: "openrouter-images",
        module: "image-models.generated.ts",
        images_provider: Some("openrouter"),
    },
    spec("openrouter"),
    spec("vercel-ai-gateway"),
    spec("xai"),
    spec("xiaomi-token-plan-ams"),
    spec("xiaomi-token-plan-cn"),
    spec("xiaomi-token-plan-sgp"),
    spec("xiaomi"),
    spec("zai-coding-cn"),
    spec("zai"),
];

const fn spec(name: &'static str) -> CatalogSpec {
    CatalogSpec {
        file: name,
        module: "",
        images_provider: None,
    }
}

impl CatalogSpec {
    /// Path under `packages/ai/src/` of this catalog's source module.
    fn module_path(&self) -> String {
        if self.module.is_empty() {
            format!("providers/{}.models.ts", self.file)
        } else {
            self.module.to_string()
        }
    }
}

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

const DELTAS: &[Delta] = &[
    Delta {
        catalog: "groq",
        model: "qwen/qwen3-32b",
        key: "thinkingLevelMap",
        action: DeltaAction::Drop,
        why: "[CYRUP-DELTA] pi `GROQ_MODELS[\"qwen/qwen3-32b\"].thinkingLevelMap` @b0c2a90e. \
              PROV-064: v0.84.1 retargeted the sole Groq thinking-level override from \
              `qwen/qwen3-32b` (v0.83.0 `ai/scripts/generate-models.ts:837`) to `qwen/qwen3.6-27b` \
              (v0.84.1 `:870`); cyrup adopted the newer behaviour and `providers/fleet.rs` \
              `groq_qwen3_32b_no_longer_carries_the_retargeted_thinking_level_map` pins it.",
    },
    // ---- the three openai-codex contextWindows: b0c2a90e is simply WRONG for the ported tag ----
    //
    // PROV-059(d) claims cyrup understates these by 100k. It is REFUTED. `CODEX_GPT_56_CONTEXT` is
    // `272000` at BOTH v0.83.0 (`ai/scripts/generate-models.ts:2352`) and v0.84.1 (`:2541`), and
    // the comment one line above it at v0.83.0 says so in words: "GPT-5.6 follows Codex's 272k
    // catalog limit (formerly 372k)". `372000` is the FORMER value, which is what b0c2a90e's
    // generated data still held 13 days before the ported tag. Taking b0c2a90e here would inflate
    // the window past the real limit and defer compaction past it.
    Delta {
        catalog: "openai-codex",
        model: "gpt-5.6-luna",
        key: "contextWindow",
        action: DeltaAction::Set("272000"),
        why: WHY_CODEX_CONTEXT,
    },
    Delta {
        catalog: "openai-codex",
        model: "gpt-5.6-sol",
        key: "contextWindow",
        action: DeltaAction::Set("272000"),
        why: WHY_CODEX_CONTEXT,
    },
    Delta {
        catalog: "openai-codex",
        model: "gpt-5.6-terra",
        key: "contextWindow",
        action: DeltaAction::Set("272000"),
        why: WHY_CODEX_CONTEXT,
    },
    // ---- the six GPT-5.6 cost rows: a signed-off v0.84.1 forward-port ----
    Delta {
        catalog: "openai-codex",
        model: "gpt-5.6-luna",
        key: "cost",
        action: DeltaAction::Set(GPT_56_LUNA_COST),
        why: WHY_GPT_56_PRICE_CUT,
    },
    Delta {
        catalog: "openai-codex",
        model: "gpt-5.6-terra",
        key: "cost",
        action: DeltaAction::Set(GPT_56_TERRA_COST),
        why: WHY_GPT_56_PRICE_CUT,
    },
    Delta {
        catalog: "openai",
        model: "gpt-5.6-luna",
        key: "cost",
        action: DeltaAction::Set(GPT_56_LUNA_COST),
        why: WHY_GPT_56_PRICE_CUT,
    },
    Delta {
        catalog: "openai",
        model: "gpt-5.6-terra",
        key: "cost",
        action: DeltaAction::Set(GPT_56_TERRA_COST),
        why: WHY_GPT_56_PRICE_CUT,
    },
    Delta {
        catalog: "azure-openai-responses",
        model: "gpt-5.6-luna",
        key: "cost",
        action: DeltaAction::Set(GPT_56_LUNA_COST_NO_TIERS),
        why: WHY_GPT_56_PRICE_CUT,
    },
    Delta {
        catalog: "azure-openai-responses",
        model: "gpt-5.6-terra",
        key: "cost",
        action: DeltaAction::Set(GPT_56_TERRA_COST_NO_TIERS),
        why: WHY_GPT_56_PRICE_CUT,
    },
    // ---- the two Fireworks GLM openai-completions rows: DRIFT-052, a v0.84.0 forward-port ----
    Delta {
        catalog: "fireworks",
        model: "accounts/fireworks/models/glm-5p2",
        key: "compat",
        action: DeltaAction::Set(FIREWORKS_OPENAI_COMPAT),
        why: WHY_FIREWORKS_GLM_COMPAT,
    },
    Delta {
        catalog: "fireworks",
        model: "accounts/fireworks/routers/glm-5p2-fast",
        key: "compat",
        action: DeltaAction::Set(FIREWORKS_OPENAI_COMPAT),
        why: WHY_FIREWORKS_GLM_COMPAT,
    },
];

/// pi's `openAICompat` for Fireworks (`ai/scripts/generate-models.ts:1239-1244` @v0.84.2), spelled
/// in upstream's own key order so the pinned object diffs against the source declaration.
const FIREWORKS_OPENAI_COMPAT: &str = r#"{"supportsStore":false,"supportsDeveloperRole":false,
     "sendSessionAffinityHeaders":true,"supportsLongCacheRetention":false}"#;

/// DRIFT-052's rationale, shared by the two Fireworks GLM compat pins.
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
        other => Err(format!(
            "unknown command {other:?} — commands are `gen-catalogs` and `feature-matrix` \
             (see each one's module docs for flags)"
        )),
    }
}

fn run_gen_catalogs() -> Result<(), String> {
    let args = parse_args()?;
    let generated = generate_all(&args)?;

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
            println!(
                "gen-catalogs --check: all {} files match pi@{}",
                generated.len(),
                args.rev
            );
            return Ok(());
        }
        return Err(format!(
            "{} file(s) differ from pi@{}: {}\nrun `cargo run -p xtask -- gen-catalogs` to refresh, \
             and account for every change in docs/gap-analysis/01-cyrup-core-and-provider.md",
            differing.len(),
            args.rev,
            differing.join(", ")
        ));
    }

    println!(
        "gen-catalogs: wrote {} of {} files from pi@{}",
        differing.len(),
        generated.len(),
        args.rev
    );
    for name in &differing {
        println!("  updated {name}");
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
fn generate_all(args: &Args) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for spec in CATALOGS {
        let src = git_show(
            &args.pi,
            &args.rev,
            &format!("packages/ai/src/{}", spec.module_path()),
        )?;
        let rows = extract_rows(spec, &src)?;
        let rows = apply_deltas(spec, rows)?;
        let mut body = Val::Arr(rows).to_json();
        body.push('\n');
        out.push((spec.file.to_string(), body));
    }
    out.push((
        "catalog_manifest".to_string(),
        manifest_json(args, out.len()),
    ));
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
fn apply_deltas(spec: &CatalogSpec, mut rows: Vec<Val>) -> Result<Vec<Val>, String> {
    for delta in DELTAS.iter().filter(|d| d.catalog == spec.file) {
        let row = rows
            .iter_mut()
            .find(|r| r.get("id").and_then(Val::as_str) == Some(delta.model))
            .ok_or_else(|| {
                format!(
                    "{}: DELTAS names model `{}`, which pi@this revision no longer ships — \
                     the exception is stale and must be re-decided, not carried. ({})",
                    spec.file, delta.model, delta.why
                )
            })?;
        match delta.action {
            DeltaAction::Drop => {
                if !row.remove(delta.key) {
                    return Err(format!(
                        "{}: DELTAS drops `{}` from `{}`, but upstream no longer sets it — the \
                         exception is a no-op and must be deleted. ({})",
                        spec.file, delta.key, delta.model, delta.why
                    ));
                }
            }
            DeltaAction::Set(json) => {
                let pinned = tsdata::parse_json(json).map_err(|e| {
                    format!(
                        "{}: DELTAS pin for {}.{} is not JSON: {e}",
                        spec.file, delta.model, delta.key
                    )
                })?;
                let upstream = row.get(delta.key).ok_or_else(|| {
                    format!(
                        "{}: DELTAS pins `{}` on `{}`, but upstream does not set that key at all — \
                         the pin would be an invention, not a divergence. ({})",
                        spec.file, delta.key, delta.model, delta.why
                    )
                })?;
                if upstream == &pinned {
                    return Err(format!(
                        "{}: DELTAS pins `{}` on `{}` to the value upstream already has — the \
                         exception is a no-op and must be deleted. ({})",
                        spec.file, delta.key, delta.model, delta.why
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
/// described by one value. After this generator runs there is no split — every catalog comes from
/// one revision — but the map is emitted anyway, because the absence of a split is exactly the
/// claim that needs to be machine-checkable.
fn manifest_json(args: &Args, catalog_count: usize) -> String {
    let source = format!("pi@{}", args.rev);
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

    let note = format!(
        "Machine-readable counterpart of the provenance prose in src/tests/catalog_data.rs. All \
         {catalog_count} embedded catalogs under providers/catalog/*.json are generated by \
         `cargo run -p xtask -- gen-catalogs` from a SINGLE pi revision — {source} \
         ({generated_at}) — so `generatedAt` and `source` describe every file, and `catalogs` below \
         records the per-provider source module so a future split cannot be hidden behind one value \
         (PROV-060). Re-run `cargo run -p xtask -- gen-catalogs --check` to prove it. `generatedAt` \
         is the staleness floor for the pi.dev overlay (DRIFT-007): a persisted remote catalog whose \
         Last-Modified is not strictly newer than this is discarded whole, so upgrading cyrup can \
         never leave a pre-upgrade overlay shadowing freshly refreshed embedded data. PROV-039: the \
         value must be the LATEST extraction revision. IRREDUCIBLE RESIDUE (PROV-060), stated here \
         and not only in the ledger: b0c2a90e is 13 days EARLIER than the ported tag v0.83.0 \
         (2026-07-30). From a9f6a3159 (b0c2a90e's direct child) onward pi gitignores \
         packages/ai/src/providers/data/ and every *.models.ts is a two-line re-export, so the \
         catalog data for that 13-day window is not in git at any tag and is NOT measurable from a \
         checkout. Any claim of catalog parity at v0.83.0 is a claim about b0c2a90e plus an \
         unbounded delta. EXCEPTIONS: providers/together.rs hand-ports Together's rows as Rust \
         literals and has no file here; and {delta_count} signed-off ROW divergences from \
         {source} are carried by the generator's DELTAS table, which is the complete list — \
         {delta_summary}. KNOWN INCOMPLETENESS in the GPT-5.6 price-cut forward-port, recorded so a \
         reader does not mistake the current state for a decision: at v0.84.1 upstream applies \
         OPENAI_GPT_56_STANDARD_COSTS to FOUR provider families — openai, the derived azure clone, \
         openai-codex, and cloudflare-ai-gateway ('Cloudflare AI Gateway passes OpenAI usage through \
         at OpenAI list prices', ai/scripts/generate-models.ts:2311-2315 @v0.84.1) — but cyrup \
         forward-ported only the first three. cloudflare-ai-gateway's gpt-5.6-luna/terra rows \
         therefore still carry b0c2a90e's PRE-cut rates, so the same model is priced 5x (Luna) / \
         1.25x (Terra) higher on that route than on the other three. Completing or reverting the \
         forward-port is an owner decision; it must move all four families together."
    );

    let mut catalogs: Vec<(String, Val)> = Vec::new();
    for spec in CATALOGS {
        catalogs.push((
            spec.file.to_string(),
            Val::Obj(vec![
                ("source".to_string(), Val::Str(source.clone())),
                (
                    "module".to_string(),
                    Val::Str(format!("packages/ai/src/{}", spec.module_path())),
                ),
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
    body
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

        for (id, row) in &next {
            match current.get(id) {
                None => {
                    println!("{name}: + {id}  (absent in cyrup, present upstream)");
                    added += 1;
                }
                Some(cur) => {
                    for line in field_diff(cur, row) {
                        println!("{name}: ~ {id}  {line}");
                        changed += 1;
                    }
                }
            }
        }
        for id in current.keys() {
            if !next.contains_key(id) {
                println!("{name}: - {id}  (shipped by cyrup, retired upstream)");
                removed += 1;
            }
        }
    }
    println!("\ntotals: {added} missing rows, {removed} retired rows, {changed} field differences");
    Ok(())
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
fn field_diff(cur: &Val, next: &Val) -> Vec<String> {
    let (Val::Obj(a), Val::Obj(b)) = (cur, next) else {
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
    #[test]
    fn the_catalog_roster_is_the_35_embedded_files() {
        assert_eq!(CATALOGS.len(), 35);
        let images: Vec<&str> = CATALOGS
            .iter()
            .filter(|c| c.images_provider.is_some())
            .map(|c| c.file)
            .collect();
        assert_eq!(images, vec!["openrouter-images"]);
    }

    #[test]
    fn module_paths_default_to_the_provider_models_module() {
        let xai = CATALOGS.iter().find(|c| c.file == "xai").unwrap();
        assert_eq!(xai.module_path(), "providers/xai.models.ts");
        let img = CATALOGS
            .iter()
            .find(|c| c.file == "openrouter-images")
            .unwrap();
        assert_eq!(img.module_path(), "image-models.generated.ts");
    }

    /// A signed-off divergence that upstream has already dropped is a no-op nobody would be told
    /// about, so the generator must refuse rather than carry it.
    #[test]
    fn a_stale_delta_is_a_hard_error() {
        let spec = spec("groq");
        let rows = vec![Val::Obj(vec![(
            "id".into(),
            Val::Str("qwen/qwen3-32b".into()),
        )])];
        let err = apply_deltas(&spec, rows).unwrap_err();
        assert!(err.contains("must be deleted"), "{err}");

        let err = apply_deltas(&spec, Vec::new()).unwrap_err();
        assert!(err.contains("stale and must be re-decided"), "{err}");
    }

    #[test]
    fn a_live_delta_removes_exactly_its_key() {
        let spec = spec("groq");
        let rows = vec![Val::Obj(vec![
            ("id".into(), Val::Str("qwen/qwen3-32b".into())),
            ("thinkingLevelMap".into(), Val::Obj(vec![])),
            ("reasoning".into(), Val::Bool(true)),
        ])];
        let out = apply_deltas(&spec, rows).unwrap();
        assert_eq!(out.len(), 1);
        assert!(out[0].get("thinkingLevelMap").is_none());
        assert_eq!(out[0].get("reasoning"), Some(&Val::Bool(true)));
    }

    /// The `openai-codex` fixture every `Set`-pin test needs: `apply_deltas` walks EVERY delta for
    /// the catalog, and a delta whose model is absent from the rows is a hard "stale and must be
    /// re-decided" error. `openai-codex` carries five (three `contextWindow` pins plus the luna and
    /// terra `cost` pins), so a fixture holding only the row under test fails on the FIRST unrelated
    /// delta and never reaches the assertion — which is exactly how these two tests were failing.
    /// Every value here is `b0c2a90e`'s, so each pin is a real replacement rather than a no-op.
    fn codex_rows() -> Vec<Val> {
        let luna_cost = tsdata::parse_json(
            r#"{"input":1,"output":6,"cacheRead":0.1,"cacheWrite":1.25,
                "tiers":[{"inputTokensAbove":272000,"input":2,"output":9,"cacheRead":0.2,"cacheWrite":2.5}]}"#,
        )
        .expect("luna cost fixture parses");
        let terra_cost = tsdata::parse_json(
            r#"{"input":2.5,"output":15,"cacheRead":0.25,"cacheWrite":3.125,
                "tiers":[{"inputTokensAbove":272000,"input":5,"output":22.5,"cacheRead":0.5,"cacheWrite":6.25}]}"#,
        )
        .expect("terra cost fixture parses");
        vec![
            Val::Obj(vec![
                ("id".into(), Val::Str("gpt-5.6-luna".into())),
                ("contextWindow".into(), Val::Num("372000".into())),
                ("cost".into(), luna_cost),
            ]),
            Val::Obj(vec![
                ("id".into(), Val::Str("gpt-5.6-terra".into())),
                ("contextWindow".into(), Val::Num("372000".into())),
                ("cost".into(), terra_cost),
            ]),
            Val::Obj(vec![
                ("id".into(), Val::Str("gpt-5.6-sol".into())),
                ("contextWindow".into(), Val::Num("372000".into())),
                ("maxTokens".into(), Val::Num("128000".into())),
            ]),
        ]
    }

    /// The index of `gpt-5.6-sol` in [`codex_rows`] — the row these tests assert on.
    const SOL: usize = 2;

    /// A `Set` pin replaces the value in place and keeps its declaration position, so the emitted
    /// row still diffs against upstream's key order.
    #[test]
    fn a_set_pin_replaces_in_place() {
        let spec = spec("openai-codex");
        let rows = codex_rows();
        let out = apply_deltas(&spec, rows).unwrap();
        let out = [out[SOL].clone()];
        let Val::Obj(entries) = &out[0] else {
            panic!("object")
        };
        let keys: Vec<&str> = entries.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["id", "contextWindow", "maxTokens"]);
        assert_eq!(
            out[0].get("contextWindow"),
            Some(&Val::Num("272000".into()))
        );
    }

    /// A pin whose value upstream has since adopted is a no-op, and a pin on a key upstream does
    /// not set at all is an invention. Both must stop the generator rather than pass silently.
    #[test]
    fn a_no_op_or_inventing_pin_is_a_hard_error() {
        let spec = spec("openai-codex");

        // Upstream has ADOPTED the pinned value: the exception is now a no-op.
        let mut already = codex_rows();
        already[SOL].set("contextWindow", Val::Num("272000".into()));
        let err = apply_deltas(&spec, already).unwrap_err();
        assert!(err.contains("value upstream already has"), "{err}");

        // Upstream does not set the key at all: pinning it would invent data rather than diverge.
        let mut missing = codex_rows();
        missing[SOL] = Val::Obj(vec![("id".into(), Val::Str("gpt-5.6-sol".into()))]);
        let err = apply_deltas(&spec, missing).unwrap_err();
        assert!(err.contains("would be an invention"), "{err}");
    }

    /// A delta naming a model the revision no longer ships must stop the generator — proved on the
    /// `Set` family too, not just the `Drop` one, since `Set` is where the GPT-5.6 pins live.
    #[test]
    fn a_set_pin_naming_a_dropped_model_is_a_hard_error() {
        let spec = spec("openai-codex");
        let mut rows = codex_rows();
        rows.remove(SOL);
        let err = apply_deltas(&spec, rows).unwrap_err();
        assert!(err.contains("stale and must be re-decided"), "{err}");
        assert!(err.contains("gpt-5.6-sol"), "{err}");
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

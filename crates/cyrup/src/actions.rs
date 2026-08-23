//! The standalone **run-and-exit actions**: the flags that print something and return an exit
//! code without ever building a [`cyrup_session_svc::SessionConfig`], a provider, a session or a
//! runtime.
//!
//! Pi keeps the same set together at the top of `main()` — `exportFromFile` (main.ts:520-531) and
//! `listModels` (`cli/list-models.ts`) both run before `createSessionManager` and `process.exit`
//! straight out. Their position in the startup sequence is load-bearing and is documented at the
//! call sites in `main.rs`; what lives here is only the work each one does.
//!
//! `--version` and `--help` are deliberately NOT here: they are a single `println!` each and adding
//! an indirection for them would obscure rather than clarify the sequence.

use anyhow::Context;

/// `--export <file> [output.html]` (Pi `exportFromFile`, main.ts:520-531): read a session `.jsonl`,
/// render it to standalone HTML, write it, and exit. The optional second positional is the output
/// path (else `<input-stem>.html`). On success prints `Exported to: {path}`; on failure prints
/// `Error: {msg}` and exits 1 (Pi's exact messages).
pub async fn export_session_html(input: &std::path::Path, output: Option<&str>) -> anyhow::Result<i32> {
    let out_path = match output {
        Some(p) => std::path::PathBuf::from(p),
        None => input.with_extension("html"),
    };
    let result: anyhow::Result<()> = async {
        let jsonl = tokio::fs::read_to_string(input)
            .await
            .with_context(|| format!("reading session file {}", input.display()))?;
        let html = cyrup_session_svc::session_jsonl_to_html(&jsonl);
        tokio::fs::write(&out_path, html)
            .await
            .with_context(|| format!("writing HTML to {}", out_path.display()))?;
        Ok(())
    }
    .await;
    match result {
        Ok(()) => {
            println!("Exported to: {}", out_path.display());
            Ok(0)
        }
        Err(err) => {
            eprintln!("Error: {err}");
            Ok(1)
        }
    }
}

/// Humanise a token count (Pi `formatTokenCount`, list-models.ts:14-24): `200000` → `200K`,
/// `1000000` → `1M`, `1500000` → `1.5M`. Whole values drop the decimal.
fn format_token_count(count: u64) -> String {
    if count >= 1_000_000 {
        let millions = count as f64 / 1_000_000.0;
        if (millions.fract()).abs() < f64::EPSILON {
            format!("{}M", millions as u64)
        } else {
            format!("{millions:.1}M")
        }
    } else if count >= 1_000 {
        let thousands = count as f64 / 1_000.0;
        if (thousands.fract()).abs() < f64::EPSILON {
            format!("{}K", thousands as u64)
        } else {
            format!("{thousands:.1}K")
        }
    } else {
        count.to_string()
    }
}

/// The `--list-models [search]` ACTION: resolve the models pi would list, then render them.
///
/// SEAM-020 — pi lists `getAvailable()`, not `getModels()`. It keeps only models whose provider has
/// COMPLETE auth configuration (`packages/ai/src/models.ts:394-405` @v0.83.0) and prints
/// `formatNoModelsAvailableMessage()` when that set is empty (`list-models.ts:37-40`). cyrup was
/// listing the whole compiled catalog, so a fresh install saw hundreds of rows for providers it has
/// no credential for and the guidance branch was unreachable.
///
/// `has_configured_auth` is the same predicate the default-launch path uses
/// ([`crate::bootstrap::resolve_default_launch_model`]): a stored credential, a known provider env
/// var, or a user-declared `models.json` block carrying its own `apiKey` (CFG-022).
pub fn list_models_action(
    dirs: &cyrup_config::ConfigDirs,
    models_json: &std::sync::Arc<cyrup_config::ModelFile>,
    search: &str,
) -> anyhow::Result<i32> {
    let auth = cyrup_config::AuthStore::at(dirs.agent_dir.join("auth.json"));
    let auth_models_json = models_json.clone();
    let has_configured_auth = move |m: &cyrup_provider::Model| {
        cyrup_config::provider_is_configured(&auth, &auth_models_json, &m.provider, None)
    };
    list_models(
        &crate::provider::available_models(models_json, &has_configured_auth),
        search,
    )
}

/// `--list-models [search]` (Pi `listModels`, list-models.ts:29-110): print the provider catalog as
/// an aligned `provider/model/context/max-out/thinking/images` table — token counts humanised, sorted
/// by provider then id, fuzzy-filtered by `search` — with Pi's `No models matching "x"` empty message.
fn list_models(models: &[cyrup_provider::Model], search: &str) -> anyhow::Result<i32> {
    use cyrup_provider::Modality;
    if models.is_empty() {
        // Pi `formatNoModelsAvailableMessage` (auth-guidance.ts:14) — the no-models guidance text.
        println!("{}", crate::format_no_models_available_message());
        return Ok(0);
    }

    // Pi: `fuzzyFilter(models, searchPattern, (m) => `${m.provider} ${m.id}`)` (list-models.ts:45
    // @v0.83.0, `:49` @v0.84.1). SEAM-068 — this used to be a hand-rolled filter that split the
    // query on WHITESPACE only, so `--list-models anthropic/sonnet` (the very `provider/model` form
    // `--model` documents) matched nothing, and pi's alphanumeric-swap retry (`o4` → `4o`,
    // `packages/tui/src/fuzzy.ts:75-92`) was absent. `cyrup_tui::fuzzy_filter` is the faithful port
    // of `fuzzyFilter`: it splits on `/[\s/]+/` (`:104-107`), requires every token (`:120-128`),
    // and stable-sorts ascending by score (`:135`).
    let keys: Vec<String> = models
        .iter()
        .map(|m| format!("{} {}", m.provider.as_str(), m.id.as_str()))
        .collect();
    let mut filtered: Vec<&cyrup_provider::Model> =
        cyrup_tui::fuzzy_filter(&keys, search, |k| k.as_str())
            .into_iter()
            .filter_map(|m| models.get(m.index))
            .collect();
    if filtered.is_empty() {
        println!("No models matching \"{search}\"");
        return Ok(0);
    }

    // Sort by provider, then by model id (Pi list-models.ts:54-58).
    filtered.sort_by(|a, b| {
        a.provider
            .as_str()
            .cmp(b.provider.as_str())
            .then_with(|| a.id.as_str().cmp(b.id.as_str()))
    });

    struct Row {
        provider: String,
        model: String,
        context: String,
        max_out: String,
        thinking: String,
        images: String,
    }
    let rows: Vec<Row> = filtered
        .iter()
        .map(|m| Row {
            provider: m.provider.as_str().to_string(),
            model: m.id.as_str().to_string(),
            context: format_token_count(m.context_window),
            max_out: format_token_count(m.max_tokens),
            thinking: if m.reasoning { "yes" } else { "no" }.to_string(),
            images: if m.input.contains(&Modality::Image) {
                "yes"
            } else {
                "no"
            }
            .to_string(),
        })
        .collect();

    let hdr = (
        "provider", "model", "context", "max-out", "thinking", "images",
    );
    let w_provider = rows
        .iter()
        .map(|r| r.provider.len())
        .chain([hdr.0.len()])
        .max()
        .unwrap_or(0);
    let w_model = rows
        .iter()
        .map(|r| r.model.len())
        .chain([hdr.1.len()])
        .max()
        .unwrap_or(0);
    let w_context = rows
        .iter()
        .map(|r| r.context.len())
        .chain([hdr.2.len()])
        .max()
        .unwrap_or(0);
    let w_max = rows
        .iter()
        .map(|r| r.max_out.len())
        .chain([hdr.3.len()])
        .max()
        .unwrap_or(0);
    let w_think = rows
        .iter()
        .map(|r| r.thinking.len())
        .chain([hdr.4.len()])
        .max()
        .unwrap_or(0);
    let w_img = rows
        .iter()
        .map(|r| r.images.len())
        .chain([hdr.5.len()])
        .max()
        .unwrap_or(0);

    println!(
        "{:<w_provider$}  {:<w_model$}  {:<w_context$}  {:<w_max$}  {:<w_think$}  {:<w_img$}",
        hdr.0, hdr.1, hdr.2, hdr.3, hdr.4, hdr.5
    );
    for r in &rows {
        println!(
            "{:<w_provider$}  {:<w_model$}  {:<w_context$}  {:<w_max$}  {:<w_think$}  {:<w_img$}",
            r.provider, r.model, r.context, r.max_out, r.thinking, r.images
        );
    }
    Ok(0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::format_token_count;

    #[test]
    fn token_counts_humanise_like_pi() {
        // Pi `formatTokenCount` (list-models.ts:14-24).
        assert_eq!(format_token_count(200_000), "200K");
        assert_eq!(format_token_count(1_000_000), "1M");
        assert_eq!(format_token_count(1_500_000), "1.5M");
        assert_eq!(format_token_count(128_000), "128K");
        assert_eq!(format_token_count(900), "900");
        assert_eq!(format_token_count(8_192), "8.2K");
    }

    /// SEAM-068 — `--list-models <search>` now runs pi's `fuzzyFilter` over `"{provider} {id}"`
    /// (`cli/list-models.ts:45` @v0.83.0, `:49` @v0.84.1) via the faithful port in
    /// `cyrup_tui::fuzzy`, replacing a hand-rolled predicate that split the query on WHITESPACE
    /// only and had no swap retry.
    ///
    /// Both assertions were RED before the change, and both are the first thing a user types:
    /// * `anthropic/sonnet` — the `provider/model` form `--model` itself documents (`cli.rs`'s help
    ///   row). pi splits the query on `/[\s/]+/` (`packages/tui/src/fuzzy.ts:104-107`), so it is two
    ///   tokens; the old filter treated it as ONE and the haystack `"anthropic claude-sonnet-4-5"`
    ///   contains no `/`, so `cyrup --list-models anthropic/sonnet` printed
    ///   `No models matching "anthropic/sonnet"` while `pi` listed the rows.
    /// * `4o` — pi's alphanumeric-swap retry (`fuzzy.ts:71-89`: a query that is letters-then-digits
    ///   or digits-then-letters is retried swapped, at a +5 penalty) reaches `o4-mini`; the old
    ///   filter found nothing.
    ///
    /// What this must NOT assert is a shortlist. `fuzzyMatch` is a SUBSEQUENCE match
    /// (`fuzzy.ts:29-56` — "all query characters appear in order, not necessarily consecutive"),
    /// so a two-character query matches nearly every row and pi ranks rather than prunes; the row
    /// the user meant simply has to come FIRST. `listModels` then re-sorts the survivors by
    /// provider/id (list-models.ts:57-62), so the rank is invisible in the printed table and only
    /// membership drives the `No models matching` branch.
    #[test]
    fn list_models_search_uses_pis_fuzzy_filter() {
        let keys = [
            "anthropic claude-sonnet-4-5".to_string(),
            "openai gpt-4o".to_string(),
            "openai o4-mini".to_string(),
        ];
        let hit = |q: &str| -> Vec<usize> {
            cyrup_tui::fuzzy_filter(&keys, q, |k| k.as_str())
                .into_iter()
                .map(|m| m.index)
                .collect()
        };
        assert_eq!(
            hit("anthropic/sonnet"),
            vec![0],
            "the slash form must split into two tokens (fuzzy.ts:104-107)"
        );

        // `4o` matches all three, and the ORDER is the whole point: `gpt-4o` scores best because
        // the two characters land consecutively on a word boundary, `o4-mini` only qualifies at all
        // through the swap retry's +5, and `claude-sonnet-4-5` is the incidental subsequence
        // ('o' of "anthropic" … '4' of "-4-5") that a 36-point gap penalty pushes to last.
        assert_eq!(
            hit("4o"),
            vec![1, 2, 0],
            "swap retry + score order (fuzzy.ts:29-89)"
        );
        // The retry ITSELF, isolated: `4o` cannot match `openai o4-mini` in order (the only `o`
        // after the `4` would have to come from `-mini`), so the score it does get can only be the
        // swapped query's score plus pi's flat +5 penalty (`fuzzy.ts:88`).
        let swapped = cyrup_tui::fuzzy_score("openai o4-mini", "o4").expect("`o4` matches directly");
        assert_eq!(
            cyrup_tui::fuzzy_score("openai o4-mini", "4o"),
            Some(swapped + 5.0),
            "`4o` may only reach this row through the swap retry, at +5"
        );

        assert_eq!(hit("claude"), vec![0]);
        assert!(hit("nothing-here").is_empty());
        // An empty query keeps every row in input order (fuzzy.ts's empty-token early return),
        // which is what `--list-models` with no search argument relies on.
        assert_eq!(hit(""), vec![0, 1, 2]);
    }
}

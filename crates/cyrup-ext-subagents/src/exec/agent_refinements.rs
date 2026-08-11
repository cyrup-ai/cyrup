//! Project-local per-agent refinement overlays — a port of pi-subagents'
//! `src/agents/agent-refinements.ts` @v0.43.0, restricted to the READ half that the spawn path
//! needs.
//!
//! A refinement overlay is a project-local markdown file at
//! `<cwd>/.cyrup-subagents/refinements/<agent>.md` holding accumulated, evidence-cited guidance for
//! ONE agent. `appendAgentRefinementOverlay` folds its current block onto the child's system prompt
//! at spawn (`runs/foreground/execution.ts:1442`), between the agent-memory block
//! (`:1438-1441`) and the output-path override (`:1443`) — so the composition order this module
//! completes is `persona -> skills -> memory -> refinement -> output-path`.
//!
//! # Scope of this port
//!
//! Only the read path is ported here: [`get_agent_refinement_path`], [`parse_refinement_file`] and
//! [`append_agent_refinement_overlay`]. Upstream's WRITE half — `collectBoundedRefinementEvidence`,
//! `validateRefinementProposal` and `handleRefinementAction` (the `refine` / `refine.show` /
//! `refine.rollback` management actions) — is a separate v0.43.0 management surface that this
//! crate does not yet register, and porting it does not belong on the spawn path. The read half is
//! independently complete: an overlay file authored by any means (upstream, or by hand) is applied
//! exactly as upstream applies it, and an absent file is the no-op that virtually every spawn hits.
//!
//! # Why the on-disk and prompt markers keep upstream's literal `pi-subagents-` spelling
//!
//! The metadata comment (`<!-- pi-subagents-refinement:v1`), the two fence languages
//! (`pi-subagents-refinement-current` / `-snapshots-json`) and the model-facing
//! `<pi-subagents-refinement>` tag are all left byte-identical to upstream, the same way
//! [`crate::discovery::skills::SUBAGENT_ORCHESTRATION_SKILL`] keeps its upstream `"pi-subagents"`
//! spelling. The first three are a FILE FORMAT — renaming them would make cyrup unable to read an
//! overlay upstream wrote and vice versa — and the last is prompt text, where byte-identical output
//! is exactly what the port is measured on. Only the containing DIRECTORY is rebranded, and that
//! comes for free from [`crate::artifacts::project_subagents_dir`] (`.cyrup-subagents`).
//!
//! # Failure policy
//!
//! Upstream wraps the whole read in `try { … } catch { return systemPrompt; }`, so EVERY failure —
//! an unusable agent name, a missing file, an unreadable file, malformed metadata, a missing
//! `current` fence, a malformed snapshot — silently leaves the system prompt untouched. That is
//! ported literally: [`append_agent_refinement_overlay`] is infallible and returns its input
//! unchanged on any error. The validation in [`parse_refinement_file`] is therefore load-bearing
//! even though nothing here reports its error text: it is what decides whether a half-written or
//! tampered-with overlay file reaches a child's system prompt at all.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// pi `REFINEMENT_DIR` (`agent-refinements.ts:10`).
const REFINEMENT_DIR: &str = "refinements";
/// pi `CURRENT_FENCE` (`agent-refinements.ts:11`).
const CURRENT_FENCE: &str = "pi-subagents-refinement-current";
/// pi `SNAPSHOTS_FENCE` (`agent-refinements.ts:12`).
const SNAPSHOTS_FENCE: &str = "pi-subagents-refinement-snapshots-json";
/// pi `MAX_EVIDENCE_ITEMS` (`agent-refinements.ts:13`) — the `evidence.maxItems` default a
/// metadata block that omits the field parses to.
const MAX_EVIDENCE_ITEMS: f64 = 8.0;
/// pi `MAX_AGE_DAYS` (`agent-refinements.ts:14`).
const MAX_AGE_DAYS: f64 = 14.0;
/// pi `MAX_ITEM_BYTES` (`agent-refinements.ts:15`).
const MAX_ITEM_BYTES: f64 = 2_048.0;
/// pi `MAX_PACKET_BYTES` (`agent-refinements.ts:16`).
const MAX_PACKET_BYTES: f64 = 16_384.0;

/// The literal prefix of pi's metadata regex `/^<!-- pi-subagents-refinement:v1\n…/`
/// (`agent-refinements.ts:178`). Anchored at index 0 (the regex has no `m` flag, so `^` is
/// start-of-STRING).
const METADATA_PREFIX: &str = "<!-- pi-subagents-refinement:v1\n";
/// The metadata regex's closing literal `\n-->\n`.
const METADATA_SUFFIX: &str = "\n-->\n";

/// pi `RefinementMetadata["base"]` (`agent-refinements.ts:58-62`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefinementBase {
    /// One of `builtin` / `package` / `user` / `project` — anything else fails the parse.
    pub source: String,
    pub file_path: String,
    pub system_prompt_sha256: String,
}

/// pi `RefinementMetadata["evidence"]` (`agent-refinements.ts:63-68`). Held as `f64` because
/// upstream's parse accepts ANY JSON number here (`typeof evidence.maxItems === "number"`) and
/// defaults per field, without an integrality check.
#[derive(Clone, Debug, PartialEq)]
pub struct RefinementEvidenceLimits {
    pub max_items: f64,
    pub max_age_days: f64,
    pub item_bytes: f64,
    pub total_bytes: f64,
}

/// pi `RefinementMetadata` (`agent-refinements.ts:54-69`).
#[derive(Clone, Debug, PartialEq)]
pub struct RefinementMetadata {
    pub agent: String,
    pub revision: f64,
    pub updated_at: String,
    pub base: RefinementBase,
    pub evidence: RefinementEvidenceLimits,
}

/// pi `RefinementSnapshot` (`agent-refinements.ts:71-79`).
#[derive(Clone, Debug, PartialEq)]
pub struct RefinementSnapshot {
    pub revision: f64,
    pub at: String,
    /// `refine` or `rollback` — anything else fails the parse.
    pub action: String,
    pub before: String,
    pub after: String,
    pub evidence_ids: Vec<String>,
    pub proposal_agent: Option<String>,
}

/// pi `ParsedRefinementFile` (`agent-refinements.ts:81-85`).
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedRefinementFile {
    pub metadata: RefinementMetadata,
    /// The `current` fence body, VERBATIM (untrimmed) — upstream trims only at the two consumers.
    pub current: String,
    pub snapshots: Vec<RefinementSnapshot>,
}

/// pi `record(value)` (`agent-refinements.ts:121-123`): an object that is not an array and not
/// `null`. `serde_json::Value::Object` already excludes both.
fn record(value: Option<&Value>) -> Option<&serde_json::Map<String, Value>> {
    value.and_then(Value::as_object)
}

/// pi `text(value)` (`agent-refinements.ts:125-127`): a string whose TRIMMED form is non-empty,
/// returned trimmed.
fn text(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// pi `textArray(value)` (`agent-refinements.ts:129-132`): a non-array is `[]`; entries that are
/// not non-empty strings are dropped.
fn text_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| text(Some(item)))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// JS `typeof x === "number" && Number.isInteger(x)`. A JSON `5.0` IS an integer under
/// `Number.isInteger`, so this deliberately tests the VALUE, not serde's integer/float tagging.
fn integer_number(value: Option<&Value>) -> Option<f64> {
    value
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite() && n.fract() == 0.0)
}

/// JS `typeof evidence.<field> === "number" ? evidence.<field> : <default>` — note the ABSENCE of
/// an integrality check here, unlike `revision`.
fn number_or(value: Option<&Value>, default: f64) -> f64 {
    value.and_then(Value::as_f64).unwrap_or(default)
}

/// pi `safeAgentFileName` (`agent-refinements.ts:147-153`): the agent name, trimmed, must match
/// `/^[A-Za-z0-9][A-Za-z0-9._-]*$/` and must not contain `..`; the file is `<name>.md`.
///
/// # Errors
///
/// Returns pi's own message when the name cannot be used as a refinement file name.
fn safe_agent_file_name(agent_name: &str) -> Result<String, String> {
    let trimmed = agent_name.trim();
    let legal = match trimmed.chars().next() {
        Some(first) if first.is_ascii_alphanumeric() => trimmed
            .chars()
            .skip(1)
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-'),
        _ => false,
    };
    if !legal || trimmed.contains("..") {
        return Err(format!(
            "Agent name '{agent_name}' cannot be used as a refinement file name."
        ));
    }
    Ok(format!("{trimmed}.md"))
}

/// pi `getAgentRefinementPath` (`agent-refinements.ts:155-163`):
/// `<getProjectSubagentsDir(cwd)>/refinements/<safeAgentFileName(agentName)>`.
///
/// Upstream additionally re-checks that `path.resolve`ing the joined path did not escape the
/// refinement root — a defence-in-depth assertion against a name that survived
/// [`safe_agent_file_name`]. The Rust equivalent is the same containment check expressed on the
/// joined [`PathBuf`]: the file name must be exactly the vetted one and the parent must be exactly
/// the refinement root, so no `..`/separator/absolute component can have crept in.
///
/// # Errors
///
/// Propagates [`safe_agent_file_name`]'s error, and repeats pi's containment message when the
/// joined path does not sit directly inside the refinement root.
pub fn get_agent_refinement_path(cwd: &Path, agent_name: &str) -> Result<PathBuf, String> {
    let refinement_root = crate::artifacts::project_subagents_dir(cwd).join(REFINEMENT_DIR);
    let file_name = safe_agent_file_name(agent_name)?;
    let resolved = refinement_root.join(&file_name);
    if resolved.file_name().map(std::ffi::OsStr::to_os_string)
        != Some(std::ffi::OsString::from(&file_name))
        || resolved.parent() != Some(refinement_root.as_path())
    {
        return Err(format!(
            "Agent name '{agent_name}' cannot be used as a refinement file name."
        ));
    }
    Ok(resolved)
}

/// pi `extractFence` (`agent-refinements.ts:171-175`): the body of the first
/// ```` \n```<fence>\n … \n``` ```` block, or `None`.
///
/// The lazy `([\s\S]*?)` plus regex backtracking means a fence OPENER with no closer is not fatal —
/// the engine retries from the next opener — so the search loops rather than committing to the
/// first opener. An empty body yields `Some("")`, not `None`: upstream's `match?.[1] ?? null`
/// coalesces only `null`/`undefined`, never `""`, and that distinction is load-bearing for the
/// snapshots fence (see [`parse_refinement_file`]).
fn extract_fence<'a>(markdown: &'a str, fence: &str) -> Option<&'a str> {
    let opener = format!("\n```{fence}\n");
    let mut search_from = 0usize;
    while let Some(rel) = markdown.get(search_from..)?.find(&opener) {
        let body_start = search_from + rel + opener.len();
        let body = markdown.get(body_start..)?;
        if let Some(end) = body.find("\n```") {
            return body.get(..end);
        }
        search_from = search_from + rel + 1;
    }
    None
}

/// pi `parseRefinementFile` (`agent-refinements.ts:177-237`).
///
/// # Errors
///
/// Returns pi's own `${label} …` message for every shape upstream throws on: absent/empty metadata
/// comment, unparseable or non-object metadata JSON, a missing/ill-typed metadata field, an
/// out-of-set `base.source`, an absent `current` fence, unparseable or non-array snapshots JSON,
/// and any snapshot entry that is not an object or is missing/ill-typed.
pub fn parse_refinement_file(markdown: &str, label: &str) -> Result<ParsedRefinementFile, String> {
    let metadata_raw = markdown
        .strip_prefix(METADATA_PREFIX)
        .and_then(|rest| rest.find(METADATA_SUFFIX).and_then(|end| rest.get(..end)))
        // JS `!metadataMatch?.[1]` — an EMPTY capture is falsy and throws the same message.
        .filter(|raw| !raw.is_empty())
        .ok_or_else(|| format!("{label} is missing refinement metadata."))?;
    let metadata_value: Value = serde_json::from_str(metadata_raw)
        .map_err(|err| format!("{label} metadata is not valid JSON: {err}"))?;
    let metadata_record = record(Some(&metadata_value))
        .ok_or_else(|| format!("{label} metadata must be an object."))?;

    let agent = text(metadata_record.get("agent"));
    let revision = integer_number(metadata_record.get("revision"));
    let updated_at = text(metadata_record.get("updatedAt"));
    let base = record(metadata_record.get("base"));
    let evidence = record(metadata_record.get("evidence"));
    let (Some(agent), Some(revision), Some(updated_at), Some(base), Some(evidence)) =
        (agent, revision, updated_at, base, evidence)
    else {
        return Err(format!("{label} metadata is invalid."));
    };

    let source = text(base.get("source"));
    let file_path = text(base.get("filePath"));
    let system_prompt_sha256 = text(base.get("systemPromptSha256"));
    if !matches!(source, Some("builtin" | "package" | "user" | "project")) {
        return Err(format!("{label} base source is invalid."));
    }
    let (Some(source), Some(file_path), Some(system_prompt_sha256)) =
        (source, file_path, system_prompt_sha256)
    else {
        return Err(format!("{label} base metadata is invalid."));
    };

    let current = extract_fence(markdown, CURRENT_FENCE)
        .ok_or_else(|| format!("{label} is missing current refinement block."))?;

    // JS `extractFence(...) ?? "[]"` coalesces only on ABSENCE — an empty snapshots fence body
    // stays `""`, and `JSON.parse("")` throws. Ported literally: `Some("")` is fed to the parser
    // and fails, exactly as upstream does.
    let snapshots_raw = extract_fence(markdown, SNAPSHOTS_FENCE).unwrap_or("[]");
    let snapshots_value: Value = serde_json::from_str(snapshots_raw)
        .map_err(|err| format!("{label} snapshots are not valid JSON: {err}"))?;
    let snapshot_items = snapshots_value
        .as_array()
        .ok_or_else(|| format!("{label} snapshots must be an array."))?;

    let mut snapshots = Vec::with_capacity(snapshot_items.len());
    for (index, entry) in snapshot_items.iter().enumerate() {
        let item = record(Some(entry))
            .ok_or_else(|| format!("{label} snapshot {index} must be an object."))?;
        let action = text(item.get("action"));
        // JS `typeof item.before === "string" ? item.before : null` — an EMPTY string is a legal
        // `before`/`after`, so this is a type test, not `text()`.
        let before = item.get("before").and_then(Value::as_str);
        let after = item.get("after").and_then(Value::as_str);
        let at = text(item.get("at"));
        let revision = integer_number(item.get("revision"));
        let (Some(revision), Some(at), Some(action @ ("refine" | "rollback")), Some(before), Some(after)) =
            (revision, at, action, before, after)
        else {
            return Err(format!("{label} snapshot {index} is invalid."));
        };
        snapshots.push(RefinementSnapshot {
            revision,
            at: at.to_string(),
            action: action.to_string(),
            before: before.to_string(),
            after: after.to_string(),
            evidence_ids: text_array(item.get("evidenceIds")),
            proposal_agent: text(item.get("proposalAgent")).map(str::to_string),
        });
    }

    Ok(ParsedRefinementFile {
        metadata: RefinementMetadata {
            agent: agent.to_string(),
            revision,
            updated_at: updated_at.to_string(),
            base: RefinementBase {
                source: source.to_string(),
                file_path: file_path.to_string(),
                system_prompt_sha256: system_prompt_sha256.to_string(),
            },
            evidence: RefinementEvidenceLimits {
                max_items: number_or(evidence.get("maxItems"), MAX_EVIDENCE_ITEMS),
                max_age_days: number_or(evidence.get("maxAgeDays"), MAX_AGE_DAYS),
                item_bytes: number_or(evidence.get("itemBytes"), MAX_ITEM_BYTES),
                total_bytes: number_or(evidence.get("totalBytes"), MAX_PACKET_BYTES),
            },
        },
        current: current.to_string(),
        snapshots,
    })
}

/// pi `appendAgentRefinementOverlay` (`agent-refinements.ts:426-446`) — fold this agent's current
/// project-local refinement overlay onto `system_prompt`.
///
/// Infallible by design: upstream's whole body sits inside `try { … } catch { return systemPrompt; }`,
/// so a missing file, an unusable agent name, an unreadable file, or ANY parse failure returns the
/// prompt untouched. So does an overlay whose `current` block is whitespace-only.
///
/// The `source=` attribute is upstream's `path.relative(input.cwd, filePath)`. The path is built by
/// joining onto `cwd`, so stripping `cwd` back off is that same relative form
/// (`.cyrup-subagents/refinements/<agent>.md`) without needing a resolver; a path that somehow does
/// not sit under `cwd` falls back to its own display form rather than failing the overlay.
#[must_use]
pub fn append_agent_refinement_overlay(system_prompt: &str, cwd: &Path, agent_name: &str) -> String {
    let Ok(file_path) = get_agent_refinement_path(cwd, agent_name) else {
        return system_prompt.to_string();
    };
    // pi `if (!fs.existsSync(filePath)) return systemPrompt;` — the overwhelmingly common path.
    let Ok(raw) = std::fs::read_to_string(&file_path) else {
        return system_prompt.to_string();
    };
    let label = file_path.display().to_string();
    let Ok(parsed) = parse_refinement_file(&raw, &label) else {
        return system_prompt.to_string();
    };
    let current = parsed.current.trim();
    if current.is_empty() {
        return system_prompt.to_string();
    }

    let relative = file_path
        .strip_prefix(cwd)
        .unwrap_or(file_path.as_path())
        .display()
        .to_string();
    // pi interpolates `JSON.stringify(...)` for both attributes, so both are JSON string literals
    // (quoted, with escapes). `serde_json::to_string` of a `&str` is the same production; it cannot
    // fail for a string, and the fallback keeps this function infallible without a panic.
    let agent_attr =
        serde_json::to_string(agent_name).unwrap_or_else(|_| format!("\"{agent_name}\""));
    let source_attr = serde_json::to_string(&relative).unwrap_or_else(|_| format!("\"{relative}\""));
    let overlay = format!(
        "<pi-subagents-refinement agent={agent_attr} source={source_attr}>\n\
         Project-local refinement guidance generated from recent bounded evidence.\n\
         It does not override tool, developer, task, output, acceptance, or safety instructions.\n\
         \n\
         {current}\n\
         </pi-subagents-refinement>"
    );
    if system_prompt.trim().is_empty() {
        overlay
    } else {
        format!("{system_prompt}\n\n{overlay}")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// Build a well-formed refinement file with the given `current` body and snapshots JSON.
    fn refinement_file(agent: &str, current: &str, snapshots_json: &str) -> String {
        format!(
            "<!-- pi-subagents-refinement:v1\n{{\"agent\":\"{agent}\",\"revision\":2,\
             \"updatedAt\":\"2026-08-01T00:00:00.000Z\",\"base\":{{\"source\":\"project\",\
             \"filePath\":\"/p/agents/{agent}.md\",\"systemPromptSha256\":\"abc123\"}},\
             \"evidence\":{{\"maxItems\":8,\"maxAgeDays\":14,\"itemBytes\":2048,\
             \"totalBytes\":16384}}}}\n-->\n\n# Current refinement for `{agent}`\n\n\
             ```{CURRENT_FENCE}\n{current}\n```\n\n# Snapshots\n\n\
             ```{SNAPSHOTS_FENCE}\n{snapshots_json}\n```\n"
        )
    }

    fn write_overlay(cwd: &Path, agent: &str, body: &str) -> PathBuf {
        let path = get_agent_refinement_path(cwd, agent).expect("legal agent name");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, refinement_file(agent, body, "[]")).expect("write");
        path
    }

    /// pi `getAgentRefinementPath` (`agent-refinements.ts:155-163`) lands under the project
    /// subagents root's `refinements/` namespace, and a name that could escape it is refused
    /// rather than resolved.
    #[test]
    fn the_refinement_path_is_namespaced_and_traversal_names_are_refused() {
        let cwd = Path::new("/p");
        assert_eq!(
            get_agent_refinement_path(cwd, "reviewer").expect("legal"),
            crate::artifacts::project_subagents_dir(cwd)
                .join("refinements")
                .join("reviewer.md")
        );
        // Leading/trailing whitespace is trimmed before validation (pi `agentName.trim()`).
        assert_eq!(
            get_agent_refinement_path(cwd, "  reviewer  ").expect("legal"),
            get_agent_refinement_path(cwd, "reviewer").expect("legal")
        );
        for illegal in [
            "../etc/passwd",
            "..",
            "a..b",
            "/abs",
            "sub/dir",
            ".hidden",
            "-leading",
            "",
            "   ",
            "na\u{0}me",
        ] {
            assert!(
                get_agent_refinement_path(cwd, illegal).is_err(),
                "'{illegal}' must not be usable as a refinement file name"
            );
        }
    }

    /// pi `appendAgentRefinementOverlay` (`agent-refinements.ts:426-446`): the overlay is appended
    /// after a blank line, carries both attributes and both boundary sentences, and wraps the
    /// TRIMMED current block.
    #[test]
    fn a_present_overlay_is_appended_with_pis_exact_block() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_overlay(tmp.path(), "reviewer", "\n  - Prefer smaller diffs.\n");
        let out = append_agent_refinement_overlay("You are a reviewer.", tmp.path(), "reviewer");
        assert_eq!(
            out,
            format!(
                "You are a reviewer.\n\n<pi-subagents-refinement agent=\"reviewer\" \
                 source=\"{}\">\nProject-local refinement guidance generated from recent bounded \
                 evidence.\nIt does not override tool, developer, task, output, acceptance, or \
                 safety instructions.\n\n- Prefer smaller diffs.\n</pi-subagents-refinement>",
                Path::new(".cyrup-subagents")
                    .join("refinements")
                    .join("reviewer.md")
                    .display()
            )
        );
    }

    /// pi's `systemPrompt.trim() ? ... : overlay` — an empty base prompt yields the overlay ALONE,
    /// with no leading blank lines.
    #[test]
    fn an_empty_system_prompt_yields_the_bare_overlay() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_overlay(tmp.path(), "reviewer", "Be terse.");
        let out = append_agent_refinement_overlay("   \n ", tmp.path(), "reviewer");
        assert!(
            out.starts_with("<pi-subagents-refinement agent=\"reviewer\""),
            "{out}"
        );
        assert!(out.ends_with("</pi-subagents-refinement>"), "{out}");
    }

    /// Every failure mode returns the prompt UNTOUCHED (pi's blanket `catch { return systemPrompt }`
    /// plus its two early returns). The absent-file case is the one virtually every spawn takes.
    #[test]
    fn every_failure_mode_leaves_the_system_prompt_byte_identical() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = "You are a reviewer.";

        // 1. No refinements directory at all.
        assert_eq!(
            append_agent_refinement_overlay(base, tmp.path(), "reviewer"),
            base
        );
        // 2. An agent name that cannot become a file name — no filesystem access at all.
        assert_eq!(
            append_agent_refinement_overlay(base, tmp.path(), "../escape"),
            base
        );
        // 3. A file with no metadata comment.
        let path = get_agent_refinement_path(tmp.path(), "reviewer").expect("legal");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, "just some markdown\n").expect("write");
        assert_eq!(
            append_agent_refinement_overlay(base, tmp.path(), "reviewer"),
            base
        );
        // 4. Valid metadata, but no `current` fence.
        std::fs::write(
            &path,
            "<!-- pi-subagents-refinement:v1\n{\"agent\":\"reviewer\",\"revision\":1,\
             \"updatedAt\":\"t\",\"base\":{\"source\":\"project\",\"filePath\":\"f\",\
             \"systemPromptSha256\":\"s\"},\"evidence\":{}}\n-->\n\nno fences here\n",
        )
        .expect("write");
        assert_eq!(
            append_agent_refinement_overlay(base, tmp.path(), "reviewer"),
            base
        );
        // 5. A `current` fence that is whitespace-only.
        std::fs::write(&path, refinement_file("reviewer", "   \n  ", "[]")).expect("write");
        assert_eq!(
            append_agent_refinement_overlay(base, tmp.path(), "reviewer"),
            base
        );
        // 6. A malformed snapshot entry poisons the whole parse (pi throws inside the map).
        std::fs::write(
            &path,
            refinement_file("reviewer", "Be terse.", "[{\"revision\":1}]"),
        )
        .expect("write");
        assert_eq!(
            append_agent_refinement_overlay(base, tmp.path(), "reviewer"),
            base
        );
        // 7. An out-of-set `base.source`.
        std::fs::write(
            &path,
            refinement_file("reviewer", "Be terse.", "[]").replace("\"project\"", "\"elsewhere\""),
        )
        .expect("write");
        assert_eq!(
            append_agent_refinement_overlay(base, tmp.path(), "reviewer"),
            base
        );
        // …and the SAME file with a legal source does produce an overlay, so the assertions above
        // are failing for the reason claimed and not because the fixture never worked.
        std::fs::write(&path, refinement_file("reviewer", "Be terse.", "[]")).expect("write");
        assert!(
            append_agent_refinement_overlay(base, tmp.path(), "reviewer")
                .contains("<pi-subagents-refinement"),
        );
    }

    /// pi's snapshots fence coalesces on ABSENCE only: a MISSING fence defaults to `"[]"` and
    /// parses, while an EMPTY fence body is `""` and `JSON.parse("")` throws.
    #[test]
    fn a_missing_snapshots_fence_defaults_but_an_empty_one_is_a_parse_error() {
        let with_no_snapshots = format!(
            "<!-- pi-subagents-refinement:v1\n{{\"agent\":\"r\",\"revision\":1,\"updatedAt\":\"t\",\
             \"base\":{{\"source\":\"user\",\"filePath\":\"f\",\"systemPromptSha256\":\"s\"}},\
             \"evidence\":{{}}}}\n-->\n\n```{CURRENT_FENCE}\nBe terse.\n```\n"
        );
        let parsed = parse_refinement_file(&with_no_snapshots, "f").expect("parses");
        assert_eq!(parsed.current, "Be terse.");
        assert!(parsed.snapshots.is_empty());
        // The omitted `evidence` fields fall back to pi's constants rather than to zero.
        assert_eq!(parsed.metadata.evidence.max_items, MAX_EVIDENCE_ITEMS);
        assert_eq!(parsed.metadata.evidence.total_bytes, MAX_PACKET_BYTES);

        let with_empty_fence = refinement_file("r", "Be terse.", "");
        assert!(parse_refinement_file(&with_empty_fence, "f").is_err());
    }

    /// A well-formed snapshot round-trips every field, including the optional `proposalAgent` and
    /// the EMPTY-string `before` that `text()` would have rejected but the type test accepts.
    #[test]
    fn snapshots_parse_with_empty_before_and_optional_proposal_agent() {
        let snapshots = "[{\"revision\":1,\"at\":\"2026-08-01T00:00:00.000Z\",\"action\":\"refine\",\
                         \"before\":\"\",\"after\":\"- x\",\"evidenceIds\":[\"live:a\",\"\",42],\
                         \"proposalAgent\":\"reviewer\"}]";
        let parsed =
            parse_refinement_file(&refinement_file("r", "- x", snapshots), "f").expect("parses");
        assert_eq!(parsed.snapshots.len(), 1);
        let snapshot = &parsed.snapshots[0];
        assert_eq!(snapshot.before, "");
        assert_eq!(snapshot.after, "- x");
        assert_eq!(snapshot.action, "refine");
        // pi `textArray` drops the empty string and the non-string.
        assert_eq!(snapshot.evidence_ids, vec!["live:a".to_string()]);
        assert_eq!(snapshot.proposal_agent.as_deref(), Some("reviewer"));

        // `action` outside {refine, rollback} is rejected.
        let bad = snapshots.replace("\"refine\"", "\"delete\"");
        assert!(parse_refinement_file(&refinement_file("r", "- x", &bad), "f").is_err());
    }

    /// `extractFence` backtracks past an unterminated opener rather than giving up (the lazy
    /// `([\s\S]*?)` in pi's regex), and an empty body is `Some("")`, never `None`.
    #[test]
    fn extract_fence_skips_an_unterminated_opener_and_distinguishes_empty_from_absent() {
        let doc = format!("head\n```{CURRENT_FENCE}\nunterminated");
        assert_eq!(extract_fence(&doc, CURRENT_FENCE), None);
        let doc = format!("head\n```{CURRENT_FENCE}\n\n```\n");
        assert_eq!(extract_fence(&doc, CURRENT_FENCE), Some(""));
        // The opener must be preceded by a newline (pi's regex begins `\n` + backticks), so a fence
        // at byte 0 is NOT matched.
        let doc = format!("```{CURRENT_FENCE}\nbody\n```\n");
        assert_eq!(extract_fence(&doc, CURRENT_FENCE), None);
        assert_eq!(extract_fence("nothing here", CURRENT_FENCE), None);
    }

    /// The metadata comment must be at byte 0 (pi's `^` with no `m` flag) and must not be empty.
    #[test]
    fn the_metadata_comment_is_anchored_at_the_start_and_must_be_non_empty() {
        let good = refinement_file("r", "Be terse.", "[]");
        assert!(parse_refinement_file(&good, "f").is_ok());
        assert!(parse_refinement_file(&format!("\n{good}"), "f").is_err());
        assert!(parse_refinement_file(&format!("{METADATA_PREFIX}{METADATA_SUFFIX}"), "f").is_err());
        // A non-integer revision fails (pi `Number.isInteger`), while `2.0` does not.
        assert!(
            parse_refinement_file(&good.replace("\"revision\":2", "\"revision\":2.5"), "f").is_err()
        );
        assert!(
            parse_refinement_file(&good.replace("\"revision\":2", "\"revision\":2.0"), "f").is_ok()
        );
    }
}

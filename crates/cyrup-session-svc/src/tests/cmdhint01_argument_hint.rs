//! CMDHINT_01 — `argument_hint` plumbing from prompt-template frontmatter through
//! `AgentSession::slash_command_catalog()`.
//!
//! Before this feature, `slash_command_catalog()`'s prompt-template arm (`session.rs:2620-2628`)
//! emitted only `name`/`description`/`source`/`sourceInfo`, so a template's parsed
//! `PromptTemplate::argument_hint` (`cyrup-resources/src/prompt.rs:41,112-113`) never reached the
//! catalog — the one thing `cyrup-tui`'s `dynamic_commands_from_catalog_gated` (`commands.rs:470`)
//! would have needed to stop hardcoding `argument_hint: None`. This proves the ASSEMBLED wire: a
//! real `AgentSession` built with a global prompt template carrying `argument-hint` frontmatter
//! surfaces an `argumentHint` STRING key on that command's catalog row, and a hintless prompt
//! template (and every non-prompt row) omits the key entirely (`None` ⇒ absent, not `null`, mirroring
//! pi's spread-if-truthy `...(cmd.argumentHint && { argumentHint: cmd.argumentHint })`,
//! `interactive-mode.ts:685-689`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyrup_provider::faux::FauxProvider;
use cyrup_provider::Provider;
use tempfile::TempDir;

use crate::{SessionBuilder, SessionConfig};

struct Fx {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, contents).unwrap();
}

fn fixture() -> Fx {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    Fx { _tmp: tmp, cwd, agent_dir }
}

async fn build_session(fx: &Fx) -> crate::AgentSession {
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    SessionBuilder::new(faux, cfg).build().await.expect("build")
}

/// The headline proof: a global prompt template with `argument-hint` frontmatter produces a catalog
/// row carrying `argumentHint` as a plain string, verbatim and un-split (the spec is explicit that
/// `argument_hint` is one opaque blob, never positional slots).
#[tokio::test]
async fn prompt_template_with_argument_hint_surfaces_it_in_the_catalog() {
    let fx = fixture();
    write(
        &fx.agent_dir.join("prompts/deploy.md"),
        "---\ndescription: Deploy the service\nargument-hint: <env> [--dry-run]\n---\nDeploy to $1",
    );
    let session = build_session(&fx).await;

    let catalog = session.slash_command_catalog();
    let row = catalog
        .iter()
        .find(|c| c.get("name").and_then(serde_json::Value::as_str) == Some("deploy"))
        .unwrap_or_else(|| panic!("no /deploy row in catalog: {catalog:?}"));
    assert_eq!(row.get("source").and_then(serde_json::Value::as_str), Some("prompt"));
    assert_eq!(
        row.get("argumentHint").and_then(serde_json::Value::as_str),
        Some("<env> [--dry-run]"),
        "row: {row:?}"
    );
}

/// A prompt template with NO `argument-hint` frontmatter must omit the key entirely — `None` maps to
/// absence, never to a JSON `null`, exactly like pi's `&&`-spread.
#[tokio::test]
async fn prompt_template_without_argument_hint_omits_the_key() {
    let fx = fixture();
    write(&fx.agent_dir.join("prompts/greet.md"), "---\ndescription: Say hello\n---\nHello $1");
    let session = build_session(&fx).await;

    let catalog = session.slash_command_catalog();
    let row = catalog
        .iter()
        .find(|c| c.get("name").and_then(serde_json::Value::as_str) == Some("greet"))
        .unwrap_or_else(|| panic!("no /greet row in catalog: {catalog:?}"));
    assert!(!row.as_object().unwrap().contains_key("argumentHint"), "row: {row:?}");
}

/// Scoping proof: even with a hinted prompt template present, extension and skill rows never carry
/// `argumentHint` — the key is written ONLY on the prompt arm (`session.rs`'s prompt-template loop),
/// so a skill row's `.get("argumentHint")` must miss by construction, not by a source-based guard.
#[tokio::test]
async fn skill_rows_never_carry_argument_hint() {
    let fx = fixture();
    write(
        &fx.agent_dir.join("prompts/deploy.md"),
        "---\ndescription: Deploy the service\nargument-hint: <env>\n---\nDeploy to $1",
    );
    write(
        &fx.agent_dir.join("skills/alpha/SKILL.md"),
        "---\nname: alpha\ndescription: alpha skill\n---\n# alpha\n\nDo the thing.\n",
    );
    let session = build_session(&fx).await;

    let catalog = session.slash_command_catalog();
    let skill_row = catalog
        .iter()
        .find(|c| c.get("name").and_then(serde_json::Value::as_str) == Some("skill:alpha"))
        .unwrap_or_else(|| panic!("no skill:alpha row in catalog: {catalog:?}"));
    assert!(!skill_row.as_object().unwrap().contains_key("argumentHint"), "row: {skill_row:?}");
}

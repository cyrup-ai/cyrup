//! SESS-059 — a bash-only session still advertises its skills, with pi's bash load instruction.
//!
//! pi v0.85.0 (#8552) picks `skillFileReadTool = ["read", "bash"].find(selected)`
//! (`system-prompt.ts:46`) and passes it to `formatSkillsForPrompt` (`skills.ts:355-366`
//! @v0.87.1); skills themselves are loaded whatever the tool set. cyrup dropped the pointer set at
//! build time unless `read` was active, so `--tools bash` lost `<available_skills>` entirely.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use crate::{SessionBuilder, SessionConfig};
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use tempfile::TempDir;

async fn prompt_with_tools(tools: &[&str]) -> String {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    let skill_dir = agent_dir.join("skills").join("demoskill");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: demoskill\ndescription: use this when you need a demo\n---\n\nBody.\n",
    )
    .unwrap();
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    cfg.tools = Some(tools.iter().map(|t| (*t).to_string()).collect());
    let session = SessionBuilder::new(Arc::new(FauxProvider::new()) as Arc<dyn Provider>, cfg)
        .build()
        .await
        .expect("build");
    session.system_prompt().to_string()
}

#[tokio::test]
async fn a_bash_only_session_lists_skills_with_the_bash_load_instruction() {
    let prompt = prompt_with_tools(&["bash"]).await;
    assert!(
        prompt.contains("<name>demoskill</name>"),
        "the skill must be advertised:\n{prompt}"
    );
    assert!(
        prompt.contains("Use bash to load a skill's file when the task matches its description."),
        "{prompt}"
    );
    assert!(
        !prompt.contains("Use the read tool to load a skill's file"),
        "{prompt}"
    );

    let prompt = prompt_with_tools(&["read", "bash"]).await;
    assert!(
        prompt.contains(
            "Use the read tool to load a skill's file when the task matches its description."
        ),
        "{prompt}"
    );

    let prompt = prompt_with_tools(&["edit"]).await;
    assert!(!prompt.contains("<available_skills>"), "{prompt}");
}

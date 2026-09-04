//! `before_agent_start` extension seam (arch-06 §3.6/§6.5, R-06-014/015).
//!
//! The agent loop builds the prompt, then for each subscribed extension calls the hook with a
//! [`BeforeAgentStartInput`]; a returned `system_prompt` replaces it. Hooks run **last**, after
//! override/context/skills assembly, in subscription order — a deterministic precedence
//! (func-06 §12).
//!
//! The payload is expressed as serializable data (ADR-0002: extension I/O crosses as serde, not
//! host pointers); this crate only provides the types + the in-process composition helper, while
//! `cyrup-agent`/`cyrup-ext` drive the actual WASM/native round-trip.

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_resources::SkillPointer;

use super::builder::PromptInputs;
use super::context_files::ContextFile;

/// Payload handed to a `before_agent_start` hook. Serialize-only: it is sent *to* the extension.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BeforeAgentStartInput {
    /// The fully-resolved prompt (post-override, post-context, post-skills, post-footer).
    pub system_prompt: String,
    /// Echoed build options so the extension can re-derive/inspect (R-06-014).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_prompt: Option<Arc<str>>,
    /// Mirrors Pi's optional `BuildSystemPromptOptions.selectedTools?: string[]`
    /// (`system-prompt.ts:12`): absent = the default set, `[]` = explicitly no tools. Echoing an
    /// empty `Vec` for both would hide that distinction from the guest exactly as it used to hide
    /// it from the builder.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_tools: Option<Vec<Arc<str>>>,
    pub prompt_guidelines: Vec<Arc<str>>,
    pub context_files: Vec<ContextFile>,
    pub skills: Vec<SkillPointer>,
    pub cwd: PathBuf,
}

impl BeforeAgentStartInput {
    /// Build the payload from the assembled `prompt` + the build inputs.
    pub fn new(prompt: String, inp: &PromptInputs) -> Self {
        Self {
            system_prompt: prompt,
            custom_prompt: inp.custom_prompt.clone(),
            selected_tools: inp.selected_tools.clone(),
            prompt_guidelines: inp.prompt_guidelines.clone(),
            context_files: inp.context_files.to_vec(),
            skills: inp.skills.to_vec(),
            cwd: inp.cwd.clone(),
        }
    }
}

/// Hook result. `system_prompt: Some(_)` replaces the prompt the model receives (R-06-014).
/// Append-style hooks (R-06-015) return `Some(prev + extra)`.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeforeAgentStartOutput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
}

impl BeforeAgentStartOutput {
    /// A no-op result (leaves the prompt unchanged).
    pub fn keep() -> Self {
        Self {
            system_prompt: None,
        }
    }

    /// Replace the prompt with `prompt`.
    pub fn replace(prompt: impl Into<String>) -> Self {
        Self {
            system_prompt: Some(prompt.into()),
        }
    }
}

/// An in-process `before_agent_start` hook. The native-side adapter for an extension implements
/// this; `cyrup-ext` provides a WASM-backed impl that serializes [`BeforeAgentStartInput`].
pub trait BeforeAgentStartHook {
    fn before_agent_start(&self, input: &BeforeAgentStartInput) -> BeforeAgentStartOutput;
}

/// Run the assembled `prompt` through `hooks` in subscription order (R-06-014/015). Each hook sees
/// the prior prompt; a `Some` result replaces it. Returns the final prompt.
pub fn apply_before_agent_start(
    mut prompt: String,
    inp: &PromptInputs,
    hooks: &[&dyn BeforeAgentStartHook],
) -> String {
    for hook in hooks {
        let payload = BeforeAgentStartInput::new(prompt.clone(), inp);
        if let Some(replacement) = hook.before_agent_start(&payload).system_prompt {
            prompt = replacement;
        }
    }
    prompt
}

//! The `before_agent_start` context-hygiene layer (port of pi `system-prompt-sanitizer.ts` +
//! `skill-prompt-sanitizer.ts`). This is pi's prompt-injection / context-pollution defense: once the
//! active tool set has been shaped, the system prompt the model sees is stripped of any advertised
//! capability the policy has HIDDEN — the "Available tools:" section + denied-tool "Guidelines:"
//! bullets ([`tools::sanitize_available_tools_section`]) and `ask`/`deny` skills in the
//! `<available_skills>` block ([`skills::resolve_skill_prompt_entries`]). It is NOT the enforcement
//! boundary (the `before_tool_call` gate is, `gate.rs`); it removes the model's incentive to attempt a
//! call it would only be blocked on, and keeps the advertised skill list honest.
//!
//! Wired at the live `before_agent_start` seam in `extension.rs` (`on_event(BeforeAgentStart)` returns
//! the sanitized prompt as a `[mutate]` and shapes the active tools via `HostServices::set_active_tools`).

pub mod skills;
pub mod tools;

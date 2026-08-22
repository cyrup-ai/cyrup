use super::*;

impl TranscriptView {
    /// Start a live `!`/`!!` bash execution block (replaces any prior uncommitted one). `cancel_hint`
    /// / `expand_hint` are the live key labels for the running + expand hints.
    pub fn start_bash(
        &mut self,
        command: impl Into<String>,
        excluded: bool,
        cancel_hint: Option<String>,
        expand_hint: Option<String>,
    ) {
        self.bump_render_generation();
        self.bash = Some(BashExecution::new(command, excluded));
        self.bash_cancel_hint = cancel_hint;
        self.bash_expand_hint = expand_hint;
    }

    /// Append a streamed chunk to the live bash block (`appendOutput`). No-op if none is live.
    pub fn bash_append(&mut self, chunk: &str) {
        self.bump_render_generation();
        if let Some(b) = self.bash.as_mut() {
            b.append_output(chunk);
        }
    }

    /// Mark the live bash block finished (`setComplete`). No-op if none is live.
    ///
    /// X13 — `truncated`/`full_output_path` are `setComplete`'s third and fourth arguments
    /// (`bash-execution.ts:98-103`), fed upstream from `result.truncated` / `result.fullOutputPath`
    /// (`interactive-mode.ts:6307-6312`). They drive the `Output truncated. Full output: …` status
    /// row. See [`Self::bash_complete_simple`] for the `!` path, which has no spool of its own.
    pub fn bash_complete(
        &mut self,
        exit_code: Option<i32>,
        cancelled: bool,
        truncated: bool,
        full_output_path: Option<String>,
    ) {
        self.bump_render_generation();
        if let Some(b) = self.bash.as_mut() {
            b.set_complete(exit_code, cancelled, truncated, full_output_path);
        }
    }

    /// [`Self::bash_complete`] with no truncation report — Pi's `catch` arm
    /// (`interactive-mode.ts:6357` `setComplete(undefined, false)`), and the shape the interactive
    /// `!` runner uses while it has no spool file to point at.
    pub fn bash_complete_simple(&mut self, exit_code: Option<i32>, cancelled: bool) {
        self.bump_render_generation();
        self.bash_complete(exit_code, cancelled, false, None);
    }

    /// Whether a bash block is live (running or finished-but-uncommitted).
    pub fn has_bash(&self) -> bool {
        self.bash.is_some()
    }

    /// Whether the live bash block is still running.
    pub fn bash_running(&self) -> bool {
        self.bash.as_ref().is_some_and(BashExecution::is_running)
    }

    /// The live bash block (test/inspection access).
    pub fn bash(&self) -> Option<&BashExecution> {
        self.bash.as_ref()
    }

    /// Toggle the live bash block's expansion (`Ctrl+O`); returns the new state if a block is live.
    pub fn toggle_bash_expanded(&mut self) -> Option<bool> {
        self.bump_render_generation();
        self.bash.as_mut().map(|b| {
            let next = !b.expanded();
            b.set_expanded(next);
            next
        })
    }

    /// Set the live bash block's expansion ABSOLUTELY — TUI-038. `setToolsExpanded` broadcasts one
    /// value to every `isExpandable` child of `loadedResourcesContainer` and `chatContainer`
    /// (`interactive-mode.ts:4040-4046` @v0.84.1), and the bash component is one of them
    /// (`components/bash-execution.ts:29` `private expanded = false`, `setExpanded` at `:70`). It is
    /// a fan-out upstream, not a choice between the bash block and the tool blocks. No-op when no
    /// block is live.
    pub fn set_bash_expanded(&mut self, expanded: bool) {
        self.bump_render_generation();
        if let Some(b) = self.bash.as_mut() {
            b.set_expanded(expanded);
        }
    }

    /// Commit the live bash block to scrollback (called once it has finished). A still-running block
    /// is committed as-is (e.g. on interrupt). No-op when none is live.
    pub fn commit_bash(&mut self) {
        self.bump_render_generation();
        if let Some(b) = self.bash.take() {
            self.pending.push(Entry::Bash(b));
        }
    }

    /// Commit an ALREADY-FINISHED `!`/`!!` execution straight to scrollback, without going through
    /// the live block. This is the replay path for a persisted `bashExecution` message (Pi
    /// `addMessageToChat`'s `bashExecution` arm — `new BashExecutionComponent(command, ui,
    /// excludeFromContext)` + `appendOutput(output)` + `setComplete(...)`, interactive-mode.ts:3310-3322),
    /// so a resumed session shows the user's own `!` commands as bash blocks instead of the
    /// ``Ran `cmd` ``  prose `convertToLlm` renders them to for the model.
    ///
    /// X13 — the persisted `bashExecution` message carries `truncated` and `fullOutputPath`, and
    /// upstream replays BOTH: `component.setComplete(message.exitCode, message.cancelled,
    /// message.truncated ? {truncated:true} : undefined, message.fullOutputPath)`
    /// (`interactive-mode.ts:3460-3465`). cyrup dropped them, so a resumed session lost the pointer
    /// to where the full output was spooled.
    #[allow(
        clippy::too_many_arguments,
        reason = "upstream's own arity: `new BashExecutionComponent(command, ui, excludeFromContext)`                   + `appendOutput(output)` + `setComplete(exitCode, cancelled, truncationResult,                   fullOutputPath)` (interactive-mode.ts:3454-3465), collapsed into one replay call"
    )]
    pub fn push_bash_execution(
        &mut self,
        command: impl Into<String>,
        excluded: bool,
        output: &str,
        exit_code: Option<i32>,
        cancelled: bool,
        truncated: bool,
        full_output_path: Option<String>,
    ) {
        self.bump_render_generation();
        let mut b = BashExecution::new(command, excluded);
        if !output.is_empty() {
            b.append_output(output);
        }
        b.set_complete(exit_code, cancelled, truncated, full_output_path);
        self.pending.push(Entry::Bash(b));
    }
}

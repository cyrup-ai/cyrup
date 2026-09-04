use super::*;

impl TranscriptView {
    /// Record a tool starting (live in the viewport): name + the raw call args (`ToolExecutionStart`).
    /// The args drive the per-tool `renderCall` header (path/command/pattern/range/…).
    ///
    /// Prefer [`Self::push_tool_start_rendered`] with the call's `toolCallId` wherever one is in
    /// hand — see [`ToolRun::call_id`]. This id-less form pairs its result by tool name alone, which
    /// cannot distinguish two concurrent calls to the same tool.
    pub fn push_tool_start(&mut self, name: impl Into<String>, args: Value) {
        self.bump_render_generation();
        self.push_tool_start_rendered(name, None, args, None);
    }

    /// [`Self::push_tool_start`] with the call's `toolCallId` and the CALL text an extension's
    /// registered renderer produced (EXT-006).
    ///
    /// `call_id` is the key the matching result is resolved by ([`ToolRun::call_id`]; Pi files each
    /// `ToolExecutionComponent` under `content.id`, interactive-mode.ts:3473). `rendered` replaces
    /// the built-in per-tool header for this run; `None` keeps the built-in dispatch (Pi prefers the
    /// extension's `renderCall` when the tool declares one, tool-execution.ts:81-112).
    pub fn push_tool_start_rendered(
        &mut self,
        name: impl Into<String>,
        call_id: Option<String>,
        args: Value,
        rendered: Option<String>,
    ) {
        self.push_tool_start_defined(name, call_id, args, rendered, None);
    }

    /// [`Self::push_tool_start_rendered`] plus what the definition registry answered for this tool
    /// name — Pi `hasRendererDefinition()` (tool-execution.ts:103-105) and `getRenderShell()`
    /// (`:108-116`) in one value, see [`ToolRun::definition`].
    ///
    /// The two forms above default it to `None`, which is the shape-preserving value for a caller
    /// with no registry in hand: an unknown name keeps drawing through `formatToolExecution`, and
    /// a known one keeps its shell. Every production path has the session —
    /// [`crate::App::ingest_session_event_owned`] resolves it off the live `getToolDefinition`
    /// registry per tool start, and the `/resume` replay walk reads the map that same bind cached.
    pub fn push_tool_start_defined(
        &mut self,
        name: impl Into<String>,
        call_id: Option<String>,
        args: Value,
        rendered: Option<String>,
        definition: Option<ToolRenderKind>,
    ) {
        self.bump_render_generation();
        self.active_tools.push(ToolRun {
            name: name.into(),
            call_id,
            args,
            result: None,
            is_error: false,
            done: false,
            started_at: Some(std::time::Instant::now()),
            duration_ms: None,
            rendered_call: rendered,
            rendered_result: None,
            definition,
            preview: None,
            images: Vec::new(),
        });
    }

    /// Update a running tool's partial result (`ToolExecutionUpdate`): the raw partial result value,
    /// rendered by the tool's `renderResult` with `isPartial = true`.
    ///
    /// Routed to the run whose [`call_id`](ToolRun::call_id) matches, as Pi does
    /// (`this.pendingTools.get(event.toolCallId)`, interactive-mode.ts:3104); `None` falls back to
    /// the latest still-running tool.
    pub fn push_tool_update(&mut self, call_id: Option<&str>, partial: Option<Value>) {
        self.bump_render_generation();
        let run = match call_id {
            Some(id) => self
                .active_tools
                .iter_mut()
                .find(|r| !r.done && r.call_id.as_deref() == Some(id)),
            None => self.active_tools.iter_mut().rev().find(|r| !r.done),
        };
        if let Some(run) = run
            && partial.is_some()
        {
            run.result = partial;
        }
    }

    /// Whether any live tool run is currently drawing a ticking `Elapsed …` figure, i.e. whether the
    /// frame goes stale on its own and must be repainted on a timer.
    ///
    /// This is Pi's `setInterval(() => context.invalidate(), 1000)` condition, verbatim: bash's
    /// `renderResult` arms that interval exactly when `state.startedAt !== undefined &&
    /// options.isPartial` and clears it on the final result (bash.ts:471-479). The `result.is_some()`
    /// term is upstream's `if (this.result)` gate on `renderResult` running at all
    /// (tool-execution.ts:281) — bash's initial empty update satisfies it immediately (bash.ts:384).
    ///
    /// Gates [`crate::App::run`]'s elapsed tick, so an idle session — or one running any tool but
    /// `bash` — never pays for a redraw.
    pub fn has_running_elapsed_tool(&self) -> bool {
        self.active_tools
            .iter()
            .any(|r| !r.done && r.name == "bash" && r.started_at.is_some() && r.result.is_some())
    }

    /// Attach `edit`'s pre-execution diff preview to a still-running call — Pi `setEditPreview`
    /// (edit.ts:263-280), the sink its `renderCall`'s `computeEditsDiff(...).then(...)` writes into
    /// (`:378-386`).
    ///
    /// `preview` is `Ok(diff)` or `Err(message)` (Pi's `EditDiffResult | EditDiffError`). Routed by
    /// `toolCallId` like every other per-run update ([`Self::push_tool_update`]); `None` falls back
    /// to the latest still-running tool. A run that has already finished is skipped — Pi drops a
    /// late preview by comparing `previewArgsKey` against the request key (`:381`), and once the
    /// result is in it is the result diff that renders (`formatEditResult`, `:220-226`).
    pub fn set_edit_preview(&mut self, call_id: Option<&str>, preview: Result<String, String>) {
        self.bump_render_generation();
        let run = match call_id {
            Some(id) => self
                .active_tools
                .iter_mut()
                .find(|r| !r.done && r.call_id.as_deref() == Some(id)),
            None => self.active_tools.iter_mut().rev().find(|r| !r.done),
        };
        if let Some(run) = run {
            run.preview = Some(preview);
        }
    }

    /// Record a tool finishing: attach the raw result/error to the matching live run, else a fresh
    /// done entry so a missed start never drops the result. Freezes the run duration for the bash
    /// `Took …` footer.
    ///
    /// Prefer [`Self::push_tool_end_rendered`] with the result's `toolCallId` — see
    /// [`ToolRun::call_id`] and [`Self::pending_run_mut`].
    pub fn push_tool_end(
        &mut self,
        name: impl Into<String>,
        is_error: bool,
        result: Option<Value>,
    ) {
        self.bump_render_generation();
        self.push_tool_end_rendered(name, None, is_error, result, None);
    }

    /// [`Self::push_tool_end`] with the result's `toolCallId` and the RESULT text an extension's
    /// registered renderer produced (EXT-006; Pi `renderResult`, extensions/types.ts:475-481).
    ///
    /// `call_id` selects the run this result belongs to — Pi's
    /// `renderedPendingTools.get(message.toolCallId)` (interactive-mode.ts:3483) / `pendingTools.get
    /// (event.toolCallId)` (`:3113`). `rendered = None` keeps the built-in body.
    pub fn push_tool_end_rendered(
        &mut self,
        name: impl Into<String>,
        call_id: Option<&str>,
        is_error: bool,
        result: Option<Value>,
        rendered: Option<String>,
    ) {
        self.bump_render_generation();
        let name = name.into();
        // Decode the result's `image` content blocks ONCE here (`tool-execution.ts:331-350`), not on
        // every frame — a screenshot-sized PNG must never be re-decoded per redraw.
        let images = result
            .as_ref()
            .map(decode_result_images)
            .unwrap_or_default();
        if let Some(run) = self.pending_run_mut(&name, call_id) {
            run.done = true;
            run.is_error = is_error;
            run.result = result;
            run.duration_ms = run.started_at.map(|s| s.elapsed().as_millis() as u64);
            run.rendered_result = rendered;
            run.images = images;
        } else {
            self.active_tools.push(ToolRun {
                name,
                call_id: call_id.map(str::to_string),
                args: Value::Null,
                result,
                is_error,
                done: true,
                started_at: None,
                duration_ms: None,
                rendered_call: None,
                rendered_result: rendered,
                // A result whose START was missed carries no registry answer; `None` keeps the
                // pre-existing `formatToolExecution` shape for an unknown name, and a built-in name
                // is dispatched by the built-in table regardless.
                definition: None,
                preview: None,
                images,
            });
        }
    }

    /// Resolve the still-running tool run a result belongs to.
    ///
    /// Pi's rule, exactly: a result is looked up by its `toolCallId` and by nothing else
    /// (interactive-mode.ts:3483 on replay, `:3113` live), because one assistant turn routinely
    /// issues several calls to the SAME tool and only the id tells them apart.
    ///
    /// The two fallbacks below never fire for a real provider turn (every `ToolCall` carries an
    /// `id`); they exist so a caller with no id in hand — a test, or a `ToolExecutionEnd` whose
    /// start was dropped — still lands somewhere sensible rather than nowhere:
    ///
    /// * `call_id: Some(id)` matches that id; failing that, a same-name run that carries NO id at
    ///   all (an id-less start being completed by an id-carrying end). It never falls back to a run
    ///   bearing a *different* id — that is precisely the mispairing this exists to prevent.
    /// * `call_id: None` takes the latest still-running run of that name (the pre-id behavior).
    fn pending_run_mut(&mut self, name: &str, call_id: Option<&str>) -> Option<&mut ToolRun> {
        match call_id {
            Some(id) => {
                if let Some(idx) = self
                    .active_tools
                    .iter()
                    .position(|r| !r.done && r.call_id.as_deref() == Some(id))
                {
                    return self.active_tools.get_mut(idx);
                }
                self.active_tools
                    .iter_mut()
                    .rev()
                    .find(|r| !r.done && r.call_id.is_none() && r.name == name)
            }
            None => self
                .active_tools
                .iter_mut()
                .rev()
                .find(|r| !r.done && r.name == name),
        }
    }

    /// Commit the active turn's tool executions into scrollback (called when the turn ends). Each
    /// becomes an [`Entry::Tool`]; still-running tools are committed as-is (marked done).
    pub fn commit_tools(&mut self) {
        self.bump_render_generation();
        for mut run in self.active_tools.drain(..) {
            run.done = true;
            self.pending.push(Entry::Tool(run));
        }
    }

    /// Progressively commit the LEADING run of already-finished tool executions to scrollback WHILE
    /// the turn is still live, so the inline viewport keeps only the actively-running tail (the
    /// currently-executing tool + any tools queued behind it) instead of stacking every completed
    /// tool of a long multi-tool turn until `AgentEnd`. This is the faithful port of Pi's behavior:
    /// each `ToolExecutionComponent` (packages/coding-agent/src/modes/interactive/components/
    /// tool-execution.ts:13) is a persistent child that, as later tool/text lines append below it,
    /// scrolls up past the diff's viewport top and lives in native scrollback thereafter
    /// (packages/tui/src/tui.ts:1455 `if (firstChanged < prevViewportTop) fullRender`). cyrup's
    /// idiomatic-ratatui equivalent (ADR-0001) is `Terminal::insert_before` for each finished entry —
    /// driven here by moving the finished leading tools into `pending` for the next
    /// [`drain_committed`](Self::drain_committed) → `insert_before` flush.
    ///
    /// Only the LEADING contiguous run of `done` tools is drained (stopping at the first still-running
    /// tool), so scrollback order always equals call order even under hypothetical parallel/interleaved
    /// tools — a still-running earlier tool blocks committing a finished later one ahead of it.
    ///
    /// Guarded on `streaming.is_none()`: a tool is never committed ahead of still-uncommitted assistant
    /// text of the same step. The confirmed event ordering (the assistant stream's terminal
    /// `StreamEvent::Done` → `commit_assistant` fires BEFORE any `ToolExecutionStart` of that step)
    /// keeps `streaming` clear whenever a tool finishes, so this guard is a safety net that also holds
    /// under interleaving.
    pub fn commit_finished_leading_tools(&mut self) {
        self.bump_render_generation();
        if self.streaming.is_some() {
            return;
        }
        let split = self
            .active_tools
            .iter()
            .position(|run| !run.done)
            .unwrap_or(self.active_tools.len());
        for run in self.active_tools.drain(..split) {
            self.pending.push(Entry::Tool(run));
        }
    }

    /// The active (live) tool executions for the current turn (test/inspection access).
    pub fn active_tools(&self) -> &[ToolRun] {
        &self.active_tools
    }

    /// Toggle the tool-output expansion (`Ctrl+O`); returns the new state.
    pub fn toggle_tool_expanded(&mut self) -> bool {
        self.bump_render_generation();
        self.tool_expanded = !self.tool_expanded;
        self.tool_expanded
    }

    /// Set the tool-output expansion absolutely — Pi `setToolsExpanded(expanded)`
    /// (`interactive-mode.ts:3887-3903`), the extension-driven counterpart of the `Ctrl+O` toggle.
    /// Returns whether the value actually changed (Pi's `if (expanded === this.toolOutputExpanded)
    /// return` early-out, `:3888`), which the caller uses to decide whether to echo Pi's
    /// `Tool output: expanded|collapsed` status line.
    pub fn set_tool_expanded(&mut self, expanded: bool) -> bool {
        self.bump_render_generation();
        let changed = self.tool_expanded != expanded;
        self.tool_expanded = expanded;
        changed
    }

    /// The live tool-output expansion (Pi `this.toolOutputExpanded`, `interactive-mode.ts:442`).
    ///
    /// X14 — read by the shell when it builds the [`ImageOpts`] it flushes committed entries with,
    /// so a branch/compaction summary honours the flag in force when it is PAINTED rather than the
    /// one that happened to be set when it was pushed (`setToolsExpanded`'s re-broadcast to every
    /// `chatContainer` child, `:4032-4046`).
    pub fn tool_expanded(&self) -> bool {
        self.tool_expanded
    }
}

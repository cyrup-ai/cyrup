//! The per-session subagent spawn budget (pi `SubagentState.subagentSpawns`): reservation,
//! grants, snapshots and reset.

use crate::extension::executor::SubagentExecutor;

/// SUBA-046 — one session's subagent spawn budget is now
/// [`crate::exec::spawn_budget::SpawnBudgetCounters`] (pi `SubagentState.subagentSpawns`,
/// `shared/types.ts:842`), which carries the resolved configured limit, the granted allowance and
/// the bounded grant log beside the count. The old two-field local struct could express neither a
/// grant nor "unlimited", which is why an exhausted cap was terminal for the whole session.
pub(crate) type SpawnBudget = crate::exec::spawn_budget::SpawnBudgetCounters;
impl SubagentExecutor {
    /// Reserve `requested` subagent spawns against THIS session's budget (pi `reserveSubagentSpawns`,
    /// `runs/foreground/subagent-executor.ts:266-282`), returning pi's exact over-limit text on
    /// breach and `Ok(())` otherwise.
    ///
    /// The reservation is charged UP FRONT (`count = used + requested`) and never refunded — pi
    /// deliberately bills a run at dispatch, so a fan-out that later fails still consumes its share
    /// of the session's budget. `requested == 0` is a no-op (pi's `if (input.requested <= 0) return
    /// undefined`), so a call that spawns nothing (e.g. an empty/`action` shape) never touches the
    /// counter. The comparison is pi's strict `used + requested > maxSpawns`, so a call that lands
    /// exactly ON the cap is allowed.
    ///
    /// The session identity is [`Self::root_parent_session`] — cyrup's analog of pi's
    /// `state.currentSessionId` (captured from the live `HostServices::session_id` at the root
    /// `SessionStart`). A change of session id resets the counter in place, exactly as pi's
    /// `if (state.subagentSpawns?.sessionId !== sessionId)` guard does, so a long-lived process that
    /// starts a second session starts that session with a fresh budget.
    ///
    /// # Call sites (SUBA-002)
    /// EVERY route into execution charges here, so the budget cannot be walked around by picking a
    /// different surface — upstream gets that property structurally (every slash handler funnels
    /// back through `executor.execute`, `slash/slash-commands.ts` `runSlashSubagent` ->
    /// `requestSlashRun` -> `extension/index.ts:512-517` -> `executeSubagentCollapsed`), this crate
    /// gets it by charging at each independent entry point exactly once:
    ///
    /// * the `subagent` TOOL — [`cyrup_core::Tool::execute`], after the dispatch guard and the
    ///   mode-exclusivity gate, covering its SINGLE/PARALLEL/CHAIN routes
    ///   ([`crate::extension::tool::task_items::count_requested_subagent_spawns`]);
    /// * `/run` — [`crate::extension::SubagentsExtension::dispatch_slash`]'s `Run` arm, billed `1` for both the
    ///   foreground and the `--background` shape;
    /// * `/chain`, `/parallel`, `/run-chain` — [`crate::extension::SubagentsExtension::run_or_background_chain`], the
    ///   single wrapper all three share, billed over the lowered graph
    ///   ([`crate::extension::tool::task_items::count_graph_requested_spawns`]).
    ///
    /// The tool path never re-enters the slash wrapper (it reaches
    /// [`Self::run_or_background_graph`] via `route_chain_mode`/`route_parallel_mode`), so no
    /// dispatch is billed twice.
    ///
    /// # Errors
    /// The over-limit notice (pi's verbatim string) when the declared run does not fit in what
    /// remains of the effective cap (configured + granted).
    ///
    /// # SUBA-046
    /// The comparison, the "unlimited" case and the message all moved into
    /// [`crate::exec::spawn_budget`], which is the port of pi's `spawn-budget.ts`. Two behaviours
    /// changed with the move, both toward upstream: a configured `0` now means UNLIMITED (it used
    /// to refuse the first delegation of every session), and the refusal text is upstream's
    /// v0.43.0 one, which points at the grant path that now exists instead of saying "complete the
    /// work directly".
    pub fn reserve_subagent_spawns(&self, requested: u32, max_spawns: u32) -> Result<(), String> {
        use crate::exec::spawn_budget as budget_ops;
        if requested == 0 {
            return Ok(());
        }
        let session_id = self.root_parent_session();
        let mut budget = self
            .spawn_budget
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        budget_ops::session_state(&mut budget, session_id.as_deref(), max_spawns);
        let snapshot = budget_ops::snapshot(&budget);
        budget_ops::preflight_spawn_budget(&snapshot, requested)?;
        budget.count = budget.count.saturating_add(requested);
        Ok(())
    }

    /// SUBA-046 — the read-only [`crate::exec::spawn_budget::SpawnBudgetSnapshot`] for the current
    /// session (pi `getSpawnBudgetSnapshot`, `spawn-budget.ts:29`), so the cap is observable in a
    /// tool result's `details.spawnBudget` even when no grant is being requested.
    #[must_use]
    pub fn spawn_budget_snapshot(
        &self,
        max_spawns: u32,
    ) -> crate::exec::spawn_budget::SpawnBudgetSnapshot {
        use crate::exec::spawn_budget as budget_ops;
        let session_id = self.root_parent_session();
        let mut budget = self
            .spawn_budget
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        budget_ops::session_state(&mut budget, session_id.as_deref(), max_spawns);
        budget_ops::snapshot(&budget)
    }

    /// SUBA-046 — apply an explicit grant to the current session's cap (pi `grantSpawnBudget`,
    /// `spawn-budget.ts:107`), after the caller has confirmed it.
    ///
    /// # Errors
    /// pi's three verbatim grant refusals (non-positive amount, no configured cap, or more than
    /// the remaining grant allowance), re-checked here against the live counters so a grant that
    /// went stale while the confirmation dialog was open cannot be applied.
    pub fn grant_subagent_spawn_budget(
        &self,
        additional: i64,
        max_spawns: u32,
    ) -> Result<crate::exec::spawn_budget::SpawnBudgetSnapshot, String> {
        use crate::exec::spawn_budget as budget_ops;
        let session_id = self.root_parent_session();
        let mut budget = self
            .spawn_budget
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        budget_ops::session_state(&mut budget, session_id.as_deref(), max_spawns);
        budget_ops::grant_spawn_budget(&mut budget, additional, crate::time::now_epoch_millis())
    }

    /// SUBA-046 / pi `hasActiveSubagentChildren` (`subagent-executor.ts:433-437` @v0.43.0) — is any
    /// child of THIS session queued or running right now?
    ///
    /// A spawn-budget grant is refused while children are in flight, because the preview the user
    /// confirmed would be measured against a `used` count that is still moving. pi's predicate is
    /// `state.subagentInProgress || state.foregroundControls.size > 0 || any async/fleet job in
    /// {queued,running}`; cyrup's equivalents are the live `foreground_controls` map (a foreground
    /// run holds an entry for exactly as long as it is driving) and the async tracker's own
    /// snapshot.
    #[must_use]
    pub fn has_active_subagent_children(&self) -> bool {
        let foreground_active = !self
            .foreground_controls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty();
        if foreground_active {
            return true;
        }
        self.tracker.snapshot().into_iter().any(|job| {
            job.last_status.is_some_and(|status| {
                matches!(
                    status.state,
                    crate::background::RunState::Queued | crate::background::RunState::Running
                )
            })
        })
    }

    /// Reset this session's spawn budget to zero under the CURRENT session id (pi
    /// `resetSessionState`'s `state.subagentSpawns = { sessionId: state.currentSessionId, count: 0 }`,
    /// `extension/index.ts:700-706` @v0.43.0). Called from the `SessionStart` handler right after the
    /// parent-session anchor is captured, so a second session on a long-lived process (SDK embedder /
    /// test harness) starts from a clean budget even when neither session had a resolvable id — the
    /// case [`Self::reserve_subagent_spawns`]' own id-change guard cannot detect on its own.
    pub fn reset_spawn_budget(&self) {
        let session_id = self.root_parent_session();
        // SUBA-046: the reset clears the GRANTS and the grant log with the count — pi rebuilds the
        // whole `subagentSpawns` record, so a grant made in the previous session cannot survive
        // into the new one. `configured_limit` is left unresolved here and re-resolved by
        // `session_state` on the next reserve/snapshot, which is the only place that knows the
        // effective config.
        *self
            .spawn_budget
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = SpawnBudget {
            session_id,
            ..SpawnBudget::default()
        };
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use crate::background::RunMode;
    use crate::error::SubagentError;
    use crate::extension::host::SubagentsExtension;
    use crate::extension::testsupport::bare_single_step;
    use crate::registration::SubagentExtensionConfig;
    use crate::spawn::chain_graph::RunnerStep;

    /// SUBA-002 regression, chain-shaped slash surfaces: `/chain`, `/parallel` and `/run-chain` all
    /// funnel through [`crate::extension::SubagentsExtension::run_or_background_chain`], which pre-fix reached
    /// `run_or_background_graph` with no spawn charge whatsoever. Each is now billed over the
    /// LOWERED graph by [`crate::extension::tool::task_items::count_graph_requested_spawns`], applying pi's per-step rule
    /// (`countRequestedSubagentSpawns`, `subagent-executor.ts:439-447`) arm for arm — asserted
    /// through the `N requested` field of the refusal notice, and for both `background: false` and
    /// `background: true`, since the charge sits ahead of that split.
    #[tokio::test]
    async fn slash_chain_surfaces_bill_the_lowered_graph_against_the_session_budget() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = SubagentsExtension::with_config_and_cwd(
            SubagentExtensionConfig {
                max_subagent_spawns_per_session: 1,
                chain: Some(crate::registration::ExtensionChainConfig {
                    dynamic_fanout: Some(crate::registration::DynamicFanoutConfig {
                        max_items: Some(7),
                    }),
                }),
                ..SubagentExtensionConfig::default()
            },
            dir.path().to_path_buf(),
        );

        let dynamic = |max_items: Option<u32>| {
            RunnerStep::DynamicGroup(crate::spawn::chain_graph::DynamicGroupSpec {
                expand: "{outputs.targets}/items".to_string(),
                template: Box::new(bare_single_step("ghost", "Handle {item}")),
                collect: "gathered".to_string(),
                concurrency: 2,
                item: None,
                key: None,
                max_items,
                on_empty: crate::spawn::chain_graph::OnEmpty::Skip,
                collect_schema: None,
                fail_fast: false,
                acceptance: None,
            })
        };

        // (graph, expected `N requested`) — two sequential steps bill 1 each; a static parallel
        // group bills its width; a dynamic group bills `expand.maxItems` when it has one, else the
        // configured `chain.dynamicFanout.maxItems` (7 here).
        let cases: Vec<(Vec<RunnerStep>, u32)> = vec![
            (
                vec![
                    RunnerStep::SingleStep(bare_single_step("ghost", "a")),
                    RunnerStep::SingleStep(bare_single_step("ghost", "b")),
                ],
                2,
            ),
            (
                vec![RunnerStep::ParallelGroup(
                    crate::spawn::chain_graph::ParallelGroupSpec {
                        steps: vec![
                            bare_single_step("ghost", "a"),
                            bare_single_step("ghost", "b"),
                            bare_single_step("ghost", "c"),
                        ],
                        concurrency: 3,
                        fail_fast: false,
                        worktree: false,
                    },
                )],
                3,
            ),
            (vec![dynamic(Some(5))], 5),
            (vec![dynamic(None)], 7),
            (
                vec![
                    RunnerStep::SingleStep(bare_single_step("ghost", "a")),
                    dynamic(Some(5)),
                ],
                6,
            ),
        ];

        for (graph, expected) in cases {
            for background in [false, true] {
                ext.executor().reset_spawn_budget();
                let err = ext
                    .run_or_background_chain(
                        dir.path(),
                        graph.clone(),
                        RunMode::Chain,
                        None,
                        background,
                        None,
                    )
                    .await
                    .expect_err("the graph is over the 1-spawn budget");
                assert!(
                    matches!(err, SubagentError::SpawnLimitExceeded(_)),
                    "background={background}: expected a spawn-budget refusal, got: {err:?}"
                );
                assert_eq!(
                    err.to_string(),
                    format!(
                        "Subagent spawn limit reached for this session (0/1 used, {expected} \
                         requested). 1 remaining; the declared run cannot fit, so no children were \
                         started. Grant budget explicitly from the root interactive session or \
                         start a new session."
                    ),
                    "background={background}: the lowered graph must bill pi's per-step count"
                );
            }
        }

        // An EMPTY graph short-circuits ahead of the charge and never touches the counter (pi's
        // `if (input.requested <= 0) return undefined`), so the budget is still untouched after it.
        ext.executor().reset_spawn_budget();
        let empty = ext
            .run_or_background_chain(dir.path(), vec![], RunMode::Chain, None, false, None)
            .await
            .expect("an empty graph is not an error");
        assert_eq!(empty, "chain has no steps to run");
        assert!(
            ext.executor().reserve_subagent_spawns(1, 1).is_ok(),
            "an empty graph must not have consumed the session's spawn"
        );
    }
}

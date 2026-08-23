---
stage: new
status: done
updated: 2026-08-23 00:06
---

# Decompose the 11 monolith modules in cyrup-mcp and cyrup-ext-subagents (59,024 lines) along the section banners already written into them

> Found by an eight-lens workspace hygiene sweep. Every count below was reproduced
> against the tree before this task was filed.
> **Priority:** medium · **Effort:** large
> **Crates:** `cyrup-mcp`, `cyrup-ext-subagents`

The workspace's two worst crates by module size, both with the split points already named in their own comments. All line counts below re-verified with `wc -l`.

**cyrup-mcp — 33,270 lines in 6 files, and the crate has zero subdirectories** (`find crates/cyrup-mcp -type d` returns only the crate root and `src`). All 21 modules are flat files:

```
7594  src/proxy.rs          (17 numbered section banners)
6037  src/ui.rs             (12 banners)
5637  src/config.rs         (15 banners — EXCLUDED, owned by queued MCP_CONFIG_LENIENT_TYPES)
5004  src/credentials.rs    (no banners; splits by impl group)
4961  src/runtime.rs
4897  src/oauth.rs          (13 banners)
4777  src/server_manager.rs (8 banners)
```

Each file carries explicit `// N - Title` banners between `// ====` rules — proxy.rs runs `0 - Constants`, `1 - details.error vocabulary`, `2 - ToolMetadata and the tool-name grammar`, `3 - search-ranking`, `4 - The collaborator seam`, … through `16 - Conformance`. Every banner maps to one module file with no logic change. credentials.rs splits instead by impl group: five separate `impl McpAuthStore` blocks plus the `AuthSecretStore` trait and its four implementations — three of which (MemorySecretStore, FailingRemoveStore, LinuxKeyringRecoveryStore) are **test doubles compiled into production**.

**cyrup-ext-subagents — 25,754 lines in 5 files.** The `exec/`, `discovery/`, `background/` and `spawn/` directories were already turned into packages, but each package's own hub file was never split and is now the largest file in it:

```
7926  src/exec/mod.rs               (workspace's single largest .rs file)
5543  src/discovery/management.rs   (17 banners)
4587  src/background/runner_main.rs
4015  src/background/control.rs
3683  src/background/mod.rs         (12 banners)
```

`exec/mod.rs` is 4,588 production lines plus a 3,339-line inline test module, holding four unrelated concerns marked by its own banners: `AgentConfig/RunOptions/SingleResult` (:226), `AgentProgress` live fold (:1002), the `SubagentSpawner` seam (:1302), `run_sync`'s fallback loop (:3573), `plan_batch` (:4525). `background/mod.rs` is 12 banner-separated type families (RunId :155, RunMode :233, RunState :253, telemetry :466, StepStatus :776, RunStatus :851, ResultFile :1076, RunDir/RunPaths :1118, workflow-graph snapshot :1576, run-id prefix resolution :2353, run-history :2527). `discovery/management.rs` covers agent CRUD, frontmatter write-back, chain CRUD, dispatch/renderers and the handleList/Get/Models/Create/Update/Delete surface.

Run this **after** WORKSPACE_FMT_BASELINE so the moves land on already-formatted code and the diffs stay reviewable as pure relocations.

## Acceptance Criteria

- [ ] cyrup-mcp has real subdirectories: proxy.rs, ui.rs, credentials.rs, runtime.rs, oauth.rs and server_manager.rs are each replaced by a package whose modules follow the file's existing numbered banners; no single .rs file in crates/cyrup-mcp/src exceeds ~1,500 lines except the excluded config.rs
- [ ] cyrup-ext-subagents' five hub files are split the same way; crates/cyrup-ext-subagents/src/exec/mod.rs no longer exceeds ~1,500 lines and its 3,339-line inline test module is moved to the crate's src/tests/ directory
- [ ] crates/cyrup-mcp/src/config.rs is untouched by this task (owned by the queued MCP_CONFIG_LENIENT_TYPES)
- [ ] The three test doubles in credentials.rs (MemorySecretStore, FailingRemoveStore, LinuxKeyringRecoveryStore) are moved behind #[cfg(test)] or a test-support module so they are no longer compiled into production builds
- [ ] Each split is a pure relocation: public item paths are preserved via re-exports from the original module path, and no function body is edited in the same commit as a move
- [ ] `cargo build --workspace`, `cargo test --workspace` and `cargo clippy --workspace --all-targets` all produce the same results as before the split

## Verifying command

```bash
cd /home/user/cyrup && wc -l crates/cyrup-mcp/src/*.rs | sort -rn | head -8 && find crates/cyrup-mcp -type d && wc -l crates/cyrup-ext-subagents/src/exec/mod.rs crates/cyrup-ext-subagents/src/discovery/management.rs crates/cyrup-ext-subagents/src/background/runner_main.rs crates/cyrup-ext-subagents/src/background/control.rs crates/cyrup-ext-subagents/src/background/mod.rs
```

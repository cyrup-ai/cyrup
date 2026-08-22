---
stage: new
status: done
updated: 2026-08-22 23:06
---

# Upstream Parity Backlog — pi-permission-system v0.8.0 → v27.0.0

## The headline

`cyrup-permission-system` is a 1:1 port of `pi-permission-system` **v0.8.0**. Upstream shipped
**v27.0.0 on 2026-08-21** — 27 major releases later — and moved from the standalone
`gotgenes/pi-permission-system` repo (archived at v5.18.1, HEAD `f1d2f61`, "docs: add monorepo move
notice") into the [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) monorepo.

Reference checkouts in this repo (gitignored, re-cloneable):

- `./tmp/pi-permission-system` — the standalone repo at its final HEAD (**archived**, v5.18.1)
- `./tmp/pi-packages/packages/pi-permission-system` — **the live source**, v27.0.0, 144 files / ~20k lines

Upstream restructured along the way: policy now compiles to an ordered ruleset over a flat
`permission` block, bash is parsed with tree-sitter into per-command units, and the code is
organised into `access-intent/`, `authority/`, `handlers/` and `presentation/`. The port's 24
modules still mirror the v0.8.0 shape.

Two structural breaks are worth knowing before reading any individual task, because they explain
most of the rest:

- **v4.0.0 replaced the config schema.** `defaultPolicy` / `tools` / `bash` / `mcp` / `skills` /
  `special` became a single flat `permission` object keyed by surface name, with a cross-cutting
  `path` surface. The port still ships and parses the legacy shape — verified directly:
  `crates/cyrup-permission-system/config/config.example.json` has exactly the six legacy keys, and
  `rg 'shellTools|authorizerChain|piInfrastructureReadPaths|reviewLogFieldMaxWidth|doublePressToConfirm'`
  over the crate returns nothing. **A v27 config file is not readable by this crate.**
- **v3.0.0 moved the config and log paths** to `extensions/<id>/` locations.

## How this backlog was produced

A 14-agent workflow: 7 agents each read one upstream area and diffed it against the Rust crate,
then a paired adversary per area tried to **refute** each claim by finding the capability in the
port under another name. 49 raw findings merged to **40** tasks.

**Read this caveat before trusting a task.** Four adversaries completed and refuted nothing — but
they did correct severities (several were pulled down from medium to low for "nothing extra gets
through the gate, only observability is lost"). **Three adversaries — `policy-config`, `prompts-ui`
and `logging-redaction` — died on a session limit and never ran.** Every task from those areas is
marked *single-source* below and carries the same warning in its own file. Re-check those against
the port before starting work.

## The backlog

40 tasks — 5 critical, 12 high, 13 medium, 10 low. 23 adversarially verified, 17 single-source.

| Severity | Task | Gap | Verification |
| --- | --- | --- | --- |
| critical | [PORT_BASH_COMMAND_ENUMERATION](./PORT_BASH_COMMAND_ENUMERATION.md) | Port the bash command enumerator (chain + nested-execution units) | verified |
| critical | [PORT_CROSS_CUTTING_PATH_SURFACE](./PORT_CROSS_CUTTING_PATH_SURFACE.md) | Port the cross-cutting `path` surface gate | **single-source** |
| critical | [PORT_FLAT_PERMISSION_POLICY_MODEL](./PORT_FLAT_PERMISSION_POLICY_MODEL.md) | Port the flat `permission` policy model and its surface rules | **single-source** |
| critical | [PORT_LOG_KEY_NAME_REDACTION](./PORT_LOG_KEY_NAME_REDACTION.md) | Port the key-name log redactor into the JSONL writer | **single-source** |
| critical | [PORT_PROJECT_TRUST_GATING](./PORT_PROJECT_TRUST_GATING.md) | Gate project-scoped config and policy on project trust | verified |
| high | [BOUND_AND_DE_ECHO_AGENT_DENIAL_TEXT](./BOUND_AND_DE_ECHO_AGENT_DENIAL_TEXT.md) | Stop echoing the full bash command in agent-facing denial text | **single-source** |
| high | [PORT_BASH_PATH_PROJECTION](./PORT_BASH_PATH_PROJECTION.md) | Port the bash path projection and its external_directory gate | verified |
| high | [PORT_DENY_WITH_REASON_PATTERN_VALUES](./PORT_DENY_WITH_REASON_PATTERN_VALUES.md) | Support deny-with-reason pattern values | **single-source** |
| high | [PORT_DOUBLE_PRESS_TO_CONFIRM](./PORT_DOUBLE_PRESS_TO_CONFIRM.md) | Port the double-press-to-confirm approval guard | **single-source** |
| high | [PORT_FAIL_CLOSED_CLAMP_INVALID_SCOPE](./PORT_FAIL_CLOSED_CLAMP_INVALID_SCOPE.md) | Fail closed when a non-global config scope is invalid | verified |
| high | [PORT_HOME_PREFIX_EXPANSION_IN_PATTERNS](./PORT_HOME_PREFIX_EXPANSION_IN_PATTERNS.md) | Expand ~ / $HOME / ${HOME} in rule patterns before matching | **single-source** |
| high | [PORT_OWNER_ONLY_LOG_FILE_MODES](./PORT_OWNER_ONLY_LOG_FILE_MODES.md) | Restrict the log file and logs directory to owner-only mode | **single-source** |
| high | [PORT_PATH_CANONICALIZATION](./PORT_PATH_CANONICALIZATION.md) | Resolve symlinks for the path boundary and policy match values | verified |
| high | [PORT_REVIEW_LOG_FIELD_WIDTH_CAP](./PORT_REVIEW_LOG_FIELD_WIDTH_CAP.md) | Port the review-log field width cap and its config knob | **single-source** |
| high | [PORT_SHELL_TOOLS_ALIAS_CONFIG](./PORT_SHELL_TOOLS_ALIAS_CONFIG.md) | Port the `shellTools` shell-alias registration | verified |
| high | [PORT_SKILL_INPUT_GATE](./PORT_SKILL_INPUT_GATE.md) | Gate /skill:<name> user input instead of granting a bypass | verified |
| high | [PORT_TOOL_ACCESS_EXTRACTOR_REGISTRY](./PORT_TOOL_ACCESS_EXTRACTOR_REGISTRY.md) | Port the tool access-extractor registry seam | **single-source** |
| medium | [NARROW_AVAILABLE_TOOLS_SECTION](./NARROW_AVAILABLE_TOOLS_SECTION.md) | Narrow the Available tools section instead of deleting it wholesale | **single-source** |
| medium | [PORT_BASH_COMMENT_STRIPPING](./PORT_BASH_COMMENT_STRIPPING.md) | Strip leading bash comment lines from the rule match value | verified |
| medium | [PORT_BASH_WRAPPER_FLOOR](./PORT_BASH_WRAPPER_FLOOR.md) | Floor indirection and opaque-shell wrappers to ask | verified |
| medium | [PORT_CROSS_EXTENSION_SERVICE_LIFECYCLE](./PORT_CROSS_EXTENSION_SERVICE_LIFECYCLE.md) | Publish the cross-extension permissions service and ready event | verified |
| medium | [PORT_DECISION_AUDIT_SESSION_SUMMARY](./PORT_DECISION_AUDIT_SESSION_SUMMARY.md) | Port the per-session decision audit and its session_summary line | verified |
| medium | [PORT_FORWARDED_ACCESS_INTENT_SERVING_POLICY](./PORT_FORWARDED_ACCESS_INTENT_SERVING_POLICY.md) | Resolve forwarded requests against the parent's recorded authority | verified |
| medium | [PORT_PROMPT_RENDER_BUDGET_AND_DIALOG](./PORT_PROMPT_RENDER_BUDGET_AND_DIALOG.md) | Port the configurable prompt render budget and structured dialog render | **single-source** |
| medium | [PORT_SESSION_APPROVAL_PATTERN_SUGGESTER](./PORT_SESSION_APPROVAL_PATTERN_SUGGESTER.md) | Port the session-approval pattern suggester and bash arity table | **single-source** |
| medium | [PORT_STRICT_CONFIG_VALIDATION_AND_ISSUES](./PORT_STRICT_CONFIG_VALIDATION_AND_ISSUES.md) | Add strict config validation and accumulated config issues | **single-source** |
| medium | [PORT_TOOL_INPUT_FORMATTER_REGISTRY](./PORT_TOOL_INPUT_FORMATTER_REGISTRY.md) | Port the tool-input formatter registry and built-in MCP formatter | **single-source** |
| medium | [PORT_TOOL_INPUT_PATH_EXTRACTION](./PORT_TOOL_INPUT_PATH_EXTRACTION.md) | Extract the gated path from MCP and extension tool inputs | verified |
| medium | [RELAY_FORWARDED_PROMPT_PAYLOAD](./RELAY_FORWARDED_PROMPT_PAYLOAD.md) | Relay the child's prompt payload and approval suggestion on a forwarded ask | verified |
| medium | [SPLIT_REVIEW_LOG_FROM_DEBUG_LOG](./SPLIT_REVIEW_LOG_FROM_DEBUG_LOG.md) | Write the review stream to its own permission-review.jsonl | **single-source** |
| low | [ADD_PERMISSION_REVIEW_LOG_TOGGLE](./ADD_PERMISSION_REVIEW_LOG_TOGGLE.md) | Add the permissionReviewLog config toggle | **single-source** |
| low | [PORT_AUTHORIZER_CHAIN_SUBSYSTEM](./PORT_AUTHORIZER_CHAIN_SUBSYSTEM.md) | Port the authorizer chain: registry, composition, and delegation envelope | verified |
| low | [PORT_CONFIRMATION_UNAVAILABLE_MARKER](./PORT_CONFIRMATION_UNAVAILABLE_MARKER.md) | Mark abandoned forwards as confirmation-unavailable with a reason | verified |
| low | [PORT_DECISION_EVENT_CHANNEL](./PORT_DECISION_EVENT_CHANNEL.md) | Broadcast every gate resolution on the decision channel | verified |
| low | [PORT_DECISION_SOURCE_PROVENANCE](./PORT_DECISION_SOURCE_PROVENANCE.md) | Stamp and relay a DecisionSource on every permission decision | verified |
| low | [PORT_FORWARDED_SESSION_APPROVAL_SCOPE](./PORT_FORWARDED_SESSION_APPROVAL_SCOPE.md) | Relay the session-approval suggestion and offer the grant-scope choice | verified |
| low | [PORT_FORWARDING_SERVING_HEARTBEAT_LIVENESS](./PORT_FORWARDING_SERVING_HEARTBEAT_LIVENESS.md) | Publish and read a serving heartbeat so a child abandons a dead parent | verified |
| low | [PORT_RESOLVED_CONFIG_PATH_AUDIT](./PORT_RESOLVED_CONFIG_PATH_AUDIT.md) | Log the resolved policy paths and legacy-file detection at start | verified |
| low | [PORT_SAFE_SYSTEM_PATHS](./PORT_SAFE_SYSTEM_PATHS.md) | Exempt safe system device paths from the external-directory check | verified |
| low | [RECORD_GATE_ERROR_AUDIT_ENTRY](./RECORD_GATE_ERROR_AUDIT_ENTRY.md) | Write a gate_error audit entry when the gate itself fails | verified |

## Suggested order

1. **PORT_BASH_COMMAND_ENUMERATION** first, alone. It is the one finding where a rule the operator
   wrote is actively bypassable: the port matches the whole command string against one wildcard, so
   an `echo *` allow also matches `echo hi && rm -rf /`. Everything else is a capability the port
   lacks; this is a capability it has but that does not hold.
2. **PORT_PROJECT_TRUST_GATING** next — opening an untrusted repo lets that repo's checked-in policy
   widen the allow set. The host already exposes `HostCtx::is_project_trusted()`, so it is a
   thread-through, but read the task's note about the `HostCtxRich::default()` trap first.
3. **PORT_FLAT_PERMISSION_POLICY_MODEL** and **PORT_CROSS_CUTTING_PATH_SURFACE** together — they are
   one change, and most of the `high` items below assume the flat model exists. Doing these early
   avoids porting features twice.
4. Then the rest by severity. The `low` cluster is almost entirely observability (decision events,
   audit counters, gate_error records) — real, but nothing reaches the tool because of them.

## Re-running this analysis

```bash
git -C tmp/pi-packages pull                    # refresh upstream
# then re-run the gap-analysis workflow (7 compare + 7 verify agents)
```

The three missing verifiers can be re-run alone against the single-source tasks.

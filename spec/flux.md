# Flux for cyrup — Port Plan

> Source feature: **Flux** (code-puppy) — a structured, file-persisted AI development pipeline:
> `new → ask → split → aug → exec → qa → tests → commit → create-pr`, driven entirely by
> markdown slash-commands whose state lives in `~/.flux/<project>/`.
>
> Target: port the feature to **cyrup**.
>
> **STATUS OF THIS REVISION (researched against the live tree):**
>
> 1. **The precursor has ALREADY LANDED.** Recursive, namespaced prompt-template discovery
>    (`prompts/flux/new.md` → `/flux/new`) is implemented in `cyrup-resources` today — see
>    §0.1 for the file:line evidence. The flat `flux-*` fallback is **dead**; do not build it.
> 2. **The complete code-puppy source is vendored under [`../tmp/code-puppy/`](../tmp/code-puppy/)**
>    (all 18 command files, all 3 renderer scripts, the installer, and the dispatch engine),
>    plus Pi's [`../tmp/pi/prompt-templates.ts`](../tmp/pi/prompt-templates.ts). Every porting
>    rule below cites the exact source file.
> 3. **No core changes are required at all.** All of it is one new crate plus its wiring. This
>    plan touches **zero** existing `crates/*/src` files except `crates/cyrup/src/main.rs`.

---

## ⚠️ ARCHITECTURE CORRECTION — read before §3

**One home: `crates/cyrup-flux`.** Everything — the 15 prompt templates, the `flux` skill, the
reference docs, the three native renderers, the `ctrl+f` overlay and the `ask_user_question` tool
— lives in a single crate inside the cyrup workspace, and reaches sessions through the
extension's `ResourcesDiscover` contribution.

The two-phase split this document describes below — a standalone **git repo** `cyrup-flux`
installed with `cyrup install` (§3.3 "Phase 1"), later duplicated into a crate named
`cyrup-ext-flux` and kept `rsync`-synchronised with it (§3.4.1 "Bundling") — is **superseded**.
There is no separate repo, no `cyrup.toml`, no `cyrup install`, and no second copy to keep in
sync.

Read the rest of this document with these substitutions:

| this document says | read as |
|---|---|
| the `cyrup-flux` **package** / **package repo** | `crates/cyrup-flux/resources/` |
| the `cyrup-ext-flux` **crate** | `crates/cyrup-flux` |
| `cyrup.toml` `[resources]` declaration | the `ResourcesDiscover` hook (§3.4.1) |
| `cyrup install <path>` / `cyrup list` | nothing — the content ships with the binary |
| "Phase 1" / "Phase 2" | one continuous sequence of tasks |
| §3.4.5 "One block in `main.rs`" | **three** blocks, one per `AppMode` |
| §3.4.1's `fn name(&self) -> &str` | `fn id(&self) -> ExtensionId` |

Everything else here — §0 research, §1 source analysis, §3.1 naming, §3.2 state model, §3.3's
per-file porting rules 1–9, §3.4.2–3.4.4's renderer/tool designs, §5 risks — is unchanged and
still authoritative. Section numbers are preserved because the task files cite them.

**The executable plan is [`flux/README.md`](flux/README.md) and `flux/FLUX_01.md` …
`flux/FLUX_12.md`** (twelve tasks; the old thirteenth existed only to synchronise the two homes).
Where a task file and this document disagree, the task file wins — it was re-verified against the
live tree and this one was not.

---

## 0. Research findings — verified against the live tree

### 0.1 The namespaced-template precursor is DONE (do not re-implement)

The precursor spec (`spec/namespaced-prompt-templates.md`, referenced from code comments; the
file itself is not in `spec/`) is fully implemented:

- **Namespaced name derivation** — [`PromptTemplate::load_with_root`](../crates/cyrup-resources/src/prompt.rs)
  (`crates/cyrup-resources/src/prompt.rs:57-105`): the template name is the path **relative to
  the scan root**, `.md` stripped, components joined with `/`, case preserved. The
  `[CYRUP-DELTA]` module doc (`prompt.rs:14-16`) states this explicitly: `flux/new.md` under a
  root registers as `/flux/new`. Edge cases are already handled: a file literally named `.md`
  errors instead of producing an empty name, and non-UTF-8 components fail loudly.
- **Recursive scanning with skip rules** — [`scan_prompt_dir`](../crates/cyrup-resources/src/discovery.rs)
  (`crates/cyrup-resources/src/discovery.rs:1772-1830`): descends into subdirectories, skips
  `.`- and `_`-prefixed dirs and `node_modules` (so `prompts/flux/_docs/` never registers —
  the exact code-puppy `_SKIP_DIR_PREFIXES = ("_", ".")` semantic,
  [`../tmp/code-puppy/customizable_commands/register_callbacks.py`](../tmp/code-puppy/customizable_commands/register_callbacks.py)),
  caps depth at `MAX_PROMPT_NAMESPACE_DEPTH = 8` (`discovery.rs:1756`), never follows directory
  symlinks (cycle-proof), loads file symlinks to regular `.md` targets, and sorts children per
  directory for deterministic first-wins tie-breaking.
- **All discovery roots use it**: global loose (`<agent_dir>/prompts`, `discovery.rs:855`),
  project loose (`.cyrup/prompts` ancestor walk, `discovery.rs:1294-1307`), package-conventional
  and manifest-declared `prompts` dirs (`discovery.rs:1102`), and CLI `--prompt-template`
  directories (`add_prompt_path`, `discovery.rs:1929-1958`).

**Consequence:** §3.1's namespaced naming works today with zero core changes. Work item 0 from
the previous revision of this plan is **complete** and is removed from the work table.

### 0.2 Dispatch order and command-name grammar (verified)

- **Extension commands run BEFORE prompt-template expansion.** Prompt preflight
  ([`AgentSession::prepare`](../crates/cyrup-session-svc/src/session.rs), `session.rs:948-955`)
  calls [`try_execute_extension_command`](../crates/cyrup-session-svc/src/session.rs) (`session.rs:1040`)
  first; only on a miss does the text reach [`expand_input_text`](../crates/cyrup-session-svc/src/session.rs)
  (`session.rs:1255`), which runs skill expansion then
  [`expand_prompt_template`](../crates/cyrup-resources/src/prompt.rs). Native commands are tried
  before WASM guest commands (`session.rs:1048-1093`).
  **Consequence:** the Phase 2 native `/flux/status` command shadows any same-named template —
  exactly the coexistence semantics this plan needs.
- **Command names may contain `/`.** Dispatch splits the submission on the first space
  (`session.rs:1041-1042`: `body.split_once(' ')`), so any non-space name — `flux/status`
  included — routes. [`InitApi::register_command`](../crates/cyrup-ext/src/native.rs) (`native.rs:318`)
  takes an arbitrary name string; [`CommandDescriptor`](../crates/cyrup-ext/src/registry.rs)
  (`registry.rs:94-98`) is just `{ description, completions }`.
- **Command output channel.** [`NativeExtension::execute_command`](../crates/cyrup-ext/src/native.rs)
  (`native.rs:580-587`): `Ok(Some(text))` is surfaced as an Info notification, `Ok(None)` is
  silent, `Err` becomes an Error notification prefixed `command:<name>: `. A handled command
  never reaches the model as a prompt.
- **Template expansion is real substitution** (unlike code-puppy's literal `$ARGUMENTS` text):
  `substitute_args` (`prompt.rs`) ports Pi's `$1 $2 $@ $ARGUMENTS ${N:-default} ${@:N} ${@:N:L}`
  with a quote-aware tokenizer — 1:1 with [`../tmp/pi/prompt-templates.ts`](../tmp/pi/prompt-templates.ts).
  The trailing `=================\n$ARGUMENTS` block in every flux command works unchanged.

### 0.3 Tool inventory and the rename map (verified)

cyrup's built-in agent tools live in [`../crates/cyrup-tools/src/tools/`](../crates/cyrup-tools/src/tools/):
`bash`, `read`, `write`, `edit`, `grep`, `find`, `ls` (`globmatch.rs` is an internal helper,
not a tool). The `subagent` tool (foreground/background/parallel fan-out, chains) comes from
`cyrup-ext-subagents`. Exact rename map, with occurrence counts grepped from the vendored source:

| code-puppy name | cyrup name | Sites (per command file) |
|---|---|---|
| `create_file` | `write` | 6 — ask, aug, exec, new, review, split |
| `replace_in_file` | `edit` | 4 — ask, aug, exec, review |
| `read_file` | `read` | 1 — auto-pilot |
| `invoke_agent` | `subagent` | 4 — aug, exec, qa, review |
| `ask_user_question` | **Phase 2 native tool** (§3.4.4); Phase 1 plain-text interim | 25 — new×2, ask, aug, exec, qa, config×3, commit×2, review, address-feedback, rebase×5, squash-commits×7 |

`review.md` also carries `run_in_background: false` wording for `invoke_agent` — port as
"foreground `subagent` calls" (see §3.3 rule 4).

**The `invoke_agent` → `subagent` row renames onto a tool with a DIFFERENT availability** (gap-analysis
`FLUX-002`): `invoke_agent` is a core code-puppy tool (`code_puppy/tools/__init__.py` `TOOL_REGISTRY`),
always present, whereas `subagent` is registered only behind `cyrup-ext-subagents`' opt-in
`is_installed` gate (`CYRUP_SUBAGENTS` truthy, or a `subagents/config.json` at user/project scope) —
and this crate is default-on (§2.4). The four renamed templates therefore carry an availability
pre-condition upstream never needed: check the tool list for `subagent` BEFORE calling it and, when it
is absent, tell the user once and take the sequential single-task path (`exec`/`aug`/`qa`) or review the
groups in-line (`review`). Arming subagents from flux was rejected: it would have a default-on extension
silently flip the gate on an OS-process-spawning subsystem. Pinned by
`crates/cyrup-flux/tests/flux_002_subagent_fallback.rs`.

### 0.4 Extension surfaces (verified)

- **Native extensions**: [`NativeExtension`](../crates/cyrup-ext/src/native.rs) —
  `init(&self, api: &mut InitApi)` (`native.rs:461`),
  `on_event(&self, ev: &HostEvent, ctx: &HostCtx) -> HookOutcome` (`native.rs:463`; returns
  `HookOutcome` directly, **not** a `Result`), `execute_command` (`native.rs:580`),
  `execute_shortcut` (`native.rs:589+`), and the late-binding
  `set_host_services(Arc<dyn HostServices>)` (`native.rs:683`). Wired by the binary via
  [`SessionBuilder::with_native_extension`](../crates/cyrup-session-svc/src/builder.rs)
  (`builder.rs:561`); the load-order pattern to copy is in
  [`../crates/cyrup/src/main.rs`](../crates/cyrup/src/main.rs) (`main.rs:692-717`: subagents →
  prompt-runtime → intercom → permission-system).
- **Interactive dialogs**: [`HostServices`](../crates/cyrup-ext/src/host/services.rs) —
  `select(prompt, options: &Value, opts) -> Option<String>` (`services.rs:203`),
  `confirm(prompt, message, opts) -> bool` (`services.rs:195`),
  `input(prompt, placeholder, opts) -> Option<String>` (`services.rs:200`),
  `open_overlay(Box<dyn InteractiveOverlay>) -> bool` (`services.rs:254`),
  `notify` (`services.rs:304`), `set_widget` (`services.rs:323`),
  `inject_message` (`services.rs:437`), `exec` (`services.rs:467`).
  All dialog methods are **sync** (the host runs them on its own executor) and default-denied.
- **Single human-interaction slot**: [`HumanInteractionLock`](../crates/cyrup-ext/src/host/services.rs)
  (`services.rs:153-187`), reached via `HostServices::human_interaction_lock()`
  (`services.rs:395`). Acquire the guard across every blocking dialog.
- **`UiKind::Select` carries a flat array of option STRINGS and replies with the chosen
  string** — option `description`s have no carrier and are dropped (the `oauth_select`
  CYRUP-DELTA comment, `cyrup-session-svc/src/host_services.rs:1696-1702`). The label→display
  projection and answer→label back-mapping pattern is at
  `host_services.rs:1703-1730`; §3.4.4 reuses it verbatim.
- **ANSI is stripped from externally supplied text** by the TUI
  ([`strip_ansi`](../crates/cyrup-tui/src/ansi.rs), the port of Pi's `stripAnsi`). The status
  renderer port therefore keeps the Python script's **layout and Unicode glyphs** (box-drawing
  rules, 🔄/✅/🔁/● — all survive stripping) and drops the ANSI color layer for text channels;
  real themed color lives in the interactive overlay, which draws ratatui lines directly
  (§3.4.3).
- **Bundled resources hook**: an extension answers `HostEvent::ResourcesDiscover`
  ([`HostEvent`](../crates/cyrup-ext/src/event.rs): `ResourcesDiscover = 5`) with
  `HookOutcome::Handled(HandledValue(json!({ "skillPaths": [...], "promptPaths": [...] })))` —
  the exact pattern is [`cyrup-ext-subagents/src/extension.rs`](../crates/cyrup-ext-subagents/src/extension.rs)
  (`extension.rs:11013-11033`). The host concatenates all contributions and loads each at
  `ResourceScope::Discovered` (precedence rank 6 — **below** user/project/package resources, so
  the bundled files are a floor, never an override) via
  [`ResourceRegistry::extend`](../crates/cyrup-resources/src/discovery.rs).
- **CRITICAL GOTCHA — contribute the DIRECTORY, not the files.** `ResourceRegistry::extend`
  routes each contributed path through [`add_prompt_path`](../crates/cyrup-resources/src/discovery.rs)
  (`discovery.rs:1929-1958`): a **file** path loads via `PromptTemplate::load` → **basename**
  name (`new` — the `flux/` namespace is LOST); a **directory** path loads via
  `scan_prompt_root` → recursive namespaced names (`flux/new`). `cyrup-ext-subagents`
  contributes individual files (flat names are fine for its recipes); **`cyrup-ext-flux` MUST
  contribute its `resources/prompts/` directory** as a single `promptPaths` entry, or every
  template registers under the wrong name. The bundled-root resolution helper to copy is
  [`registration/resources.rs`](../crates/cyrup-ext-subagents/src/registration/resources.rs)
  (`bundled_resources_dir()` — `CARGO_MANIFEST_DIR`-relative `resources/` with an env
  override).
- **MCP**: MCP tools reach the model through the pi-mcp-adapter extension capability
  (http-client + proc capabilities, `cyrup-ext/src/host/services.rs:477,501`), so a configured
  Jira MCP server's `get_issue_by_key_or_link` appears to the model exactly as in code-puppy.
  The conditional MCP branch in `flux/new.md` ports verbatim.

### 0.5 Vendored sources (citation map)

| Source | Vendored at |
|---|---|
| 18 flux command files | [`../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/`](../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/) |
| Reference docs (`README`, `pipeline`, `cheatsheet`, `synopsis`) | [`../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/_docs/`](../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/_docs/) |
| `flux_status.py` (345 lines) | [`../tmp/code-puppy/flux_bootstrap/bundled/scripts/flux_status.py`](../tmp/code-puppy/flux_bootstrap/bundled/scripts/flux_status.py) |
| `flux_cheatsheet.py` (248 lines) | [`../tmp/code-puppy/flux_bootstrap/bundled/scripts/flux_cheatsheet.py`](../tmp/code-puppy/flux_bootstrap/bundled/scripts/flux_cheatsheet.py) |
| `flux_about.py` (154 lines) | [`../tmp/code-puppy/flux_bootstrap/bundled/scripts/flux_about.py`](../tmp/code-puppy/flux_bootstrap/bundled/scripts/flux_about.py) |
| `installer.py` + `register_callbacks.py` (flux_bootstrap) | [`../tmp/code-puppy/flux_bootstrap/`](../tmp/code-puppy/flux_bootstrap/) |
| `customizable_commands` dispatch engine | [`../tmp/code-puppy/customizable_commands/register_callbacks.py`](../tmp/code-puppy/customizable_commands/register_callbacks.py) |
| Pi `prompt-templates.ts` | [`../tmp/pi/prompt-templates.ts`](../tmp/pi/prompt-templates.ts) |

---

## 1. What Flux actually is (source analysis)

Flux is **not** a plugin with pipeline logic in code. It is three thin mechanisms plus a set of
prompt files; all intelligence lives in the prompts and the agent.

### 1.1 Distribution — `flux_bootstrap` plugin

[`../tmp/code-puppy/flux_bootstrap/`](../tmp/code-puppy/flux_bootstrap/) ships the whole feature
as package data:

```
flux_bootstrap/bundled/
├── commands/flux/          # 18 markdown command files (+ _docs/ reference docs)
│   ├── new.md  ask.md  split.md  aug.md  exec.md  qa.md  tests.md  commit.md
│   ├── create-pr.md  review.md  address-feedback.md  auto-pilot.md  rebase.md
│   ├── squash-commits.md  config.md  status.md  cheatsheet.md  about.md
│   └── _docs/{README,pipeline,cheatsheet,synopsis}.md
└── scripts/
    ├── flux_status.py      # ANSI status panel renderer (reads ~/.flux, prints table)
    ├── flux_cheatsheet.py  # ANSI cheatsheet renderer (parses _docs/pipeline.md at runtime)
    └── flux_about.py       # Rich-markdown about renderer (reads about.md at runtime)
```

[`installer.py`](../tmp/code-puppy/flux_bootstrap/installer.py) copies the payload into
`~/.code_puppy/{commands,scripts}/` on the `startup` hook, version-gated by a marker file,
idempotent, non-destructive (SHA-256 manifest; user-edited files get `.bak` backups;
pre-existing user files are never claimed), flock-guarded against concurrent first-launch
installs. **All of this is replaced by cyrup's package manager (Phase 1) and built-in
registration (Phase 2) — nothing of it is ported.**

### 1.2 Dispatch — `customizable_commands` plugin

[`../tmp/code-puppy/customizable_commands/register_callbacks.py`](../tmp/code-puppy/customizable_commands/register_callbacks.py) is
the engine cyrup's prompt-template system already replaces:

- Recursively loads `*.md` from `~/.code_puppy/commands/` (global, trusted) and project dirs.
  **Subdirectories become namespaces**: `commands/flux/new.md` → `/flux/new`. Dirs prefixed
  `_`/`.` are skipped (so `flux/_docs/` never registers). — **cyrup equivalent: §0.1.**
- YAML frontmatter: `name`, `description`, `argument-hint`, and optionally `exec: <shell>`.
- **No `exec:`** → the body is fed to the agent as user input, with
  `"\n\nAdditional context: {args}"` appended; `$ARGUMENTS` is literal text the model resolves.
  — **cyrup equivalent: prompt templates, with REAL substitution (§0.2).**
- **`exec:` present** (trusted global dir only) → runs the shell line directly
  (`subprocess.run(shell=True)`, 30 s timeout, `{python}`/`{script:…}`/`{command:…}` token
  expansion). The agent is bypassed. Used by `status`, `cheatsheet`, `about`.
  — **cyrup equivalent: Phase 2 native commands (§3.4). Deliberately NOT ported as a
  frontmatter directive** (§5.3).

### 1.3 State model — `~/.flux/`

Everything persists under a per-project directory; the LLM computes it via a bash snippet
embedded in every command:

```bash
FLUX_ROOT="${FLUX_ROOT:-$HOME/.flux}"
FLUX_DIR=$(printf '%s' "$(pwd -P)" | tr -cs 'a-zA-Z0-9' '-')   # flatten cwd
FLUX_BASE="$FLUX_ROOT/$FLUX_DIR"
```

`tr -cs 'a-zA-Z0-9' '-'` = complement-squeeze: **every maximal run of non-alphanumerics becomes
ONE `-`** (leading `/` → leading `-`). The Python renderer's equivalent is
`re.sub(r"[^a-zA-Z0-9]+", "-", cwd)` ([`flux_status.py`](../tmp/code-puppy/flux_bootstrap/bundled/scripts/flux_status.py)
`flatten_cwd`). Both preserve case.

```
~/.flux/<flattened-cwd>/
├── config.env          # TEST_CMD=…          (written by /flux/config, read by /flux/tests)
├── stack.env           # detected tech stack (written once by aug/exec/qa/review)
├── session.env         # SESSION_TS=YYYY-MM-DD-HH-MM (written by /flux/new)
├── todo/*.md           # active task files
├── done/<SESSION_TS>/*.md   # QA-passed (and split-original) tasks, grouped by session
├── review/<severity>/*      # code-review output (critical/high/medium/low)
└── research/           # research-deliverable tasks
```

Each task file is markdown with YAML frontmatter `stage: <pipeline-step>`,
`status: done|in-progress|needs-rework|completed|complete`, `updated: YYYY-MM-DD HH:MM`. The
frontmatter is the pipeline state machine; `/flux/status` just renders it. (`split.md` uses
`status: complete` on the preserved original — the renderer's tolerant unknown-status path
handles it, §3.4.2.)

### 1.4 What the prompts ask of the harness

| Capability used by flux prompts | code-puppy tool | cyrup answer |
|---|---|---|
| Shell snippets (mkdir/ls/mv/git/gh) | `bash`-style shell tool | `bash` — identical |
| Write task files | `create_file` | `write` |
| Edit task frontmatter/body in place | `replace_in_file` | `edit` |
| Read files | `read_file` | `read` |
| Structured user questions (25 sites) | `ask_user_question` | **Phase 2 native tool (§3.4.4)**; Phase 1 plain-text interim |
| Parallel multi-task mode (`/flux/exec 3`) | `invoke_agent` | `subagent` (foreground, parallel) |
| Jira ticket fetch (optional, `/flux/new PROJ-123`) | Jira MCP `get_issue_by_key_or_link` | MCP via pi-mcp-adapter capability (§0.4) — branch kept verbatim |
| External CLIs | `gh`, `git`, `lsd`/`find`, `bun` | unchanged via `bash` |

Prompt-engineering conventions to preserve **verbatim** in the port:

- **MANDATORY OVERRIDE** preamble on ask/aug/exec/qa ("frontmatter does not mean done; run NOW").
- **HARD CONSTRAINTS** blocks: exact `FLUX_BASE` path reuse ("copy it character-for-character"),
  per-command file-touch allow-lists, **no-git** rule for exec/qa, scope guard (research tasks
  must not touch `src/`).
- Single-task vs multi-task argument grammar: empty → interactive pick; `all`/`1` → sequential;
  integer N>1 → N parallel subagents; otherwise filename with `$FLUX_BASE/todo/` prefix + `.md`
  suffix inference. The "pure integer" guard (`grep -qE '^[0-9]+$'`, "CMPAN_5 is a filename")
  is load-bearing prompt text — keep it.
- QA loop: 10/10 → frontmatter `stage: qa, status: completed` + `mv` to `done/$SESSION_TS/`;
  <10 → `needs-rework` + body rewritten to outstanding items only. The "`stage` MUST be the
  literal string `qa` — NEVER `done`" warning is load-bearing (the renderer groups by
  directory, not stage).
- Every command ends by proposing the next `/flux/…` step from a fixed whitelist
  ("Valid //flux commands: … Do NOT suggest any command not on this list."). Note the list
  omits `cheatsheet`/`about`/`squash-commits` — that is upstream's list; keep it byte-identical.

---

## 2. cyrup architecture — what we map onto

cyrup is a Rust port of the Pi harness (19 crates; agent loop, provider layer, tools, session
tree, TUI, WASM extension host, native extensions). The relevant surfaces, all verified in §0:

### 2.1 Prompt templates — the direct equivalent of code-puppy markdown commands

[`crates/cyrup-resources/src/prompt.rs`](../crates/cyrup-resources/src/prompt.rs) (ports Pi
[`prompt-templates.ts`](../tmp/pi/prompt-templates.ts) 1:1, plus the landed namespacing delta):

- A template is a `*.md` file under a scanned root; **name = root-relative path minus `.md`,
  `/`-joined** (§0.1). `prompts/flux/new.md` → `/flux/new`.
- Frontmatter: `description`, `argument-hint` (both surfaced in the command list). `name` in
  frontmatter is ignored (name comes from the path) — drop it when porting.
- Expansion: `/name args` → `substitute_args(body, args)` with real substitution
  (`$1 $2 $@ $ARGUMENTS ${N:-default} ${@:N} ${@:N:L}`).
- Wired in at prompt preflight: `AgentSession::expand_input_text`
  ([`session.rs:1255`](../crates/cyrup-session-svc/src/session.rs)) — AFTER extension-command
  dispatch (§0.2). Same semantics as code-puppy's `MarkdownCommandResult`.
- Surfaced in the TUI command list via `getCommands() = [...extensionCommands, ...templates,
  ...skills]` ([`host_services.rs:300-303`](../crates/cyrup-session-svc/src/host_services.rs)).

**Discovery roots** ([`discovery.rs`](../crates/cyrup-resources/src/discovery.rs)):

- Global: `<agent_dir>/prompts/` (i.e. `~/.cyrup/agent/prompts/`)
- Project: `.cyrup/prompts/` (ancestor walk from cwd)
- Packages: `<package>/prompts/` (manifest-declared or auto-discovered)
- CLI: `--prompt-template <path>`; settings `prompts` arrays can list/filter.

### 2.2 Package system — the distribution mechanism

[`crates/cyrup-resources/src/package/`](../crates/cyrup-resources/src/package/):

- A package is a directory with `cyrup.toml` (preferred) or `package.json` (`pi`/`cyrup` key)
  declaring `prompts`, `skills`, `themes`, `extensions`, `agents` paths; conventional dirs are
  auto-discovered without a manifest (`resolve_manifest`).
- `cyrup install <source>` (git/local; [`subcommands.rs`](../crates/cyrup/src/subcommands.rs) —
  `install`/`remove`/`uninstall`/`update`/`list`/`config`) installs into the global store or
  project scope, with a lockfile.
- Precedence: project > global > built-in, first-wins by normalized key
  ([`ResourceSet::build`](../crates/cyrup-resources/src/discovery.rs)).

This replaces `flux_bootstrap`'s hand-rolled installer entirely: versioned, lockfiled,
removable, update-able, and no startup-hook copy step.

### 2.3 Native extensions — for the parts prompts can't do

[`cyrup-ext/src/native.rs`](../crates/cyrup-ext/src/native.rs): in-process Rust extensions
implementing `NativeExtension`, registered by the binary via
`SessionBuilder::with_native_extension`. `InitApi` offers `register_tool`
(`native.rs:313`, overrides a same-named built-in), `register_command` (`native.rs:318`),
`register_shortcut` (`native.rs:366`), renderers, bus topics, subscriptions. The
`HostServices` dialog/overlay/notify surface and the `set_host_services` late-bind seam are
enumerated in §0.4. `cyrup-ext-subagents` is the structural template: one crate registering a
tool, 12+ slash commands, renderers, subscriptions, and bundled resources
([`extension.rs:11013-11033`](../crates/cyrup-ext-subagents/src/extension.rs)).

### 2.4 Gaps vs. code-puppy — final disposition

| code-puppy | cyrup status | Resolution (prescriptive) |
|---|---|---|
| `ask_user_question` tool | Does not exist | **Phase 2: native tool in `cyrup-ext-flux`** (§3.4.4). Phase 1: plain-text interim + `FLUX-GAP` markers. |
| `invoke_agent` | `subagent` tool — equivalent, richer | Rename in prompts (§3.3 rule 4). |
| `create_file` / `replace_in_file` / `read_file` | `write` / `edit` / `read` | Rename in prompts. |
| `exec:` frontmatter directive | Not supported (by design, §5.3) | Phase 2 native commands (§3.4.2–3.4.3). |
| Namespaced commands `/flux/new` | **Landed** (§0.1) | Use `/flux/<step>` spellings verbatim. |
| Jira MCP | MCP via pi-mcp-adapter capability (§0.4) | Keep the conditional branch in `flux/new.md` verbatim. |
| `ui-mode: flux-status` overlay (Wibey leftover) | n/a | Native overlay on a shortcut (§3.4.3). |

---

## 3. Port plan

### 3.1 Naming decision (final)

**Namespaced `/flux/<step>` spellings, byte-identical to code-puppy** —
`/flux/new`, `/flux/ask`, `/flux/split`, `/flux/aug`, `/flux/exec`, `/flux/qa`,
`/flux/tests`, `/flux/commit`, `/flux/create-pr`, `/flux/review`,
`/flux/address-feedback`, `/flux/auto-pilot`, `/flux/rebase`, `/flux/squash-commits`,
`/flux/config`, `/flux/status`, `/flux/cheatsheet`, `/flux/about`.

- Enabled by the already-landed recursive scanner (§0.1). Files live at `prompts/flux/<step>.md`
  in the package; reference docs at `prompts/flux/_docs/` (skipped by the `_`-prefix rule).
- Prompt bodies keep their original `/flux/…` cross-references **verbatim** — no rename sweep,
  no porting drift.
- The flat `flux-*` fallback from the previous revision is **removed** — the precursor is
  merged, so there is nothing to fall back to.

### 3.2 State model — keep byte-identical

Keep `FLUX_ROOT` (`~/.flux`, overridable via the `FLUX_ROOT` env var), the cwd-flattening rule,
the directory layout, the frontmatter schema, `config.env` / `stack.env` / `session.env`, and
the `done/<SESSION_TS>` move semantics **exactly as in code-puppy**. Rationale:

- The bash snippets in the prompts are already portable (POSIX sh, `tr`, `mkdir`, `mv`) and run
  under cyrup's `bash` tool unchanged.
- A user switching between code-puppy and cyrup on the same project sees one shared,
  crash-resumable task state — flux's core promise.
- The Phase 2 Rust renderer must parse the shared state tolerantly (§3.4.2), including
  code-puppy's variants (`status: complete` from split, missing frontmatter, odd `done/`
  dirnames passed through by `format_timestamp`).

### 3.3 The bundled content set (pure content, no core changes)

> **SUPERSEDED FRAMING** (see the correction banner at the top): the deliverable is not a git repo
> and is not installed. It is the crate's bundled resource tree,
> `crates/cyrup-flux/resources/`, contributed via `ResourcesDiscover`. The `cyrup.toml` block and
> the `cyrup install` validation below no longer apply. **Porting rules 1–9 are unchanged and are
> the authoritative per-file work list** — FLUX_02–FLUX_05 apply them verbatim.

Deliverable, as the task files implement it (`crates/cyrup-flux/resources/`):

```
crates/cyrup-flux/resources/
├── prompts/
│   └── flux/
│       ├── new.md  ask.md  split.md  aug.md  exec.md  qa.md
│       ├── tests.md  commit.md  create-pr.md  review.md
│       ├── address-feedback.md  auto-pilot.md  rebase.md
│       ├── squash-commits.md  config.md
│       └── _docs/            # README pipeline cheatsheet synopsis about (never registers)
└── skills/
    └── flux/
        ├── SKILL.md          # pipeline overview + when-to-use (distilled from _docs/)
        └── reference/        # the four _docs files, copied for /skill:flux readers
```

(status/cheatsheet/about are intentionally absent from `prompts/` — they must not invoke the
model; they are native commands. A template named `flux/status` would also be permanently
shadowed by the native command — §0.2 — so shipping one would be dead weight with a misleading
body.)

**No manifest.** The tree above is reached by the extension's `ResourcesDiscover` contribution
(§3.4.1), which hands the host the `prompts` DIRECTORY and the `skills/flux/SKILL.md` FILE.
There is no `cyrup.toml`: this is a crate, not an installable package. The superseded manifest
this section used to prescribe was:

```toml
# SUPERSEDED — do not create this file.
# [package] name = "cyrup-flux" …  [resources] prompts = ["prompts"]  skills = ["skills"]
```

**Per-file porting rules** (apply to all 15 templates; source files cited in §4):

1. **Frontmatter**: keep `description` and `argument-hint` (cyrup reads both). Drop `name`
   (cyrup derives it from the path). Drop `exec`/`ui-mode` (unsupported — only present on
   status/cheatsheet/about, which are not ported as templates).
2. **Command references**: none — `/flux/…` spellings carry over verbatim (§3.1).
3. **Argument substitution**: cyrup really substitutes `$ARGUMENTS` (and `$1`, `${@:2}`…). The
   trailing `=================\n$ARGUMENTS` block works as-is. Remove any wording that relies on
   code-puppy's "Additional context:" append behavior (none of the 15 files mention it — the
   substitution model is strictly richer; no edits needed here beyond the tool renames).
4. **Tool renames** (§0.3): `create_file`→`write`, `replace_in_file`→`edit`,
   `read_file`→`read`, `invoke_agent`→`subagent`. Where prompts say "use the `invoke_agent`
   tool … foreground / `run_in_background: false`", write "use the `subagent` tool — parallel
   foreground calls only; NEVER background" (the `subagent` tool supports foreground parallel
   fan-out natively; the flux refill loop is prompt-driven on top of it, §3.5).
5. **`ask_user_question`** (25 sites, §0.3): replace each with the interim instruction —
   "ask the user one question at a time in plain text with 2–4 lettered options (A/B/C/D, each
   with a one-line implication); wait for the reply before continuing" — and mark each site
   with an HTML comment `<!-- FLUX-GAP: ask_user_question -->` on the line above, so Phase 2
   upgrades them back to the real tool with one mechanical sweep (§3.4.4).
6. **Stack detection / tree listing**: keep the bash blocks byte-identical; they already fall
   back (`lsd … || find …`, `bun … || echo "JavaScript/TypeScript"`). cyrup's `bash` tool runs
   them unchanged.
7. **MCP/Jira branch** in [`new.md`](../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/new.md):
   keep verbatim, including the "not configured → stop" path (tool absence is detected the same
   way — the model sees its available tools).
8. **`_docs/`**: ship as `prompts/flux/_docs/` (skipped namespace, co-located reference docs)
   **and** distill into the `flux` skill (`skills/flux/SKILL.md` + `reference/` copies), so
   `/skill:flux` and auto-skill-loading surface the pipeline docs. `SKILL.md` frontmatter gets
   `name: flux` and a `description` covering when to load (pipeline orchestration, task files,
   `~/.flux` state); the body is the TL;DR from
   [`_docs/README.md`](../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/_docs/README.md)
   plus the command table from
   [`_docs/pipeline.md`](../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/_docs/pipeline.md),
   with `/flux/status`, `/flux/cheatsheet`, `/flux/about` listed as "native commands (Phase 2)"
   and the Phase 1 interim noted: `~/.flux` state can be inspected manually with
   `ls ~/.flux/$(printf '%s' "$(pwd -P)" | tr -cs 'a-zA-Z0-9' '-')/todo/`.
9. **Sweep verification** (one-time, part of doing the port — not a test suite): after the
   edits, run
   `rg -n 'create_file|replace_in_file|read_file|invoke_agent|ask_user_question' prompts/`
   in the package; the only remaining hits must be `ask_user_question` inside `FLUX-GAP`
   comments. Also `rg -n '//flux' prompts/` and confirm every cross-reference is single-slash.

**Smoke validation (manual, definitional):** build and install the binary
(`cargo build -p cyrup && cargo install --path crates/cyrup --force`), then in a scratch repo run
`cyrup -p "/flux/new add a dark mode toggle"` and confirm: the template expands through the
same preflight path (§0.2), the agent creates
`~/.flux/<flattened-scratch>/todo/DARK_MODE.md` with `stage: new, status: done` frontmatter and
writes `session.env`. Then walk `/flux/ask`, `/flux/split`, `/flux/aug`, `/flux/exec`,
`/flux/qa` on the scratch task and confirm the frontmatter transitions and the
`done/<SESSION_TS>/` move on a 10/10. Phase 1 is done when the full pipeline A loop runs
end-to-end against a scratch repo with correct on-disk state transitions.

### 3.4 The native extension crate

> **NAME CORRECTION**: the crate is **`crates/cyrup-flux`**, not `cyrup-ext-flux`, and it holds
> the bundled content of §3.3 as well as the native surfaces below — there is no second home.
> Two further corrections verified against the live tree: `NativeExtension`'s first method is
> `fn id(&self) -> ExtensionId` (not `fn name(&self) -> &str`), and the wiring in §3.4.5 is
> **three** blocks in `main.rs`, one per `AppMode`, not one. See `flux/FLUX_01.md` Facts 1 and 3.

Deliverable: `crates/cyrup-flux`, a **default-on built-in** wired in
[`crates/cyrup/src/main.rs`](../crates/cyrup/src/main.rs) next to the subagents wiring
(`main.rs:692-717`) via `factory_builder.with_native_extension(...)`. Modeled on
`cyrup-ext-subagents`' structure but far smaller. Default-on is the decision (not an option):
it is the only way to ship `/flux/status` and the question tool with no install step, and the
crate is small and inert until invoked.

Crate layout:

```
crates/cyrup-flux/
├── Cargo.toml
├── resources/
│   ├── prompts/flux/       # the 15 templates + _docs/ (the ONLY home; see "bundling" below)
│   └── skills/flux/        # SKILL.md + reference/
└── src/
    ├── lib.rs              # flux_extension() constructor
    ├── extension.rs        # NativeExtension impl (init/on_event/execute_command/execute_shortcut)
    ├── state.rs            # FLUX_BASE resolution + frontmatter/task/review/done model
    ├── render_status.rs    # port of flux_status.py
    ├── render_cheatsheet.rs# port of flux_cheatsheet.py (parses embedded pipeline.md)
    ├── render_about.rs     # port of flux_about.py (embeds about text)
    ├── overlay.rs          # InteractiveOverlay status panel (themed)
    └── ask_tool.rs         # ask_user_question Tool impl
```

#### 3.4.1 Extension skeleton (core pattern — copy this shape)

```rust
// extension.rs
use std::sync::{Arc, OnceLock};
use cyrup_ext::{
    CommandDescriptor, EventKind, ExtError, HandledValue, HookOutcome, HostCtx, HostEvent,
    InitApi, NativeExtension,
};
use cyrup_ext::host::HostServices;

pub struct FluxExtension {
    /// Late-bound by the host (native.rs:683) — the cyrup-ext-subagents OnceLock pattern
    /// (extension.rs:139, 751-757).
    host_services: Arc<OnceLock<Arc<dyn HostServices>>>,
}

#[async_trait::async_trait]
impl NativeExtension for FluxExtension {
    // CORRECTED: the trait's first method is `fn id(&self) -> ExtensionId` (native.rs:459).
    // `fn name(&self) -> &str` does not exist on `NativeExtension` — see flux/FLUX_01.md Fact 1.
    fn id(&self) -> ExtensionId { self.id.clone() }   // id: "cyrup-flux"

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::ResourcesDiscover]);
        let cmd = |d: &str| CommandDescriptor { description: d.into(), completions: vec![] };
        api.register_command("flux/status", cmd("Flux pipeline status panel (todo/done/review)"));
        api.register_command("flux/cheatsheet", cmd("Flux pipeline cheatsheet (stages A–D)"));
        api.register_command("flux/about", cmd("About the Flux pipeline"));
        api.register_shortcut("ctrl+f", Some("Flux status overlay".into()));
        api.register_tool(Arc::new(crate::ask_tool::AskUserQuestionTool::new(
            Arc::clone(&self.host_services),
        )));
        Ok(())
    }

    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        let _ = self.host_services.set(services);
    }

    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if matches!(ev, HostEvent::ResourcesDiscover { .. }) {
            // Contribute DIRECTORIES, not files — add_prompt_path loads a file by BASENAME
            // (losing the `flux/` namespace) and a directory via the recursive namespaced
            // scanner (discovery.rs:1929-1958). This is the load-bearing detail.
            return HookOutcome::Handled(HandledValue(serde_json::json!({
                "promptPaths": [crate::bundled_dir().join("prompts")],
                "skillPaths":  [crate::bundled_dir().join("skills/flux/SKILL.md")],
            })));
        }
        HookOutcome::Noop
    }

    async fn execute_command(
        &self,
        name: &str,
        args: &str,
        ctx: &HostCtx,
    ) -> Result<Option<String>, ExtError> {
        ctx.require_command_tier()?;
        match name {
            "flux/status" => Ok(Some(crate::render_status::render(args))),
            "flux/cheatsheet" => Ok(Some(crate::render_cheatsheet::render(args))),
            "flux/about" => Ok(Some(crate::render_about::render())),
            _ => Err(ExtError::Component(format!(
                "native extension has no handler for command `{name}`"
            ))),
        }
    }

    async fn execute_shortcut(&self, key: &str, ctx: &HostCtx) -> Result<(), ExtError> {
        ctx.require_command_tier()?;
        if key == "ctrl+f" {
            crate::overlay::open_status_overlay(&self.host_services);
        }
        Ok(())
    }
}
```

`bundled_dir()` mirrors
[`bundled_resources_dir()`](../crates/cyrup-ext-subagents/src/registration/resources.rs):
`env!("CARGO_MANIFEST_DIR").join("resources")` behind a `CYRUP_FLUX_RESOURCES_DIR` env override.

> **Superseded by FLUX-001 (`docs/gap-analysis/14-cyrup-flux.md`).** `CARGO_MANIFEST_DIR` is the
> build machine's source path, so a binary run anywhere else lost every template silently. The
> tree is now EMBEDDED at build time (`build.rs` → `src/bundle.rs`) and materialised under
> `<agent_dir>/flux/resources/` by `src/install.rs`, a port of upstream's `installer.py` (the
> copy/manifest/version-gate/`.bak`/flock the row below said was deleted). `resources.rs` decides
> `BundledRoot::{Vendored, Managed}` once at construction; `CYRUP_FLUX_RESOURCES_DIR` still names a
> vendored tree that is read as-is, and a miss on either root is now a `notify` warning naming the
> path, never a silent `Noop`.

**Bundling = single source of truth.** The 15 templates + `_docs/` + skill live in the crate's
`resources/` tree and are contributed at `ResourceScope::Discovered` (rank 6 — a floor, never
an override; a user/project/package `flux/*` template still wins, §0.4).

> **SUPERSEDED**: the sentences that followed described a standalone `cyrup-flux` package as a
> second distribution channel to be kept `rsync`-identical with the crate. There is no second
> channel and nothing to keep in sync — `resources/` is the only home. A user who wants to pin,
> audit or override does it the way cyrup already supports: a same-named `flux/*` template at
> user or project scope, which outranks this crate's `Discovered`-scope floor. The
> **directory-contribution** rule above is the load-bearing part of this section and is
> unchanged.

#### 3.4.2 `/flux/status` — Rust port of `flux_status.py`

Port [`flux_status.py`](../tmp/code-puppy/flux_bootstrap/bundled/scripts/flux_status.py)'s data
model and layout **function-for-function** into `state.rs` + `render_status.rs`:

- `flatten_cwd(cwd)` — runs of non-ASCII-alphanumerics → one `-` (§1.3):

  ```rust
  pub fn flatten_cwd(cwd: &str) -> String {
      let mut out = String::with_capacity(cwd.len());
      let mut pending_dash = false;
      for ch in cwd.chars() {
          if ch.is_ascii_alphanumeric() {
              if pending_dash { out.push('-'); pending_dash = false; }
              out.push(ch);
          } else {
              pending_dash = true;
          }
      }
      if pending_dash { out.push('-'); } // matches re.sub: trailing run collapses too
      out
  }
  ```

- `derive_base(explicit)` — `FLUX_ROOT` env override first (`${FLUX_ROOT:-$HOME/.flux}`
  semantics), then `~/.flux/<flattened cwd>`.
- `parse_frontmatter(path)` — port the Python tolerance exactly: file must START with `---`;
  read lines until the next `---`; `key: value` split on the FIRST `:`; missing/malformed →
  empty map (never error). This is what lets one renderer serve both code-puppy and cyrup state
  trees (§5.6).
- `collect_todos` / `collect_done` / `collect_reviews` — same globs, same sorts
  (`todo/*.md` sorted; `done/<ts>` dirs reverse-sorted with `format_timestamp`
  `YYYY-MM-DD-HH-MM` → `YYYY-MM-DD HH:MM` 5-part split, odd names passed through; `review/`
  walked in the FIXED severity order `critical, high, medium, low`).
- `render(base, sections)` — same layout constants: `name_w = min(max_name_len + 2, 50)`,
  `stage_w = 8`, `_SECTION_PAD = 18`, `_MIN_PANEL_W = 48`; same section order
  (TODO → COMPLETED → REVIEW); same glyphs: `𝕱` header, `═`/`─` rules, `🔄` in-progress,
  `✅` done/completed, `🔁` needs-rework, `●` severity dots, unknown status → `(unknown)`.
- **No ANSI.** The TUI strips ANSI from external text (§0.4), so the returned string is the
  Python script's `--no-color` output — aligned columns + Unicode glyphs, which carry the
  semantics color carried. Color is reserved for the overlay (§3.4.3), which draws themed
  ratatui lines natively.
- **Args**: positional section filter — `/flux/status`, `/flux/status todo`,
  `/flux/status todo review` — same as the Python's positional override; an invalid section
  name → self-issued Error notify + `Ok(None)` (the `execute_command` output-channel contract,
  §0.2). Empty/missing base dir → the `(no flux state at <base>)` line.

#### 3.4.3 `/flux/cheatsheet`, `/flux/about`, and the `ctrl+f` overlay

- **`/flux/cheatsheet`** — port
  [`flux_cheatsheet.py`](../tmp/code-puppy/flux_bootstrap/bundled/scripts/flux_cheatsheet.py):
  the pipeline definitions are **parsed at runtime from `pipeline.md`** (single source of
  truth; nothing hardcoded but presentation). The Rust port embeds
  `resources/prompts/flux/_docs/pipeline.md` via `include_str!` and reimplements the two parses
  (the PIPELINE A–D section blocks and the command table), normalizing `//flux` → `/flux`
  exactly as the Python does. Optional positional arg `A|B|C|D` renders one pipeline.
- **`/flux/about`** — port
  [`flux_about.py`](../tmp/code-puppy/flux_bootstrap/bundled/scripts/flux_about.py): the
  overview text lives in
  [`about.md`](../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/about.md); the Python
  strips frontmatter + the AI-only preamble and normalizes `//cmd` → `/cmd`. The Rust port
  embeds the same body via `include_str!` from `resources/` and applies the same two
  transforms, returning plain text (no Rich — the glyph/table layout is already terminal
  markdown; the notification channel shows it as-is).
- **`ctrl+f` overlay** — `overlay.rs` implements
  [`InteractiveOverlay`](../crates/cyrup-ext/src/host/overlay.rs) rendering the same status
  model with themed colors (the subagents fleet modal,
  `cyrup-ext-subagents/src/background/fleet_view.rs`, is the pattern to copy). The shortcut
  handler acquires the host backend from the `OnceLock` and calls
  [`HostServices::open_overlay`](../crates/cyrup-ext/src/host/services.rs) (`services.rs:254`);
  on a headless surface it returns `false` — fall back to `notify` with the plain table. ESC
  closes (the Wibey `ui-mode: flux-status` behavior, restored natively).

#### 3.4.4 `ask_user_question` tool (`ask_tool.rs`)

Agent-callable tool bridging to `HostServices::select` / `confirm` / `input` under the
`HumanInteractionLock`. Schema mirrors code-puppy's
(`question`, `header`, `options[{label, description}]`, `multiple` flag):

```rust
// ask_tool.rs — core pattern
pub struct AskUserQuestionTool {
    host: Arc<OnceLock<Arc<dyn HostServices>>>,
    params: serde_json::Value, // JSON schema: {question, header?, options[{label, description?}], multiple?}
}

#[async_trait::async_trait]
impl cyrup_core::Tool for AskUserQuestionTool {
    fn name(&self) -> &str { "ask_user_question" }
    fn parameters(&self) -> &serde_json::Value { &self.params }

    async fn execute(
        &self,
        _call_id: cyrup_core::ToolCallId,
        params: serde_json::Value,
        cancel: cyrup_core::CancelToken,
        _on_update: cyrup_core::ToolUpdateSink,
    ) -> Result<cyrup_core::ToolResult, cyrup_core::ToolError> {
        let host = self.host.get().cloned()
            .ok_or_else(|| cyrup_core::ToolError::new("ask_user_question: no interactive host"))?;
        let q: Question = serde_json::from_value(params)
            .map_err(|e| cyrup_core::ToolError::new(format!("ask_user_question: {e}")))?;
        let lock = host.human_interaction_lock()
            .ok_or_else(|| cyrup_core::ToolError::new("ask_user_question: interaction unavailable"))?;
        let _guard = lock.acquire().await; // single human-interaction slot (services.rs:153-187)

        // UiKind::Select carries a FLAT STRING ARRAY and replies with the chosen string;
        // descriptions have no carrier (host_services.rs:1696-1702). Project
        // {label, description} -> "label — description" display rows and map the answer back
        // to the bare label (the oauth_select pattern, host_services.rs:1703-1730).
        // select() is a blocking sync host call — hop off the async executor:
        let picked: Option<String> = tokio::task::spawn_blocking(move || {
            let rows: Vec<serde_json::Value> = q.options.iter()
                .map(|o| serde_json::Value::String(display_row(o)))
                .collect();
            host.select(&q.question, &serde_json::Value::Array(rows), &Default::default())
                .map(|chosen| back_to_label(&chosen, &q.options))
        })
        .await
        .map_err(|e| cyrup_core::ToolError::new(e.to_string()))?;
        // `multiple: true` loops the same select with a synthetic "✔ Done" first row,
        // accumulating labels until Done or cancel. `None` (Esc/cancel) short-circuits.
        let answer = picked.unwrap_or_else(|| "(cancelled — no selection made)".to_string());
        // Same construction the built-in tools use (cyrup-tools/src/tools/bash.rs:454).
        Ok(cyrup_core::ToolResult {
            content: vec![cyrup_core::Content::text(answer)],
            ..Default::default()
        })
    }
}
```

- Registered in `init` via `api.register_tool` (§3.4.1) — it overrides any future same-named
  built-in (`register_tool` overrides by name, `native.rs:313`), so if cyrup later grows this
  tool in `cyrup-tools` proper, delete this impl with no conflict.
- After the tool lands, sweep the 25 `FLUX-GAP` markers from Phase 1 and restore the
  structured-question wording at each site (mechanical edit per site: question text, header,
  2–4 options with descriptions — the original code-puppy wording is the guide).

#### 3.4.5 Wiring

**THREE** blocks in [`crates/cyrup/src/main.rs`](../crates/cyrup/src/main.rs) — one per
`AppMode`, each after that arm's permission-system attach: the interactive arm
(`main.rs:635` → `:692-717`), the `AppMode::Rpc` arm (`:895-914`) and the
`AppMode::Print | AppMode::Json` arm (`:1015-1037`). Attaching in only one makes flux work in
the TUI and vanish everywhere else. Same seam in all three:

```rust
// `flux_extension_for_env()` returns `None` inside a subagent CHILD (`CYRUP_SUBAGENT_CHILD`),
// so a child does not pay for 15 templates + a skill in its system prompt.
if let Some(ext) = cyrup_flux::flux_extension_for_env() {
    factory_builder = factory_builder.with_native_extension(ext);
}
```

Add `cyrup-flux` to the workspace `Cargo.toml` `members` **and `default-members`** (a crate
outside `default-members` is skipped by a bare `cargo check`/`cargo clippy`) and to
`[workspace.dependencies]`, then to `crates/cyrup/Cargo.toml` dependencies. No other existing
file changes.

### 3.5 Parallel exec hardening

`/flux/exec 3` / `/flux/aug 2` / `/flux/qa 2` map to N parallel foreground `subagent` calls:

- Read the `subagent` tool's parallel foreground fan-out semantics
  ([`cyrup-ext-subagents`](../crates/cyrup-ext-subagents/src/extension.rs) — parallel `tasks:`
  array with `concurrency`, plus `wait` for rolling fleets) and align the prompt wording in
  `aug.md`/`exec.md`/`qa.md` multi-task sections with the tool's actual scheduling: prescribe
  "issue up to N `subagent` calls in one parallel block; as each returns, issue the next until
  every `$FLUX_BASE/todo/*.md` is done" (the refill-as-they-finish loop stays prompt-driven).
- Keep the prompts' dependency-collision guidance (don't parallelize tasks touching the same
  files; exhaust namespace-collision avoidance first) — pure prompt text, no code.
- Crash-resume semantics carry over for free: state is in the task files; rerun the step.

---

## 4. File-by-file port table

Every row's source is under
[`../tmp/code-puppy/flux_bootstrap/bundled/`](../tmp/code-puppy/flux_bootstrap/bundled/).
"GAP" counts are `ask_user_question` sites; "renames" are `create_file`/`replace_in_file`/
`read_file`/`invoke_agent` sites.

| code-puppy source | cyrup target | Mechanism | Porting notes |
|---|---|---|---|
| `commands/flux/new.md` | `prompts/flux/new.md` | template | Jira/MCP branch verbatim (§3.3.7); writes `session.env`; GAP×2; `create_file`×1 |
| `commands/flux/ask.md` | `prompts/flux/ask.md` | template | GAP×1 (STEP 4 — the core clarifying-questions loop); `create_file`/`replace_in_file`×1 each |
| `commands/flux/split.md` | `prompts/flux/split.md` | template | unchanged logic; `status: complete` on the preserved original; `create_file`×1 |
| `commands/flux/aug.md` | `prompts/flux/aug.md` | template | `invoke_agent`→`subagent`; research-only constraints verbatim; GAP×1; `create_file`/`replace_in_file`×1 each |
| `commands/flux/exec.md` | `prompts/flux/exec.md` | template | no-git + scope-guard verbatim; `invoke_agent`→`subagent`; GAP×1; `create_file`/`replace_in_file`×1 each |
| `commands/flux/qa.md` | `prompts/flux/qa.md` | template | 10/10 → `done/$SESSION_TS/` move verbatim; "stage MUST be `qa`" warnings verbatim; `invoke_agent`→`subagent`; GAP×1 |
| `commands/flux/tests.md` | `prompts/flux/tests.md` | template | `TEST_CMD` from `config.env`; merge-base worktree baseline; regression-only rule; git allow-list (`merge-base`/`worktree`/`remote` only) |
| `commands/flux/commit.md` | `prompts/flux/commit.md` | template | confirm-before-commit; heredoc inline message rule; amend mode; GAP×2 |
| `commands/flux/create-pr.md` | `prompts/flux/create-pr.md` | template | `gh` CLI; idempotent existing-PR lookup; default-branch guard |
| `commands/flux/review.md` | `prompts/flux/review.md` | template | `gh` + parent-branch detection; merge-base scoping; agent-count table; `invoke_agent`→`subagent` (`run_in_background: false` → foreground); writes `review/<severity>/`; GAP×1; `create_file`/`replace_in_file`×1 each |
| `commands/flux/address-feedback.md` | `prompts/flux/address-feedback.md` | template | review.zip → todo tasks; GAP×1 |
| `commands/flux/auto-pilot.md` | `prompts/flux/auto-pilot.md` | template | orchestrates the other `/flux/*` commands; `read_file`→`read`; max-3-cycles rules |
| `commands/flux/rebase.md` | `prompts/flux/rebase.md` | template | heaviest git user; GAP×5 (confirmations) |
| `commands/flux/squash-commits.md` | `prompts/flux/squash-commits.md` | template | GAP×7 (confirmations) |
| `commands/flux/config.md` | `prompts/flux/config.md` | template | writes `config.env`; GAP×3 (new-file flow writes immediately, no questions) |
| `commands/flux/status.md` + `scripts/flux_status.py` | `/flux/status` native command + `ctrl+f` overlay | **cyrup-ext-flux** | §3.4.2, §3.4.3 |
| `commands/flux/cheatsheet.md` + `flux_cheatsheet.py` | `/flux/cheatsheet` native command | **cyrup-ext-flux** | §3.4.3 — parses embedded `pipeline.md` |
| `commands/flux/about.md` + `flux_about.py` | `/flux/about` native command | **cyrup-ext-flux** | §3.4.3 — embeds about body |
| `commands/flux/_docs/*` | `prompts/flux/_docs/` + `skills/flux/` | content | §3.3 rule 8 |
| `installer.py` + `register_callbacks.py` | ~~**deleted**~~ `src/install.rs` + `extension.rs::materialise_bundle` (FLUX-001) | ~~replaced by `cyrup install` (Phase 1) + built-in registration (Phase 2)~~ — `cyrup install` is the EXTENSION installer and never vendored this tree; ported after all | copy/manifest/version-gate/`.bak`/flock kept; marker = crate version + bundle sha256; target `<agent_dir>/flux/resources/` not the scanned `prompts/` root; no mode bits; no command-cache rescan |
| `customizable_commands.py` (dispatch) | **n/a** | cyrup-resources prompt templates already provide it (§0.1) | |

---

## 5. Gaps, risks, decisions

1. **`ask_user_question` absence** — the only real capability gap. Phase 1 plain-text fallback
   degrades `/flux/ask` and `/flux/config` UX but not correctness; Phase 2 closes it with the
   §3.4.4 native tool. Decided: native tool in `cyrup-ext-flux` (not a `cyrup-tools` change) —
   `register_tool` overrides by name if a built-in ever appears.
2. ~~No namespaced commands~~ — **already landed** (§0.1). Non-issue.
3. **No `exec:` directive** — deliberately not ported. Shell-out-from-frontmatter is a
   trust/sandbox question cyrup answers with the extension capability model instead; the three
   exec commands become native renderers. This is a **CYRUP-DELTA** relative to code-puppy, by
   design.
4. **Tool-name drift inside prompts** — mechanical but easy to miss (25 `ask_user_question`
   sites across 11 files; 15 rename sites across 8 files — §0.3). The §3.3-rule-9 `rg` sweep is
   the gate; every `FLUX-GAP` marker must survive Phase 1 intact so Phase 2's upgrade sweep
   finds every site.
5. **Parallel-mode fidelity** — flux's refill-as-they-finish scheduling is prompt-driven; the
   `subagent` tool's batching semantics may differ slightly. Phase 3 (§3.5) aligns the wording
   with the tool's real scheduling before any smart-scheduling claim is made to users.
6. **Cross-harness state sharing** — keeping `~/.flux` identical means the `/flux/status`
   renderer must parse code-puppy's frontmatter variants (`status: completed` vs `done` vs
   `complete`, missing frontmatter, odd `done/` dirnames). §3.4.2 ports the Python renderer's
   tolerance, not just the happy path.
7. **Trust model** — project-scope packages can shadow global `flux/*` templates (precedence:
   project > global > Discovered). That matches Pi/cyrup resource semantics and is a feature
   (per-project pipeline customization); the Phase 2 bundled resources are rank-6
   `Discovered`, so they are always the floor, never an override (§0.4).
8. **ANSI stripping** — the TUI strips ANSI from external text (§0.4), so the native renderers
   return glyph-bearing plain text; themed color lives only in the `ctrl+f` overlay, which
   draws ratatui lines natively. Do not emit ANSI into `Ok(Some(text))`.

---

## 6. Work items

> **SUPERSEDED by [`flux/README.md`](flux/README.md)'s twelve-task table.** The rows below are
> kept for their effort sizing and section cross-references only; every "package" cell means
> `crates/cyrup-flux/resources/`, and every `cyrup-ext-flux` cell means `crates/cyrup-flux`.
> Items 3 and 8 have merged into FLUX_01 (there is no manifest to write and no second home to
> bundle into).

| # | Item | Where | Effort | task |
|---|---|---|---|---|
| ~~0~~ | ~~Precursor: recursive prompt scanner~~ | **DONE** — landed in `cyrup-resources` (§0.1) | — | — |
| 1 | Port 15 command templates to `resources/prompts/flux/*.md` (§3.3 rules 1–9) | `crates/cyrup-flux` | M | 02–05 |
| 2 | `flux` skill from `_docs/` (§3.3 rule 8) | same crate | S | 06 |
| 3 | ~~`cyrup.toml` manifest~~ → crate scaffold + workspace registration | `crates/cyrup-flux` | S | 01 |
| 4 | State model + status/cheatsheet/about renderers (§3.4.1–3.4.3) | `crates/cyrup-flux` | M | 07–08 |
| 5 | `ask_user_question` native tool + `FLUX-GAP` sweep (§3.4.4) | `crates/cyrup-flux` | S | 10–11 |
| 6 | `ctrl+f` status overlay (§3.4.3) | `crates/cyrup-flux` | S | 09 |
| 7 | Wire extension in `main.rs` (×3 arms) + workspace manifests (§3.4.5) | `crates/cyrup` | XS | 01 |
| 8 | Bundle prompts/skill as built-in resources — **directory contribution** (§3.4.1) | `crates/cyrup-flux` | S | 01 |
| 9 | Parallel-exec alignment with `subagent` semantics (§3.5) | bundled prompts | S | 12 |

No cyrup core changes remain.

## 7. Definition of done

> **The "Phase 1 (package)" heading below is superseded.** There is no install step: the content
> ships with the binary from FLUX_01 onward. Read "`cyrup install <path>` then" as "with the
> binary built and installed, then". Every on-disk state transition it lists is unchanged and is
> still the acceptance bar.

- **Content set**: with the binary built, in a scratch repo the full pipeline-A
  loop — `/flux/new` → `/flux/ask` → `/flux/split` → `/flux/aug` → `/flux/exec` → `/flux/qa` —
  runs end-to-end with correct on-disk transitions: task file created with `stage: new,
  status: done`; per-step frontmatter `stage`/`status`/`updated` rewrites; subtask files from
  split; the split original moved to `done/<SESSION_TS>/`; a 10/10 QA moving the task to
  `done/<SESSION_TS>/` with `stage: qa, status: completed`; a <10 QA leaving `needs-rework`
  with a rewritten body. The §3.3-rule-9 `rg` sweeps are clean. `cyrup -p "/flux/new …"`
  expands the template (same preflight path, §0.2).
- **Native crate**: with the extension wired per §3.4.5 (three `main.rs` arms) and **nothing
  installed**,
  `/flux/new` … `/flux/config` are available out of the box (bundled `Discovered`-scope
  templates registered under their `flux/<step>` names — the directory-contribution check,
  §3.4.1); `/flux/status` prints the aligned glyph table reflecting a hand-built
  `~/.flux/<dir>` fixture (todo rows, session-grouped done rows, severity-dot review grid,
  `(no todos)` and `(no flux state at …)` paths); `/flux/cheatsheet` and `/flux/about` print
  their rendered bodies; `ctrl+f` opens and ESC closes the status overlay in the TUI;
  `ask_user_question` appears in the model's tool list and a `/flux/ask` run uses it (no
  `FLUX-GAP` markers remain in the bundled templates).
- **Parallel exec**: `/flux/exec 2` on a two-task fixture runs both tasks via foreground
  `subagent` fan-out with per-file frontmatter transitions as each completes.

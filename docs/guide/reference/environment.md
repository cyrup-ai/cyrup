# Environment variables

Every environment variable cyrup reads, and the handful it sets for you. For the flags that cover
the same ground, see [Command line](cli.md); for the on-disk equivalents, see
[settings.json](settings.md).

## How values are read

Each core variable has a `PI_*` migration alias. Both spellings are checked, `CYRUP_*` first, and
the first one set to a non-empty value wins.

**Truthiness is narrow, and it is not the same everywhere.** The core flags below accept exactly
`1`, `true` and `yes` — `true` and `yes` case-insensitively — and the value is **not** trimmed. `on`
is not accepted, and `CYRUP_OFFLINE=" 1"` with a leading space is not truthy. The four feature
opt-ins (`CYRUP_SUBAGENTS`, `CYRUP_PERMISSION_SYSTEM`, `CYRUP_INTERCOM`, `CYRUP_EXPERIMENTAL`) use a
wider rule that does trim and does accept `on`. Two core variables are narrower still and want the
literal string `1`. Each table below says which rule applies.

## Core

| Variable | `PI_*` alias | Values | Default | Meaning |
|---|---|---|---|---|
| `CYRUP_AGENT_DIR` | `PI_CODING_AGENT_DIR` | path | `~/.cyrup/agent` | The agent directory, holding `settings.json`, `auth.json`, `trust.json` and friends |
| `CYRUP_SESSION_DIR` | `PI_CODING_AGENT_SESSION_DIR` | path | `<agent dir>/sessions` | Session storage root; beats the `sessionDir` setting, loses to `--session-dir` |
| `CYRUP_PACKAGE_DIR` | `PI_PACKAGE_DIR` | path | `<agent dir>/packages` | Installed-package root |
| `CYRUP_OFFLINE` | `PI_OFFLINE` | `1`, `true`, `yes` | off | Disable startup network operations; same as `--offline` |
| `CYRUP_SKIP_VERSION_CHECK` | `PI_SKIP_VERSION_CHECK` | `1`, `true`, `yes` | off | Disable the package update check only; telemetry is unaffected |
| `CYRUP_TELEMETRY` | `PI_TELEMETRY` | tri-state, see below | *unset* | Override the `enableInstallTelemetry` setting |
| `CYRUP_CACHE_RETENTION` | `PI_CACHE_RETENTION` | `short`, `long` | `short` | Cache retention policy; trimmed and case-insensitive, anything else falls back to `short` |
| `CYRUP_CLEAR_ON_SHRINK` | `PI_CLEAR_ON_SHRINK` | exactly `1` | off | Fallback for the `terminal.clearOnShrink` setting |
| `CYRUP_HARDWARE_CURSOR` | `PI_HARDWARE_CURSOR` | exactly `1` | off | Fallback for the `showHardwareCursor` setting |

Path values are expanded for a leading `~`, a Windows `~\`, and `file://` URLs. A relative path is
left relative.

`CYRUP_OFFLINE` is a master kill switch for four startup operations: the package update check,
install telemetry, analytics, and the model catalog refresh. It does not stop inference requests.

### `CYRUP_TELEMETRY` is tri-state

Its two siblings are on or off. `CYRUP_TELEMETRY` has three states:

- **unset** — defer to the `enableInstallTelemetry` setting, which defaults to on.
- **set but empty, or set to a non-truthy value** — an explicit off that beats the setting.
- **set to `1`, `true` or `yes`** — an explicit on.

The middle state is what makes the usual idiom for neutralising an inherited variable work:

```sh
CYRUP_TELEMETRY= cyrup "summarise the diff"
```

## The three directory variables that are not synonyms

`CYRUP_AGENT_DIR`, `CYRUP_CODING_AGENT_DIR` and `CYRUP_HOME` look interchangeable and are not. They
are read by different parts of cyrup and mean different directories.

| Variable | Read by | Default | Relocates |
|---|---|---|---|
| `CYRUP_AGENT_DIR` | the config layer | `~/.cyrup/agent` | `settings.json`, `auth.json`, `trust.json`, `models.json`, themes, prompts, sessions, packages |
| `CYRUP_CODING_AGENT_DIR` | intercom and subagents only | `~/.cyrup` — no `agent` segment | the intercom broker socket and config, and the subagent directories those two crates resolve |
| `CYRUP_HOME` | subagents, intercom and the permission system only | `$HOME` | the home directory those three use to derive `~/.cyrup/...` paths |

Three consequences worth internalising:

**`CYRUP_HOME` does not move your configuration.** Setting it leaves `settings.json`, `auth.json`
and `trust.json` exactly where they were. Only `CYRUP_AGENT_DIR` moves those. If `CYRUP_HOME` is set
to something unusable, the three extensions that read it fall back to the temporary directory.

`CYRUP_CODING_AGENT_DIR` is not the same directory as `CYRUP_AGENT_DIR` even when both are set to the
same value — it carries no `agent` segment, so the defaults differ by one path component. An
absolute value is used verbatim; a relative value is joined onto the current directory; a blank value
is ignored.

`PI_CODING_AGENT_DIR` — note the `PI_` prefix — is the alias for `CYRUP_AGENT_DIR`, not for
`CYRUP_CODING_AGENT_DIR`. The names cross over.

## Feature opt-ins

The three native extensions and the experimental gate are off by default. These use the wider
truthiness rule: `1`, `true`, `on` or `yes`, trimmed — except `CYRUP_EXPERIMENTAL`, which wants the
literal string `1`.

| Variable | Values | Meaning |
|---|---|---|
| `CYRUP_SUBAGENTS` | `1`, `true`, `on`, `yes` | Install the [subagents](../extensions/subagents.md) extension |
| `CYRUP_PERMISSION_SYSTEM` | `1`, `true`, `on`, `yes` | Install [the permission system](../extensions/permissions.md) even with no policy file |
| `CYRUP_INTERCOM` | `1`, `true`, `on`, `yes` | Install [intercom](../extensions/intercom.md) |
| `CYRUP_EXPERIMENTAL` (alias `PI_EXPERIMENTAL`) | exactly `1` | Enable experimental features, including the first-run setup wizard |

**A config file arms these too.** Subagents also switch on when `<agent dir>/subagents/config.json`
or `.cyrup/subagents/config.json` exists. Intercom switches on when
`<agent dir>/intercom/config.json` exists. The permission system switches on when any policy file
exists, when the policy `agents/` directory is non-empty, or when its `config.json` differs from the
template cyrup generates — which means unsetting the variable is not enough to turn it off once a
policy file is on disk. Set `"enabled": false` in the extension's `config.json` for that.

## Provider credentials

One variable per provider, read when there is no stored credential for it. A stored credential from
`/login` suppresses the environment fallback. The full precedence is `--api-key`, then the stored
credential, then the environment variable, then a key configured in `models.json`.

| `--provider` id | API-key variable(s) |
|---|---|
| `amazon-bedrock` | *ambient — see below* |
| `ant-ling` | `ANT_LING_API_KEY` |
| `anthropic` | `ANTHROPIC_OAUTH_TOKEN`, then `ANTHROPIC_API_KEY` |
| `azure-openai-responses` | `AZURE_OPENAI_API_KEY`, plus `AZURE_OPENAI_BASE_URL` or `AZURE_OPENAI_RESOURCE_NAME`, and optionally `AZURE_OPENAI_API_VERSION` and `AZURE_OPENAI_DEPLOYMENT_NAME_MAP` |
| `cerebras` | `CEREBRAS_API_KEY` |
| `cloudflare-ai-gateway` | `CLOUDFLARE_API_KEY` + `CLOUDFLARE_ACCOUNT_ID` + `CLOUDFLARE_GATEWAY_ID` |
| `cloudflare-workers-ai` | `CLOUDFLARE_API_KEY` + `CLOUDFLARE_ACCOUNT_ID` |
| `deepseek` | `DEEPSEEK_API_KEY` |
| `fireworks` | `FIREWORKS_API_KEY` |
| `github-copilot` | `COPILOT_GITHUB_TOKEN` |
| `google` | `GEMINI_API_KEY` |
| `google-vertex` | `GOOGLE_CLOUD_API_KEY`, or ambient credentials — see below |
| `groq` | `GROQ_API_KEY` |
| `huggingface` | `HF_TOKEN` |
| `kimi-coding` | `KIMI_API_KEY` |
| `minimax` | `MINIMAX_API_KEY` |
| `minimax-cn` | `MINIMAX_CN_API_KEY` |
| `mistral` | `MISTRAL_API_KEY` |
| `moonshotai` | `MOONSHOT_API_KEY` |
| `moonshotai-cn` | `MOONSHOT_API_KEY` — the same variable |
| `nvidia` | `NVIDIA_API_KEY` |
| `openai` | `OPENAI_API_KEY` |
| `openai-codex` | *none — OAuth only* |
| `opencode` | `OPENCODE_API_KEY` |
| `opencode-go` | `OPENCODE_API_KEY` — the same variable |
| `openrouter` | `OPENROUTER_API_KEY` |
| `together` | `TOGETHER_API_KEY` |
| `vercel-ai-gateway` | `AI_GATEWAY_API_KEY` |
| `xai` | `XAI_API_KEY` |
| `xiaomi` | `XIAOMI_API_KEY` |
| `xiaomi-token-plan-ams` | `XIAOMI_TOKEN_PLAN_AMS_API_KEY` |
| `xiaomi-token-plan-cn` | `XIAOMI_TOKEN_PLAN_CN_API_KEY` |
| `xiaomi-token-plan-sgp` | `XIAOMI_TOKEN_PLAN_SGP_API_KEY` |
| `zai` | `ZAI_API_KEY` |
| `zai-coding-cn` | `ZAI_CODING_CN_API_KEY` |

`ANTHROPIC_AUTH_TOKEN` is not honoured. Use `ANTHROPIC_API_KEY`, `ANTHROPIC_OAUTH_TOKEN`, or
`/login`.

An API-key value may be a template: a leading `!` runs the rest as a shell command and uses its
trimmed output, and `$VAR` or `${VAR}` interpolates. Write `$$` or `$!` for a literal.

### Ambient-credential providers

Two providers have no API-key variable and instead detect credentials already present in your
environment.

`amazon-bedrock` is considered configured when any one of these is set: `AWS_PROFILE`; both
`AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY`; `AWS_BEARER_TOKEN_BEDROCK`;
`AWS_CONTAINER_CREDENTIALS_RELATIVE_URI`; `AWS_CONTAINER_CREDENTIALS_FULL_URI`; or
`AWS_WEB_IDENTITY_TOKEN_FILE`. Set `AWS_REGION` for the region.

`google-vertex` accepts `GOOGLE_CLOUD_API_KEY`, or application default credentials. For the ADC
route all three of these must hold: `GOOGLE_APPLICATION_CREDENTIALS` points at a credentials file, or
`~/.config/gcloud/application_default_credentials.json` exists; `GOOGLE_CLOUD_PROJECT` or
`GCLOUD_PROJECT` is set; and `GOOGLE_CLOUD_LOCATION` is set.

### OAuth callback

| Variable | Alias | Default | Meaning |
|---|---|---|---|
| `CYRUP_OAUTH_CALLBACK_HOST` | `PI_OAUTH_CALLBACK_HOST` | `127.0.0.1` | Host the OAuth loopback listener binds for `/login` |

### Variables that name a provider cyrup does not have

`QWEN_TOKEN_PLAN_API_KEY`, `QWEN_TOKEN_PLAN_CN_API_KEY`, `RADIUS_API_KEY` and `BASETEN_API_KEY`
exist in the credential resolver, mapped to the ids `qwen-token-plan`, `qwen-token-plan-cn`,
`qwen-token-plan-individual`, `radius` and `baseten`. **None of those is a valid `--provider`
value** — there is no built-in provider behind them. They are reachable only by declaring a provider
with that id yourself in `models.json`. If you find one of these names in the wild, exporting it on
its own does nothing.

## Subagents

Read only when the [subagents](../extensions/subagents.md) extension is installed.

| Variable | Values | Meaning |
|---|---|---|
| `CYRUP_SUBAGENT_MAX_DEPTH` | integer | Recursion ceiling; a malformed value is treated as unset |
| `CYRUP_SUBAGENT_MAX_SPAWNS_PER_SESSION` (alias `PI_SUBAGENT_MAX_SPAWNS_PER_SESSION`) | number | Per-session spawn cap |
| `CYRUP_SUBAGENT_TOOL_BUDGET` | JSON | Tool budget handed to each child |
| `CYRUP_SUBAGENT_WAIT_TOOL_ENABLED` | see below | Enable or disable the background `wait` tool |
| `CYRUP_SUBAGENT_EXTRA_AGENT_DIRS` | path list | Extra read-only directories to discover agent files in |
| `CYRUP_SUBAGENT_BUILTIN_AGENTS_DIR` | path | Relocate the bundled personas |
| `CYRUP_SUBAGENTS_WORKTREE_DIR` | path | Git worktree root for isolated runs |
| `CYRUP_SUBAGENTS_TEMP_ROOT` | path | Root for nested-run temporary artifacts |
| `CYRUP_SUBAGENT_BINARY`, `CYRUP_SUBAGENT_STEP_BINARY` | path | Override the binary used to spawn children |
| `CYRUP_SUBAGENTS_MAX_HASH_ENTRIES`, `_FILE_BYTES`, `_TOTAL_BYTES` | numbers | Caps on the worktree change scan |

`CYRUP_SUBAGENT_WAIT_TOOL_ENABLED` has its own vocabulary, trimmed and lowercased: `1`, `true`,
`yes`, `on` and `enabled` are on; `0`, `false`, `no`, `off` and `disabled` are off. **Anything else
is a hard configuration error**, not a fallback.

`CYRUP_HOME` (above) also affects subagents: it decides which `~/.cyrup/agents` directory their agent
files are discovered in.

## The permission system

Read only when [the permission system](../extensions/permissions.md) is installed.

| Variable | Values | Meaning |
|---|---|---|
| `CYRUP_PERMISSION_SYSTEM_POLICY_AGENT_DIR` | path | Relocate the global policy root holding `cyrup-permissions.jsonc`, `agents/`, `settings.json` and `mcp.json`; project paths are unaffected |
| `CYRUP_PERMISSION_SYSTEM_CONFIG_PATH` | path | Point the extension at a different `config.json` |
| `CYRUP_PERMISSION_SYSTEM_LOGS_DIR` | path | Log directory |
| `CYRUP_PERMISSION_SYSTEM_FORWARDING_AGENT_DIR` | path | Root of the child-to-parent permission forwarding spool |
| `CYRUP_PERMISSION_FORWARDING_TIMEOUT_MS` | positive number | How long a child waits for a forwarded decision; the default is ten minutes |

The policy directory variable is trimmed, an empty value counts as unset, and the result is made
absolute.

## Intercom

Read only when [intercom](../extensions/intercom.md) is installed.

| Variable | Values | Meaning |
|---|---|---|
| `CYRUP_INTERCOM_TRANSPORT` | `tcp` | Opt into the loopback TCP transport |
| `CYRUP_INTERCOM_TCP` | `1`, `true` | The legacy TCP opt-in, still honoured |
| `CYRUP_INTERCOM_ASK_TIMEOUT_MS` | milliseconds | Timeout for an intercom ask; default 600000. An invalid value fails extension startup |
| `CYRUP_INTERCOM_NAME_POLL_MS` | milliseconds | Name-resolution poll interval |
| `CYRUP_INTERCOM_LIVENESS_INTERVAL_MS`, `_TIMEOUT_MS` | milliseconds | Broker liveness heartbeat and timeout |
| `CYRUP_INTERCOM_STABLE_ID` | string | Restart-stable registration id for this session |
| `CYRUP_INTERCOM_BROKER_BINARY` | path | Override the broker binary |

`CYRUP_CODING_AGENT_DIR` (above) decides where intercom keeps its socket, pid file and config.

## Diagnostics

| Variable | Values | Meaning |
|---|---|---|
| `CYRUP_TIMING` | exactly `1` | Emit startup phase timings |
| `CYRUP_STARTUP_BENCHMARK` | truthy | Startup benchmarking mode |

## Variables cyrup sets for you — outputs

These are **not inputs**. cyrup exports them into the environment of every command the `bash` tool
runs, so a script invoked by the agent can see which session it is part of. Setting them yourself
before launching cyrup has no effect — both the `CYRUP_*` and `PI_*` spellings are stripped from the
child environment and repopulated.

| Variable | Value |
|---|---|
| `CYRUP_SESSION_ID` | The current session id |
| `CYRUP_SESSION_FILE` | Path to the session `.jsonl` file |
| `CYRUP_PROVIDER` | The active provider id |
| `CYRUP_MODEL` | The active model id |
| `CYRUP_REASONING_LEVEL` | The active thinking level, one of the seven values |

`CYRUP_REASONING_LEVEL` reports the thinking level; it does not set it. `CYRUP_REASONING_LEVEL=high
cyrup` does nothing. Use `--thinking high`, a `:high` suffix on `--model`, or `Shift+Tab` in the
interface. The value is republished whenever you change the level, so the next `bash` command sees
the new one.

cyrup also sets a family of `CYRUP_SUBAGENT_PARENT_*` and `CYRUP_SUBAGENT_CHILD_*` variables on child
agent processes. They are process plumbing, not configuration; do not set them yourself.

## Variables outside the `CYRUP_` namespace

| Variable | Meaning |
|---|---|
| `VISUAL`, then `EDITOR` | The external editor, when the `externalEditor` setting is unset. Falls back to `nano`, or `notepad` on Windows |
| `HTTPS_PROXY`, `HTTP_PROXY`, `https_proxy`, `http_proxy` | Proxy URL, checked in that order, when the `httpProxy` setting is unset or blank |
| `NO_PROXY`, `ALL_PROXY` | Honoured by the model catalog fetch |
| `HOME` | The home directory, used to derive `~/.cyrup/agent` when `CYRUP_AGENT_DIR` is unset |

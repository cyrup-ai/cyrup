# settings.json

Every key cyrup reads from `settings.json`, what it defaults to, and where to put it. If you are
looking for a flag instead, see [Command line](cli.md); for an environment variable, see
[Environment variables](environment.md).

## Where settings live

There are two files:

```text
~/.cyrup/agent/settings.json     global — applies everywhere
<repo>/.cyrup/settings.json      project — applies in this repository
```

The global file is read first, then the project file is merged on top. Objects merge recursively
key by key; **arrays and scalars are replaced wholesale**. Setting `enabledModels` in a project
file replaces the global list rather than extending it.

**The project layer is only read when the project is trusted.** In an untrusted folder,
`<repo>/.cyrup/settings.json` is not read at all, and a write to it is refused. See
[Project context](../guides/project-context.md) for how trust is decided, and `/trust` in
[the terminal interface](../guides/tui.md) for changing it.

Keys cyrup does not recognise survive a load-and-save round trip. Hand-added keys and blocks
belonging to extensions are preserved when cyrup writes the file back. If a file fails to load —
malformed JSON, for instance — that scope refuses further writes rather than overwriting whatever
is there.

Writes go to the file for the scope being written: the global file by default, the project file
only when a command explicitly asks for project scope.

A dotted key name in the tables below means a nested object. `compaction.enabled` is
`{ "compaction": { "enabled": true } }` on disk, not a key with a dot in it.

## Model and provider

| Key | Type | Default | Meaning |
|---|---|---|---|
| `defaultProvider` | string | *unset* | Provider used when none is selected. |
| `defaultModel` | string | *unset* | Model used when none is selected. |
| `defaultThinkingLevel` | `off`\|`minimal`\|`low`\|`medium`\|`high`\|`xhigh`\|`max` | `"off"` | Starting thinking level. |
| `enabledModels` | string[] | *unset* | Restricts the `Ctrl+P` cycling set. See below. |

`defaultProvider` and `defaultModel` are only used together, and only when that provider has
authentication configured. See [Models and thinking](../guides/models.md).

## Appearance and the terminal interface

| Key | Type | Default | Meaning |
|---|---|---|---|
| `theme` | string | *unset* | Theme name, or an auto pair. See below. |
| `hideThinkingBlock` | bool | `false` | Hide thinking blocks in responses. |
| `showCacheMissNotices` | bool | `false` | Per-message cache-miss notices; no consumer reads this yet. |
| `showHardwareCursor` | bool | `false` | Show the terminal's hardware cursor. |
| `editorPaddingX` | integer | `0` | Input-editor horizontal padding; the settings editor clamps this to 0–3. |
| `outputPad` | `0`\|`1` | `1` | Chat-output horizontal padding; only an explicit `0` removes it. |
| `autocompleteMaxVisible` | integer | `5` | Autocomplete rows; the settings editor clamps this to 3–20. |
| `doubleEscapeAction` | `fork`\|`tree`\|`none` | `"tree"` | What double-`Esc` on an empty editor opens. |
| `treeFilterMode` | `default`\|`no-tools`\|`user-only`\|`labeled-only`\|`all` | `"default"` | Starting filter in `/tree`; an unrecognised value falls back. |
| `collapseChangelog` | bool | `false` | Show a condensed changelog after updates. |
| `quietStartup` | bool | `false` | Suppress verbose startup printing. |
| `terminal.showImages` | bool | `true` | Render images inline. |
| `terminal.imageWidthCells` | number | `60` | Inline image width in terminal cells. |
| `terminal.showTerminalProgress` | bool | `false` | Report progress to the terminal's tab bar. |
| `terminal.clearOnShrink` | bool | `false` | Clear empty rows when content shrinks. |
| `images.autoResize` | bool | `true` | Resize large images to 2000×2000 before sending. |
| `images.blockImages` | bool | `false` | Never send images to providers. |
| `markdown.codeBlockIndent` | string | `"  "` | Indent applied to rendered code fences. |
| `markdown.mermaid` | `off`\|`final`\|`streaming` | `"streaming"` | Mermaid fence rendering; an unrecognised value falls back to `streaming`. |

`showHardwareCursor` and `terminal.clearOnShrink` fall back to `CYRUP_HARDWARE_CURSOR=1` and
`CYRUP_CLEAR_ON_SHRINK=1` when the setting is absent.

## Conversation flow

| Key | Type | Default | Meaning |
|---|---|---|---|
| `steeringMode` | `all`\|`one-at-a-time` | `"one-at-a-time"` | How messages queued while streaming are delivered. |
| `followUpMode` | `all`\|`one-at-a-time` | `"one-at-a-time"` | How queued follow-up messages are delivered. |
| `compaction.enabled` | bool | `true` | Auto-compact context when it gets large. |
| `compaction.reserveTokens` | integer | `16384` | Tokens held back for compaction. |
| `compaction.keepRecentTokens` | integer | `20000` | Recent tokens preserved verbatim. |
| `branchSummary.reserveTokens` | integer | `16384` | Tokens reserved for branch summarization. |
| `branchSummary.skipPrompt` | bool | `false` | Skip the branch-summary prompt. |
| `thinkingBudgets.minimal` | integer | *unset* | Token budget for the `minimal` thinking level. |
| `thinkingBudgets.low` | integer | *unset* | Token budget for the `low` thinking level. |
| `thinkingBudgets.medium` | integer | *unset* | Token budget for the `medium` thinking level. |
| `thinkingBudgets.high` | integer | *unset* | Token budget for the `high` thinking level. |

The four `thinkingBudgets` fields are parsed independently — one bad field does not discard the
others. They apply to providers that take a token budget rather than an effort string.

## Network, transport and retry

| Key | Type | Default | Meaning |
|---|---|---|---|
| `transport` | `sse`\|`websocket`\|`websocket-cached`\|`auto` | `"auto"` | Preferred transport for providers that offer more than one. |
| `httpIdleTimeoutMs` | number \| numeric string \| `"disabled"` | `300000` | Longest idle gap while awaiting HTTP headers or body. See below. |
| `websocketConnectTimeoutMs` | number \| numeric string | *unset* | WebSocket connect timeout. See below. |
| `httpProxy` | string | *unset* | Proxy URL; blank or whitespace falls through to the proxy environment variables. |
| `retry.enabled` | bool | `true` | Retry failed requests. |
| `retry.maxRetries` | integer | `3` | Retry attempts cyrup makes. |
| `retry.baseDelayMs` | integer | `2000` | Base backoff delay. |
| `retry.provider.maxRetryDelayMs` | integer | `60000` | Backoff ceiling inside the provider SDK. |
| `retry.provider.timeoutMs` | integer | *unset* | Request timeout inside the provider SDK. |
| `retry.provider.maxRetries` | integer | *unset* | Retry attempts inside the provider SDK. |

With `httpProxy` unset, cyrup reads `HTTPS_PROXY`, `HTTP_PROXY`, `https_proxy`, `http_proxy` in
that order.

## Telemetry and privacy

| Key | Type | Default | Meaning |
|---|---|---|---|
| `enableInstallTelemetry` | bool | `true` | Anonymous version and update ping. |
| `enableAnalytics` | bool | `false` | Opt-in usage analytics. |
| `trackingId` | string | *unset* | Non-secret analytics identifier, a random UUID. |
| `lastChangelogVersion` | string | *unset* | Version whose changelog was last shown. |

`CYRUP_TELEMETRY` overrides `enableInstallTelemetry`, and `CYRUP_OFFLINE` disables telemetry,
analytics, the update check and the model-catalog refresh together. Setting `enableAnalytics` to
`true` generates a `trackingId` if there is not one already, in the same write.

## Trust

| Key | Type | Default | Meaning |
|---|---|---|---|
| `defaultProjectTrust` | `ask`\|`always`\|`never` | `"ask"` | Trust policy for a folder with no saved decision. See below. |

## Resources and packages

| Key | Type | Default | Meaning |
|---|---|---|---|
| `packages` | array of string or object | `[]` | Package sources to load. See below. |
| `extensions` | string[] | `[]` | Extension files or directories to load. |
| `skills` | string[] | `[]` | Skill files or directories. |
| `prompts` | string[] | `[]` | Prompt-template files or directories. |
| `themes` | string[] | `[]` | Theme files or directories. |
| `enableSkillCommands` | bool | `true` | Register discovered skills as `/skill:<name>` commands. |

Entries in `skills`, `prompts` and `themes` may carry a leading `+`, `-` or `!` marker. `cyrup
config` writes those markers to enable and disable individual resources without deleting the path.

## Shell, editor and paths

| Key | Type | Default | Meaning |
|---|---|---|---|
| `sessionDir` | string | *unset* | Session storage root; `~` is expanded. Defaults to `<agent dir>/sessions`. |
| `shellPath` | string | *unset* | Shell binary the `bash` tool runs; `~` is expanded. |
| `shellCommandPrefix` | string | *unset* | Prefix prepended to every shell command. |
| `npmCommand` | string[] | *unset* | Override the npm invocation, in argv form. |
| `externalEditor` | string | *unset* | Editor launched by `Ctrl+G`. See below. |

`CYRUP_SESSION_DIR` beats the `sessionDir` setting, and `--session-dir` beats both.

## Warnings

| Key | Type | Default | Meaning |
|---|---|---|---|
| `warnings.anthropicExtraUsage` | bool | *unset* | Anthropic paid-extra-usage warning toggle. |

No default is applied when this key is absent; what an unset value means to its consumer is not
established.

## packages

Each entry is either a source string or an object:

```json
{
  "packages": [
    "git:github.com/acme/cyrup-pack",
    { "source": "./tools/local-pack", "skills": ["review"], "autoload": false }
  ]
}
```

The object form accepts `source`, `autoload`, and the four per-type lists `extensions`, `skills`,
`prompts` and `themes`.

With `autoload` at its default, the per-type lists are **include filters** — only the named
resources load. With `"autoload": false`, nothing in the package loads by default and the same
lists become **add-back deltas** — only what you name is loaded. A bare
`{ "source": "...", "autoload": false }` therefore contributes nothing at all.

This array is a separate channel from `cyrup install`. Installing a package records it in
`packages.json`, not here; `packages` is a list you write yourself. See
[Installing extensions](../extensions/managing.md).

## enabledModels

Unset and `[]` are different values.

- **Unset** — every available model is in the `Ctrl+P` cycling set.
- **`[]`** — no model is in the cycling set.

Use `/scoped-models` to edit the set interactively and write it back here.

## httpIdleTimeoutMs and websocketConnectTimeoutMs

Both accept a number or a numeric string. `httpIdleTimeoutMs` additionally accepts the string
`"disabled"`, which means no idle timeout.

```json
{ "httpIdleTimeoutMs": "disabled" }
```

**A present-but-invalid value is an error, not a fallback.** `"httpIdleTimeoutMs": "5 minutes"`
fails at startup rather than quietly reverting to the default. Remove the key entirely if you want
the default.

## theme

`theme` takes one of three shapes:

- **A theme name** — `"dark"`, `"light"`, or the `name` of a theme file you have installed. It is
  used verbatim, with no terminal probing.
- **An auto pair spelled `light/dark`** — the first name is used on a light terminal and the second
  on a dark one, decided by probing the terminal's background.
- **Unset** — cyrup detects the terminal's polarity, and writes a confident detection back to this
  key.

See [Themes](../guides/themes.md).

## externalEditor

`Ctrl+G` opens the current input buffer in an editor. When `externalEditor` is unset or blank,
cyrup falls back to `$VISUAL`, then `$EDITOR`, then `nano` — `notepad` on Windows.

## defaultProjectTrust

This key is **global scope only**. It is stripped from project settings before the merge, so a
repository cannot declare itself trusted.

- `ask` — prompt on first use of a folder that has project resources.
- `always` — trust any folder with no saved decision.
- `never` — trust nothing that has no saved decision.

Non-interactive runs (`-p`, `--mode json`, `--mode rpc`) cannot prompt, so under `ask` an undecided
project is treated as untrusted.

## The subagents block

The [subagents](../extensions/subagents.md) native extension reads a `subagents` object from the
merged settings document. cyrup's own configuration layer does not model this block — it is
preserved as an unknown key, so you can hand-write it and it will survive any settings write.

| Key | Type | Default | Meaning |
|---|---|---|---|
| `subagents.agentOverrides.<name>` | object | `{}` | Per-agent override delta, keyed by agent name. |
| `subagents.defaultModel` | string | *unset* | Fallback model for every subagent. |
| `subagents.defaultThinking` | string | *unset* | Crate-wide thinking default; a malformed value aborts agent discovery. |
| `subagents.defaultExtensions` | string[] | *unset* | Extension allowlist applied to agents that declare none. |
| `subagents.disableBuiltins` | bool | `false` | Exclude the bundled agent personas from discovery. |
| `subagents.disableThinking` | bool | `false` | Force extended thinking off for all agents. |
| `subagents.modelScope` | object | *unset* | Model allow-list policy. |

**Subagent settings are read from two different files.** The block above comes from the merged
`settings.json` pair. Agent *discovery* separately reads a `subagents` object from
`~/.cyrup/agents/settings.json` (and `<repo>/.cyrup/agents/settings.json`). Those are not the same
file as `~/.cyrup/agent/settings.json` — note `agents` against `agent` — and a malformed discovery
settings file aborts discovery rather than falling back to defaults.

## Legacy keys migrated on load

If your file predates the current key names, cyrup rewrites it as it loads. Nothing is lost, but
the key you wrote may not be the key you find afterwards.

| Old | Becomes |
|---|---|
| `queueMode` | `steeringMode`, only if `steeringMode` is absent |
| `websockets: true` \| `false` | `transport: "websocket"` \| `"sse"`, then `websockets` is deleted |
| `skills` as an object `{ enableSkillCommands, customDirectories }` | top-level `enableSkillCommands` plus a `skills` array; an empty `customDirectories` deletes `skills` |
| `retry.maxDelayMs` | `retry.provider.maxRetryDelayMs`, then `maxDelayMs` is deleted |
| `apiKeys` | moved into `auth.json`, then stripped from `settings.json` |

## Editing settings from inside cyrup

### /settings

`/settings` opens a grid that cycles each value in place, applies it live, and persists it to the
**global** scope. It exposes:

`theme`, `compaction.enabled`, `terminal.showImages`, `terminal.imageWidthCells`,
`images.autoResize`, `images.blockImages`, `enableSkillCommands`, `showHardwareCursor`,
`terminal.clearOnShrink`, `editorPaddingX`, `outputPad`, `autocompleteMaxVisible`,
`httpIdleTimeoutMs`, `hideThinkingBlock`, `collapseChangelog`, `quietStartup`,
`enableInstallTelemetry`, `terminal.showTerminalProgress`, `steeringMode`, `followUpMode`,
`transport`, `doubleEscapeAction`, `treeFilterMode`, `defaultProjectTrust`, plus submenus for
warnings and the thinking level.

The two image rows appear only when your terminal supports an image protocol. Everything else in
this page is edited by hand.

### cyrup config

`cyrup config` is a different, narrower surface. It only toggles entries in the `skills`, `prompts`
and `themes` arrays, writing `+pattern` and `-pattern` markers rather than adding or removing
paths.

```sh
cyrup config          # global scope
cyrup config -l       # project scope, requires a trusted project
```

`-l` (or `--local`) is the only route to a project-scope write.

## Other files in the agent directory

`settings.json` has siblings in `~/.cyrup/agent`:

| File or directory | Contents |
|---|---|
| `auth.json` | Stored credentials. Mode `0600`, written under a cross-process lock. |
| `trust.json` | Per-folder project-trust decisions. |
| `models.json` | Custom providers and models you declare yourself. |
| `models-store.json` | Cached model catalog fetched from the network. |
| `keybindings.json` | Key customisations — see [Keys and slash commands](keybindings.md). |
| `themes/` | Theme files. |
| `prompts/` | Prompt templates. |
| `extensions/` | Globally loaded extensions. |
| `sessions/` | Session storage, unless `sessionDir` moves it. |
| `packages/` | Installed packages and their registry. |

`CYRUP_AGENT_DIR` relocates all of these together. See
[Environment variables](environment.md).

## A complete example

```json
{
  "defaultProvider": "anthropic",
  "defaultModel": "claude-opus-4-8",
  "defaultThinkingLevel": "medium",
  "theme": "light/dark",
  "doubleEscapeAction": "tree",
  "autocompleteMaxVisible": 10,
  "editorPaddingX": 1,
  "steeringMode": "all",
  "httpIdleTimeoutMs": 600000,
  "defaultProjectTrust": "ask",
  "enableInstallTelemetry": false,
  "externalEditor": "hx",
  "enabledModels": [
    "anthropic/claude-opus-4-8",
    "openai/gpt-5.5",
    "google/gemini-3.1-pro-preview"
  ],
  "compaction": {
    "enabled": true,
    "keepRecentTokens": 30000
  },
  "retry": {
    "maxRetries": 5
  }
}
```

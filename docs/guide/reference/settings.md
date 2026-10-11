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
| `defaultThinkingLevel` | `off`\|`minimal`\|`low`\|`medium`\|`high`\|`xhigh`\|`max` | `"medium"` | Starting thinking level. See below. |
| `enabledModels` | string[] | *unset* | Restricts the `Ctrl+P` cycling set. See below. |

`defaultProvider` and `defaultModel` are only used together, and only when that provider has
authentication configured. See [Models and thinking](../guides/models.md).

**`defaultThinkingLevel` defaults to `medium`, not `off`.** With the key absent the settings layer
reports "unset" and each consumer falls back to the same constant, `medium` — so a fresh install
starts sessions with reasoning on. Earlier builds collapsed "unset" into `off` and disabled
reasoning for anyone who had never written the key; if you added `"defaultThinkingLevel": "medium"`
to work around that, the line is now redundant but harmless. Write `"off"` if you actually want
reasoning off by default.

## Appearance and the terminal interface

| Key | Type | Default | Meaning |
|---|---|---|---|
| `theme` | string | *unset* | Theme name, or an auto pair. See below. |
| `hideThinkingBlock` | bool | `false` | Hide thinking blocks in responses. |
| `showCacheMissNotices` | bool | `false` | Per-message cache-miss notices, and the one-line `Cache warmed: $…` notice each [cache warm](#cachewarming) prints. |
| `showHardwareCursor` | bool | `false` | Show the terminal's hardware cursor. |
| `editorPaddingX` | integer | `0` | Input-editor horizontal padding; the settings editor clamps this to 0–3. |
| `outputPad` | `0`\|`1` | `1` | Chat-output horizontal padding; only an explicit `0` removes it. |
| `autocompleteMaxVisible` | integer | `5` | Autocomplete rows; the settings editor clamps this to 3–20. |
| `doubleEscapeAction` | `fork`\|`tree`\|`none` | `"tree"` | What double-`Esc` on an empty editor opens. |
| `treeFilterMode` | `default`\|`no-tools`\|`user-only`\|`labeled-only`\|`all` | `"default"` | Starting filter in `/tree`; an unrecognised value falls back. |
| `collapseChangelog` | bool | `false` | Show a condensed changelog after updates. |
| `quietStartup` | bool \| `"header"` | `false` | `true` hides the startup header and the loaded-resource listing. `"header"` keeps the header (version and key hints) but hides the model scope line and the listing. Any other value reads as `false`. |
| `terminal.showImages` | bool | `true` | Render images inline. |
| `terminal.imageWidthCells` | number | `60` | Inline image width in terminal cells. |
| `terminal.showTerminalProgress` | bool | `false` | Report progress to the terminal's tab bar. |
| `terminal.clearOnShrink` | bool | `false` | Clear empty rows when content shrinks. |
| `terminal.images` | `kitty`\|`iterm2`\|`false`\|`auto` | `"auto"` | Force the inline-image protocol, or `false` for none (images fall back to block characters); `auto` keeps the detected one. |
| `terminal.hyperlinks` | bool\|`auto` | `"auto"` | Force OSC 8 hyperlinks on or off; `auto` keeps the detected answer. |
| `terminal.trueColor` | bool\|`auto` | `"auto"` | Force 24-bit colour on or off (off renders 256 colours); `auto` keeps the detected answer. |
| `images.autoResize` | bool | `true` | Re-encode a new image to fit the request model's [resize profile](../guides/models.md#how-big-an-image-gets-sent) — 2000×2000 and 4.5 MB unless the model declares otherwise — before it enters the conversation. `false` still converts an unsupported format, but sends the original size. |
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

## Tools and codemode

| Key | Type | Default | Meaning |
|---|---|---|---|
| `defaultTools` | string[] | `["read", "bash", "edit", "write"]` | Tools active at startup. Plain names replace the default; `+name` adds a tool and `-name` removes one, applied in order. A project list made only of `+`/`-` entries is appended to the user list instead of replacing it. |
| `codemode.mode` | `on`\|`only` | `"on"` | How the [`codemode`](../guides/codemode.md) tool presents tools while it is active. `on` leaves declared tools declared; `only` hides them from the model and lists them in the `codemode` description. Read when a session is built; `/reload` applies a change. |
| `codemode.inlineBudget` | number | `3000` | Estimated tokens (characters / 4) the `codemode` description may spend on tool declarations. Read when a session is built, like `codemode.mode`. |

`"defaultTools": ["+codemode"]` activates `codemode` next to the four default tools; the same line in a project's `.cyrup/settings.json` counts only when the project is trusted, and `--no-extensions` removes the tool (see [Enable codemode](../guides/codemode.md#enable-codemode)). `defaultTools` names any registered tool, including extension and MCP tools that start inactive (`codemode`, `tool_search`, tools registered as inactive, `deferred` or `codemode` exposure). A name that no tool answers to is reported once as a startup warning; an MCP tool name whose server connects after startup activates when it registers, provided that is before the first prompt. An explicit `--tools` list, `--no-tools` or `--no-builtin-tools` replaces `defaultTools`, and `--exclude-tools` removes names from it. An armed [permission system](../guides/tools-and-permissions.md#the-permission-system) overrides the setting: it activates every tool its policy does not deny, so `defaultTools` does not narrow that set.

`/reload` activates names newly added to `defaultTools`, that is, names the setting had no entry for when the session was last loaded (built or reloaded). It does not deactivate names removed from it, and does not re-enable a tool you turned off during the session while the setting is unchanged; a name you turned off, then removed from the setting and later added again, is activated by that reload. A session resumed in a new process has no earlier load to compare with, so there the names the session's first system prompt declared stand in for it.

## Network, transport and retry

| Key | Type | Default | Meaning |
|---|---|---|---|
| `transport` | `sse`\|`websocket`\|`websocket-cached`\|`auto` | `"auto"` | Preferred transport for providers that offer more than one. |
| `httpIdleTimeoutMs` | number \| numeric string \| `"disabled"` | `300000` | Longest idle gap while awaiting HTTP headers or body. See below. |
| `cacheWarming` | `off`\|`streaming`\|`idle` | `"streaming"` | Keep a provider's prompt cache alive across a long turn. **Global scope only.** See below. |
| `websocketConnectTimeoutMs` | number \| numeric string | *unset* | WebSocket connect timeout. See below. |
| `httpProxy` | string | *unset* | Proxy URL. **Global scope only** — see below. Blank or whitespace falls through to the proxy environment variables. |
| `retry.enabled` | bool | `true` | Retry failed requests. |
| `retry.maxRetries` | integer | `3` | Retry attempts cyrup makes. |
| `retry.baseDelayMs` | integer | `2000` | Base backoff delay. |
| `retry.maxAgentDelayMs` | integer | `60000` | Ceiling on each doubling backoff delay. |
| `retry.provider.maxRetryDelayMs` | integer | `60000` | Backoff ceiling inside the provider SDK. |
| `retry.provider.timeoutMs` | integer | *unset* | Request timeout inside the provider SDK. |
| `retry.provider.maxRetries` | integer | *unset* | Retry attempts inside the provider SDK. |

With `httpProxy` unset, cyrup reads `HTTPS_PROXY`, `HTTP_PROXY`, `https_proxy`, `http_proxy` in
that order.

**`httpProxy` is honoured only in the global file.** Like `defaultProjectTrust`, it is stripped from
the project layer before the merge, so a checked-in `.cyrup/settings.json` cannot redirect a
session's egress — even in a trusted project. A project-layer `httpProxy` contributes nothing at
all.

The setting is installed at process start, before any subcommand is dispatched and before a session
exists, so the pre-session network paths honour it too: `cyrup update --models`, `cyrup auth check`
and `cyrup auth print-bearer-token`'s OAuth refresh. Earlier builds configured the proxy only when
a session was built, so those three went direct to the network and reported success. An ambient
`HTTP_PROXY`/`HTTPS_PROXY` still wins over the setting at the point a request is made.

## cacheWarming

A provider's prompt cache expires on its own clock — five minutes on Anthropic's short tier, an
hour on the long one. If a single turn runs longer than that, because a tool call or a subagent
took a while, the cache entry is gone by the time the next request goes out and you pay a full
cache **write** of the whole prompt instead of a cheap cache **read**.

Cache warming avoids that. Shortly before the entry would expire, cyrup re-sends the request it
already sent, unchanged except for a one-token output cap, which refreshes the cache for the price
of one cache read plus one output token.

| Value | Meaning |
|---|---|
| `off` | Never send a warm request. |
| `streaming` | Warm while the agent is running. The default. |
| `idle` | Also warm between runs, for as long as continuing to pay for it still looks cheaper than losing the cache. |

It only ever warms when the arithmetic says it saves money: cyrup prices the cache write you would
otherwise pay, prices the warm, and sends nothing unless the expected saving is at least **$0.05**.
In `idle` it additionally discounts that saving by the chance you come back and use the cache at
all, so an idle session with a small prompt stops warming rather than paying to hold a cache nobody
will read. Two fixed safety limits cap a run whatever the economics say — one hour from the first
request while streaming, thirty minutes once idle — and a successful warm never pushes them out.

**Today this only applies to models served directly by Anthropic.** Warming needs to know when the
cache expires, and cyrup records a cache lifetime only for Anthropic's own API (`anthropic` over
`anthropic-messages`). Every other provider — including a gateway or proxy that speaks Anthropic's
wire format — is left unannotated on purpose: its cache is its own, and guessing Anthropic's five
minutes for someone else's cache would spend your money on a request that refreshes nothing. On
those models `/session` reports `Inactive (cache lifetime unavailable)`, which is the setting
working as intended rather than a fault. A model declared in
[`models.json`](../guides/models.md#custom-providers-and-models) can carry its own `promptCache`
lifetimes, and warming then applies to it too.

A request that asked for no caching at all is reported as `Inactive (request disabled prompt
caching)`. An Anthropic request using budget-based thinking is never warmed: Anthropic derives the
thinking budget from the output cap and keys the cache on it, so a one-token replay would be a
different request — and could still think for thousands of tokens, which is the opposite of cheap.

Each warm is recorded in the session as a `usage` entry, so it shows up in `/session`'s cost and in
the per-model cost breakdown; a warm is real spend and cyrup does not hide it. With
`showCacheMissNotices` on, each one also prints a dim `Cache warmed: $0.0012` line in the
transcript.

`/session` shows the live decision — the mode, the status, and, when both the prompt size and the
model's prices are known, what a cache miss would cost against what a warm costs. `/settings` has
the same three values under **Cache warming**, and changing it there reaches the running session
immediately: switching to `off` disarms a warm that was already scheduled rather than waiting for
the next turn. See [Sessions](../guides/sessions.md#prompt-cache-warming).

Like `httpProxy`, this key is **global scope only** and is stripped from the project layer before
the merge, so a repository you open cannot start spending your money on warm requests.

An extension can override each decision, forcing a warm cyrup would have skipped or vetoing one it
would have sent, by handling the `cache_warming_decision` event — the guest export
`events.on-cache-warming-decision`. Where two handlers disagree the last one to answer wins, and a
handler that faults, hangs or answers with anything but `warm` or `stop` leaves cyrup's own decision
in force. See [Writing an extension](../extensions/authoring.md).

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

The built-in extensions are named `builtin:llama.cpp`, `builtin:codemode`, `builtin:tool-search`
and `builtin:mcp` in `extensions`. They load by default; `"extensions": ["-builtin:llama.cpp"]` in
the global settings disables one. `--no-extensions` disables them too, and `-e builtin:<name>`
loads one explicitly, also under `--no-extensions` (except `codemode`, which that flag still
drops). Two parts of pi's behaviour are not ported yet: a `+builtin:<name>` or
`-builtin:<name>` entry in project settings does not override the global one, and `cyrup config`
does not list the built-ins.

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

The settings layer keeps the key's three states apart — `true`, `false` and absent. The one place
that reads it, the `Warnings` submenu under `/settings`, shows an absent key as on. **Nothing emits
the warning yet.** Toggling the row writes `warnings.anthropicExtraUsage` and nothing else changes,
because the warning it would suppress is not built in this release.

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

A present-but-invalid value is an error, not a fallback. `"httpIdleTimeoutMs": "5 minutes"`
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

This key is **global scope only**. It, `httpProxy` and `cacheWarming` are the keys stripped from
project settings before the merge, so a repository cannot declare itself trusted, redirect your
egress, or change what cache warming spends.

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

`-l` (or `--local`) opens the picker already in project scope. `Tab` switches scope from inside the
picker, but only in a trusted project — in an untrusted one the binding is not armed and the hint is
not shown, which is the same rule that makes `cyrup config -l` refuse there.

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
| `packages/` | Installed packages and their registry: `packages.json` plus one directory per package id, directly under `packages/`. |

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

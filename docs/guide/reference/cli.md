# Command line

Every flag and subcommand `cyrup` accepts. For the environment variables that change the same
behaviour, see [Environment variables](environment.md).

## Synopsis

```text
cyrup [options] [@files...] [messages...]

cyrup install <source> [-l] [--approve|--no-approve]
cyrup remove <source>  [-l] [--approve|--no-approve]
cyrup uninstall <source> [-l] [--approve|--no-approve]
cyrup update [source|cyrup|self|pi] [--self|--extensions|--models|--all] [--extension <source>]
             [--approve|--no-approve] [--force]
cyrup list [--approve|--no-approve]
cyrup config [-l] [--approve|--no-approve]

cyrup auth print-api-key      [--provider <p>] [--model <m>]
cyrup auth print-bearer-token [--provider <p>] [--model <m>] [--min-expiry <duration>]
cyrup auth check              [--provider <p>] [--model <m>] [--json] [--credentials] [--no-refresh]
```

Subcommands are dispatched from the first non-flag token, before the option parser runs. A first
token starting with `-` or `@` is never a subcommand, so `cyrup @notes.md install` sends a file
called `install` — not the installer.

With no subcommand, cyrup starts the [terminal interface](../guides/tui.md), unless `--print`,
`--mode json`, `--mode rpc`, `--mode acp`, or a non-TTY stdin or stdout selects a non-interactive
mode.

## Subcommands

### cyrup install

Installs a package — a git repository or a local directory that may contain extensions, skills,
prompt templates, themes and subagent personas.

```sh
cyrup install git:github.com/user/repo
```

| Flag | Argument | Meaning |
|---|---|---|
| `-l`, `--local` | — | Install into the project (`.cyrup/`) instead of globally |
| `-a`, `--approve` | — | Trust project-local files for this command |
| `-na`, `--no-approve` | — | Ignore project-local files for this command |

Accepted sources: `git:github.com/user/repo`, `git:git@github.com:user/repo`,
`https://github.com/user/repo`, `ssh://git@github.com/user/repo`, `github:user/repo`, and local
paths such as `./my-package`. A trailing `@ref` pins a tag or a commit, and a pinned package is
skipped by bulk updates. `npm:` sources are rejected — there is no JS runtime. A local package is
referenced where it sits; it is not copied.

`-l` requires the project to be trusted. In an untrusted project it prints `Project is not trusted.
Use --approve to modify local package config.` and exits 1.

**Installing does not touch `settings.json`.** Installed packages are recorded in a separate
`packages.json` registry — under the package directory for a global install, and at
`.cyrup/packages.json` for `-l`. The `packages` array in `settings.json` is a different,
hand-authored channel that you maintain yourself; `cyrup install` never writes to it, and
`cyrup remove` never removes from it. `cyrup install --help` says exactly this: "Install a package
and record it in the package registry".

### cyrup remove

Removes an installed package. `cyrup uninstall` is an alias.

```sh
cyrup remove git:github.com/user/repo
```

Flags are the same as `install`: `-l`/`--local`, `-a`/`--approve`, `-na`/`--no-approve`. As with
`install`, the change lands in `packages.json`, not in `settings.json`.

The source you pass does not have to be spelled the way you installed it. `remove` normalises the
argument the same way `install` did before looking it up — an `https://` URL, an `scp`-style
`git@host:user/repo`, a `.git` suffix or a relative path all resolve to the id the registry holds —
and falls back to the literal string for rows written by older builds. A source that matches
nothing prints `No matching package found for <source>` and exits 1.

### cyrup update

Updates installed packages, or refreshes the model catalogs.

```sh
cyrup update --extensions
```

| Flag | Argument | Meaning |
|---|---|---|
| `--self` | — | Target cyrup itself (the default when no target is given) — unavailable, see below |
| `--extensions` | — | Update installed packages only |
| `--models` | — | Refresh the remote model catalogs only |
| `--all` | — | Update cyrup and installed packages |
| `--extension` | `<source>` | Update one package only; may be given once |
| `--force` | — | Reinstall cyrup even when the current version is the latest — unavailable |
| `-a`, `--approve` | — | Trust project-local files for this command |
| `-na`, `--no-approve` | — | Ignore project-local files for this command |

A bare source argument updates that one package: `cyrup update git:github.com/user/repo`. It is
normalised the same way `cyrup remove`'s argument is. Three positional spellings mean cyrup itself:
`cyrup update cyrup`, `cyrup update self` and `cyrup update pi` all select the self-update target.
`--models`, `--extension`, `--all` and a positional source are mutually exclusive; combining them
prints the conflict and the usage line, and exits 1.

`cyrup update --models` refreshes the remote model catalog for every authenticated provider and
prints `Model catalogs refreshed`, or `Error: <message>` and exit 1. It is dispatched before the
trust and settings work the other targets do, so it needs neither a trusted project nor any package
state.

**`cyrup update` cannot update cyrup itself in this build.** Any self-update target — bare
`cyrup update`, `--self`, `--all`, `--force`, or a `cyrup`/`self`/`pi` positional — prints three
lines to stderr and exits 1:

```text
error: cyrup cannot self-update this installation.
Update it with: cargo install --git https://github.com/cyrup-ai/cyrup cyrup

Location of cyrup executable: /path/to/cyrup
```

An invocation that names no target at all — bare `cyrup update`, and `cyrup update --force`, since
`--force` selects nothing on its own — prints one line on stdout ahead of those three:
`Extensions are skipped. Run cyrup update --extensions to update extensions.` Naming any target,
`--self` included, suppresses it. `--all` does its package work first and reaches the stub after, so
it exits 1 even when every package updated cleanly.

`cyrup update --help` says the same thing: it leads with "Self-update is unavailable in this build"
and marks the four self-update routes `(UNAVAILABLE)`. Reinstall from source to upgrade — see
[Install](../getting-started/install.md). Packages pinned to a tag or commit are skipped by
`--extensions` and `--all`.

### cyrup list

Lists installed packages, grouped into user and project blocks, each line showing the source, a
`(filtered)` marker when the package has disabled resources, and the on-disk path when it exists.
Prints `No packages installed.` when there are none.

```sh
cyrup list
```

Accepts `-a`/`--approve` and `-na`/`--no-approve`.

### cyrup config

Opens a terminal picker for enabling and disabling the skills, prompt templates and themes
contributed by installed packages. Your choices are written into the `skills`, `prompts` and
`themes` arrays in `settings.json` as `+pattern` / `-pattern` entries.

```sh
cyrup config
```

`-l`/`--local` writes to the project `settings.json` instead of the global one, and requires the
project to be trusted; without it the command prints `Project is not trusted. Use --approve to
modify local resource config.` and exits 1. `Tab` inside the picker switches the write scope
between global and project.

`cyrup config -h` / `--help` prints its own usage block and exits 0 — the flag no longer falls
through and opens the picker. Any other flag prints `Unknown option <flag> for "config".` and a
stray positional prints `Unexpected argument <arg>.`; both exit 1.

### cyrup auth

Prints or checks credentials for external clients. There is no `cyrup auth login`; you sign in with
`/login` inside the interactive interface, or by exporting a provider environment variable — see
[Connect a provider](../getting-started/authenticate.md).

Every `auth` form requires at least one of `--provider` or `--model`.

```sh
cyrup auth print-api-key --provider openai --model gpt-5.5
```

| Form | Flags | Meaning |
|---|---|---|
| `print-api-key` | `--provider <p>`, `--model <m>` | Print the resolved API key on stdout |
| `print-bearer-token` | `--provider <p>`, `--model <m>`, `--min-expiry <duration>` | Print an OAuth bearer token, refreshing it if expired |
| `check` | `--provider <p>`, `--model <m>`, `--json`, `--credentials`, `--no-refresh` | Report whether the provider is ready to use |

`--min-expiry` takes a duration of the form `<number><unit>` where the unit is `ms`, `s`, `m` or
`h` — `30m`, `1h`, `500ms`. Days are not a unit, and a bare number is rejected. The token is
refreshed if it would expire inside that window.

`check` prints one of `ready`, `not_ready` or `invalid`. `--credentials` prints the credential
itself instead of the status word; `--json` prints the whole result object; `--no-refresh` stops it
refreshing an expired OAuth credential. See [Exit codes](#exit-codes).

```sh
cyrup auth check --provider anthropic --json
```

## Options

Options apply to a normal `cyrup` run, not to the subcommands above.

### Model and provider

| Flag | Argument | Meaning |
|---|---|---|
| `--provider` | `<name>` | Provider id, e.g. `anthropic`, `openai`, `openrouter` |
| `--model` | `<pattern>` | Model pattern or id; accepts `provider/id` and a `:<thinking>` suffix |
| `--models` | `<patterns>` | Comma-separated patterns for the `Ctrl+P` cycling set; supports globs |
| `--api-key` | `<key>` | Runtime API key for this run; requires `--model`, `--provider` or `--models` |
| `--thinking` | `<level>` | Starting thinking level: `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max` |
| `--list-models` | `[search]` | List the models you have configured access to, then exit |

```sh
cyrup --model openai/gpt-4o "explain this repo"
```

**There is no default provider.** `cyrup --help` prints `--provider <name>  Provider name (default:
google)`; that string is wrong and nothing in the code implements it. With no `--provider` and no
`provider/` prefix, cyrup picks a starting model by walking this ladder:

1. `--provider` together with `--model`, resolved directly.
2. The first model in `--models` — skipped when `--continue` or `--resume` is in play.
3. `defaultProvider` plus `defaultModel` from [`settings.json`](settings.md), used only if that
   provider has credentials configured.
4. The first provider, in a fixed internal order, whose curated default model is among the models
   you can actually reach.
5. Nothing — the session starts with no model.

An invalid `--thinking` value does not abort the run: cyrup warns, drops the flag, and continues
with no level set. Model patterns, globs and the `:level` suffix are covered in
[Models and thinking](../guides/models.md).

`--list-models` lists only models whose provider has credentials configured, so an empty listing
means nothing is authenticated rather than that the catalog is broken. Its search pattern is
optional, and cyrup only claims the following token when it starts with neither `-` nor `@` —
`cyrup --list-models @notes.md` lists the whole catalog and leaves `@notes.md` as a file argument,
while `cyrup --list-models gpt` filters. With no match it prints `No models matching "<pattern>"`.

### Session

| Flag | Argument | Meaning |
|---|---|---|
| `-c`, `--continue` | — | Continue the most recent session for this directory |
| `-r`, `--resume` | — | Open the session picker |
| `--session` | `<path\|id>` | Use a specific session file or partial UUID |
| `--session-id` | `<id>` | Use an exact project session id, creating it if missing |
| `--fork` | `<path\|id>` | Fork a session file or partial UUID into a new session |
| `--session-dir` | `<dir>` | Directory for session storage and lookup |
| `--no-session` | — | Do not save the session |
| `-n`, `--name` | `<name>` | Set the session display name |
| `--export` | `<file>` | Render a session `.jsonl` to standalone HTML and exit |

```sh
cyrup --continue "what did we decide about the retry policy?"
```

`--fork` cannot be combined with `--session`, `--continue`, `--resume` or `--no-session`.
`--session-id` cannot be combined with `--session`, `--continue` or `--resume`, and its value must be
non-empty, alphanumeric at both ends, and otherwise made only of letters, digits, `-`, `_` and `.`.
`--name` must be non-empty after trimming.

`--export` takes an optional output path — the first *message* positional, so an `@file` token in
the same command line is never mistaken for it. Without a path it writes alongside the input with
an `.html` extension. On success it prints `Exported to: <path>`; on failure, `Error: <message>` and
exit 1.

```sh
cyrup --export session.jsonl output.html
```

`--export` runs and exits immediately after `--version`, before the session-flag validators, the
RPC `@file` guard and the `--api-key requires a model` check. So it still exports when the rest of
the command line is contradictory — `cyrup --export s.jsonl --fork X --continue` writes the HTML
rather than erroring — which is the state a session is usually in when you reach for it.

More in [Sessions](../guides/sessions.md).

### Output mode

| Flag | Argument | Meaning |
|---|---|---|
| `--mode` | `<text\|json\|rpc\|acp>` | Output mode; `text` is the default |
| `-p`, `--print` | — | Run the prompt to completion, print the final text, exit |
| `--acp` | — | Alias for `--mode acp`; serve the Agent Client Protocol on stdio |
| `--tui-mode` | `<regular\|fullscreen>` | TUI renderer; `regular` is the default |

Precedence when several are given: `acp`, then `rpc`, then `json`, then `print`. A non-TTY stdin or stdout
selects print mode on its own, which is what makes `cyrup -p` redundant inside a pipe.

`--mode rpc` ends when its input does, as pi's does: closing stdin asks for the shutdown. The run in
flight, a running `bash` command, a compaction and every open dialog are aborted at once, not waited
for, and the process exits as soon as what was aborted has reported (at most five seconds). A client
that wants a run's output keeps stdin open until it has read `agent_settled`, then closes it.
`printf '{"type":"prompt","message":"hi"}\n' | cyrup --mode rpc` therefore stops the run it starts;
use `cyrup -p` for a one-shot prompt.

A token beginning with `---` immediately after `-p`/`--print` is taken as the prompt, not as a flag:
`cyrup -p ---weird` sends the literal text `---weird`. It keeps its place among the positionals. A
genuine unknown long flag (`--weird`) is still captured as an extension flag.

`--tui-mode fullscreen` parses but the alternate-screen renderer is not built in this release; cyrup
warns and falls back to `regular`.

`--acp` is a cyrup addition, kept because ACP editors are configured to launch `cyrup --acp`.
Because it is a known flag, an extension cannot register a flag of that name and receive it. There
are no `--json`, `--rpc` or `--output-format` shorthands: use `--mode json`, `--mode rpc` and
`--print`. See [Scripting and automation](../guides/scripting.md).

### Tools

| Flag | Argument | Meaning |
|---|---|---|
| `-nt`, `--no-tools` | — | Disable every tool, built-in and extension |
| `-nbt`, `--no-builtin-tools` | — | Start with every built-in tool off; extension and custom tools stay |
| `-t`, `--tools` | `<tools>` | Comma-separated allowlist of tool names or patterns (`*`) |
| `-xt`, `--exclude-tools` | `<tools>` | Comma-separated denylist of tool names or patterns (`*`) |

Eight built-in tools are registered — `read`, `bash`, `edit`, `write`, `grep`, `find`, `ls` and `powershell` — but
**only four of them are active in a default session**: `read`, `bash`, `edit` and `write`. `grep`,
`find`, `ls` and `powershell` are registered and reachable; they simply are not in the default active set, so name
them to switch them on: `--tools read,grep,find`.

`--no-builtin-tools` turns off every built-in tool, not only the four defaults: `read`, `bash`,
`edit`, `write`, and also `grep`, `find`, `ls` and `powershell`. What stays active is what is not a
built-in and starts active — the `mcp` gateway tool, for one. The built-ins remain registered, so
something that activates tools by name, an extension for one, can still turn one on: an armed
[permission system](../guides/tools-and-permissions.md#the-permission-system) does, and with a policy
present `-nbt` leaves every tool its policy does not deny active.
`codemode` and `tool_search` start inactive, so this flag does not switch them on either; to run
with `codemode` and no other built-in, name it: `--tools codemode`. Use `--no-tools` for a run with no
tools at all; it wins over `--no-builtin-tools` when both are given. `--exclude-tools` is applied
last, on top of whichever set the other three produced.

Values are comma-split and trimmed, so `--tools "read, grep"` works.

An entry with a `*` is a pattern that matches any run of characters. MCP tools are named by the
`toolPrefix` setting of the MCP configuration, `<server>_<tool>` unless it says otherwise, so
`--exclude-tools "github_*"` drops every tool of a server named `github` and `--tools "read,docs_*"`
activates the tools of one named `docs`. With `"toolPrefix": "mcp"` the same tools are
`mcp__github_<tool>` and `mcp__docs_<tool>`.

MCP tools are treated apart. A `--tools` list with no entry that starts with `mcp__` leaves MCP
tools registered, and what that reaches depends on how the server registers them (see
[MCP tools](../guides/codemode.md#mcp-tools)): the tools of a `directTools: "search"` server stay
callable from `codemode` scripts and loadable by `tool_search`, while eager tools and the `mcp`
gateway are callable from a script only while active, so `--tools read,codemode` leaves a script
without them. An entry that starts with `mcp__`, or an empty list (as `--no-tools` makes), makes the
list decide for MCP tools too. The default `toolPrefix` gives no tool a name that starts with `mcp__`,
so by default such an entry matches nothing and cuts off every MCP tool; to filter by server, set
`"toolPrefix": "mcp"` and write `mcp__<server>_*`. `--exclude-tools` applies to every tool, MCP tools
included. A registered MCP tool is declared to the model only when the list names it or matches it,
or when it is a `deferred` (`directTools: "search"`) tool and `tool_search` is active.

**A repeated `--tools`, `-t`, `--exclude-tools` or `--models` replaces the earlier one — it does not
append.** `--tools read --tools bash` enables `bash` alone. The comma form is the way to name
several: `--tools read,bash`. The `=` spelling counts as an occurrence too, so
`--tools read --tools=bash` is also just `bash`.

```sh
cyrup --tools read,grep,find,ls -p "review the code in src/"
```

See [Tools and permissions](../guides/tools-and-permissions.md).

Built-in extensions add two more tools. Both start inactive. An MCP server that connects does not
turn them on; name them in `--tools` or in `defaultTools`. The one exception is an armed permission
system: with a policy present, every tool the policy does not deny is active, these two included (see
[The permission system](../guides/tools-and-permissions.md#the-permission-system)).

| Built-in extension | Purpose |
|---|---|
| `codemode` | Run JavaScript that calls the other tools, for example in parallel with `Promise.allSettled`; only the script's output reaches the model |
| `tool_search` | Search the tools that are not declared to the model (`codemode` and `deferred` exposure, such as the tools of an MCP server in search mode) and declare the matches for the next call |

#### Enable codemode

To turn `codemode` on for every session, add it to the default tools in `~/.cyrup/agent/settings.json`:

```json
{
  "defaultTools": ["+codemode"]
}
```

This keeps `read`, `bash`, `edit`, and `write` and adds `codemode`. For one invocation, list every
tool, since `--tools` replaces the selection:

```sh
cyrup --tools read,bash,edit,write,codemode
```

The same line in a project's `.cyrup/settings.json` takes effect only in a trusted project, which in
`-p`, `--mode json` and `--mode rpc` means passing `--approve`. `--no-extensions` removes the tool.
With a permission policy present `codemode` is already active and needs no line; to keep it off
there, deny it in the policy or pass `--exclude-tools codemode`.

Codemode is useful without MCP: a script can run several tool calls in parallel, filter large output
before it reaches the model, call classifier models through `models.classify()` and generate images
through `models.generateImages()`. Scripts run in a V8 sandbox, each in a process of its own, and
reach the other tools through `tools.<name>(args)` under the same permission gate as any call.
[Codemode](../guides/codemode.md) describes the script API, which tools a script can call, the
security model, the limits and what you see while a script runs.

#### Tool search

`tool_search` is off by default (a permission policy turns it on, like `codemode`); enable it with
`"defaultTools": ["+tool_search"]` or `--tools`. It ranks the tools that are not declared yet, the
same set `searchTools()` finds in a script, and declares the matches from the model's next call on.
A script cannot call `tool_search`.

### Resources

| Flag | Argument | Meaning |
|---|---|---|
| `-e`, `--extension` | `<path>` | Load an extension file or directory, or a built-in extension such as `builtin:llama.cpp`; repeatable |
| `-ne`, `--no-extensions` | — | Disable extension discovery and the built-in extensions; explicit `-e` paths still load, so `cyrup -ne -e builtin:llama.cpp` keeps only llama.cpp |
| `--no-mcp` | — | Disable built-in MCP support for this run: no servers connect and no MCP tools |
| `--skill` | `<path>` | Load a skill file or directory; repeatable |
| `-ns`, `--no-skills` | — | Disable skill discovery and loading |
| `--prompt-template` | `<path>` | Load a prompt template file or directory; repeatable |
| `-np`, `--no-prompt-templates` | — | Disable prompt template discovery and loading |
| `--theme` | `<path>` | Load a theme file or directory; repeatable |
| `--use-theme` | `<name[/name]>` | Start the interactive UI on this theme (or `light/dark` pair) for this run only; not saved to settings |
| `--no-themes` | — | Disable theme discovery and loading |
| `-nc`, `--no-context-files` | — | Do not load `AGENTS.md` and `CLAUDE.md` |
| `--system-prompt` | `<text\|path>` | Replace the assembled system prompt |
| `--append-system-prompt` | `<text\|path>` | Append after the assembled system prompt; repeatable |

`--system-prompt` and `--append-system-prompt` take either literal text or a path — cyrup decides by
checking whether the value names an existing file. Multiple `--append-system-prompt` values are
joined with a blank line.

`--no-extensions` also turns off installed-package extensions and the native extensions
(subagents, the permission system, intercom, and the built-in MCP and `codemode` ones, so the `mcp`
gateway tool and `codemode` do not exist in that run), and removes the built-in [llama.cpp](../llama-cpp.md)
provider and its `/llama` command. `-e` paths survive it. Relative resource paths resolve
against the current directory.

```sh
cyrup -e ./target/wasm32-wasip2/debug/my_ext.wasm --no-extensions
```

`--no-themes` has no short alias. See [Project context and skills](../guides/project-context.md),
[Themes](../guides/themes.md) and [How extensions work](../extensions/overview.md).

### Trust

| Flag | Argument | Meaning |
|---|---|---|
| `-a`, `--approve` | — | Trust project-local files for this run |
| `-na`, `--no-approve` | — | Ignore project-local files for this run |

Neither flag is written to disk; both override the saved decision for this run only, and `--approve`
wins if you pass both. Project settings, project extensions, project packages and project context
files load only when the project is trusted.

### Startup and diagnostics

| Flag | Argument | Meaning |
|---|---|---|
| `--offline` | — | Disable startup network operations; same as `CYRUP_OFFLINE=1` |
| `--verbose` | — | Force verbose startup, overriding the `quietStartup` setting |
| `-h`, `--help` | — | Print help and exit |
| `-v`, `--version` | — | Print the version and exit |

`--offline` governs startup work only — the update check, install telemetry, analytics and the model
catalog refresh. It does not stop an inference request once you take a turn.

## `@file` arguments and messages

Positional arguments are either file references or message text.

```sh
cyrup @prompt.md @screenshot.png "what is wrong with this layout?"
```

An argument beginning with `@` is a file reference: the path is tilde-expanded and resolved against
the current directory. Text files are inlined into the first message; image files are attached as
images. An oversized image is not downscaled here — the session does that once the request model is
known, against that model's
[resize profile](../guides/models.md#how-big-an-image-gets-sent), which is what the
`images.autoResize` setting governs. Empty files are skipped, and a missing file is an error that
exits 1.

Write `@@` to send a message that legitimately starts with `@` — `@@channel` becomes the literal text
`@channel`.

Everything else is message text. The first message becomes the initial prompt, prepended with piped
stdin and the contents of any `@file` arguments. Additional messages are queued and replayed one
prompt at a time after the first run completes.

```sh
cyrup "read package.json" "what dependencies do we have?"
```

## Exit codes

| Code | When |
|---|---|
| `0` | Success; `cyrup auth check` reporting `ready` |
| `1` | Usage errors, a missing `@file`, a failed `--export`, `cyrup install -l` in an untrusted project, a failed `cyrup auth print-api-key` or `print-bearer-token`, `cyrup auth check` reporting `not_ready`, and a non-interactive run with no configured provider |
| `2` | `cyrup auth check` reporting `invalid`, or failing outright |

An interactive run with no configured provider does not exit — it starts with no model and tells you
to use `/login`.

## Flags cyrup does not define

An unrecognised `--flag` is not an error. It is captured and handed to loaded extensions, so
`--help` output varies with what you have installed: an extension that registers `--plan` adds that
line to the help body. An unrecognised single-dash flag is still a usage error, and that includes a
bare `-`: `cyrup -` prints `Unknown option: -` and exits 1 without contacting a provider. A bare
`--` is left alone for the extension-flag capture.

Two subcommands, `__intercom-broker` and `__subagent-runner`, exist so cyrup can re-execute itself as
a child process. They are not part of the user-facing surface and should not be called directly.

`cyrup --help` also lists `CYRUP_SHARE_VIEWER_URL`. It is read — by `/share`, as the base of the
viewer link — and the help row now carries its default,
`https://pi.dev/session/`. See [Environment variables](environment.md).

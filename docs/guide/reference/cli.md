# Command line

Every flag and subcommand `cyrup` accepts. For the environment variables that change the same
behaviour, see [Environment variables](environment.md).

## Synopsis

```text
cyrup [options] [@files...] [messages...]

cyrup install <source> [-l] [--approve|--no-approve]
cyrup remove <source>  [-l] [--approve|--no-approve]
cyrup uninstall <source> [-l] [--approve|--no-approve]
cyrup update [source|self|pi] [--self|--extensions|--all] [--extension <source>] [--force]
cyrup list [--approve|--no-approve]
cyrup config [-l]

cyrup auth print-api-key      [--provider <p>] [--model <m>]
cyrup auth print-bearer-token [--provider <p>] [--model <m>] [--min-expiry <duration>]
cyrup auth check              [--provider <p>] [--model <m>] [--json] [--credentials] [--no-refresh]
```

Subcommands are dispatched from the first non-flag token, before the option parser runs. A first
token starting with `-` or `@` is never a subcommand, so `cyrup @notes.md install` sends a file
called `install` — not the installer.

With no subcommand, cyrup starts the [terminal interface](../guides/tui.md), unless `--print`,
`--mode json`, `--mode rpc`, or a non-TTY stdin or stdout selects a non-interactive mode.

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

**Installing does not touch `settings.json`.** The help text says a package is "added to settings";
it is not. Installed packages are recorded in a separate `packages.json` registry — under the
package directory for a global install, and at `.cyrup/packages.json` for `-l`. The `packages` array
in `settings.json` is a different, hand-authored channel that you maintain yourself; `cyrup install`
never writes to it, and `cyrup remove` never removes from it.

### cyrup remove

Removes an installed package. `cyrup uninstall` is an alias.

```sh
cyrup remove git:github.com/user/repo
```

Flags are the same as `install`: `-l`/`--local`, `-a`/`--approve`, `-na`/`--no-approve`. As with
`install`, the change lands in `packages.json`, not in `settings.json`, despite what the help text
says. The `npm:` examples printed by `cyrup remove --help` cannot be produced by `cyrup install` in
this build.

### cyrup update

Updates installed packages.

```sh
cyrup update --extensions
```

| Flag | Argument | Meaning |
|---|---|---|
| `--self` | — | Target cyrup itself (the default when no target is given) |
| `--extensions` | — | Update installed packages only |
| `--all` | — | Update cyrup and installed packages |
| `--extension` | `<source>` | Update one package only; may be given once |
| `--force` | — | Reinstall cyrup even when the current version is the latest |
| `-a`, `--approve` | — | Trust project-local files for this command |
| `-na`, `--no-approve` | — | Ignore project-local files for this command |

A bare source argument updates that one package: `cyrup update git:github.com/user/repo`.
`cyrup update pi` is an alias for `--self`.

**`cyrup update` cannot update cyrup itself in this build.** Any self-update target prints
`Self-update is not available in this build; update cyrup via your package manager.` Reinstall from
source to upgrade — see [Install](../getting-started/install.md). Packages pinned to a tag or commit
are skipped by `--extensions` and `--all`.

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
project to be trusted. `cyrup config` has no `--help` of its own — passing it runs the picker.

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

`--export` takes an optional second positional as the output path; without one it writes alongside
the input with an `.html` extension.

```sh
cyrup --export session.jsonl output.html
```

More in [Sessions](../guides/sessions.md).

### Output mode

| Flag | Argument | Meaning |
|---|---|---|
| `--mode` | `<text\|json\|rpc>` | Output mode; `text` is the default |
| `-p`, `--print` | — | Run the prompt to completion, print the final text, exit |
| `--json` | — | Alias for `--mode json` |
| `--rpc` | — | Alias for `--mode rpc` |
| `--output-format` | `<text\|json>` | Alias: `text` means `--print`, `json` means `--mode json` |
| `--tui-mode` | `<regular\|fullscreen>` | TUI renderer; `regular` is the default |

Precedence when several are given: `rpc`, then `json`, then `print`. A non-TTY stdin or stdout
selects print mode on its own, which is what makes `cyrup -p` redundant inside a pipe.

`--tui-mode fullscreen` parses but the alternate-screen renderer is not built in this release; cyrup
warns and falls back to `regular`.

`--json`, `--rpc` and `--output-format` are cyrup additions. Because they are known flags, an
extension cannot register a flag of the same name and receive it. See
[Scripting and automation](../guides/scripting.md).

### Tools

| Flag | Argument | Meaning |
|---|---|---|
| `-nt`, `--no-tools` | — | Disable all tools by default, built-in and extension |
| `-nbt`, `--no-builtin-tools` | — | Disable built-in tools but keep extension and custom tools |
| `-t`, `--tools` | `<tools>` | Comma-separated allowlist of tool names |
| `-xt`, `--exclude-tools` | `<tools>` | Comma-separated denylist of tool names |

The built-in tool names are `read`, `bash`, `edit`, `write`, `grep`, `find` and `ls`. Values are
comma-split and trimmed, so `--tools "read, grep"` works. `--no-tools` wins over
`--no-builtin-tools` when both are given.

```sh
cyrup --tools read,grep,find,ls -p "review the code in src/"
```

See [Tools and permissions](../guides/tools-and-permissions.md).

### Resources

| Flag | Argument | Meaning |
|---|---|---|
| `-e`, `--extension` | `<path>` | Load an extension file or directory; repeatable |
| `-ne`, `--no-extensions` | — | Disable extension discovery; explicit `-e` paths still load |
| `--skill` | `<path>` | Load a skill file or directory; repeatable |
| `-ns`, `--no-skills` | — | Disable skill discovery and loading |
| `--prompt-template` | `<path>` | Load a prompt template file or directory; repeatable |
| `-np`, `--no-prompt-templates` | — | Disable prompt template discovery and loading |
| `--theme` | `<path>` | Load a theme file or directory; repeatable |
| `--no-themes` | — | Disable theme discovery and loading |
| `-nc`, `--no-context-files` | — | Do not load `AGENTS.md` and `CLAUDE.md` |
| `--system-prompt` | `<text\|path>` | Replace the assembled system prompt |
| `--append-system-prompt` | `<text\|path>` | Append after the assembled system prompt; repeatable |

`--system-prompt` and `--append-system-prompt` take either literal text or a path — cyrup decides by
checking whether the value names an existing file. Multiple `--append-system-prompt` values are
joined with a blank line.

`--no-extensions` also turns off installed-package extensions and the three native extensions
(subagents, the permission system, intercom). `-e` paths survive it. Relative resource paths resolve
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
images, downscaled to fit 2000×2000 when the `images.autoResize` setting is on. Empty files are
skipped, and a missing file is an error that exits 1.

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
line to the help body. An unrecognised single-dash flag is still a usage error.

Two subcommands, `__intercom-broker` and `__subagent-runner`, exist so cyrup can re-execute itself as
a child process. They are not part of the user-facing surface and should not be called directly.

`cyrup --help` also lists `CYRUP_SHARE_VIEWER_URL`. Nothing in this build reads it.

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
| `--mode` | `<text\|json\|rpc>` | Output mode; `text` is the default |
| `-p`, `--print` | — | Run the prompt to completion, print the final text, exit |
| `--json` | — | Alias for `--mode json` |
| `--rpc` | — | Alias for `--mode rpc` |
| `--output-format` | `<text\|json>` | Alias: `text` means `--print`, `json` means `--mode json` |
| `--tui-mode` | `<regular\|fullscreen>` | TUI renderer; `regular` is the default |

Precedence when several are given: `rpc`, then `json`, then `print`. A non-TTY stdin or stdout
selects print mode on its own, which is what makes `cyrup -p` redundant inside a pipe.

A token beginning with `---` immediately after `-p`/`--print` is taken as the prompt, not as a flag:
`cyrup -p ---weird` sends the literal text `---weird`. It keeps its place among the positionals. A
genuine unknown long flag (`--weird`) is still captured as an extension flag.

`--tui-mode fullscreen` parses but the alternate-screen renderer is not built in this release; cyrup
warns and falls back to `regular`.

`--json`, `--rpc` and `--output-format` are cyrup additions. Because they are known flags, an
extension cannot register a flag of the same name and receive it. See
[Scripting and automation](../guides/scripting.md).

### Tools

| Flag | Argument | Meaning |
|---|---|---|
| `-nt`, `--no-tools` | — | Disable every tool, built-in and extension |
| `-nbt`, `--no-builtin-tools` | — | Drop the four default built-ins; see below |
| `-t`, `--tools` | `<tools>` | Comma-separated allowlist of tool names |
| `-xt`, `--exclude-tools` | `<tools>` | Comma-separated denylist of tool names |

Seven built-in tools are registered — `read`, `bash`, `edit`, `write`, `grep`, `find` and `ls` — but
**only four of them are active in a default session**: `read`, `bash`, `edit` and `write`. `grep`,
`find` and `ls` are registered and reachable; they simply are not in the default active set, so name
them to switch them on: `--tools read,grep,find`.

`--no-builtin-tools` is narrower than its name suggests. It drops exactly those four defaults, and
the active set becomes everything that is *not* one of them — `grep`, `find` and `ls` included,
alongside extension and custom tools. Use `--no-tools` for a run with no tools at all; it wins over
`--no-builtin-tools` when both are given. `--exclude-tools` is applied last, on top of whichever set
the other three produced.

Values are comma-split and trimmed, so `--tools "read, grep"` works.

**A repeated `--tools`, `-t`, `--exclude-tools` or `--models` replaces the earlier one — it does not
append.** `--tools read --tools bash` enables `bash` alone. The comma form is the way to name
several: `--tools read,bash`. The `=` spelling counts as an occurrence too, so
`--tools read --tools=bash` is also just `bash`.

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
line to the help body. An unrecognised single-dash flag is still a usage error, and that includes a
bare `-`: `cyrup -` prints `Unknown option: -` and exits 1 without contacting a provider. A bare
`--` is left alone for the extension-flag capture.

Two subcommands, `__intercom-broker` and `__subagent-runner`, exist so cyrup can re-execute itself as
a child process. They are not part of the user-facing surface and should not be called directly.

`cyrup --help` also lists `CYRUP_SHARE_VIEWER_URL`. It is read — by `/share`, as the base of the
viewer link — and the help row now carries its default,
`https://pi.dev/session/`. See [Environment variables](environment.md).

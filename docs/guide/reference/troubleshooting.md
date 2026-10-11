# Troubleshooting

Symptoms you are likely to hit, and what to do about each. If your problem is not here, `/debug`
and the tracing output described at the end of this page are the fastest way to find out what cyrup
thinks is going on.

## "No models available" on startup, or `--list-models` prints nothing

Nothing is authenticated. `--list-models` lists only models whose provider has complete
credentials, so an empty catalog means cyrup found no usable provider — not that the catalog is
broken.

Run `/login` inside a session, or export the provider's API key before launching:

```sh
export ANTHROPIC_API_KEY=sk-ant-...
cyrup
```

The message cyrup prints points you at `docs/providers.md` and `docs/models.md`. Those files are
not shipped with this repository — read [Connect a provider](../getting-started/authenticate.md)
and [Models and thinking](../guides/models.md) instead.

## A non-interactive run exits 1 immediately

`-p`, `--mode json` and `--mode rpc` cannot open a login prompt. With no configured provider they
print the same "no models available" guidance to stderr and exit 1.

Give the run a credential it can use without asking:

```sh
ANTHROPIC_API_KEY=sk-ant-... cyrup -p "summarise the changes on this branch"
```

Or pass `--api-key`, which requires one of `--model`, `--provider` or `--models` alongside it. See
[Scripting and automation](../guides/scripting.md).

## An environment variable seems to be ignored

A credential stored in `auth.json` beats the environment variable for that provider. If you ran
`/login` at some point, the key you are exporting now is never consulted.

Run `/logout` and pick the provider. It only lists providers that have a stored credential, and it
removes only that — environment variables and `models.json` are untouched. After that the
environment variable takes effect.

## `CYRUP_OFFLINE=on` does nothing

The core environment flags accept exactly `1`, `true` and `yes`, and they do not trim whitespace.
`on` is not a truthy value, and `CYRUP_OFFLINE=" 1"` is not either.

```sh
CYRUP_OFFLINE=1 cyrup
```

The same rule applies to `CYRUP_SKIP_VERSION_CHECK` and `CYRUP_TELEMETRY`. The extension opt-ins —
`CYRUP_SUBAGENTS`, `CYRUP_INTERCOM`, `CYRUP_PERMISSION_SYSTEM` — use a wider rule that does trim
and does accept `on`, which is why `on` appears to work for some variables and not others.

## Project settings, skills or extensions are not loading

The project is untrusted. An untrusted folder's `.cyrup/settings.json` is not read at all, and its
extensions, skills, prompts, themes and context files are skipped.

The interface says so directly. In an untrusted project it prints a warning banner after the initial
replay, and again after any `/resume`, `/fork` or `/import` that swaps sessions — the swap can bring
a different working directory and a different trust decision with it, so the answer is re-evaluated
each time:

```text
This project is not trusted. Project .cyrup resources and packages are ignored. Use /trust to save
a trust decision, then restart cyrup.
```

Run `/trust` inside the session to see and change the decision for the folder, or start the run
with `--approve` for a one-off override that is not saved:

```sh
cyrup --approve -p "review the diff"
```

To stop being asked, set `defaultProjectTrust` to `always` in the global `settings.json`. Note that
`-p`, `--mode json` and `--mode rpc` cannot prompt, so under the default `ask` policy an undecided
project is treated as untrusted in every non-interactive run.

## The permission system turned itself on unexpectedly

**A policy file is enough to arm the gate.** The permission system installs itself when
`CYRUP_PERMISSION_SYSTEM` is truthy, *or* when a `cyrup-permissions.jsonc` exists in the agent
directory or in `<repo>/.cyrup/agent/`, *or* when an `agents/` directory in either location is
non-empty, *or* when its own `config.json` differs from the template.

An armed gate also sets the active tools: every tool its policy does not deny is active, so a session
that had four tools gains `grep`, `find`, `ls`, `powershell`, `codemode` and `tool_search`, whatever
`defaultTools` says. See
[The permission system](../guides/tools-and-permissions.md#the-permission-system).

Unsetting the environment variable does not help while any of those hold. Turn it off explicitly:

```json
{ "enabled": false }
```

in `~/.cyrup/agent/cyrup-permission-system/config.json`. See
[The permission system](../extensions/permissions.md).

## Setting `CYRUP_HOME` did not move the config

`CYRUP_HOME` is not the variable that relocates the agent directory. `settings.json`, `auth.json`
and `trust.json` follow `CYRUP_AGENT_DIR`:

```sh
CYRUP_AGENT_DIR=/opt/cyrup-agent cyrup
```

`CYRUP_HOME` is read by the native extensions only. `CYRUP_CODING_AGENT_DIR` does move the config —
the config layer accepts it as a fallback when `CYRUP_AGENT_DIR` is unset — but it means a different
directory, `~/.cyrup` with no `agent` segment, and it also moves the intercom and subagent trees.
The three are compared side by side in [Environment variables](environment.md).

## Subagent files are not discovered

The subagents home is `~/.cyrup/agents` — not `~/.cyrup/agent/agents`. The singular `agent`
directory is where `settings.json` lives; the plural `agents` directory is where agent definitions
live. Project agents go in `<repo>/.cyrup/agents`.

Two other reasons a file is skipped: it is missing a `name` or a `description` in its frontmatter,
or it sits under a path segment named `skills`. See [Subagents](../extensions/subagents.md).

## `cyrup update` does not update cyrup

Self-update is not implemented. Every self-update route — bare `cyrup update`, `--self`, `--all`,
`--force`, and the `cyrup`/`self`/`pi` positionals — prints three lines to stderr and exits 1:

```text
error: cyrup cannot self-update this installation.
Update it with: cargo install --git https://github.com/cyrup-ai/cyrup cyrup

Location of cyrup executable: /path/to/cyrup
```

Bare `cyrup update` and `cyrup update --force` print one more line first, on stdout:
`Extensions are skipped. Run cyrup update --extensions to update extensions.` That is not a second
error — it is the note that neither of those two forms selected a package target.

Run that `cargo install` line to upgrade. The `--help` for `update` says the same, marking the four
unavailable routes.

`cyrup update <source>`, `cyrup update --extension <source>` and `cyrup update --extensions` do work
— they update installed packages. So does `cyrup update --models`, which refreshes the remote model
catalogs and prints `Model catalogs refreshed`.

## `cyrup install npm:...` fails

npm sources are rejected outright. cyrup has no JavaScript runtime, so there is nothing to run an
npm package with. Neither `cyrup install --help` nor `cyrup remove --help` offers an `npm:` example
any more — both show a git source and a local path.

Install from git or from a local path instead:

```sh
cyrup install git:github.com/acme/cyrup-pack
cyrup install ./tools/local-pack
```

## A custom theme does not appear in the `/settings` picker

The picker lists the two built-in themes, `dark` and `light`, and nothing else. Custom themes on
disk are loaded but not offered there.

Select one by name in `settings.json`:

```json
{ "theme": "solarized-night" }
```

The name is the `name` field inside the theme file, not the filename. See
[Themes](../guides/themes.md).

## `Shift+Enter` does nothing in my terminal

Many terminals do not send a distinct `Shift+Enter`. Use `Ctrl+J` for a newline, or end the line
with a backslash and press `Enter` — the backslash is removed and a newline inserted instead of
submitting.

## Undo or the char-jump keys do nothing

Your terminal does not implement the kitty keyboard protocol, so `Ctrl+-` and `Ctrl+]` never reach
cyrup. Press `Ctrl+7` for undo and `Ctrl+5` to jump forward — cyrup decodes them to the same
actions. `Ctrl+4` arrives as `Ctrl+\`, which has no default binding of its own.

## `xhigh` or `max` thinking has no effect

Those two levels are not implicit. A model supports them only if it declares them explicitly;
`off` through `high` are available on any model that reasons at all. When you ask for a level a
model does not support, the request is clamped to the nearest supported level rather than rejected
— so `max` silently becomes `high` on most models.

Providers that take a token budget rather than an effort string collapse `xhigh` and `max` into
`high` by design. `Shift+Tab` only cycles through levels the active model actually supports.

## cyrup runs under `ulimit -v`, and a wasm extension does not load

The wasm runtime reserves a pool of address space when it starts. Under a limited address space
(`ulimit -v`, a container's `--ulimit as`, a CI job wrapper) that reservation fails, so cyrup builds
the runtime without the pool and logs a `warn` line once: the session and the built-in extensions
(`codemode`, `mcp`, `flux`, the permission system) start normally. A wasm extension may still fail to
load under the limit, and a failure to load is reported like any other extension load failure.
Raise the limit, or remove the extension.

`codemode` scripts need more room than that: the script sandbox is a process of its own and V8
reserves a large range of address space when it starts. A script failed at a limit of 32 GiB and
below with `Script sandbox failed: ... terminated by signal 5 (SIGTRAP)` and ran at 40 GiB and above;
the message now names the limit when it sees one. The rest of the session is not affected.

## The first build takes forever

`cargo install --git ...` compiles a large dependency graph, including the WebAssembly host. A cold
build takes many minutes on a laptop and produces long stretches with no output. It is not hung —
run cargo with `-v` if you want to watch it move.

## A "not valid subagents config JSON" warning

`<agent dir>/subagents/config.json` failed to parse. cyrup warns on stderr and continues with the
default subagent configuration, so nothing is broken — but none of your settings in that file are
in effect.

One case is easy to miss: an unrecognised key inside the `missions` block rejects the **whole
file**, not just that block. Check spelling there first.

## `httpIdleTimeoutMs` errors on startup

An invalid value for `httpIdleTimeoutMs` or `websocketConnectTimeoutMs` is an error, not a fallback
— cyrup will not quietly substitute the default. Valid values are a number, a numeric string, or
the string `"disabled"` for `httpIdleTimeoutMs`:

```json
{ "httpIdleTimeoutMs": 600000 }
```

Delete the key to get the default back.

## `codemode` is missing, or a script cannot find a tool

`codemode` is registered but off by default, unless a permission policy is armed, which activates every
tool it does not deny. Turn it on with `"defaultTools": ["+codemode"]` in the global `settings.json`,
or for one run with `--tools read,bash,edit,write,codemode`. If it is still missing, one of these
holds: the line is in a project's `.cyrup/settings.json` and that project is not trusted (a `-p` run
needs `--approve`); `--tools` does not name it; `--no-extensions` is set, which removes the tool;
`--exclude-tools` matches it; the permission policy denies it. Startup says so when a `defaultTools`
name matches no tool: `defaultTools: no activatable tool is registered as "codemode"`. If `codemode`
is on when you did not turn it on, a permission policy file is the cause (see
[The permission system](../guides/tools-and-permissions.md#the-permission-system)): deny it there or
pass `--exclude-tools codemode`.

Inside a script, `tools.grep does not exist` means the tool is not active (`grep`, `find` and `ls`
are off in a default session, but on under a permission policy) or a permission rule, `--tools` or
`--exclude-tools` left it out; the error lists the tools that exist. The other symptoms — a timeout,
running out of memory, an approval that a `-p` run cannot give, temp files that pile up — are in
[Codemode](../guides/codemode.md#troubleshooting).

## Getting more detail

`CYRUP_TIMING=1` prints phase timings for startup, which tells you whether a slow launch is
discovery, the model catalog, or extension loading:

```sh
CYRUP_TIMING=1 cyrup
```

`/debug` (or `Ctrl+Shift+D`, which works even with a picker open) prints the terminal size, the
active theme and its generation, the thinking level, whether images are enabled, and the streaming
state.

cyrup's tracing output goes to stderr, at `warn` by default (`notify`, the directory-watch crate, at
`error`) and at `debug` under `--verbose`. `RUST_LOG` overrides both, so redirect stderr if you want
it on disk:

```sh
RUST_LOG=debug cyrup -p "hello" 2> cyrup.log
```

The one exception is the terminal interface when stderr is the terminal itself: the screen is the
UI's, so a `WARN` or `ERROR` line would land in the middle of it. From the moment the directories are
resolved, tracing goes to `<agent dir>/logs/cyrup.log` instead (mode `0600`, started over when it passes
8 MiB). If stderr is redirected, the terminal interface writes there as before.

When the permission system is on and `debug` is enabled — `/permission-system debug on` — every
decision is appended as JSONL to
`~/.cyrup/agent/cyrup-permission-system/logs/cyrup-permission-system-debug.jsonl`. That file is the
authoritative answer to "why was this tool call blocked".

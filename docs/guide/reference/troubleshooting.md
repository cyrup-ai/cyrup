# Troubleshooting

Symptoms you are likely to hit, and what to do about each. If your problem is not here, `/debug`
and the debug log at the end of this page are the fastest way to find out what cyrup thinks is
going on.

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

`CYRUP_HOME` and `CYRUP_CODING_AGENT_DIR` are read by the native extensions only, and they mean
different directories again. The three are compared side by side in
[Environment variables](environment.md).

## Subagent files are not discovered

The subagents home is `~/.cyrup/agents` — not `~/.cyrup/agent/agents`. The singular `agent`
directory is where `settings.json` lives; the plural `agents` directory is where agent definitions
live. Project agents go in `<repo>/.cyrup/agents`.

Two other reasons a file is skipped: it is missing a `name` or a `description` in its frontmatter,
or it sits under a path segment named `skills`. See [Subagents](../extensions/subagents.md).

## `cyrup update` does not update cyrup

Self-update is not implemented; `cyrup update` prints as much and updates nothing. Reinstall from
source to upgrade:

```sh
cargo install --git https://github.com/cyrup-ai/cyrup
```

`cyrup update <source>` and `cyrup update --extensions` do work — they update installed packages.

## `cyrup install npm:...` fails

npm sources are rejected outright. cyrup has no JavaScript runtime, so there is nothing to run an
npm package with. The `cyrup remove` help text still shows an `npm:` example; ignore it.

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

## Getting more detail

`CYRUP_TIMING=1` prints phase timings for startup, which tells you whether a slow launch is
discovery, the model catalog, or extension loading:

```sh
CYRUP_TIMING=1 cyrup
```

`/debug` (or `Ctrl+Shift+D`, which works even with a picker open) prints the terminal size, the
active theme and its generation, the thinking level, whether images are enabled, and the streaming
state.

The debug log is at `~/.cyrup/agent/debug.log`.

When the permission system is on and `debug` is enabled — `/permission-system debug on` — every
decision is appended as JSONL to
`~/.cyrup/agent/cyrup-permission-system/logs/cyrup-permission-system-debug.jsonl`. That file is the
authoritative answer to "why was this tool call blocked".

# Tools and permissions

cyrup does work by calling tools — reading files, running commands, editing code. This page covers
the built-in tools, how to narrow what the agent may reach for, and the two gates that stand in
front of a tool call: project trust and the permission system.

## The built-in tools

Seven tools ship in the binary.

| Tool | What it does |
|---|---|
| `read` | Read a file. Text and images (jpg, png, gif, webp, bmp); images arrive as attachments. |
| `write` | Write a file, creating parent directories and overwriting what is there. |
| `edit` | Replace exact text in one file. Each edit must match a unique, non-overlapping region. |
| `bash` | Run a shell command in the working directory and return stdout and stderr. |
| `grep` | Search file contents for a regex or literal. Respects `.gitignore`. |
| `find` | Find files by glob pattern. Respects `.gitignore`. |
| `ls` | List a directory, dotfiles included, directories suffixed with `/`. |

Every tool truncates its output — `read` and `bash` at 2000 lines or 50KB, `grep` at 100 matches,
`find` at 1000 results, `ls` at 500 entries, all capped at 50KB. When `bash` output is truncated the
full text is written to a temp file and the path is reported.

**Only four are active by default:** `read`, `bash`, `edit`, and `write`. `grep`, `find` and `ls` are
registered but off, so the model reaches for `bash` to search unless you turn them on with `--tools`.

## Narrowing the tool set

Four flags shape what is available, and all four apply to built-in, extension and custom tools
alike.

| Flag | Short | Effect |
|---|---|---|
| `--tools <names>` | `-t` | Allowlist. Only the named tools are active. |
| `--exclude-tools <names>` | `-xt` | Denylist. The named tools are removed. |
| `--no-tools` | `-nt` | Start with nothing active. |
| `--no-builtin-tools` | `-nbt` | Drop the default built-ins, keep extension and custom tools. |

Names are comma-separated and trimmed, so `--tools "read, grep"` works. An explicit `--tools`
allowlist wins over `--no-tools` and `--no-builtin-tools`.

The read-only review session:

```sh
cyrup --tools read,grep,find,ls -p "review src/ for error-handling bugs"
```

That set has no `write`, no `edit` and no `bash`, so the run cannot change anything on disk or shell
out — and it is the one case where naming `grep`, `find` and `ls` explicitly matters, since they are
not on by default.

## Project trust

Trust is the first gate, and it runs before the model is even asked anything. A project you have not
trusted contributes no configuration to the session.

### What triggers the prompt

cyrup asks about a folder when it finds anything a project could use to change cyrup's behaviour:

- `.cyrup/settings.json`
- `.cyrup/extensions`, `.cyrup/skills`, `.cyrup/prompts`, `.cyrup/themes`
- `.cyrup/SYSTEM.md` or `.cyrup/APPEND_SYSTEM.md`
- an `.agents/skills` directory in the repository or any ancestor of it

A repository with none of those is trusted implicitly — there is nothing to decide.

### What trust gates

Untrusted, cyrup still loads your global configuration, your global extensions, and anything you
passed on the command line with `-e`. What it will not load is project settings, project context
files, project extensions, and project packages. An untrusted `.cyrup/settings.json` is not read at
all, and writes to it are refused.

### The prompt

```text
Trust project folder?
/Users/you/work/repo

This allows cyrup to load .cyrup settings and resources, install missing project packages, and
execute project extensions.
```

You get up to five options:

- **Trust** — remembered for this folder.
- **Trust parent folder (`<parent>`)** — remembered one level up, so sibling checkouts inherit it.
  Offered only when there is a parent.
- **Trust (this session only)** — nothing is written to disk.
- **Do not trust** — remembered.
- **Do not trust (this session only)** — nothing is written to disk.

Decisions live in `~/.cyrup/agent/trust.json`, keyed by canonical absolute path. Lookup walks from
your working directory up to the root and takes the first explicit decision it finds, which is what
makes the parent-folder option cover everything beneath it.

### Overriding trust for one run

```sh
cyrup --approve -p "..."
```

`--approve` (`-a`) trusts the project for that run; `--no-approve` (`-na`) refuses it. Neither is
written to `trust.json`, and `--approve` wins if you somehow pass both.

To skip the question everywhere, set `defaultProjectTrust` in your global settings — `ask` (the
default), `always`, or `never`. It is a global-only key: cyrup strips it from project settings, so a
repository cannot vote itself trusted.

In non-interactive modes an undecided project is untrusted. `-p`, `--mode json` and `--mode rpc`
have nobody to ask, so a folder with no saved decision and no `--approve` gets the safe answer. This
is the usual reason a CI run silently ignores a repository's `.cyrup/` directory — see
[Scripting and automation](scripting.md).

## The permission system

The permission system is the second gate, and it is optional. It turns every tool call into an
allow / ask / deny decision driven by a policy file, and it can shape the tool set and sanitise the
system prompt on top of that.

**It arms itself if a policy file merely exists.** `CYRUP_PERMISSION_SYSTEM=1` turns it on, but so
does the presence of `~/.cyrup/agent/cyrup-permissions.jsonc` or
`.cyrup/agent/cyrup-permissions.jsonc` in the repository — dropping a policy file into a project is
enough. Removing the environment variable does not disarm it; to switch it off with a policy file
present, set `"enabled": false` in `~/.cyrup/agent/cyrup-permission-system/config.json`.

When a rule says `ask`, you get a dialog naming the tool and what it wants to do, with four choices:

- **Allow Once** — this call only.
- **Allow Always** — this call and matching ones for the rest of the session.
- **Reject** — refuse.
- **Reject with Reason** — refuse and type a sentence the model sees, which is how you redirect it
  rather than just blocking it.

`Esc`, dismissing the dialog, or letting it time out all count as a plain reject.

The policy syntax, the four layers, and how a project policy can only tighten a global one are
covered in [The permission system](../extensions/permissions.md).

## What the bash tool exports

Every command the `bash` tool runs starts with five variables describing the session that launched
it:

| Variable | Value |
|---|---|
| `CYRUP_SESSION_ID` | The session's id. |
| `CYRUP_SESSION_FILE` | Path to the session file; unset for an ephemeral session. |
| `CYRUP_PROVIDER` | The active provider id. |
| `CYRUP_MODEL` | The active model id. |
| `CYRUP_REASONING_LEVEL` | The active thinking level. |

They are resolved when each command starts, so switching model or thinking level takes effect on the
next command with no restart. Read them, do not set them — cyrup scrubs all five from the child
environment before writing its own values, and none of them is a configuration input.
`CYRUP_PROVIDER` and `CYRUP_MODEL` are set together and only when a model is selected.

Two settings control how commands are run:

```json
{
  "shellPath": "/opt/homebrew/bin/bash",
  "shellCommandPrefix": "source ~/.env.work &&"
}
```

`shellPath` picks the shell binary; a path that does not exist fails the call with
`Custom shell path not found: ...`. `shellCommandPrefix` is prepended to every command, which is how
you get a login environment, a `nix develop` wrapper, or a fixed `PATH` into every shell the agent
runs.

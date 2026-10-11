# Tools and permissions

cyrup does work by calling tools — reading files, running commands, editing code. This page covers
the built-in tools, how to narrow what the agent may reach for, and the two gates that stand in
front of a tool call: project trust and the permission system.

## The built-in tools

Eight tools ship in the binary.

| Tool | What it does |
|---|---|
| `read` | Read a file. Text and images (jpg, png, gif, webp, bmp); images arrive as attachments, re-encoded to fit the model's [resize profile](models.md#how-big-an-image-gets-sent). |
| `write` | Write a file, creating parent directories and overwriting what is there. |
| `edit` | Replace exact text in one file. One call carries a list of edits, each matching a unique, non-overlapping region of the original. |
| `bash` | Run a shell command in the working directory and return stdout and stderr. |
| `powershell` | Run a PowerShell command. Registered on every platform, off by default. |
| `grep` | Search file contents for a regex or literal. Respects `.gitignore`. |
| `find` | Find files by glob pattern. Respects `.gitignore`. |
| `ls` | List a directory, dotfiles included, directories suffixed with `/`. |

Every tool truncates its output — `read` and `bash` at 2000 lines or 50KB, `grep` at 100 matches,
`find` at 1000 results, `ls` at 500 entries, all capped at 50KB. When `bash` output is truncated the
full text is written to a temp file and the path is reported.

`read` takes a whole file into memory before it cuts a window out of it, so it refuses what it cannot
hold: a file over 512 MiB (`File size (<bytes>) is greater than the 512.0MB limit for read`; over
2 GiB the message is Node's `File size (<bytes>) is greater than 2 GiB`), and anything that is not a
regular file, such as a device (`/dev/zero`), a pipe or a socket. Use `bash` (`head`, `tail`,
`sed -n`, `grep`) to look at part of a file that large.

**Only four are active by default:** `read`, `bash`, `edit`, and `write`. `grep`, `find`, `ls` and
`powershell` are registered but off, so the model reaches for `bash` to search unless you turn them on with `--tools`.

Two more tools come from built-in extensions and start off as well. `codemode` lets the model write
one JavaScript script that calls the other tools, in parallel if it likes, and only the script's
output comes back to it. `tool_search` finds tools that are not declared to the model yet and
declares the matches. Turn `codemode` on with `"defaultTools": ["+codemode"]` in `settings.json`, or
for one run with `--tools read,bash,edit,write,codemode`; see [Codemode](codemode.md). None of this
holds once [the permission system](#the-permission-system) is armed: then the policy decides which
tools are active, and every tool it does not deny is, `grep`, `find`, `ls`, `powershell`, `codemode`
and `tool_search` included.

Two details that matter if you read session files or debug a provider rejection:

- Every result carries a `details.truncation` record into the session JSONL — total and emitted line
  and byte counts, the caps in force, and `truncatedBy`, which is `"lines"`, `"bytes"`, or `null`
  when nothing was cut. The field is always present, so a reader never has to infer truncation from
  a missing key.
- No built-in tool schema sets `additionalProperties`. That is deliberate: schemas go to the model
  verbatim and some providers enforce them (Gemini does), which would reject the legacy flat
  `{path, oldText, newText}` shape `edit` accepts and folds into a one-entry `edits` list.

## Narrowing the tool set

Four flags shape what is available, and all four apply to built-in, extension and custom tools
alike.

| Flag | Short | Effect |
|---|---|---|
| `--tools <names>` | `-t` | Allowlist. Only the named tools are active. |
| `--exclude-tools <names>` | `-xt` | Denylist. The named tools are removed. |
| `--no-tools` | `-nt` | Start with nothing active. |
| `--no-builtin-tools` | `-nbt` | Turn off every built-in tool; extension and custom tools stay. |

Names are comma-separated and trimmed, so `--tools "read, grep"` works. An explicit `--tools`
allowlist wins over `--no-tools` and `--no-builtin-tools`, and `--exclude-tools` is applied on top
of whatever survives, so a name in both lists is removed.

`--tools`, `--exclude-tools` and `--no-tools` bound what is registered, not only what starts active.
A built-in the flags do not allow cannot be switched back on later by an extension, by the
permission system or by `setActiveTools`, and a `codemode` script cannot call it: `tools.bash` does
not exist in a session started with `--tools read,grep,codemode`. (`--no-builtin-tools` only changes
the starting set, so the built-ins stay switchable, and an armed permission system switches them back
on.)

**Repeating a flag replaces it — it does not add to it.** `--tools read --tools bash` gives you
`bash` alone; write `--tools read,bash` for both. The same last-one-wins rule applies to
`--exclude-tools` and to `--models`.

`--no-builtin-tools` turns off every built-in tool: the four that are on by default and `grep`, `find`,
`ls` and `powershell`, which are registered but off. It leaves extension and custom tools alone, such
as the `mcp` gateway tool, and it does not switch on `codemode`, which starts off. A run with `-nbt`
therefore has no file or shell tool of its own; for no tools at all, use `--no-tools`. The built-ins
stay registered, so something that activates tools by name, an extension for one, can still turn one on;
the permission system does when a policy is armed.

The read-only review session:

```sh
cyrup --tools read,grep,find,ls -p "review src/ for error-handling bugs"
```

That set has no `write`, no `edit` and no `bash`, so the run cannot change anything on disk or shell
out — and it is the one case where naming `grep`, `find` and `ls` explicitly matters, since they are
not on by default.

## Project trust

Trust is the first gate, and it runs before the model is even asked anything. A project you have not
trusted contributes no configuration to the session, with one exception that only restricts: its
permission policy is still read, and can add denies and asks but no `allow` (see
[the four layers](../extensions/permissions.md#the-four-layers)).

### What triggers the prompt

cyrup asks about a folder when it finds anything a project could use to change cyrup's behaviour:

- `.cyrup/settings.json`
- `.cyrup/extensions`, `.cyrup/skills`, `.cyrup/prompts`, `.cyrup/themes`
- `.cyrup/SYSTEM.md` or `.cyrup/APPEND_SYSTEM.md`
- `.cyrup/agent/cyrup-permissions.jsonc` or a `.cyrup/agent/agents` directory, the project's
  [permission policy](../extensions/permissions.md#the-four-layers) (not counted in your home
  directory, where that path is your own global policy)
- an `.agents/skills` directory in the repository or any ancestor of it

A repository with none of those is trusted implicitly — there is nothing to decide.

### What trust gates

Untrusted, cyrup still loads your global configuration, your global extensions, and anything you
passed on the command line with `-e`. What it will not load is project settings, project context
files, project extensions, and project packages. An untrusted `.cyrup/settings.json` is not read at
all, and writes to it are refused. The project's permission policy is the exception: it is read
untrusted too, because ignoring a repository's `deny` would help no one, but only its `deny` and `ask`
rules count. Its `allow` rules, and a `defaultPolicy` of `allow`, are dropped until you trust the
project, so a repository cannot approve its own tool calls for you.

You are told. In the terminal interface an untrusted project prints a warning-coloured banner after
the initial replay:

```text
This project is not trusted. Project .cyrup resources and packages are ignored. Use /trust to save a
trust decision, then restart cyrup.
```

It prints again after any `/resume`, `/fork` or `/import` that swaps sessions. A swap can bring a
different working directory and trust decision with it, so the question is re-asked every time —
including when the session you land on is in the same untrusted folder.

Saving a decision only records it: `/trust` answers
`project trust → trusted (/reload to apply to this session)`, so run `/reload` (or restart) before
the project's resources are actually loaded.

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

**It arms itself if a policy artefact merely exists.** `CYRUP_PERMISSION_SYSTEM=1` turns it on, and
so does any of these, in the agent directory or in `.cyrup/agent/` in the repository:

- a `cyrup-permissions.jsonc` policy file — dropping one into a project is enough;
- a non-empty `agents/` directory, since an agent file's `permission:` front matter is an enforced
  policy layer;
- a `cyrup-permission-system/config.json` whose contents differ from the template cyrup writes for
  itself (an untouched template does not count, so the system cannot latch itself on).

A project's policy file arms the system whether or not the project is trusted: an untrusted
project's policy is read and applied, but it can only tighten (see [Project trust](#project-trust)).

Removing the environment variable does not disarm it; to switch it off with a policy file present,
set `"enabled": false` in `~/.cyrup/agent/cyrup-permission-system/config.json`.

**An armed permission system also decides which tools are active.** At the start of each prompt it
sets the active tools to every registered tool its policy does not `deny`, whatever `defaultTools` or
`--no-builtin-tools` say. Measured with only a `cyrup-permissions.jsonc` present (`"tools"` set to `allow`
or to `ask` in its `defaultPolicy`) and no `defaultTools`: the first request declared `read`, `bash`, `edit`, `write`,
`grep`, `find`, `ls`, `powershell`, `mcp`, `ask_user_question`, `codemode` and `tool_search`, and the
system prompt of that request lists the same tools and carries the rules for them. That
includes a policy file in `.cyrup/agent/` of an untrusted project. `--tools`, `--no-tools` and
`--exclude-tools` still bound the set, and so does a `deny` rule: `"tools": { "codemode": "deny" }`
keeps `codemode` out. `--no-extensions` skips the permission system along with the other built-in
extensions. Check what a policy leaves active before relying on a smaller default tool set.

When a rule says `ask`, you get a dialog naming the tool and what it wants to do, with four choices:

- **Allow Once** — this call only.
- **Allow Always** — this call and matching ones for the rest of the session.
- **Reject** — refuse.
- **Reject with Reason** — refuse and type a sentence the model sees, which is how you redirect it
  rather than just blocking it.
- **Reject All From This Script** — offered only for a call a codemode script made: refuse it and
  every other call that script makes, without a dialog for each.

`Esc`, dismissing the dialog, or letting it time out all count as a plain reject.

Where nobody can answer, an `ask` is a block: `-p` and `--mode json` have no UI, so the call fails
with `requires approval, but no interactive UI is available`. `--mode rpc` does have one, the client:
the dialog reaches it as an `extension_ui_request` and its `extension_ui_response` decides.

A [codemode](codemode.md#permissions) script's calls are gated one by one, like any other: you are
asked for the script and again for each call that resolves to `ask`, and the prompt of such a call
ends with `(from codemode script)`.

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

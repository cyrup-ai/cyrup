# The permission system

The permission system is an allow / ask / deny gate in front of every tool call, driven by a layered
policy file. This page covers how it arms itself, how a policy is written and resolved, and what you
see when a call is held for approval.

**A policy file is enough to arm it.** The permission system is off by default, but it turns itself
on if it finds a `cyrup-permissions.jsonc` anywhere it looks — no environment variable needed. That
means dropping a policy file into a repository turns the gate on for anyone who runs cyrup there.
That is the feature working as designed; it is also the behaviour that surprises people, so know it
before you commit a policy file. It changes the tool set too: with the system armed, every tool the
policy does not `deny` is active, `grep`, `find`, `ls`, `powershell`, `codemode` and `tool_search`
included, whatever `defaultTools` says. See
[Tools and permissions](../guides/tools-and-permissions.md#the-permission-system).

## How it arms

Any one of these turns it on:

- `CYRUP_PERMISSION_SYSTEM=1` in the environment;
- `~/.cyrup/agent/cyrup-permissions.jsonc` or `<project>/.cyrup/agent/cyrup-permissions.jsonc`
  exists;
- `~/.cyrup/agent/agents/` or `<project>/.cyrup/agent/agents/` is non-empty, because agent files
  carry `permission:` blocks that are an enforced policy layer;
- the extension's own `config.json` exists and has been edited away from the template cyrup
  generates.

Note the project path: `.cyrup/**agent**/cyrup-permissions.jsonc`. That extra `agent/` segment is
specific to this extension — most project config lives directly under `.cyrup/`, and a policy file
placed there is not found.

## The four layers

Policy is resolved across four layers, evaluated in this order:

| Layer | Where | Trusted |
|---|---|---|
| Global | `~/.cyrup/agent/cyrup-permissions.jsonc` | yes |
| Project | `<project>/.cyrup/agent/cyrup-permissions.jsonc` | no |
| Agent | `permission:` frontmatter in `~/.cyrup/agent/agents/<name>.md` | yes |
| Project agent | `permission:` frontmatter in `<project>/.cyrup/agent/agents/<name>.md` | no |

Within that order, **the last match wins**: a rule in a later layer overrides an earlier one for the
same key.

That rule has one exception, and it is the important one. **An untrusted layer can tighten but never
relax a trusted `deny`.** A repository you have not vetted can add its own denies and asks, and it
can turn your `allow` into an `ask`. It cannot turn your `deny` into an `allow`. Everything under
`~/.cyrup/agent` is trusted because you wrote it; everything under a project's `.cyrup/` is not,
because someone else may have.

**A project you have not trusted can only tighten, whatever the file says.** The two project layers are
read in an untrusted project too, and cyrup drops every `allow` from them there, a rule or a
`defaultPolicy` of `allow`, so the layers below decide that call and an `ask` of yours stays an `ask`.
Their `deny` and `ask` rules stand. A project is untrusted when you chose "Do not trust", or when
the run could not ask and you passed no `--approve` (`-p`, `--mode json`; see
[Project trust](../guides/tools-and-permissions.md#project-trust)). A project policy file is one of
the things that start the trust question, so a repository that ships only that file is asked about too.
In a trusted project the layers apply in full, under the `deny` floor above. Upstream
`pi-permission-system` has no such rule: its project layer can turn an `ask` into an `allow` in any
project.

The two "agent" layers apply only when a call runs under a named agent persona — see
[Subagents](subagents.md).

**Those two layers read a different directory from the one subagents reads.** The permission system
looks for `<name>.md` under `~/.cyrup/agent/agents/` and `<project>/.cyrup/agent/agents/` — the
singular `agent` directory. Subagents discovers its personas under `~/.cyrup/agents`, `~/.agents`,
`<project>/.agents` and `<project>/.cyrup/agents` — no `agent` segment. The two sets do not overlap,
so a `permission:` block in the persona file subagents actually runs is **not** picked up as a policy
layer; to have it enforced you need a file of the same name under the permission system's `agents/`
directory as well. The arming rule above reads the same permission-system directory, so a populated
subagents home does not arm the extension either.

## Writing a policy

Start from the shipped example. With the extension armed, `/permission-system example` prints it,
and it is a complete policy:

```jsonc
{
  "defaultPolicy": {
    "tools": "ask",
    "bash": "ask",
    "mcp": "ask",
    "skills": "ask",
    "special": "ask"
  },
  "tools": {
    "read": "allow",
    "read:/home/alice/project/generated/*": "allow",
    "write": "deny"
  },
  "bash": {
    "git status": "allow",
    "git *": "ask"
  },
  "mcp": {
    "mcp_status": "allow"
  },
  "skills": {
    "*": "ask"
  },
  "special": {
    "doom_loop": "deny",
    "external_directory": "ask",
    "external_directory:/home/alice/shared/*": "allow"
  }
}
```

Write it to `~/.cyrup/agent/cyrup-permissions.jsonc` and it takes effect on your next session. The
file is JSONC, so comments are allowed. Add a `"$schema"` key pointing at the schema
`/permission-system schema` prints if you want completion in your editor.

The three states are exactly `allow`, `deny` and `ask`. There is no fourth.

### defaultPolicy

Required. It is what applies when nothing else matches, and it must set `tools`, `bash`, `mcp` and
`skills`. `special` is optional here.

Starting every category at `ask` and carving out allows is the sane way to begin — you find out what
your workflow actually calls before you decide what to permit.

### tools

Keyed by tool name: `read`, `write`, `edit`, `bash`, `powershell`, `grep`, `find`, `ls`, and any tool
an extension registers (`codemode` and `tool_search` included). `*` wildcards are allowed, so
`"*": "ask"` covers everything you did not name.

Tools that take a path also accept **resource-qualified** keys, which apply only to matching paths:

```jsonc
{
  "tools": {
    "read": "allow",
    "read:/home/alice/project/generated/*": "allow"
  }
}
```

A qualified key applies only to calls whose path matches it; the bare tool name covers the rest.

### bash

Keyed by command pattern, with `*` as the wildcard:

```jsonc
{
  "bash": {
    "git status": "allow",
    "git log *": "allow",
    "git push *": "deny",
    "git *": "ask"
  }
}
```

`bash` is the tool the model uses to run anything the other tools cannot, so this block is usually
the one worth the most care.

### mcp and skills

`mcp` is keyed by the target names invoked through a registered `mcp` tool. `skills` is keyed by
skill name, with `*` supported. Both behave the same way as `tools`.

### special

A closed set — the only keys accepted are:

| Key | Meaning |
|---|---|
| `doom_loop` | The agent repeating the same failing action |
| `external_directory` | Touching a path outside the project |

`external_directory` also accepts resource-qualified keys, so
`"external_directory:/home/alice/shared/*": "allow"` permits one directory outside the project while
the bare key stays at `ask`. Any other key in this block is rejected.

One path outside the project needs no such rule: the `codemode` script reference that cyrup writes to
`<agent dir>/docs/codemode.md` and that the `codemode` tool description sends the model to. A `read`
of exactly that file skips `external_directory`; the `read` rule still applies, and so does
`external_directory` for every other path, for a `write` of that file and for a path spelled with `..`
into the rest of the agent directory.

The same exemption covers the files cyrup itself wrote during the session for the model to read back:
the temp file that a truncated `bash` result names (`Full output: /tmp/cyrup-bash-<id>.log`), and the
`pi-codemode-<id>.txt` or image file a `codemode` result names (`Full output: ...`, `Image saved to
...`). Only those exact paths qualify, they are remembered for the life of the process, and only for
a `read`: another file in the same directory, a `write` of the file, or a path that has since become
a symbolic link meets `external_directory` as usual.

## The dialog

When a rule resolves to `ask`, the call stops and you get four options:

| Option | Effect |
|---|---|
| Allow Once | Permit this call |
| Allow Always | Permit it and remember the approval for the rest of the session |
| Reject | Refuse it |
| Reject with Reason | Refuse it and type an explanation the model receives |

The dialog of a call a [`codemode`](../guides/codemode.md) script made has a fifth option,
**Reject All From This Script**: it refuses that call and every other call the same script makes, the
ones waiting behind the dialog included, without another dialog. Each fails with `the user rejected
this script's tool calls`; the script is not aborted, and other scripts are asked as before.

Pressing `Esc` counts as a reject, and so does a timeout. **The gate fails closed on anything but an
explicit allow.** A headless run — a script, a piped invocation (`-p`, `--mode json`), anything with no
interactive UI — has nobody to ask, so an `ask` becomes a block. Policies you intend to use in
[scripts](../guides/scripting.md) should resolve to `allow` or `deny`, never `ask`. `--mode rpc` is the
exception: the dialog goes to the client as an `extension_ui_request` and the client's
`extension_ui_response` decides.

When the call comes from a subagent child, which has no human of its own, the child's `ask` is
forwarded up to the session that started it through a filesystem spool, and you answer it there. That
needs a session with a UI. A session without one (`-p`, `--mode json`) leaves a `no-ui` marker in its
spool, and a child whose `ask` fires reads it and refuses the call at once, with the same text a
headless session gives (`... requires approval, but no interactive UI is available`), instead of
waiting. The marker names the process that wrote it, so it holds only while that process is running:
one left by a run that was killed, crashed or finished is ignored, and the next headless run removes
it. A session id outlives its process, so `cyrup -p ...` followed by `cyrup -c` with a UI is one
session in two processes: the process with the UI removes the marker on its first turn and its
children's questions are put to you. A child whose parent does have a UI, and is not answered, gives
up after 10 minutes and the call is denied.

### Calls made by a `codemode` script

A call a [`codemode`](../guides/codemode.md) script makes with `tools.*` is gated like a call the model
makes: same rules, same dialog, same fail-closed rule. Two things differ. The `codemode` call has its
own rule, so with `ask` you answer once for the script and again for each nested call that resolves to
`ask`. And the dialog text, the headless block text and the audit entry say the call came from a
script: the prompt ends with `(from codemode script)`, and the entry has a `parentToolCallId`. In a
headless run (`-p`, `--mode json`) the script receives the block as an error that says so and names
the fix; in `--mode rpc` the client is asked.

## The /permission-system command

Four forms:

```sh
/permission-system
```

Opens a live settings overlay with two rows, `debug` and `yoloMode`, toggled in place.

```sh
/permission-system debug on
/permission-system yoloMode off
```

Sets one setting directly.

```sh
/permission-system schema
/permission-system example
```

Prints the JSON Schema for a policy file, and the starter policy above, respectively.

The command needs an interactive interface. Without one it warns and does nothing.

### debug

`debug` arms a JSONL audit trail at

```text
~/.cyrup/agent/cyrup-permission-system/logs/cyrup-permission-system-debug.jsonl
```

One line per decision, which is how you work out why a call was blocked when you expected it to pass.
Turn it off when you are done — it records every tool call.

### yoloMode

`yoloMode` auto-approves. It is there for a run where the gate is in your way and you have decided
to accept the consequences for that session. While it is on, a `yolo` pill sits in the status bar so
the state is never invisible.

## The extension config file

`~/.cyrup/agent/cyrup-permission-system/config.json`:

```json
{
  "enabled": true,
  "debug": false,
  "yoloMode": false,
  "forwardedPromptTimeoutSeconds": 30
}
```

That is the file cyrup writes for you the first time it needs one, with every value at its default.

| Key | Type | Default | Meaning |
|---|---|---|---|
| `enabled` | bool | `true` | `false` disables the extension entirely; nothing else disables it |
| `debug` | bool | `false` | The JSONL audit trail |
| `yoloMode` | bool | `false` | Auto-approve everything |
| `forwardedPromptTimeoutSeconds` | number | `30` | How long the dialog of a forwarded ask stays open in your session before it is rejected for you. The child itself waits at most 10 minutes for any answer |

Point `CYRUP_PERMISSION_SYSTEM_CONFIG_PATH` at a different file to relocate it.

**Removing the environment variable does not turn the gate off.** If a policy file exists, that
alone keeps arming it. To disable the permission system while keeping your policy on disk, set:

```json
{ "enabled": false }
```

Only the literal `false` disables it. Deleting your policy files works too, and so does
`cyrup --no-extensions` for one run.

## When a policy is malformed

A policy file cyrup cannot parse does not fail open. The gate falls back to asking about everything
and surfaces a single warning — deduplicated, so a broken file does not flood your session with the
same message on every tool call. If you suddenly get asked about `read`, check your policy file for
a syntax error.

## Other environment variables

| Variable | Meaning |
|---|---|
| `CYRUP_PERMISSION_SYSTEM` | Arm the extension |
| `CYRUP_PERMISSION_SYSTEM_POLICY_AGENT_DIR` | Relocate the global policy root; project paths are unaffected |
| `CYRUP_PERMISSION_SYSTEM_CONFIG_PATH` | Relocate the extension's `config.json` |
| `CYRUP_PERMISSION_SYSTEM_LOGS_DIR` | Where the audit trail is written |
| `CYRUP_PERMISSION_SYSTEM_FORWARDING_AGENT_DIR` | Root of the child-to-parent forwarding spool |
| `CYRUP_PERMISSION_FORWARDING_TIMEOUT_MS` | Shorten a child's forwarded-ask wait; default ten minutes |

For the simpler allowlist that works without any of this, see
[Tools and permissions](../guides/tools-and-permissions.md).

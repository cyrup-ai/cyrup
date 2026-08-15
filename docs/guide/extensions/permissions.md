# The permission system

The permission system is an allow / ask / deny gate in front of every tool call, driven by a layered
policy file. This page covers how it arms itself, how a policy is written and resolved, and what you
see when a call is held for approval.

**A policy file is enough to arm it.** The permission system is off by default, but it turns itself
on if it finds a `cyrup-permissions.jsonc` anywhere it looks — no environment variable needed. That
means dropping a policy file into a repository turns the gate on for anyone who runs cyrup there.
That is the feature working as designed; it is also the behaviour that surprises people, so know it
before you commit a policy file.

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

The two "agent" layers apply only when a call runs under a named agent persona — see
[Subagents](subagents.md).

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

Keyed by tool name: `read`, `write`, `edit`, `bash`, `grep`, `find`, `ls`, and any tool an extension
registers. `*` wildcards are allowed, so `"*": "ask"` covers everything you did not name.

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

## The dialog

When a rule resolves to `ask`, the call stops and you get four options:

| Option | Effect |
|---|---|
| Allow Once | Permit this call |
| Allow Always | Permit it and remember the approval for the rest of the session |
| Reject | Refuse it |
| Reject with Reason | Refuse it and type an explanation the model receives |

Pressing `Esc` counts as a reject, and so does a timeout. **The gate fails closed on anything but an
explicit allow.** A headless run — a script, a piped invocation, anything with no interactive UI —
has nobody to ask, so an `ask` becomes a block. Policies you intend to use in
[scripts](../guides/scripting.md) should resolve to `allow` or `deny`, never `ask`.

When the call comes from a subagent child, which has no human of its own, the child's `ask` is
forwarded up to your session through a filesystem spool and you answer it in the parent.

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
| `forwardedPromptTimeoutSeconds` | number | `30` | How long a child waits for you to answer a forwarded ask |

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

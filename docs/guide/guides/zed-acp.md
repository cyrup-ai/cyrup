# Zed and other ACP editors

cyrup speaks the [Agent Client Protocol](https://agentclientprotocol.com) over stdio, so an editor
that supports ACP can drive it the way it drives any other agent. Zed is the reference client. This
page covers the flag, the Zed configuration, and what works today.

## The flag

```sh
cyrup --acp
```

`--acp` is shorthand for `--mode acp`. cyrup then reads JSON-RPC 2.0 requests on stdin and writes
responses and notifications on stdout.

You do not normally run this yourself. The editor launches cyrup as a child process and owns both
pipes. Running `cyrup --acp` in a terminal leaves it waiting for a JSON-RPC frame that never
arrives.

ACP mode is checked before every other mode, so an editor launching cyrup with pipes on both ends
never falls through to print mode.

## Configuring Zed

Open `settings.json` with **zed: open settings** from the command palette, and add an entry under
`agent_servers`:

```json
{
  "agent_servers": {
    "cyrup": {
      "command": "/usr/local/bin/cyrup",
      "args": ["--acp"],
      "env": {}
    }
  }
}
```

Use the absolute path to the binary. `which cyrup` will tell you where it is. A bare `cyrup` works
only if Zed's environment has it on `PATH`, which is not always the shell `PATH` you see in a
terminal.

Open the agent panel, choose **External Agent**, and pick `cyrup`. Zed spawns the process and
bridges ACP over stdio.

To run cyrup against a specific directory regardless of which project is open, add `cwd`:

```json
{
  "agent_servers": {
    "cyrup": {
      "command": "/usr/local/bin/cyrup",
      "args": ["--acp"],
      "env": {},
      "cwd": "/path/to/project"
    }
  }
}
```

Without `cwd`, the editor passes the project directory when it creates a session.

## Credentials

Creating a session does not need credentials. Sending a prompt does. If no provider is configured,
the prompt fails with an `auth_required` error rather than a crash, and the client offers one
method, *Launch cyrup in the terminal*, which runs:

```sh
cyrup --terminal-login
```

That command requires a real terminal — both stdin and stdout must be a TTY. When the editor cannot
give it one, cyrup prints:

```
cyrup: --terminal-login needs an interactive terminal (stdin and stdout must both be a TTY).
Run `cyrup` yourself in a terminal and use /login to configure credentials.
```

Do that once and the credentials are shared with the ACP host. See
[Connect a provider](../getting-started/authenticate.md).

## Thinking levels

Each session exposes six modes through ACP's mode selector, which Zed renders as a dropdown:

| Mode | |
|---|---|
| `off` | No thinking budget |
| `minimal` | |
| `low` | |
| `medium` | The default |
| `high` | |
| `max` | |

Changing the mode in the editor applies to the next prompt. More in
[Models and thinking](models.md).

## Slash commands

cyrup advertises its commands to the client when the session opens, so they appear in the editor's
command palette. The built-in set is `compact`, `autocompact`, `export`, `session`, `name`,
`steering`, `follow-up`, `mcp` and `mcp-auth`. Installed skills and prompt templates are advertised
alongside them — a project with Flux installed also lists `flux/status`, `flux/commit` and the rest.

## What you get

Everything the terminal interface gives you, inside the editor:

- **The full tool set** — `read`, `write`, `edit`, `bash`, `grep`, `find`, `ls` — with each call
  shown as it runs, and file edits rendered as diffs in the editor's own review UI.
- **Permission prompts** raised through `session/request_permission`, so you approve a command in
  the editor rather than in a terminal you cannot see.
- **The same session tree.** Sessions created here are the JSONL sessions the TUI reads. Start in
  Zed, pick it up later with `cyrup --continue`.
- **Session management** — `session/list`, `session/load` and `session/delete` — so the editor can
  show your history and resume any of it.
- **Terminal output streamed live** as commands run, not just at the end.
- **Cancellation** that actually stops the run, wired to the editor's stop button.

## Troubleshooting

**Zed reports the agent exited immediately.** Check the `command` path is absolute and executable.
Running `/usr/local/bin/cyrup --version` in a terminal confirms both.

**The agent connects but every prompt fails.** No provider is configured. Run `cyrup` in a terminal
and use `/login`.

**Commands are missing from the palette.** They are sent once, just after the session opens. If the
editor was already showing a stale session, start a new thread.

**Nothing happens and no error appears.** Run the same command by hand to see the startup output:

```sh
echo '{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}}' | cyrup --acp
```

A JSON response means the binary and its configuration are fine, and the problem is in the editor's
launch environment.

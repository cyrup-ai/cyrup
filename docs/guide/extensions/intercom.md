# Intercom

Intercom lets concurrent cyrup sessions and subagent children find each other and exchange messages,
asks and replies. This page covers turning it on, what it puts on disk, and how you use it.

Intercom is a [native extension](overview.md) and is off by default. **It is Unix only in this
milestone** — macOS and Linux. There is no Windows transport.

## What it is

A broker process listening on a Unix domain socket. Every cyrup session that has intercom enabled
registers with the broker, which gives each session an address other sessions can reach. From there
a session can list who else is running, send a message, ask a question and wait for the answer, or
reply to something it received.

The two situations it exists for: several cyrup windows open on the same machine, working on
different parts of the same problem; and a [subagent](subagents.md) child that needs to reach the
session that spawned it.

cyrup starts the broker for you by re-executing its own binary. There is nothing separate to run.

## Turning it on

```sh
CYRUP_INTERCOM=1 cyrup
```

Or create `~/.cyrup/agent/intercom/config.json` and intercom arms itself without the variable. An
empty `{}` is enough.

A subagent child carrying orchestrator metadata attaches regardless, because that is how it reaches
its parent.

## What it puts on disk

Everything lives in `~/.cyrup/agent/intercom/`:

| File | Purpose |
|---|---|
| `broker.sock` | The Unix socket sessions connect to |
| `broker.pid` | The running broker's process id |
| `broker.spawn.lock` | Prevents two sessions racing to start a broker |
| `config.json` | Your configuration; its presence also arms the extension |

The directory is created mode `0700` and the runtime files mode `0600` — owner only. That is the
access control: anyone who can read the socket can talk to your sessions.

## Configuration

`~/.cyrup/agent/intercom/config.json`:

```json
{
  "stableId": "backend",
  "inboundTrigger": "replies",
  "confirmSend": true
}
```

| Key | Type | Default | Meaning |
|---|---|---|---|
| `enabled` | bool | `true` | `false` declines to attach |
| `stableId` | string | *unset* | A restart-stable address for this session |
| `inboundTrigger` | `"always"`, `"replies"`, `"never"` | `"always"` | Whether an inbound message may start a turn on its own |
| `confirmSend` | bool | `false` | Confirm before sending (also applies to handovers) |
| `busyDelivery` | `"steer"`, `"human-first"` | `"steer"` | A peer arriving during a run steers into it, or waits for a turn boundary with no pending human input |
| `replyHint` | bool | `true` | Include a reply hint with delivered messages |
| `status` | string | *unset* | Custom suffix on the status display |
| `brokerCommand` | string | `"npx"` | Command that launches the broker; the default is a sentinel, see below |
| `brokerArgs` | string[] | `["--no-install","tsx"]` | Arguments for it; the default is a sentinel |
| `crossMachine.machineName` | string | lowercased short host name | The name peers use for this machine in their Herdr saved-machine lists |
| `crossMachine.remoteCommand` | string | `"cyrup intercom"` | The command run over non-interactive SSH on the remote machine to relay a message |

`brokerCommand` and `brokerArgs` still carry pi's Node-flavoured defaults, and while **both** are
left at exactly those values cyrup ignores them and starts the broker by re-executing its own binary
with the `__intercom-broker` subcommand — there is no `npx` or `tsx` involved. Change either one away
from that default and cyrup takes you at your word: it runs `brokerCommand` with `brokerArgs`
followed by `__intercom-broker`. So they are a live setting, not a compatibility stub; the pair only
looks inert because its default is the "unconfigured" sentinel.

`CYRUP_INTERCOM_BROKER_BINARY` wins over both. When it is set, that binary is run with
`__intercom-broker` and nothing else — `brokerArgs` is dropped.

The broker always runs with its working directory set to the intercom runtime directory
(`<agent dir>/intercom`), never the directory of the session that happened to start it, so a
long-lived broker does not hold your project directory open (on Windows a process's working
directory cannot be renamed or deleted). A custom `brokerCommand` can be a name on `PATH` or an
absolute path; a relative file path such as `./bin/broker` — in the command or in `brokerArgs` —
resolves from that runtime directory, not from where you launched cyrup.

`stableId` is worth setting if you keep the same session role open across restarts. Without it a
session's address changes every time you start cyrup, so anything holding a reference to it has to
look you up again. With `"stableId": "backend"`, other sessions can address you as `backend`
tomorrow as well as today. A `stableId` set to whitespace is a hard error rather than an omission.

`inboundTrigger` is the one to reach for if intercom is interrupting you: `"replies"` limits
auto-starting a turn to answers you asked for, and `"never"` means an inbound message waits for you.

**A config file cyrup cannot parse is a hard failure.** Unlike most configuration in cyrup, intercom
does not fall back to defaults on a malformed file. It reports
`Failed to load intercom config at <path>: <reason>` and stops, because a silently defaulted
intercom is indistinguishable from one that is connected but never triggers.

## Using it

`/intercom` opens the session list: the sessions currently registered with the broker, with their
directory, model and Herdr location. **Alt+M** opens the same list. Pick a session with Enter to
compose a message to it, or press **h** to hand your session over to it (see below). Without an
interactive terminal (print or JSON mode) `/intercom` prints the list instead, and
`/intercom <session> <message>` sends a message directly.

`/intercom-id` inserts a handoff snippet into the editor — your session's address, in a form you can
paste into another session or into a task description so something else can reach you.

`/alias <name>` names the current session. The name is what other sessions see in their lists,
in send and reply results and in the header of every message you send, so give each session one
others can target. `/alias` alone asks for the name.

`/handover` summarizes this session and hands it over to another one — see
[Handing over a session](#handing-over-a-session).

The model has an `intercom` tool with these actions:

| Action | Effect |
|---|---|
| `list` | Registered sessions |
| `list-cwd` | Registered sessions, filtered by working directory |
| `send` | Send a message |
| `ask` | Send a question and wait for the reply |
| `handover` | Summarize this session with the current model and send the summary; the receiver acts on it |
| `reply` | Answer something received |
| `pending` | Asks awaiting an answer |
| `status` | This session's intercom state |
| `cancel` | Withdraw a pending ask |

Its parameters are `cwd`, `to`, `message`, `attachments`, `replyTo`, `messageId`, `supersedes`,
`retryOf`, `openProjectPaneIfMissing` and `focus`. `openProjectPaneIfMissing: true` with a `cwd`
opens a visible Herdr project pane and starts cyrup there when no session is running in that
directory; `focus` (default `true`) decides whether that new pane takes focus. The command started
in the pane is `CYRUP_INTERCOM_CYRUP_BIN`, else `CYRUP_BIN`, else the running cyrup binary. Herdr
types it into the pane's shell: a plain name or path goes in as-is, and on Linux and macOS one
containing spaces or other shell characters is single-quoted so the shell reads it as one word. On
Windows it is never quoted, so there it should be a name on `PATH` or a path without spaces.
For `handover`,
`message` is the optional next task, and `replyTo`, `supersedes`, `retryOf` and `attachments` are
refused.

A subagent child also gets `contact_supervisor` when it has orchestrator metadata and no native
supervisor channel is available — a direct line back to the session that spawned it.

Inbound messages render as their own entries in the transcript rather than as ordinary output, so
you can tell what came from another session.

### Keyboard shortcuts

| Key | Where | Action |
|---|---|---|
| Alt+M | anywhere | Open the session list |
| ↑ / ↓ | session list, handover picker | Move the selection |
| Enter | session list | Compose a message to the highlighted session |
| Enter | compose box | Send |
| Enter | handover picker | Hand over to the highlighted target |
| h | session list | Hand over to the highlighted session |
| Tab | handover picker | Move between the list and the next-task field |
| Escape | any of them | Close without doing anything |

## Handing over a session

When you move work from one session to another — say from a session in one repository to one
already running in another — a handover carries what the first session learned so the second one
does not have to rediscover it.

```
/handover
/handover mcp-worker port the schema fix to the adapter
/handover ~/dev/adapter port the schema fix to the adapter
```

`/handover` on its own opens a picker. It lists the other sessions on this machine, most recently
active first, with their directory, model, status and context use; context at 80% or more is
highlighted. Your own session and subagent child sessions are hidden. "+ New session in a project
path…" asks for a directory and opens a Herdr project pane there. "Fetch sessions from other
machines" lists the cyrup sessions on your enabled saved Herdr machines; it only runs when you choose
it, because each machine is reached over SSH and can take a few seconds. Each machine shows its
sessions or the reason it could not be reached. Type the optional next task in the field at the
bottom (Tab moves between the list and the field) and press Enter. Pressing **h** on a session in
the Alt+M list opens the same picker with that session selected and the next-task field focused.

With arguments, the first one is the target: a session name, ID, ID prefix, or `name@machine`. A
target starting with `/`, `./`, `../` or `~/`, or `~` on its own, is a project path; if no session is running there,
cyrup opens a Herdr project pane and starts a session in it. The rest of the line is the next task
and is optional.

Either way, cyrup generates the handover while showing "Generating handover..." (Escape cancels it
and stops the model call), opens the text in an editor for you to review and change, and sends it
when you save. Saving an empty text sends nothing. `/handover` needs the interactive terminal UI;
elsewhere, use the tool's `handover` action, which does the same without the review step:

```typescript
intercom({ action: "handover", to: "mcp-worker", message: "Port the schema fix to the adapter" })
intercom({ action: "handover", cwd: "/home/me/dev/adapter", openProjectPaneIfMissing: true })
```

What is sent: the current model reads this session's conversation — as it would be sent to the
model after compaction and context edits — and writes a summary with the next task, key decisions
and rejected approaches, relevant files and repositories, current state, and open questions. A short
header names the sender, its working directory, and its git branch and commit. For a target on the
same machine the header also gives the path of the sender's session file, so the receiver can read
the full transcript when it needs more; for a `name@machine` target that path is left out.

The receiver gets the handover as an ordinary intercom message asking it to act on the next task, so
its `inboundTrigger` and `busyDelivery` settings decide when it starts. The handover tells the
receiver to treat it as a peer's report and to check its claims against the repository.

Privacy: the summary is generated from your session transcript by your current model and provider,
like compaction. The model is told to leave out secrets, tokens, credentials and private keys, but
review the text when the session handled sensitive material. `confirmSend: true` applies to
handovers too, so with it on you confirm after editing.

## Other machines

A target of the form `name@machine` reaches a session on another machine: `machine` is the label of
an enabled saved machine in Herdr (`herdr machine list`), and `name` is a session name there or a
full session id. cyrup asks that machine's Herdr which cyrup sessions are running and relays the
message over SSH by running `crossMachine.remoteCommand` there. Only plain messages and handovers
cross machines — asks, replies, attachments and project panes do not.

The relay is the `cyrup` binary itself: the default `remoteCommand`, `cyrup intercom`, runs as
`ssh <target> 'cyrup intercom relay --envelope-stdin --json'`, so a machine needs nothing installed
beyond cyrup to receive relays, and a cyrup session must be running there for the message to land.
What it does need is `cyrup` on the `PATH` of a **non-interactive** SSH command, and that `PATH` is
often shorter than your login shell's — `~/.cargo/bin`, where `cargo install` puts cyrup, is usually
added by a login profile that `ssh host command` never reads. Check with `ssh <target> 'command -v
cyrup'`. If it prints nothing, set `remoteCommand` to the absolute path on the **sending** machine,
for example `"/home/me/.cargo/bin/cyrup intercom"`. When the remote shell cannot find the command
the send fails with `Remote command "…" was not found on "…"`; an error that says the remote "has no
compatible relay support and needs upgrading" means the command ran but is not a relay-capable
build, so update cyrup on that machine.

Cross-machine relay and handover work between cyrup sessions only. When either side lists a
machine's sessions it keeps just the Herdr agents of its own kind: cyrup keeps those of kind
`cyrup`, pi-intercom those of kind `pi`. So `name@machine` from cyrup never reaches a pi-intercom
session, and pi-intercom's own `name@machine` send never reaches a cyrup session — a message from pi
cannot arrive at cyrup over this path at all. That is also why the `send`-back hint on a relayed
message always uses cyrup's own `intercom` send: the message can only have come from another cyrup.

The receiving side cannot verify who sent a relayed message: the sender's name, session id and
machine are asserted by whoever could log in over SSH, and the message is shown as coming from an
**unverified** origin. Treat it accordingly.

## Scripting from the shell

`cyrup intercom` is a small client for the same broker, for scripts and for `ssh` to another
machine. It registers as a session of its own while it runs, so it appears in the roster.

```sh
cyrup intercom list [--json]
cyrup intercom send --to worker --text "build failed" [--name <session-name>] [--json]
cyrup intercom send --to reviewer@workstation --text "ship it" [--json]
cyrup intercom ask --to worker --text "status?" [--timeout-ms N] [--json]
```

It talks to the broker a running cyrup session on this machine owns and never starts one, so it
fails when no session is running. It exits 0 on success, 1 on a usage, connection or delivery
failure, and 2 when an `ask` gets no reply in time (120 s by default). The `cyrup-intercom` package
also builds the same client as a standalone `cyrup-intercom-cli` binary.

## Coordinating two sessions

A typical setup is two terminals on the same repository — one working on the API, one on the client.
The config file is shared by every session, so give each terminal its own address on the command
line instead:

```sh
CYRUP_INTERCOM_STABLE_ID=api cyrup
```

Then `/intercom-id` in one session gives you the snippet to paste into the other, and from that
point either side can `ask` the other a question and get an answer without you carrying it across
by hand. With `inboundTrigger` at `"replies"`, neither session starts a turn on an unsolicited
message — you stay in control of when each one acts.

## A naming overlap worth knowing

When intercom is **not** attached, the [subagents](subagents.md) extension registers a tool of its
own under the bare name `intercom`. So a session can have a tool called `intercom` without the
intercom extension running at all, and the two are not the same tool. If a tool named `intercom`
behaves unlike anything on this page, check whether intercom is actually on — `/intercom` does not
exist unless it is.

## Environment variables

| Variable | Meaning |
|---|---|
| `CYRUP_INTERCOM` | Turn the extension on (`1`, `true`, `on`, `yes`) |
| `CYRUP_INTERCOM_ASK_TIMEOUT_MS` | How long an `ask` waits for a reply; default `600000` |
| `CYRUP_INTERCOM_STABLE_ID` | Restart-stable address, same as the `stableId` key |
| `CYRUP_INTERCOM_NAME_POLL_MS` | Name-resolution poll interval |
| `CYRUP_INTERCOM_LIVENESS_INTERVAL_MS`, `_TIMEOUT_MS` | Broker liveness heartbeat and timeout |
| `CYRUP_INTERCOM_BROKER_BINARY` | Override the broker binary |
| `CYRUP_CODING_AGENT_DIR` | The directory the broker itself resolves its paths against |

An invalid `CYRUP_INTERCOM_ASK_TIMEOUT_MS` fails extension construction — it is not ignored and not
defaulted, so a value cyrup cannot parse stops intercom from starting. See
[Environment variables](../reference/environment.md) for the rest of cyrup's variables, including
why `CYRUP_CODING_AGENT_DIR` is not a synonym for `CYRUP_AGENT_DIR`.

## Turning it off

Unset `CYRUP_INTERCOM` and remove `~/.cyrup/agent/intercom/config.json`, or set `"enabled": false`
in it to keep the file. `cyrup --no-extensions` disables it for one run along with everything else.

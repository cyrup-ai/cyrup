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
| `confirmSend` | bool | `false` | Confirm before sending |
| `replyHint` | bool | `true` | Include a reply hint with delivered messages |
| `status` | string | *unset* | Custom suffix on the status display |
| `brokerCommand` | string | `"npx"` | Command that launches the broker; the default is a sentinel, see below |
| `brokerArgs` | string[] | `["--no-install","tsx"]` | Arguments for it; the default is a sentinel |

`brokerCommand` and `brokerArgs` still carry pi's Node-flavoured defaults, and while **both** are
left at exactly those values cyrup ignores them and starts the broker by re-executing its own binary
with the `__intercom-broker` subcommand — there is no `npx` or `tsx` involved. Change either one away
from that default and cyrup takes you at your word: it runs `brokerCommand` with `brokerArgs`
followed by `__intercom-broker`. So they are a live setting, not a compatibility stub; the pair only
looks inert because its default is the "unconfigured" sentinel.

`CYRUP_INTERCOM_BROKER_BINARY` wins over both. When it is set, that binary is run with
`__intercom-broker` and nothing else — `brokerArgs` is dropped.

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

`/intercom` opens the session picker: the sessions currently registered with the broker, and a
compose view for sending one a message.

`/intercom-id` inserts a handoff snippet into the editor — your session's address, in a form you can
paste into another session or into a task description so something else can reach you.

The model has an `intercom` tool with these actions:

| Action | Effect |
|---|---|
| `list` | Registered sessions |
| `list-cwd` | Registered sessions, filtered by working directory |
| `send` | Send a message |
| `ask` | Send a question and wait for the reply |
| `reply` | Answer something received |
| `pending` | Asks awaiting an answer |
| `status` | This session's intercom state |
| `cancel` | Withdraw a pending ask |

Its parameters are `cwd`, `to`, `message`, `attachments`, `replyTo`, `messageId`, `supersedes` and
`retryOf`.

A subagent child also gets `contact_supervisor` when it has orchestrator metadata and no native
supervisor channel is available — a direct line back to the session that spawned it.

Inbound messages render as their own entries in the transcript rather than as ordinary output, so
you can tell what came from another session.

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

# Sessions

Every conversation is a session on disk, written as it happens. This page covers where sessions
live, how to get back into one, how to branch off a conversation, and how to get the contents out.

## Where sessions live

Sessions are JSONL files under `~/.cyrup/agent/sessions`, nested one level in a directory named
after the working directory they were started in:

```text
~/.cyrup/agent/sessions/--Users-you-code-myrepo--/2026-08-14T09-31-02-114Z_5f3a68ae-3e2b-4c9d-9a71-0b4f8d2c61ee.jsonl
```

That per-project nesting is what "current project" means in the session picker. It also means
starting cyrup from a different directory in the same repository gives you a different set of
recent sessions.

Three things can move the root, highest precedence first:

1. `--session-dir <dir>` on the command line
2. the `CYRUP_SESSION_DIR` environment variable
3. the `sessionDir` setting in `settings.json`

An explicit session directory is used verbatim — cyrup does not add the per-project subdirectory
underneath it, so every project's sessions land in the same folder. The picker still separates them:
when the directory is one you chose, current-project scope filters on the directory each session
recorded rather than on where the file sits.

## Getting back into a session

```sh
cyrup -c "and now the tests"
```

`--continue` (`-c`) picks up the most recent session started in this directory. If there is none,
you get a new session rather than an error.

`--resume` (`-r`) opens a picker before the interface starts, so you choose which one.

When you already know which session you want:

| Flag | Argument | Behaviour |
|---|---|---|
| `--session` | path or id | Opens a specific session file. A partial UUID is enough. |
| `--session-id` | id | Uses that exact project session id, creating it if it does not exist. |
| `--fork` | path or id | Forks that session into a new one. |
| `--no-session` | — | Runs without saving anything. |
| `--name`, `-n` | name | Sets the session's display name. |

`--session` matches this project's sessions first, then every other project's. If the only match
belongs to a different directory, cyrup tells you where it came from and asks whether to fork it
into the current one.

`--session-id` is the one to use from a script that needs a stable, addressable conversation across
several invocations — see [Scripting and automation](scripting.md). It cannot be combined with
`--session`, `--continue`, or `--resume`. `--no-session` is for the opposite case: a throwaway
question you do not want in your history, with nothing written to disk at all.

Resuming restores the model and thinking level the session was using. If that model is no longer
available, cyrup says so and falls back to your default.

## The `/resume` picker

`/resume` inside a running session opens the same picker. It is dense, so it is worth learning.

Type anything to filter. Two prefixes change how the search works:

- `re:pattern` treats the rest as a regular expression.
- `"phrase"` matches the phrase exactly.

The keys, all shown in the two hint rows above the search box:

| Key | Effect |
|---|---|
| `Tab` | Toggle between this project's sessions and all projects |
| `Ctrl+S` | Cycle sort: threaded, recent, fuzzy |
| `Ctrl+N` | Show only named sessions |
| `Ctrl+P` | Show or hide the file-path column |
| `Ctrl+R` | Rename the highlighted session inline |
| `Ctrl+D` | Delete it, with an inline confirm |
| `Enter` | Open it |
| `Esc` | Cancel |

`Ctrl+D` replaces the hint row with `Delete session? enter confirm · escape/ctrl+c cancel`, so a
stray keystroke does not lose anything. Rename replaces the search box with an edit field; `Enter`
saves, `Esc` backs out.

`Ctrl+N` is more useful than it looks, but only if you name things. Give the sessions you expect to
return to a name — `/name refactor auth` — and named-only filtering turns a week of accumulated
conversations into a short, deliberate list.

## Branching

A session is a tree, not a line. Forking rewinds to an earlier point in the conversation and
continues from there in a new session, leaving the original untouched.

That matters because context is the expensive part. When the agent has spent twenty turns building
up an understanding of your codebase and then goes down a wrong path, you do not want to start
over — you want to go back three messages and try a different instruction with all that
understanding intact. That is what a fork is.

| Command | What it does |
|---|---|
| `/fork` | Pick an earlier user message, branch from it, and switch to the branch |
| `--fork <path\|id>` | Fork a session you name into a new one in the current project |
| `/clone` | Duplicate the session at its current position, without switching to the copy |
| `/tree` | Navigate the whole tree of branches |

The difference between `/fork` and `/clone` is where you end up. `/fork` moves you onto the new branch — it is
the "go back and try again" move, and if you fork from before a message, that message's text comes
back into your editor to re-edit. `/clone` leaves you exactly where you are and writes a copy
alongside, which is the "snapshot this before I do something risky" move.

Both leave the original session untouched, and both write a new session file in the same directory.

### The tree navigator

`/tree` shows every message in the session as a navigable tree.

| Key | Effect |
|---|---|
| `Alt+Left` | Fold the current node, or move up |
| `Alt+Right` | Unfold, or move down |
| `Ctrl+D` | Default filter |
| `Ctrl+T` | Hide tool calls |
| `Ctrl+U` | User messages only |
| `Ctrl+L` | Labelled messages only |
| `Ctrl+A` | Show everything |
| `Ctrl+O` | Cycle filters forward, `Ctrl+Shift+O` back |
| `Shift+L` | Edit the label on the current node |
| `Shift+T` | Toggle the label timestamp column |

Typing accumulates a search query; `Backspace` removes a character. `Esc` clears the query first
and only leaves the tree once the query is empty. On macOS, `Alt` is Option.

Labels are yours to write. `Shift+L` on the message where a piece of work actually started makes
that point findable later, which is the difference between a tree you navigate and a tree you
scroll.

The default filter is the `treeFilterMode` setting: `default`, `no-tools`, `user-only`,
`labeled-only`, or `all`.

Confirming a row offers to summarise the branch you are leaving, so its content carries forward
without carrying its full token cost.

## Double-Escape

On an empty prompt, pressing `Esc` twice within 500ms opens the session tree. The
`doubleEscapeAction` setting changes that: `tree` is the default, `fork` opens the fork picker
instead, and `none` disables it.

## Compaction

A conversation eventually fills the model's context window. Compaction replaces the older part of
the transcript with a summary, freeing room while keeping the thread of what happened.

It runs automatically. cyrup compacts before the next request when the conversation exceeds the
model's context window minus `compaction.reserveTokens` — by default, 16k tokens short of full. The
footer's context percentage tells you how close you are; see
[The terminal interface](tui.md).

You can also do it deliberately, which usually produces a better result because you get to say what
matters:

```text
/compact keep the database schema decisions and the failing test output
```

Those instructions are added to the summarisation prompt as additional focus — they steer the
summary rather than replacing it. Run `/compact` at a natural boundary, when the current piece of
work is finished, rather than mid-task.

Four settings control the behaviour:

| Key | Default | Meaning |
|---|---|---|
| `compaction.enabled` | `true` | Compact automatically |
| `compaction.reserveTokens` | `16384` | Tokens held back to do the compaction |
| `compaction.keepRecentTokens` | `20000` | Recent conversation kept verbatim |
| `branchSummary.reserveTokens` | `16384` | Tokens reserved for a `/tree` branch summary |

Turning `compaction.enabled` off means long conversations eventually hit the context limit instead
of compacting. Do that only if you would rather fork than summarise.

## Getting the conversation out

`/export` writes the session to a file:

```text
/export ~/notes/auth-refactor.html
```

A target ending in `.jsonl` writes the raw transcript, message for message. Any other target
produces styled HTML — readable, self-contained, suitable for sending to someone. With no target at
all, the HTML is rendered into the transcript instead of written to disk.

From outside cyrup:

```sh
cyrup --export session.jsonl notes.html
```

`--export` names the session file to convert; the optional second argument is where the HTML goes,
defaulting to the input filename with an `.html` extension. It converts and exits without starting
a session, so it works on any session file, not just the current one.

`/import <path>` takes a `.jsonl` transcript, copies it into the current project's session
directory, and resumes it. That is the other half of the raw export — the way to move a
conversation between machines. The file you point at is left where it is.

`/share` renders the session to HTML and publishes it as a secret GitHub gist, then prints the URL.
It shells out to the `gh` CLI, so `gh` has to be installed and logged in; the gist belongs to
whichever account `gh` is authenticated as. "Secret" means unlisted, not private — anyone with the
URL can read it, so check what is in the transcript first.

`/copy` puts the last agent message on your clipboard, which is what you want far more often than
a full export.

## Session stats

`/session` prints a table for the current session: its file on disk, its id, message counts split by
role, tool calls and results, input, output, cache-read and cache-write tokens, and cost. It is the
quickest way to answer "how much has this conversation cost me" and "which file am I actually in".

`/name <session name>` sets the display name, which is what the footer and the `/resume` picker
show. `--name` does the same at launch.

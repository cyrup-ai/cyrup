# The terminal interface

This page makes you fluent in the cyrup interface: what each part of the screen is telling you,
the keys worth memorising, and the things you can type that are not prose. The exhaustive key
tables live in [Keys and slash commands](../reference/keybindings.md).

On macOS, `Alt` is the Option key.

## Reading the screen

Top to bottom, the screen is a transcript, a status band, an input editor, and a footer.

### The transcript

Everything that has happened, in order: your messages, the model's response streaming in
token by token, thinking blocks, tool executions, the output of `!` commands, and compaction
summaries. Rows that are finished scroll into your terminal's own scrollback, so your normal
scroll wheel and `Cmd+F` work on them. `PageUp` and `PageDown` scroll the live region by ten lines —
unless your editor buffer is more than one line tall, in which case they page the cursor through
what you are writing instead.

On a fresh session, before you submit anything, the bottom of the transcript holds a hint bar:

```text
escape interrupt · ctrl+c/ctrl+d clear/exit · / commands · ! bash · ctrl+o more
```

Below it, a line offering `Ctrl+O` for full startup help and loaded resources, and a closing line
saying cyrup can explain its own features. The keys in the bar are read from your live keymap, so a
rebind shows up here. The whole block disappears on your first submission and does not come back;
on a short terminal it sheds its outer lines first, so the hint bar itself survives down to one row.

### The status band

Two fixed rows above the editor. When cyrup is idle they are blank — the editor never jumps
when work starts. When something is running you get a braille spinner and a message:

| Spinner message | What is happening |
|---|---|
| `Working...` | A turn is streaming |
| `Retrying (2/5) in 8s...` | The request failed and cyrup is counting down to a retry |
| `Compacting context...` | Context is being compacted |
| `Summarizing branch...` | A `/tree` branch is being summarised |

The last three carry a `(<key> to cancel)` hint resolved from your live keymap. `Working...` does
not — there is nothing to say that `Esc` does not already do.

### The input editor

A block of text between two horizontal rules, with no prompt glyph. It grows as you type, up to
about a third of the terminal height.

**The rule colour is a mode signal.** The `bashMode` colour — green in both built-in themes — means
the buffer starts with `!`: you are about to run a shell command, not talk to the model. Otherwise
the rules take the colour of your current thinking level, so the border tells you your reasoning
depth without looking anywhere else.

### The footer

The first line is your working directory (with `~` for home), the git branch in parentheses, and
the session name after a bullet. It truncates from the right with `...`.

The second line is usage on the left and model on the right:

```text
↑12.4k ↓3.1k R98k W12k CH76.3% $0.184 41.2%/200k (auto)        (anthropic) claude-sonnet-4-5 • high
```

- `↑` input tokens and `↓` output tokens, summed across every turn in the session.
- `R` cache reads and `W` cache writes — tokens served from or written to the provider's prompt
  cache. Cache reads are much cheaper than fresh input, so a large `R` is a good sign.
- `CH` the cache hit rate of the most recent turn.
- `$` the running cost. ` (sub)` after it means the cost is covered by a provider subscription.
- `41.2%/200k` — how much of the model's context window is occupied, and how big that window is.
  ` (auto)` means automatic compaction is on. Just after a compaction the occupancy is unknown until
  the next response and the segment reads `?/200k`.

**Watch the context percentage.** It turns amber above 70% and red above 90%. When it goes amber,
finish the thought you are on and then run `/compact` at a natural boundary — you get a much
better summary from a deliberate compaction than from the automatic one that fires when you run
out of room mid-task. See [Sessions](sessions.md) for what compaction actually does.

The right cluster is model and thinking level, with the provider in parentheses in front of it —
that prefix appears only when you have more than one provider configured and only when the row is
wide enough for it. A bold `xp` appears in the left cluster when experimental features are enabled
(`CYRUP_EXPERIMENTAL=1`).

A third footer line appears when an extension publishes a status; it is the only line cyrup leaves
unstyled, so an extension's own colours survive.

## Interrupting

`Esc` is the universal "stop that". It has an order of precedence, and it does exactly one thing
per press:

1. Abort a branch summarization.
2. Abort a compaction.
3. Abort the streaming turn — queued messages are put back in your editor first, so nothing is
   lost.
4. Cancel a running `!` command.
5. Clear the buffer, if it starts with `!`.
6. On an empty buffer, arm a double-press: hit `Esc` again within 500ms to open the session tree.

With an autocomplete popup open, `Esc` just closes the popup.

That last step is configurable. `doubleEscapeAction` is `tree` by default; set it to `fork` to open
the fork picker instead, or `none` to disable it.

## Quitting

`Ctrl+D` on an empty buffer quits. With text in the buffer it forward-deletes a character instead,
so it will not surprise you mid-sentence.

`Ctrl+C` clears the buffer. A second `Ctrl+C` within 500ms quits, whether the buffer is empty or
not. `Ctrl+Z` suspends cyrup to the background like any other job.

## Editing

`Enter` submits. To write a multi-line message, use `Shift+Enter` or `Ctrl+J`.

Some terminals cannot send `Shift+Enter` at all. For those, end the line with a backslash and press
`Enter`: cyrup deletes the backslash and inserts a newline instead of submitting.

`Ctrl+G` opens the buffer in `$VISUAL` or `$EDITOR`. Write, quit, and the edited text comes back
into the prompt — the right move for a long, structured instruction.

The editor has the emacs-style motion and kill-ring keys you would expect: `Ctrl+A`/`Ctrl+E` for
line ends, `Ctrl+W` to kill a word back, `Ctrl+K` to kill to end of line, `Ctrl+Y` to yank. The
full list is in [Keys and slash commands](../reference/keybindings.md).

## Switching model and thinking level

`Ctrl+P` moves forward through your model cycle set and `Ctrl+Shift+P` moves back. The cycle set
is whatever `/scoped-models` says, or every available model if you have not scoped it. You can
switch mid-conversation; the session keeps going.

`Ctrl+L` opens the model selector directly — the same picker `/model` opens, unfiltered.

`Shift+Tab` cycles the thinking level on the live model. The editor border changes colour to match
and the footer's right cluster updates. See [Models and thinking](models.md).

## Inspecting tool output

`Ctrl+O` toggles tool and bash output between collapsed and expanded, for the whole transcript.
Collapsed is the default so a noisy `cargo build` does not bury the conversation.

`Ctrl+T` hides and shows thinking blocks, reporting `Thinking blocks: hidden` or
`Thinking blocks: visible` and persisting the choice as the `hideThinkingBlock` setting. It changes
the block that is streaming and everything after it — rows already committed have gone to your
terminal's own scrollback and keep the form they were drawn with.

`Ctrl+X` copies the last assistant message to the clipboard, exactly as `/copy` does.

## Things you can type that are not prose

### `/command`

A leading `/` opens command completion immediately. The built-in commands are listed below.

An unrecognised `/foo` is **not** an error — it is sent to the model as literal text. That is
usually what you want when you are writing about paths or dates, but it does mean a typo in a real
command name gets quietly turned into prose.

### `!cmd` and `!!cmd`

A leading `!` runs the rest of the line as a shell command in a live block in the transcript.

```text
!cargo test -p cyrup-config
```

The distinction that matters: **`!cmd` puts the output into the model's context; `!!cmd` does
not.** Use `!` when you want the agent to see what happened — a failing test, a `git diff`, the
contents of a log. Use `!!` when you just want to look at something yourself: `!!ls -la`, `!!git
log --oneline -20`, or anything that dumps thousands of lines you would rather not pay for.

`Esc` cancels a running block. A bare `!` with nothing after it is ordinary text.

### `@file` mentions

Typing `@` opens a fuzzy file picker over the whole tree — no `Tab` needed. Accepting inserts
`@path` followed by a space. Paths with whitespace are quoted automatically as `@"my file.md"`, and
typing an opening quote yourself lets you keep typing across spaces.

The candidate list is capped at 2000 files. It comes from `fd` when that is installed, which means
your `.gitignore` is respected and `.git` is excluded; without `fd` cyrup falls back to a bounded
in-process walk that ignores `.gitignore` and skips `.git`, `node_modules`, `target`, `.cyrup` and
`.jj` by name.

A bare path token — anything containing `/`, or starting with `.` or `~/` — opens a directory-scan
popup instead. Directories complete without a trailing space so you can keep drilling down.

### Pasting an image

`Ctrl+V` (or `Alt+V`) with an image on the clipboard writes it to a temp `.png` and inserts **the
file path as text** at your cursor. The image itself is not attached to the message, so nothing
enters context until the agent decides to read that file. If the clipboard holds text rather than
an image, `Ctrl+V` pastes it normally.

### Large pastes

A bracketed paste over 10 lines or 1000 characters collapses into a single marker like
`[paste #1 +240 lines]`, which behaves as one atomic character in the editor. The full content is
restored when you submit — the marker is a display convenience, not a truncation.

## Queueing work while the agent is busy

You do not have to wait for a turn to finish. Which queue a message lands in depends on the key:
plain `Enter` during a streaming turn **steers** it, and `Alt+Enter` queues a **follow-up**. When
nothing is streaming, `Alt+Enter` is an ordinary submit.

Each queued message gets a dim row above the status band, and below them a hint naming the key that
pulls them back:

```text
Steering: also check the windows path handling
Follow-up: then write a test for it
↳ Alt+Up to edit all queued messages
```

(On macOS that hint reads `Option+Up`.)

**Steering** messages are delivered into the turn that is currently running — use them to redirect
work in flight. **Follow-up** messages wait for the current turn to end and then start the next
one. Both default to `one-at-a-time` delivery, so a queue of three gets handed over one per turn
rather than all at once; `steeringMode` and `followUpMode` change that to `all`.

Anything you submit while a compaction is running goes into a third queue and is delivered when the
compaction finishes — an extension's own command still runs immediately.

`Alt+Up` pulls every queued message back into the editor for editing. Aborting a turn with `Esc`
does the same thing automatically. Swapping session — `/resume`, `/fork`, `/import` — clears all
three queues: they belonged to the session you left.

A queued message is **not** written into the transcript until the turn that carries it starts. If
you see it above the editor rather than in the conversation, it has not been sent yet.

## Slash commands

| Command | Argument | Effect |
|---|---|---|
| `/settings` | — | Open the settings grid |
| `/model` | `provider/model` | Exact match switches directly; anything else opens the picker |
| `/scoped-models` | — | Choose which models `Ctrl+P` cycles through |
| `/export` | optional path | `.jsonl` writes the raw transcript; any other path writes styled HTML; no path writes `cyrup-session-<file stem>.html` in the current directory |
| `/import` | path to `.jsonl` | Import a session and resume it |
| `/share` | — | Publish the session as a secret GitHub gist and print the viewer link |
| `/copy` | — | Copy the last agent message to the clipboard |
| `/name` | optional session name | With a name, sets the session's display name; bare, prints the current one |
| `/session` | — | Print a stats table for the current session |
| `/changelog` | — | Push a "What's New" block into the transcript; there are no entries yet |
| `/hotkeys` | — | Print the current keybindings into the transcript |
| `/fork` | — | Fork from an earlier user message into a new session |
| `/clone` | — | Duplicate the session at its current position |
| `/tree` | — | Open the session tree navigator |
| `/trust` | — | Set the trust decision for this project folder |
| `/login` | provider | Run a provider's authentication flow |
| `/logout` | — | Sign out of a provider |
| `/new` | — | Start a new session |
| `/compact` | instructions | Compact the context now |
| `/resume` | — | Open the session picker |
| `/reload` | — | Reload keybindings, extensions, skills, prompts, themes and context files |
| `/quit` | — | Quit |

**Only six commands take an argument at all:** `/model`, `/export`, `/import`, `/name`, `/login` and
`/compact`. Every other name matches on exact equality, so `/quit now`, `/copy that` and `/new
session` are not commands — the whole line is sent to the model as a prompt. `/export` and `/import`
take one quote-aware token (`/export "my session.html"` works, and the quotes are stripped); the
other four take the rest of the line whole.

The session commands — `/resume`, `/fork`, `/clone`, `/tree`, `/compact`, `/export` — are covered in
depth in [Sessions](sessions.md), and `/trust` in
[Project context and skills](project-context.md).

`/hotkeys` prints the *live* bindings, not a static list, so it stays correct after you customise
keys or load an extension that registers its own. It is the fastest way to answer "what is that
key again".

There is no `/theme` or `/think` command — the theme lives in `/settings` and the thinking level
on `Shift+Tab`.

Extensions, prompt templates, and skills add their own commands to the completion list, in that
order after the builtins: prompt templates first, then extension commands, then skills. Skills
appear as `/skill:<name>` and can be turned off with the `enableSkillCommands` setting.

## Autocomplete

The popup opens by itself: `/` at the start of a line opens command completion, `@` anywhere opens
file completion. `Up`/`Down` move, `Esc` dismisses.

`Tab` accepts the highlighted item and leaves you in the editor. `Enter` accepts it too — and if
the item is a slash command, submits immediately. That is the difference worth internalising:
`Tab` when you have more to type, `Enter` when the command is the whole message.

## Customising keys

Every key resolves through a table, and you can replace any of them. Bindings live in
`~/.cyrup/agent/keybindings.json` — a flat map from binding id to a key spec or a list of them:

```json
{
  "app.interrupt": "ctrl+q",
  "tui.input.newLine": ["shift+enter", "ctrl+j"]
}
```

Each id you set replaces that action's entire key set. The full id list and the key-spec grammar are
in [Keys and slash commands](../reference/keybindings.md). Four ids — `app.session.new`,
`app.session.tree`, `app.session.fork` and `app.session.resume` — ship with no default key at all,
so they exist only if you bind them.

A bad entry costs you that entry and nothing else. A value that is neither a string nor a list of
strings leaves that action on its default; an unparseable key spec drops that one key and the other
keys in the same list still apply. Each rejection is reported by binding id —
`warning: <path>: ignoring <id>: <reason>` at startup, `keybindings: ignoring <id>: <reason>` after
a `/reload` — and the rest of the file takes effect. An id no keymap owns is skipped without a
message. Only JSON that does not parse, or a top level that is not an object, drops the whole
document.

Run `/reload` to apply an edit without restarting. It resets every map to its defaults before
merging your file, so deleting an entry restores the default binding.

One key is not rebindable: `Ctrl+Shift+D` runs `/debug`. It is checked before all focus routing,
so it works even with a picker or dialog open.

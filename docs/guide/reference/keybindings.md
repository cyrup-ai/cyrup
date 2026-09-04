# Keys and slash commands

Every key cyrup binds by default, every built-in slash command, and how to rebind them. `/hotkeys`
prints an abbreviated version of this inside the session, resolved from your live bindings. For
what the screen is telling you while you use these, see
[The terminal interface](../guides/tui.md).

Keys are written `Ctrl+P`, `Shift+Tab`, `Alt+Enter`. **On macOS, `Alt` is the Option key.**

## Global keys

These are active whenever no picker, dialog or overlay owns the keyboard.

| Key | Effect |
|---|---|
| `Ctrl+D` | Quit — only when the editor is empty. Otherwise it forward-deletes. |
| `Ctrl+C` | Clear the editor. A second press within 500 ms quits, empty or not. |
| `Esc` | Interrupt. Context-dependent — see below. |
| `Ctrl+Z` | Suspend cyrup to the background. |
| `Ctrl+O` | Expand or collapse tool and bash output. |
| `Ctrl+G` | Open the editor buffer in your external editor. |
| `PageUp` / `PageDown` | Scroll the active region by ten lines. Goes to the editor when the buffer spans more than one visual line. |
| `Shift+Tab` | Cycle the thinking level on the current model. |
| `Ctrl+P` | Next model in the cycling set. |
| `Ctrl+Shift+P` | Previous model. |
| `Ctrl+L` | Open the model selector — the same unfiltered picker a bare `/model` opens. |
| `Ctrl+T` | Show or hide thinking blocks. Persists `hideThinkingBlock` and reports `Thinking blocks: hidden` / `visible`. |
| `Ctrl+X` | Copy the last assistant message to the clipboard — the same action as `/copy`. |
| `Alt+Enter` | Queue the buffer as a follow-up message. No-op on an empty buffer. |
| `Alt+Up` | Pull queued messages back into the editor. |
| `Ctrl+V` / `Alt+V` | Paste an image from the clipboard as a file path. Falls through to ordinary text paste when the clipboard holds no image. |

`Ctrl+T` toggles thinking-block *visibility*; `Shift+Tab` changes the thinking *level* on the model.
They are separate bindings for separate things.

Four more actions are bindable but ship with **no default key**, matching upstream: `app.session.new`,
`app.session.tree`, `app.session.fork` and `app.session.resume`. Name them in
`keybindings.json` to reach `/new`, `/tree`, `/fork` and `/resume` from a chord.

`Ctrl+Shift+D` runs `/debug`. It is checked before any focus routing, so it works with a picker or
overlay open, and it is **hardcoded — you cannot rebind it**.

Shortcuts registered by extensions are matched after this table and before the editor.

## Esc behaviour

`Esc` does exactly one of the following, in this order:

1. Branch summarization is running — abort it and stop.
2. Compaction is running — abort compaction.
3. A turn is streaming — restore queued messages to the editor, then abort the turn.
4. A `!` or `!!` bash block is running — cancel it.
5. The buffer starts with `!` — clear the buffer.
6. The buffer is empty — start a 500 ms double-`Esc` window. A second `Esc` fires
   the [`doubleEscapeAction` setting](settings.md): `fork` opens the fork picker, `tree` (the
   default) opens the session tree, `none` does nothing.
7. Otherwise nothing happens.

With the autocomplete popup open, `Esc` only dismisses the popup.

## Editor keys

### Motion

| Key | Effect |
|---|---|
| `Left` / `Ctrl+B` | Character left |
| `Right` / `Ctrl+F` | Character right |
| `Up` / `Down` | Line up / down; at the buffer edge, browse prompt history |
| `Alt+Left` / `Ctrl+Left` / `Alt+B` | Word left |
| `Alt+Right` / `Ctrl+Right` / `Alt+F` | Word right |
| `Home` / `Ctrl+Home` / `Ctrl+A` | Start of line |
| `End` / `Ctrl+End` / `Ctrl+E` | End of line |
| `PageUp` / `Ctrl+PageUp` | Page the caret up |
| `PageDown` / `Ctrl+PageDown` | Page the caret down |

Dedicated history bindings exist (`tui.editor.historyPrevious` and `historyNext`) but are unbound
by default.

### Deletion and the kill ring

| Key | Effect |
|---|---|
| `Backspace` | Delete backward |
| `Delete` / `Ctrl+D` | Delete forward |
| `Ctrl+W` / `Alt+Backspace` | Kill the word before the caret |
| `Alt+D` / `Alt+Delete` | Kill the word after the caret |
| `Ctrl+U` | Kill to start of line |
| `Ctrl+K` | Kill to end of line |
| `Ctrl+Y` | Yank the last kill |
| `Alt+Y` | Yank-pop through earlier kills |
| `Ctrl+-` | Undo |

### Char-jump

| Key | Effect |
|---|---|
| `Ctrl+]` | Jump forward to a character |
| `Ctrl+Alt+]` | Jump backward to a character |

After pressing either, the next printable character you type is the jump target. Any other key
cancels the jump.

### Submit and newline

| Key | Effect |
|---|---|
| `Enter` | Submit |
| `Shift+Enter` / `Ctrl+J` | Insert a newline |
| `Tab` | Trigger completion |

**On terminals without the kitty keyboard protocol**, `Ctrl+7`, `Ctrl+5` and `Ctrl+4` decode to
`Ctrl+-`, `Ctrl+]` and `Ctrl+\` respectively — so undo and jump-forward still work in Terminal.app,
stock iTerm2, gnome-terminal and xterm. Nothing binds `Ctrl+\` by default; it is there for your own
bindings.

## Autocomplete popup

| Key | Effect |
|---|---|
| `Up` / `Down` | Move the selection |
| `Tab` | Accept and keep editing |
| `Enter` | Accept — and submit immediately if the item is a slash command |
| `Esc` | Dismiss |

## Shared selector keys

Every picker and dialog binds these:

| Key | Effect |
|---|---|
| `Up` / `Down` | Move |
| `Enter` | Confirm |
| `Esc` or `Ctrl+C` | Cancel |
| `PageUp` / `PageDown` | Page |

Individual pickers add keys on top.

### /resume — the session picker

| Key | Effect |
|---|---|
| `Ctrl+S` | Cycle sort: threaded, recent, fuzzy |
| `Ctrl+N` | Toggle between all sessions and named sessions |
| `Ctrl+D` | Delete, with an inline confirm — `Enter` confirms, `Esc` cancels |
| `Ctrl+Backspace` | Same delete confirm, but only while the search box is empty; otherwise the key goes to the search box |
| `Ctrl+P` | Show or hide the path column |
| `Ctrl+R` | Rename the highlighted session inline |
| `Tab` | Toggle between this project and all projects |
| any printable character | Type into the search box |
| `Backspace` | Delete from the search box |

### /tree — the session-tree navigator

| Key | Effect |
|---|---|
| `Alt+Left` / `Ctrl+Left` | Fold, or move up |
| `Alt+Right` / `Ctrl+Right` | Unfold, or move down |
| `Shift+L` | Edit the row's label inline |
| `Shift+T` | Toggle the label-timestamp column |
| `Ctrl+D` | Filter: default |
| `Ctrl+T` | Filter: no tools |
| `Ctrl+U` | Filter: user messages only |
| `Ctrl+L` | Filter: labeled only |
| `Ctrl+A` | Filter: all |
| `Ctrl+O` / `Ctrl+Shift+O` | Cycle filters forward / back |
| any printable character | Add to the search query |

`Esc` clears the search query first, and only cancels the picker once the query is empty. While the
label editor is open it captures every key.

### /scoped-models

| Key | Effect |
|---|---|
| `Enter` | Toggle the highlighted model — the picker stays open |
| `Alt+Up` / `Alt+Down` | Reorder |
| `Ctrl+A` | Enable all |
| `Ctrl+X` | Clear all |
| `Ctrl+P` | Toggle every model of the highlighted provider |
| `Ctrl+S` | Save to settings and close |
| `Ctrl+C` | Clear the search box; cancels only when it is empty |
| `Esc` | Cancel |

### /model

| Key | Effect |
|---|---|
| `Tab` | Toggle between the full catalog and your scoped models |
| `Up` / `Down` | Move — the list wraps |
| `Enter` | Confirm |
| any printable character | Fuzzy-search |

### /settings

| Key | Effect |
|---|---|
| `Enter` | Open a submenu row, or cycle the value in place and apply it live |
| `Space` | Activate a row — only while the search box is empty |
| any printable character | Search the rows |

## Slash commands

| Command | Argument | Effect |
|---|---|---|
| `/settings` | — | Open the settings grid |
| `/model` | `provider/model` | An exact match switches straight away; anything else opens the picker pre-filtered |
| `/scoped-models` | — | Edit the `Ctrl+P` cycling set |
| `/export` | path, optional | A `.jsonl` path writes the raw transcript; any other path writes styled HTML; with no path it writes `cyrup-session-<session file stem>.html` in the current directory |
| `/import` | path to a `.jsonl` | Import a session and resume it |
| `/share` | — | Publish the session as a secret GitHub gist and print the viewer link |
| `/copy` | — | Copy the last agent message to the clipboard |
| `/name` | session name, optional | With a name, set it; bare, report the current one |
| `/session` | — | Print a table of file, id, message and tool counts, tokens and cost |
| `/changelog` | — | Show the "What's New" block; this build ships no entries |
| `/hotkeys` | — | Print the shortcut tables into the transcript |
| `/fork` | — | Fork from an earlier user message |
| `/clone` | — | Duplicate the session at the current position |
| `/tree` | — | Open the session-tree navigator |
| `/trust` | — | Open the project-trust picker |
| `/login` | provider, optional | Authenticate a provider |
| `/logout` | — | Remove a stored credential |
| `/new` | — | Start a new session |
| `/compact` | instructions, optional | Compact the context now |
| `/resume` | — | Open the session picker |
| `/reload` | — | Reload keybindings, extensions, skills, prompts, themes and context files |
| `/quit` | — | Quit |

There is no `/theme`, `/think` or `/show-images` command — the theme and image settings live in
`/settings`, and the thinking level is on `Shift+Tab`.

**Only six commands take an argument at all**: `/model`, `/export`, `/import`, `/name`, `/login` and
`/compact`. Every other builtin matches by exact equality, so `/quit now`, `/copy that`,
`/new session` and `/trust me` are not commands — they fall through and are sent to the model as
ordinary prompts.

`/export` and `/import` take **one** quote-aware token, not the rest of the line. A leading `"` or
`'` runs to its matching close and the quotes are stripped, so `/export "my session.html"` writes
the file you meant; a second token is ignored rather than becoming part of the path; and an
unterminated quote is refused, which lands in the same branch as passing no argument at all.
`/name`, `/compact`, `/model` and `/login` still take their remainder whole.

Three commands report a missing or unusable argument on the warning or error channel rather than as
a neutral status: bare `/name` with no name set prints `Usage: /name <name>`, `/import` with no path
prints `Usage: /import <path.jsonl>`, and a `/compact` you cancel with `Esc` prints
`Compaction cancelled` (a genuine failure prints `Compaction failed: <reason>`).

`/debug` is dispatched but hidden from autocomplete. It prints the terminal size, the active theme
and its generation, the thinking level, whether images are shown, and the streaming state — the
first thing to reach for when the interface is behaving oddly. `Ctrl+Shift+D` runs it too. Two
further hidden commands, `/arminsayshi` and `/dementedelves`, print a picture and a one-line banner
respectively, and do nothing else.

**Dynamic commands.** Extensions, prompt templates and skills contribute commands to the
autocomplete list, in that order after the builtins: builtins, then your prompt templates, then
extension commands, then skills. Skills appear as `skill:<name>` and only when `enableSkillCommands`
is on. A builtin wins a name collision.

Most non-builtin entries carry a scope tag in their description — `[u]` for user, `[p]` for project,
`[t]` for a package. An entry that reports no source at all gets no tag: its description renders
unprefixed rather than being labelled `[t]`, which would be a provenance claim cyrup cannot make.

The `/` menu's order is a display decision only. The `get_commands` RPC payload keeps its own order.

An unrecognised `/foo` is not an error. It is sent to the model as literal text.

## Non-prose input

| Typed | Meaning |
|---|---|
| `!cmd` | Run `cmd` in a live bash block; the output goes into the context |
| `!!cmd` | The same, but the output is kept out of the context |
| `!` alone | Ordinary text, not a command |
| `@path` | Mention a file. Typing `@` opens whole-tree fuzzy completion immediately |
| `@"path with spaces"` | Mention a file whose path contains whitespace |
| trailing `\` before `Enter` | Soft newline — the backslash is removed and a newline inserted instead of submitting |
| `re:pattern` | In the `/resume` search box: advertised as regex search, but **not wired** — see below |
| `"phrase"` | In the `/resume` search box: exact-phrase search |

There is no `#` prefix of any kind in cyrup.

The picker's own hint row offers `re:<pattern> regex`, and that form does not work: the prefix is
recognised but no regex engine is attached to it, so a `re:` query matches nothing and you get an
empty list with no error. Unquoted words (fuzzy, all must match) and `"phrase"` are the two forms
that do filter. See [Sessions](../guides/sessions.md).

## Customising keybindings

Bindings live in one file:

```text
~/.cyrup/agent/keybindings.json
```

Note the path — it is inside the agent directory, **not** `~/.cyrup/keybindings.json`. If you have
moved the agent directory with `CYRUP_AGENT_DIR`, the file moves with it.

The format is a flat JSON object mapping a binding id to a key spec, or to an array of key specs:

```json
{
  "app.interrupt": "ctrl+q",
  "tui.input.newLine": ["shift+enter", "ctrl+j"],
  "app.tree.editLabel": "shift+l"
}
```

A recognised id **replaces that action's entire key set** — it does not add to it. Unknown ids are
ignored silently.

**A bad entry costs you that entry, not the file.** A value that is neither a string nor an array of
strings skips the entry and the action keeps its default. A key spec that will not parse drops that
one key and the rest of the entry still applies; when it was the only key, the action ends up
unbound. Each rejection is reported by binding id and reason — `warning: <path>: ignoring <id>:
<reason>` at startup, `keybindings: ignoring <id>: <reason>` on `/reload`. Only unparseable JSON, or
a top level that is not an object, discards the whole document.

### Key-spec grammar

A key spec is modifiers and a key joined with `+`, and is case-insensitive.

- **Modifiers:** `ctrl` (`control`), `shift`, `alt` (`option`, `meta`), `super` (`cmd`, `command`).
- **Named keys:** `enter` (`return`), `tab`, `backtab`, `esc` (`escape`), `space`, `up`, `down`,
  `left`, `right`, `home`, `end`, `backspace`, `delete` (`del`), `insert` (`ins`), `pageup`
  (`pgup`, `pageUp`), `pagedown` (`pgdn`, `pageDown`), and `f1` through `f12`.
- **Anything else must be a single character.** `ctrl+shift+p` is valid; `ctrl+q1` is not.

`clear` is deliberately unsupported — the terminal backend has no counterpart for it, so there is
nothing to bind.

`shift+a` matches a terminal that reports `A` with Shift held. Caps Lock and Num Lock state is
stripped before matching, so neither can defeat a binding.

`/hotkeys` and the on-screen key hints render these back through the same grammar, so what you read
is what you can write: `PageUp`, `PageDown`, `Escape`, `F5`. `Shift+Tab` prints once, not as
`Shift+Shift+Tab`, and a chord your terminal reports under several spellings collapses to one
label.

### Binding ids

There are 73 ids in four namespaces:

| Namespace | Covers |
|---|---|
| `tui.editor.*` | Editor motion, deletion, kill ring, char-jump, undo, history |
| `tui.input.*` | `newLine`, `submit`, `tab`, `copy` |
| `tui.select.*` | The shared picker keys — up, down, page up, page down, confirm, cancel |
| `app.*` | Interrupt, clear, exit, suspend, thinking level and thinking-block visibility, model cycling and the model selector, tool output, external editor, message queueing, copy-last-message, clipboard, and the session, tree and scoped-model pickers |

Older spellings still work: bare legacy names such as `cursorUp`, `submit` and `interrupt`, the
`editor.*` prefix, `app.pageUp` and `app.pageDown`, and `tui.autocomplete.*`. The autocomplete popup
also honours `tui.select.up`, `tui.select.down`, `tui.select.confirm`, `tui.select.cancel` and
`tui.input.tab`.

Legacy ids are renamed as the file is read, and the file is rewritten once at startup so ids appear
in declaration order, with unrecognised ids sorted and appended at the end. A file with nothing to
migrate is left byte-for-byte unchanged.

### Applying changes

`/reload` re-reads `keybindings.json` without restarting. It resets every keymap to its defaults
before merging your file, so **deleting an entry restores that action's default binding**.

### An example file

```json
{
  "app.interrupt": ["esc", "ctrl+q"],
  "app.model.cycleForward": "ctrl+n",
  "app.model.cycleBackward": "ctrl+shift+n",
  "tui.input.newLine": ["shift+enter", "ctrl+j"],
  "tui.editor.historyPrevious": "alt+p",
  "tui.editor.historyNext": "alt+n"
}
```

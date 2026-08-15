# Themes

cyrup ships two themes, `dark` and `light`, and reads custom ones from JSON files. This page covers
picking a theme, writing your own, and where the files go.

## Picking a built-in theme

Open `/settings` and select the **Theme** row. The picker previews each theme live as you move the
highlight, so you can see the real colours before committing. `Enter` confirms and writes the
`theme` setting to `~/.cyrup/agent/settings.json`; `Esc` cancels and restores what you had.

## Following the terminal

The `theme` setting takes three kinds of value.

An explicit name — `"dark"`, `"light"`, or the name of a custom theme — is used verbatim, with no
terminal probing at all.

An auto pair — any two theme names with exactly one slash between them, `"light/dark"` being the
obvious one — means "match the terminal": cyrup detects your terminal's polarity at startup and
picks the matching half. It detects **once, at boot**, and does not re-theme when the terminal
changes afterwards. (Terminals can push a colour-scheme notification, but nothing in cyrup consumes
it, and enabling it without a consumer would feed escape sequences into your prompt as stray
keystrokes. Committed transcript rows have also already gone to the terminal's own scrollback and
could not be recoloured anyway.)

Leaving `theme` unset also detects polarity, and if the detection is high-confidence, cyrup writes
the result back into your settings. That is why an untouched install ends up with a concrete
`theme` value after the first run.

Detection asks the terminal what it prefers before guessing at it: for an auto pair, the
colour-scheme query first, then the background-colour query, then `COLORFGBG`. With no `theme`
setting at all only the background-colour query and `COLORFGBG` are consulted. Terminals that answer
none of these get the default.

## Writing a custom theme

A theme is a JSON file with four fields. `colors` is a **closed set of 51 required roles**: a file
that omits any of them does not load at all. The excerpt below shows the shape; it is not a complete
file, and would be rejected as written. The full role list is in the table further down.

```json
{
  "name": "harbour",
  "vars": {
    "ink": "#c9d1d9",
    "signal": "#58a6ff",
    "quiet": "#6e7681"
  },
  "colors": {
    "text": "ink",
    "accent": "signal",
    "border": "signal",
    "borderAccent": "signal",
    "borderMuted": "#30363d",
    "muted": "#8b949e",
    "dim": "quiet",
    "error": "#f85149",
    "warning": "#d29922",
    "success": "#3fb950",
    "bashMode": "#3fb950",
    "userMessageBg": "#161b22",
    "mdHeading": "#d29922",
    "toolDiffAdded": "#3fb950",
    "toolDiffRemoved": "#f85149",
    "thinkingMedium": "#bc8cff",
    "thinkingHigh": "#d2a8ff"
  },
  "export": {
    "pageBg": "#0d1117",
    "cardBg": "#161b22",
    "infoBg": ""
  }
}
```

`vars` is your own palette. `colors` assigns those colours to roles, covering text and borders,
status colours, message and tool-block backgrounds, markdown, syntax highlighting, diffs, and one
colour per thinking level. `export` styles the HTML that `/export` and `/share` produce; its three
keys are optional, as is `vars`.

Every colour value may be:

- a hex string, `#rrggbb`
- a bare name from `vars` (a leading `$` is accepted too)
- an empty string, meaning inherit
- an integer from 0 to 255, resolved through the xterm-256 palette

These are the 51 roles, grouped the way the schema declares them. Every one of them must be present
in `colors`:

| Group | Roles |
|---|---|
| Core UI | `accent`, `border`, `borderAccent`, `borderMuted`, `success`, `error`, `warning`, `muted`, `dim`, `text`, `thinkingText` |
| Backgrounds and content text | `selectedBg`, `userMessageBg`, `userMessageText`, `customMessageBg`, `customMessageText`, `customMessageLabel`, `toolPendingBg`, `toolSuccessBg`, `toolErrorBg`, `toolTitle`, `toolOutput` |
| Markdown | `mdHeading`, `mdLink`, `mdLinkUrl`, `mdCode`, `mdCodeBlock`, `mdCodeBlockBorder`, `mdQuote`, `mdQuoteBorder`, `mdHr`, `mdListBullet` |
| Tool diffs | `toolDiffAdded`, `toolDiffRemoved`, `toolDiffContext` |
| Syntax highlighting | `syntaxComment`, `syntaxKeyword`, `syntaxFunction`, `syntaxVariable`, `syntaxString`, `syntaxNumber`, `syntaxType`, `syntaxOperator`, `syntaxPunctuation` |
| Thinking-level borders | `thinkingOff`, `thinkingMinimal`, `thinkingLow`, `thinkingMedium`, `thinkingHigh`, `thinkingXhigh` |
| Bash mode | `bashMode` |

`thinkingMax` is the one optional role: leave it out and the `max` level borrows `thinkingXhigh`.
Extra keys are allowed — their values are still checked for validity — but nothing reads them.

**Naming a role is required; giving it a colour is not.** Set a role to `""` and it inherits the
terminal's own colour, which is how you decline one without dropping it. A reference to a `vars` key
that does not exist, or one that forms a cycle, degrades to inherit too — that case does not fail
the load.

A file that is missing required roles is reported like this and then skipped:

```text
Invalid theme "/Users/you/.cyrup/agent/themes/harbour.json":

Missing required color tokens:
  - syntaxOperator
  - toolOutput

Please add these colors to your theme's "colors" object.
See the built-in themes (dark.json, light.json) for reference values.
```

That message is built but **nothing prints it today** — a theme that fails validation is dropped
during discovery and the failure is not surfaced anywhere. If a theme you wrote never appears, an
incomplete `colors` object is the first thing to check. (The message names `dark.json` and
`light.json`, which are compiled into the binary rather than shipped as files; the role table above
is the reference it means.)

The other way a file is rejected is a `name` containing `/` — that character is reserved for the
auto pair.

## Where theme files go

| Location | Scope |
|---|---|
| `~/.cyrup/agent/themes/*.json` | Available in every project |
| `.cyrup/themes/*.json` | This repository only; loaded once the project is trusted |
| Paths in the `themes` settings array | Files or directories you name yourself |

Installed packages can also ship themes, which are discovered the same way. See
[How extensions work](../extensions/overview.md).

A project theme in `.cyrup/themes/` is only loaded once you have trusted the project — the same
gate that governs project settings and project extensions. Until then cyrup behaves as if the
directory were not there. `/trust` sets that decision; see
[Project context and skills](project-context.md).

To make a theme file available for one run without adding it to any settings file, pass it on the
command line. It is loaded, not selected — `theme` still has to name it.

```sh
cyrup --theme ./design/harbour.json
```

`--no-themes` goes the other way and disables theme discovery entirely.

## Selecting a custom theme

**The `/settings` theme picker only lists the two compiled-in themes.** Custom themes do not appear
in it, no matter where the file lives. To name one, set `theme` to its `name` in
`~/.cyrup/agent/settings.json` directly:

```json
{
  "theme": "harbour"
}
```

The name that matters is the `name` field inside the file, not the filename.

**Naming a custom theme does not paint the interface with it at boot.** The startup theme resolver
only knows the two compiled-in themes and falls back to `dark` for any other name, so a fresh run
with `"theme": "harbour"` comes up in the built-in dark palette. What the name *does* do is pick the
file cyrup watches: the theme is discovered, validated and listed in the startup resources panel, and
the first save of that file after startup repaints the whole interface with its colours (see
[Hot reload](#hot-reload) below). Editing and saving the file once is the working way to get a custom
theme onto the screen today; there is no way to boot straight into one.

## Hot reload

When the active theme resolves to a file on disk, cyrup watches that file. Save an edit and the
interface repaints with the new colours — no restart, no `/reload`. This makes iterating on a theme
practical: keep the JSON open in one pane and cyrup in another.

## Colour depth

Colours are projected onto whatever your terminal supports: truecolor, 256 colours, or 16. On a
16-colour terminal your carefully chosen `#58a6ff` becomes the nearest ANSI blue, so check any
theme you intend to share on a limited terminal as well as your own.

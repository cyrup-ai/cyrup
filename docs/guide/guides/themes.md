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

The auto pair, spelled `"light/dark"`, means "match the terminal": cyrup detects your terminal's
polarity and picks the matching half, re-detecting when it changes.

Leaving `theme` unset also detects polarity, and if the detection is high-confidence, cyrup writes
the result back into your settings. That is why an untouched install ends up with a concrete
`theme` value after the first run.

Detection tries the terminal's own background-colour queries first and falls back to `COLORFGBG`.
Terminals that answer none of these get the default.

## Writing a custom theme

A theme is a JSON file with four fields:

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

`vars` is your own palette. `colors` assigns those colours to roles — around fifty of them, covering
text and borders, status colours, message and tool-block backgrounds, markdown, syntax
highlighting, diffs, and one colour per thinking level. `export` styles the HTML that `/export` and
`/share` produce.

Every colour value may be:

- a hex string, `#rrggbb`
- a bare name from `vars` (a leading `$` is accepted too)
- an empty string, meaning inherit
- an integer from 0 to 255, resolved through the xterm-256 palette

The roles fall into groups, which is the practical way to work through them:

| Group | Covers |
|---|---|
| `text`, `accent`, `muted`, `dim`, `success`, `error`, `warning` | The base palette everything else falls back on |
| `border`, `borderAccent`, `borderMuted` | Rules around the editor and selectors |
| `userMessageBg`, `customMessageBg`, `selectedBg`, `tool*Bg` | Block backgrounds in the transcript |
| `md*` | Rendered markdown: headings, links, code, quotes, lists |
| `syntax*` | Syntax highlighting inside code blocks |
| `toolDiffAdded`, `toolDiffRemoved`, `toolDiffContext` | Diffs from the `edit` and `write` tools |
| `thinkingOff` … `thinkingMax` | One colour per thinking level, used for the editor border |
| `bashMode` | The editor border while the buffer starts with `!` |

A role you do not name inherits. So does a reference to a `vars` key that does not exist, or one
that forms a cycle — an unresolvable reference degrades to inherit rather than failing to load. You
can start from a handful of roles and fill in the rest as they bother you.

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
in it, no matter where the file lives. To use one, set `theme` to its `name` in
`~/.cyrup/agent/settings.json` directly:

```json
{
  "theme": "harbour"
}
```

The name that matters is the `name` field inside the file, not the filename.

## Hot reload

When the active theme resolves to a file on disk, cyrup watches that file. Save an edit and the
interface repaints with the new colours — no restart, no `/reload`. This makes iterating on a theme
practical: keep the JSON open in one pane and cyrup in another.

## Colour depth

Colours are projected onto whatever your terminal supports: truecolor, 256 colours, or 16. On a
16-colour terminal your carefully chosen `#58a6ff` becomes the nearest ANSI blue, so check any
theme you intend to share on a limited terminal as well as your own.

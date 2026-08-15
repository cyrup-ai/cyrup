# Installing extensions

`cyrup install` fetches **packages**. A package is a repository or a directory that may contain
extensions, skills, prompt templates and themes. This page covers installing, listing, updating and
removing them, and where they end up on disk.

The CLI help uses "package" and "extension" interchangeably in places. They are not the same thing:
an [extension](overview.md) is one WebAssembly component, a package is the shipping container that
may hold several of them plus other resources.

## Installing

```sh
cyrup install git:github.com/user/repo
```

The package is cloned, its manifest is read, and a record is written to cyrup's package registry.
Its extensions, skills, prompt templates and themes become available to your next session.

Accepted source forms:

| Form | Example |
|---|---|
| Local path | `./tools/my-package`, `../shared/pkg` |
| `git:` with a host path | `git:github.com/user/repo` |
| `git:` with an SSH address | `git:git@github.com:user/repo` |
| HTTPS / HTTP URL | `https://github.com/user/repo` |
| SSH URL | `ssh://git@github.com/user/repo` |
| Git protocol URL | `git://github.com/user/repo` |
| scp-style address | `git@github.com:user/repo` |
| GitHub shorthand | `github:user/repo` |

Anything that is not one of the recognised prefixes is treated as a local path, including a bare
name. Without the `git:` prefix, only explicit-protocol URLs and the scp-style form are recognised
as remote; a bare `host/user/repo` works only behind `git:`.

**`npm:` sources are rejected.** cyrup has no JavaScript runtime, so an npm package has nothing to
run. `cyrup install npm:@scope/name` fails with an unsupported-source error. The help text for
`cyrup remove` still shows an `npm:` example; that example cannot be installed in the first place.

A local path is **referenced in place, not copied**. cyrup checks that the path exists and records
it; edits you make to the directory afterwards are live. That makes a local path the right form for
a package you are developing.

### Pinning to a ref

Append `@<ref>` to any git source:

```sh
cyrup install git:github.com/user/repo@v1.2.0
cyrup install git:github.com/user/repo@a1b2c3d4e5f
```

A ref that is 7 to 40 hexadecimal characters is treated as a commit; anything else is treated as a
tag. Either way the package is **pinned**, and pinned packages are skipped by bulk updates. To move
a pinned package, install it again at the new ref.

### Project scope

```sh
cyrup install -l ./tools/repo-linter
```

`-l` (long form `--local`) installs into the repository instead of your home directory, so the
package travels with the project. It is accepted for `install` and `remove` only.

Project scope **requires project trust**. In an untrusted repository the command refuses with
`Project is not trusted. Use --approve to modify local package config.` and exits non-zero. Pass
`-a`/`--approve` to override the saved trust decision for that one command, or `-na`/`--no-approve`
to force the opposite.

cyrup writes a self-ignoring `.gitignore` into the install root the first time it creates one, so a
project-scoped clone does not show up as untracked noise in your repository.

### The security notice

Every install prints it:

```text
Packages run with full system access: extensions execute code and skills can instruct the model to
run anything. Only install packages you trust.
```

That is accurate. The [capability sandbox](overview.md) constrains a WebAssembly extension, but a
package's skills and prompt templates are instructions to the model, and the model has your tools.
Read what you install.

## Listing

```sh
cyrup list
```

Prints `No packages installed.` when there are none, otherwise a `User packages:` block followed by
a `Project packages:` block. Each line shows the source you installed with, marked `(filtered)` if
some of the package's resources are disabled, and — when the directory exists — the on-disk path
indented beneath it.

## Removing

```sh
cyrup remove git:github.com/user/repo
```

`uninstall` is an alias. Add `-l` to remove a project-scoped package.

Remove using the **same source string you installed with**. `install` normalises a source into an
internal id before recording it, and `remove` matches on the argument you give; a form that differs
from the one you typed at install time is not guaranteed to match. If a removal does not take, run
`cyrup list` and use the source string it prints.

## Updating

```sh
cyrup update --all
```

| Invocation | Effect |
|---|---|
| `cyrup update` | Self-update only |
| `cyrup update --self` | Self-update only |
| `cyrup update --extensions` | Every installed package that is not pinned |
| `cyrup update --all` | Self plus every unpinned package |
| `cyrup update --extension <source>` | One package |
| `cyrup update <source>` | One package |
| `cyrup update --force` | Reinstall cyrup even if it is already current — moot while self-update is unimplemented |

**`cyrup update` does not update cyrup itself.** The self-update path prints
`Self-update is not available in this build; update cyrup via your package manager.` Reinstall from
source to upgrade — see [Install](../getting-started/install.md).

Pinned packages are skipped by `--extensions` and `--all`. Name them with `--extension <source>` to
move them.

## Where packages land

| Scope | Registry | Working tree |
|---|---|---|
| Global (default) | `~/.cyrup/agent/packages/packages.json` | `~/.cyrup/agent/packages/packages/<id>` |
| Project (`-l`) | `<project>/.cyrup/packages.json` | `<project>/.cyrup/packages/<id>` |

The global working tree really does repeat the `packages` segment: the registry file sits at
`~/.cyrup/agent/packages/packages.json` and a cloned package's checkout sits one level below it at
`~/.cyrup/agent/packages/packages/<id>`. That is the current layout. Locally-installed packages have
no checkout there at all, since a local path is referenced where it lives.

`<id>` is a sanitised form of the source: `git:<host>/<user>/<repo>` or `path:<absolute-path>` with
every character outside `[A-Za-z0-9._-]` replaced by `-`.

**`cyrup install` does not write to `settings.json`.** The help text says it adds the package to
settings; it does not. The registry above is a separate file, and installing, removing or updating a
package touches nothing else. If you are looking for a package you installed and it is not in
`settings.json`, that is why.

## The `packages` array in settings.json

`settings.json` has its own `packages` array. It is a **separate, hand-authored channel** — you
write it yourself, `cyrup install` never touches it, and entries there are resolved independently of
the registry. Use it when you want package sources checked into a repository's
`.cyrup/settings.json` rather than installed per-machine.

Each entry is either a source string or an object:

```json
{
  "packages": [
    "git:github.com/user/tools",
    {
      "source": "git:github.com/user/big-pack",
      "extensions": ["linter"],
      "skills": [],
      "prompts": ["review"],
      "themes": []
    }
  ]
}
```

| Field | Type | Meaning |
|---|---|---|
| `source` | string | The package source, same forms as `cyrup install` |
| `autoload` | bool | Whether the package's resources load by default; default `true` |
| `extensions` | string[] | Which extensions to take from the package |
| `skills` | string[] | Which skills to take |
| `prompts` | string[] | Which prompt templates to take |
| `themes` | string[] | Which themes to take |

With `autoload` at its default, the four per-type lists are **include filters**: name the resources
you want and the rest stay out. Leaving a list empty means no filter for that type.

`"autoload": false` inverts what those lists mean. They stop being filters and become **add-back
deltas** — nothing loads unless you name it. A bare `{"source": "...", "autoload": false}` therefore
contributes nothing at all, which is occasionally what you want and more often a mistake.

See [settings.json](../reference/settings.md) for the rest of the file and how its layers merge.

## What an installed package contributes

cyrup reads a package's manifest to find its resources — `cyrup.toml`, or a `package.json` carrying
a `cyrup` key. Failing both, it falls back to convention and picks up `extensions/`, `skills/`,
`prompts/`, `themes/` and `agents/` directories at the package root.

Extensions found this way load alongside your own. `--no-extensions` drops them along with
everything else; see [How extensions work](overview.md).

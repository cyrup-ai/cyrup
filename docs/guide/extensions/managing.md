# Installing extensions

`cyrup install` fetches **packages**. A package is a repository or a directory that may contain
extensions, skills, prompt templates and themes. This page covers installing, listing, updating and
removing them, and where they end up on disk.

The CLI help uses "package" and "extension" interchangeably in places. They are not the same thing:
an [extension](overview.md) is one WebAssembly component, a package is the shipping container that
may hold several of them plus other resources. The per-command help (`cyrup install --help`,
`cyrup remove --help`) says "package" and names the package registry; the `Commands:` block of
`cyrup --help` still says "extension source" and "settings", which is the older, wrong wording — the
installer writes the registry, not `settings.json`.

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
| `git:` with a GitHub shorthand | `git:user/repo` |
| `git:` with an SSH address | `git:git@github.com:user/repo` |
| HTTPS / HTTP URL | `https://github.com/user/repo` |
| SSH URL | `ssh://git@github.com/user/repo` |
| Git protocol URL | `git://github.com/user/repo` |

**Without a `git:` prefix, only a URL carrying an explicit `https://`, `http://`, `ssh://` or
`git://` scheme is read as remote.** Everything else becomes a local path, including a bare name, a
bare `host/user/repo`, an scp-style `git@github.com:user/repo`, and a `github:user/repo` shorthand.
Those last two look remote and are not — they are stored verbatim as a path, and the install then
fails with `local package path does not exist: github:user/repo`. Put `git:` in front and both
resolve: `git:git@github.com:user/repo`, `git:user/repo`. This is upstream pi's behaviour, and the
misleading part of it is upstream's too.

**`npm:` sources are rejected.** cyrup has no JavaScript runtime, so an npm package has nothing to
run. `cyrup install npm:@scope/name` fails with an unsupported-source error, and no `npm:` example
appears in `cyrup install --help` or `cyrup remove --help`.

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

You do not have to reproduce the exact string you installed with. `remove` normalises its argument
the same way `install` did — stripping the scheme, a `git@`, a trailing `/` and a `.git` suffix, and
canonicalising a local path — so `https://github.com/user/repo.git` removes the row installed as
`git:github.com/user/repo`. The raw argument is also tried as a fallback, which is what still
removes a row written by an older build.

A miss prints `No matching package found for <source>` and exits 1. If that happens, run
`cyrup list` and use the source string it prints.

## Updating

```sh
cyrup update --extensions
```

| Invocation | Effect |
|---|---|
| `cyrup update` | Self-update only — unavailable |
| `cyrup update --self` | Self-update only — unavailable |
| `cyrup update cyrup` | Self-update only — unavailable (`self` and `pi` are the other accepted spellings) |
| `cyrup update --extensions` | Every installed package that is not pinned |
| `cyrup update --all` | Self plus every unpinned package |
| `cyrup update --models` | Refresh the remote model catalogs; updates no packages |
| `cyrup update --extension <source>` | One package |
| `cyrup update <source>` | One package |
| `cyrup update --force` | Reinstall cyrup even if it is already current — unavailable |

**`cyrup update` does not update cyrup itself.** Every invocation in that table marked *unavailable*
reaches the same stub, which writes to stderr and exits 1:

```text
error: cyrup cannot self-update this installation.
Update it with: cargo install --git https://github.com/cyrup-ai/cyrup cyrup

Location of cyrup executable: /Users/you/.cargo/bin/cyrup
```

Reinstall from source to upgrade — see [Install](../getting-started/install.md). `cyrup update
--help` says the same thing and marks those flags `(UNAVAILABLE)`.

An `update` that names no target at all — bare `cyrup update`, or `cyrup update --force`, since
`--force` selects nothing — prints one extra line on stdout before the stub:
`Extensions are skipped. Run cyrup update --extensions to update extensions.` Naming any target,
including `--self`, suppresses it.

`--all` still updates your packages first and only then hits that stub, so it does real work and
then exits 1 anyway. Use `--extensions` if you want a clean exit code.

Pinned packages are skipped by `--extensions` and `--all`. Name them with `--extension <source>` to
move them.

`--models` is the one target that has nothing to do with packages. It refreshes the remote model
catalog for every provider cyrup can find a credential for — a stored entry in `auth.json`, a
runtime key, *or* the provider's environment variable — with a 15-second bound on the whole pass,
then prints `Model catalogs refreshed`. A failure prints
`Error: …` and exits 1. It cannot be combined with `--self`, `--extensions`, `--all`, `--extension`
or a positional source, and it is answered before the project-trust check, so it works in an
untrusted repository.

## Where packages land

| Scope | Registry | Working tree |
|---|---|---|
| Global (default) | `~/.cyrup/agent/packages/packages.json` | `~/.cyrup/agent/packages/<id>` |
| Project (`-l`) | `<project>/.cyrup/packages.json` | `<project>/.cyrup/packages/<id>` |

The registry file and the checkouts share one directory: `packages.json` sits beside the `<id>`
directories rather than above them, and the two scopes have the same shape. Locally-installed
packages have no checkout at all, since a local path is referenced where it lives.

Earlier builds wrote the global working tree one level deeper, at
`~/.cyrup/agent/packages/packages/<id>` — the `packages` segment doubled. Trees left there are moved
up by a one-time migration that runs at startup and inside the package subcommands; it announces
itself once with `Migrated N installed package(s) out of <package_dir>/packages`. The registry file
path never doubled and did not move.

`<id>` is a sanitised form of the source: `git:<host>/<user>/<repo>` or `path:<absolute-path>` with
every character outside `[A-Za-z0-9._-]` replaced by `-`.

**`cyrup install` does not write to `settings.json`.** It writes only the registry above; installing,
removing or updating a package touches nothing else. If you are looking for a package you installed
and it is not in `settings.json`, that is why. (`cyrup install --help` names the package registry
and `packages.json` correctly — an older version of that help text said settings, and it was wrong.)

## The `packages` array in settings.json

`settings.json` has its own `packages` array. It is a **separate, hand-authored channel** — you
write it yourself, `cyrup install` never touches it. Use it when you want package sources checked
into a repository's `.cyrup/settings.json` rather than named on each machine's command line.

**A declared entry is never fetched for you.** cyrup performs no network install while assembling a
session, so a `git:` entry resolves only if that package is *already installed at the same scope*;
if it is not, the entry becomes a loud diagnostic — `package "…" is declared in settings but its
install location could not be resolved` — and contributes nothing. A local path resolves against
`<project>/.cyrup/` for a project entry and against `~/.cyrup/agent/` for a user entry, not against
the project root, and a local path that does not exist is skipped in silence.

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
you want and the rest stay out. Omitting a key entirely is what means "no filter" — that type keeps
everything the package ships. An **explicitly empty list disables the type outright**, so the
example above takes only the `linter` extension and only the `review` prompt, and takes no skills
and no themes at all. `[]` and a missing key are not the same thing.

`"autoload": false` inverts what those lists mean. They stop being filters and become **add-back
deltas** — nothing loads unless you name it. A bare `{"source": "...", "autoload": false}` therefore
contributes nothing at all, which is occasionally what you want and more often a mistake.

See [settings.json](../reference/settings.md) for the rest of the file and how its layers merge.

## What an installed package contributes

cyrup reads a package's manifest to find its resources, in this order: `cyrup.toml`, then a
`package.json` carrying a `pi` or a `cyrup` key (`pi` wins if both are present, so a package
published for upstream pi works unchanged). Failing all of those, it falls back to convention and
picks up `extensions/`, `skills/`, `prompts/`, `themes/` and `agents/` directories at the package
root.

Extensions found this way load alongside your own. `--no-extensions` drops them along with
everything else; see [How extensions work](overview.md).

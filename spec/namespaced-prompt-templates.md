# Namespaced prompt templates

The decision of record for cyrup's one deliberate divergence from pi's prompt-template loader,
tracked in the gap ledger as `CFG-077` (`docs/gap-analysis/05-cyrup-config-and-resources.md`).
Every `[CYRUP-DELTA]` in `crates/cyrup-resources` that cites this file points at a section below.

## 1. What pi does

`packages/coding-agent/src/core/prompt-templates.ts` @v0.87.1: `loadTemplatesFromDir` is
documented "Scan a directory for .md files (non-recursive)". It reads one `readdirSync` level, and
`loadTemplateFromFile` names each template `basename(filePath).replace(/\.md$/, "")`. A
subdirectory under a prompt root contributes nothing, and two files with the same basename in
different subdirectories are indistinguishable.

## 2. Why cyrup differs

`crates/cyrup-flux` ships its prompts as a DIRECTORY, `<resources>/prompts/flux/*.md`, so they
register as `/flux/new`, `/flux/aug`, … (`crates/cyrup-flux/src/extension.rs`: "This one line is
why `/flux/new` is `/flux/new` and not `/new`"). Under pi's rule that root's only child is the
directory `flux/`, so the fifteen `/flux/*` commands would not flatten — they would vanish. The
semantics follow code-puppy's `_command_name_from_path` and `_is_in_skipped_namespace`
(`customizable_commands/register_callbacks.py`).

A parity sweep must not "restore" the flat scan.

## 3. Rules

### 3.1 Name derivation

1. The name is the template's path relative to its scan root, `.md` stripped from the leaf,
   components joined with `/`, case preserved (`PromptTemplate::load_with_root`,
   `crates/cyrup-resources/src/prompt.rs`). `flux/new.md` under a root is `/flux/new`.
2. A non-UTF-8 component (leaf or intermediate directory) or an empty leaf stem (a file named
   `.md`) is a load error, reported as a prompt warning; the stem comes from the file name with
   `.md` stripped, not `Path::file_stem`.
3. `/name` matching is case-sensitive on the derived name.

### 3.2 Skip rules

No descent into a directory whose name starts with `.` or `_`, or is `node_modules`, at any depth.
The rules apply to directory names only; a file's name is never skipped. The `_` rule is what keeps
flux's `_docs/*.md` from registering as commands.

### 3.3 Depth and symlinks

- The root is depth 0. A directory at depth 8 is still scanned; each of its subdirectories is
  refused with one `namespace depth exceeds 8` prompt warning (`MAX_PROMPT_NAMESPACE_DEPTH`).
- Entries are classified with `symlink_metadata`. Directory symlinks are never followed, which
  makes the walk cycle-proof by construction. A file symlink loads when its target is a regular
  `.md` file, as pi's own symlink handling does.
- Children are sorted per directory, so first-wins tie-breaking is deterministic.

### 3.4 Scan channels

All four directory-scan call sites use the recursive scan (`scan_prompt_root`): the global
`<agent_dir>/prompts` root, each package manifest's prompt directory, the project
`.cyrup/prompts` root, and the directory arm of an explicitly added prompt path (the one
extensions reach).

### 3.5 Keys and precedence

The registry key is the lower-cased name, so `flux/new` and `FLUX/NEW` collide and the
higher-precedence scope wins, keeping its own case and body. `flux/new` and `flux-new` are
distinct keys and coexist.

## 4. Loading

### 4.1 Single-file loads

`PromptTemplate::load` relativizes a file against its own parent, which reproduces pi's basename
name exactly for every caller that has no meaningful root.

### 4.2 Load failures

A template that fails to load — unreadable, a bad name (§3.1), or front matter whose YAML does not
parse (pi's `loadTemplateFromFile`, `CFG-083`) — is not registered and becomes one prompt warning
naming its path; the scan continues.

## 7. Compatibility

A template directly under a root keeps its flat basename, so every pi-shaped prompt directory
loads with pi's names unchanged. Only subdirectories, which pi ignores, gain namespaced names.

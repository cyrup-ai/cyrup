---
title: Pi's third compact read header kind docs is not implemented
priority: LOW
tool: read
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: aug
status: in-progress
updated: 2026-08-27
---

# Pi's third compact read header kind docs is not implemented

## What pi does

read.ts:38-41 declares `kind: "docs" | "resource" | "skill"`; read.ts:104-121 `getPiDocsClassification` resolves the read path against `dirname(getReadmePath())` and returns `{kind:"docs", label}` when the file is `README.md` or lives under `docs/` or `examples/`; read.ts:136-137 consults it before the resource set, and read.ts:161-167 renders `read docs <label> (<key> to expand)` for a collapsed call.

## What cyrup-tools does

/home/user/cyrup/crates/cyrup-tui/src/transcript/tool_args.rs:152-190 `compact_read_classification` implements only the `SKILL.md` branch (:170-180) and the `COMPACT_RESOURCE_FILE_NAMES` branch (:181-189), returning `None` otherwise. The omission is stated explicitly at tool_args.rs:145-148 ("The `docs` arm is the one piece that cannot be ported here"). `rg 'read docs' crates/` finds no other implementation.

## User-visible impact

A collapsed read of pi's own `README.md` / `docs/…` / `examples/…` shows the compact `read docs <relative path>` header upstream; cyrup shows the generic `read <path>` header instead.

## Parity action

Add a package-root accessor (the cyrup analogue of `getReadmePath()`) to the TUI's render context and port `getPiDocsClassification` as a third branch of `compact_read_classification`, checked between the SKILL.md and resource branches, rendering `read docs <label>`.

## Why this gap is real

An adversary agent was tasked with **refuting** this finding by locating the capability in the Rust under another name. It could not:

> Confirmed absent after exhaustive search. compact_read_classification (crates/cyrup-tui/src/transcript/tool_args.rs:152-191) implements only the `skill` (SKILL.md, :170-180) and `resource` (COMPACT_RESOURCE_FILE_NAMES, :181-189) arms and returns None otherwise; its only caller (crates/cyrup-tui/src/transcript/tool_builtin.rs:15-19) then falls through to the generic `read <path>` header. The prerequisite is missing too: no getReadmePath/getPackageDir counterpart exists anywhere in crates/ (searches for readme_path, getReadmePath, package_root, packageRoot, current_exe hit only the package-install store in cyrup-resources/src/package/store.rs and doctor diagnostics). The nearest relative, DocsPointers (crates/cyrup-session/src/prompt/builder.rs:19-28, readme/docs/examples), belongs to the system-prompt progressive-disclosure section, not to read-header classification, and builder.rs:355-358 states the sole production caller still passes DocsPointers::default() (SESS-035) with the path helpers unimplemented (cyrup-config/src has no readme/docs path resolution at all). cyrup-tools/src contains no read-header rendering, so there is no alternate implementation there either. Severity corrected down: nothing is silently wrong — the same file is read and the same content rendered; only the collapsed one-line header label differs, and only for files inside the agent's own shipped package tree, which a Rust binary does not ship, so the branch is effectively unreachable even if ported. Purely cosmetic.

## Definition of done

1. The capability described under *Parity action* is implemented in `crates/cyrup-tools`.
2. A test pins the new behaviour against the pi semantics quoted above.
3. `cargo check --workspace --all-targets` and `cargo clippy` stay clean.
4. Behaviour that pi does NOT have is not introduced — this is a parity task, not a redesign.

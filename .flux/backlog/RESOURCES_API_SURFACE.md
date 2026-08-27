---
stage: new
status: done
updated: 2026-08-22 23:08
---

# Reconcile cyrup-resources Public API Surface With Its Actual Consumers

**Owns files:** `crates/cyrup-resources/src/lib.rs`, `crates/cyrup-resources/src/package/manifest.rs`,
`crates/cyrup-resources/src/theme.rs`

> Two sub-items also touch `src/discovery.rs`, which another wave-1 task owns. Defer those two to the
> discovery task or to a follow-up — do not edit `discovery.rs` from here.

## Description

Four places where the exported surface and the documented story disagree with reality. All were
verified by workspace-wide grep.

### 1. `ResourceHandle` has no consumer, but `lib.rs` presents it as the live mechanism

`grep -rn -w ResourceHandle crates/ xtask/` outside this crate returns **zero hits**. Inside, the
only non-definition use is one unit test. Every real holder stores `Arc<ResourceRegistry>` directly
(`cyrup-tui/src/theme_access.rs`, `cyrup-tui/src/app/extension_ui.rs`, `cyrup-session-svc`).

`lib.rs:12-13` and `:73-74` describe it as the `/reload` swap path, which reads as a description of
what the system does rather than what it offers.

**Fix — keep it, correct the docs.** State that it is the R-09-023 swap primitive offered to
embedders, and that in-tree consumers currently hold `Arc<ResourceRegistry>` directly. Deleting a
documented public primitive is an API break for a stated architectural requirement; that is a design
decision, not hygiene. Do not delete it.

### 2. `CyrupManifest` / `PackageMeta` are `pub` but purely internal

`grep -rn -wE 'CyrupManifest|PackageMeta'` across the workspace returns **4 lines, all inside
`manifest.rs`**: the two definitions, one field, one `toml::from_str` target. They are reachable as
`cyrup_resources::package::manifest::CyrupManifest` for no reason. Their sibling `PiPackageJson`
(`:73`) is already private — these two are the inconsistency.

**Fix:** drop `pub` from `manifest.rs:61` (`CyrupManifest`) and `:15` (`PackageMeta`). Keep the
`package: PackageMeta` field — removing it would make `[package]` optional in `cyrup.toml`, which is
a behavior change.

### 3. `ManifestKind` is not re-exported at the crate root

`lib.rs:54-59` re-exports `ResolvedManifest`, whose `pub kind: ManifestKind` field type is **not**
nameable at the crate root — only at `cyrup_resources::package::ManifestKind`. A root-exported
struct with a non-root-nameable field type is an awkward surface.

**Fix:** add `ManifestKind` to the `pub use package::{...}` list in `lib.rs`.

### 4. `Theme::resolve_export` / `ExportColors` have exactly one caller, a test

Verified: definitions at `theme.rs:271,377,385`, the `lib.rs:67` re-export, and one call in
`src/tests/resources/themes.rs:269`. The arch-12 HTML-export consumer is not in tree.

**Fix:** extend the doc comment at `theme.rs:375-376` to say the consumer is pending and the only
current caller is the test, so a reader does not hunt for a caller that does not exist.

## Acceptance Criteria

- [ ] `ResourceHandle` retained; its two doc claims say what it offers, not what consumers do
- [ ] `CyrupManifest` and `PackageMeta` are private; `cargo check -p cyrup-resources --all-targets` clean
- [ ] `cyrup_resources::ManifestKind` resolves from the crate root
- [ ] `Theme::resolve_export` doc names its pending-consumer status
- [ ] `cargo test --workspace` shows no new failures

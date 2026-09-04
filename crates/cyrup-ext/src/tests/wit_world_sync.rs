//! The `cyrup:ext` WIT world has TWO on-disk copies — `crates/cyrup-ext/wit/world.wit` (consumed by
//! the host's `wasmtime::component::bindgen!`) and `crates/cyrup-ext-sdk/wit/world.wit` (consumed by
//! the guest's `wit-bindgen`). Nothing in the build enforces that they agree: if they drift, the host
//! links against one shape and the guest exports another, and the failure surfaces as a raw wasmtime
//! instantiation error at test time rather than a compile error.
//!
//! This is that enforcement — and, since EXT-028, the enforcement of the WORLD VERSION too. Comparing
//! the two copies to each other proves nothing about versions: `f777e44` RE-SIGNED the
//! `events.on-tool-result` export (adding `usage-json`) in BOTH copies, byte-identically, without
//! bumping `HOST_WORLD`. That left a pre-`f777e44` guest declaring the still-current `cyrup:ext@0.2`
//! passing `ExtensionManifest::check_world` and then dying inside wasmtime with an opaque link error
//! — exactly the failure the version gate exists to turn into a typed `ExtError::WorldVersion`.
//! So the tests below tie `HOST_WORLD` to the `package cyrup:ext@…` line of both copies, and tie the
//! header's event-count claim to the exports actually declared.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};

fn host_wit() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("wit/world.wit")
}

fn guest_wit() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../cyrup-ext-sdk/wit/world.wit")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Extract the `package cyrup:ext@MAJOR.MINOR.PATCH;` declaration.
fn package_version(src: &str, path: &Path) -> String {
    src.lines()
        .find_map(|l| {
            Some(
                l.trim()
                    .strip_prefix("package ")?
                    .trim_end_matches(';')
                    .trim(),
            )
        })
        .unwrap_or_else(|| panic!("no `package ...;` line in {}", path.display()))
        .to_string()
}

#[test]
fn the_host_and_guest_wit_world_copies_are_identical() {
    let host = host_wit();
    let guest = guest_wit();
    let host_src = read(&host);
    let guest_src = read(&guest);

    if host_src != guest_src {
        let first_diff = host_src
            .lines()
            .zip(guest_src.lines())
            .enumerate()
            .find(|(_, (a, b))| a != b)
            .map(|(i, (a, b))| format!("line {}:\n  host : {a}\n  guest: {b}", i + 1))
            .unwrap_or_else(|| {
                format!(
                    "line counts differ: host {} vs guest {}",
                    host_src.lines().count(),
                    guest_src.lines().count()
                )
            });
        panic!(
            "the host and guest WIT world copies have drifted — change BOTH:\n  {}\n  {}\n{first_diff}",
            host.display(),
            guest.display()
        );
    }
}

/// EXT-028, the durable half: `HOST_WORLD` and the `package` line move TOGETHER, in BOTH copies.
///
/// `HOST_WORLD` is `cyrup:ext@MAJOR.MINOR`; the WIT package line carries a full semver
/// `cyrup:ext@MAJOR.MINOR.PATCH`. The gate compares MAJOR+MINOR, so those are what must agree.
#[test]
fn host_world_matches_the_wit_package_version_in_both_copies() {
    for path in [host_wit(), guest_wit()] {
        let declared = package_version(&read(&path), &path);
        let (pkg, ver) = declared.split_once('@').unwrap_or_else(|| {
            panic!(
                "`package {declared};` is not `name@version` in {}",
                path.display()
            )
        });
        let mut parts = ver.split('.');
        let major = parts.next().unwrap_or("");
        let minor = parts.next().unwrap_or("");
        let major_minor = format!("{pkg}@{major}.{minor}");

        assert_eq!(
            major_minor,
            crate::HOST_WORLD,
            "{} declares `package {declared};` but the host gate is {} — ANY change to an EXPORT \
             (added, removed, or RE-SIGNED) must bump BOTH, or a guest built against the old world \
             passes `check_world` and then fails inside wasmtime with a raw link error (EXT-028)",
            path.display(),
            crate::HOST_WORLD,
        );
    }
}

/// EXT-028's named residual: line 1 of each copy is a `// cyrup:ext@MAJOR.MINOR.PATCH` marker, and
/// it is a SECOND declaration of the version that no test tied to anything.
///
/// That is exactly how the original defect happened: both copies sat at `// cyrup:ext@0.3.0` while
/// their `package` line read `0.4.0`, through two consecutive bumps, because the existing tie test
/// parses only the `package` line. A stale marker is the first thing a reader of the world sees, so
/// it is the first thing that misleads them about which world they are editing.
#[test]
fn the_header_version_marker_matches_the_package_line_in_both_copies() {
    for path in [host_wit(), guest_wit()] {
        let src = read(&path);
        let declared = package_version(&src, &path);
        let marker = src
            .lines()
            .next()
            .and_then(|l| l.trim().strip_prefix("//"))
            .map(|rest| rest.split_whitespace().next().unwrap_or("").to_string())
            .unwrap_or_else(|| panic!("no leading `// cyrup:ext@…` marker in {}", path.display()));
        assert_eq!(
            marker,
            declared,
            "{} opens with `// {marker}` but declares `package {declared};` — the marker is a \
             second copy of the version and must move with it (EXT-028)",
            path.display(),
        );
    }
}

/// EXT-028, the header half: the `// … exports N `on-*` event hooks` claim in the world's own
/// preamble is checked against the exports actually declared, so it cannot rot the way the old
/// "30-event catalog" line did (the `events` interface has long declared 31).
#[test]
fn the_header_event_count_matches_the_declared_event_exports() {
    let path = host_wit();
    let src = read(&path);

    // The `events` interface body, from its opening line to the first column-0 `}`.
    let body: String = src
        .lines()
        .skip_while(|l| !l.starts_with("interface events {"))
        .take_while(|l| *l != "}")
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !body.is_empty(),
        "no `interface events {{` block in {}",
        path.display()
    );

    let declared = body
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            l.starts_with("  ") && t.starts_with("on-") && t.contains(':')
        })
        .count();

    let claimed: usize = src
        .lines()
        .find_map(|l| {
            let (_, rest) = l.split_once("interface exports ")?;
            rest.split_whitespace().next()?.parse().ok()
        })
        .unwrap_or_else(|| {
            panic!(
                "no `The `events` interface exports N …` claim in {}",
                path.display()
            )
        });

    assert_eq!(
        claimed,
        declared,
        "the world header claims {claimed} `on-*` event exports but {} declares {declared}",
        path.display(),
    );
}

/// EXT-028, the cache half: the Tier-1 artifact key folds a compile-time fingerprint of the ABI
/// sources that `hash_source_tree` cannot see — both `world.wit` copies and the `cyrup-ext-sdk`
/// guest crate. Recomputing it from the same files with the same hasher proves the baked-in value
/// actually tracks them; before EXT-028 there was no fingerprint at all and an SDK/WIT edit left the
/// key untouched, so a rebuild could serve a component built against the old world from cache.
#[test]
fn the_cache_key_tracks_the_wit_and_sdk_sources_outside_the_extension_crate() {
    let crates_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/ is the parent of cyrup-ext/")
        .to_path_buf();

    let files = crate::build::abi::abi_source_files(&crates_dir);
    for expected in ["cyrup-ext/wit/world.wit", "cyrup-ext-sdk/wit/world.wit"] {
        assert!(
            files.iter().any(|f| f.ends_with(expected)),
            "{expected} is an ABI source and must be fingerprinted; got {files:?}"
        );
    }
    assert!(
        files
            .iter()
            .any(|f| f.ends_with("cyrup-ext-sdk/src/guest.rs")),
        "the cyrup-ext-sdk guest crate must be fingerprinted; got {files:?}"
    );

    let recomputed = crate::build::abi::hash_abi_sources(&crates_dir);
    assert_eq!(recomputed.len(), 64, "blake3 hex is 64 chars");
    assert_eq!(
        crate::build::ABI_FINGERPRINT,
        recomputed,
        "the ABI fingerprint baked in by build.rs is stale — an edit to a `world.wit` copy or to \
         cyrup-ext-sdk did not bust the Tier-1 artifact cache (EXT-028)"
    );

    let id = crate::build::world_abi_id();
    assert!(
        id.starts_with(crate::HOST_WORLD),
        "the world identity leads with HOST_WORLD: {id}"
    );
    assert!(
        id.ends_with(&recomputed),
        "the world identity carries the ABI fingerprint: {id}"
    );
}

// ---------------------------------------------------------------------------
// EXT-072 / EXT-073 — the citation lint.
//
// Under this project's rules an in-tree `pi` citation IS the evidence that a port matches upstream,
// so a citation that resolves to an unrelated line is worth less than none — and worse than none
// when it resolves to a PLAUSIBLE line, which every one of EXT-072's neighbour-doc offsets did.
// EXT-036 asked for this guard, EXT-072 asked again, and both times the citations were rewritten by
// hand with nothing left behind to keep them rewritten: `:1257-1266` was named by name in EXT-036's
// write-up, fixed in the `.rs` sites, and left standing in the WIT for two more sweeps.
//
// These two tests are the durable half, and they are deliberately OFFLINE — a test that needs a pi
// checkout skips in CI, and a skipped guard is not a guard.
// ---------------------------------------------------------------------------

/// Every ABI-adjacent file that carries pi citations. `world.wit` is checked in BOTH copies because
/// they are byte-identical by test and a fix applied to one only would pass every other pin here.
fn cited_files() -> Vec<PathBuf> {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut files = vec![
        crate_dir.join("wit/world.wit"),
        crate_dir.join("../cyrup-ext-sdk/wit/world.wit"),
        crate_dir.join("src/host/services.rs"),
        crate_dir.join("src/host/live.rs"),
        crate_dir.join("src/event.rs"),
        crate_dir.join("src/native.rs"),
        crate_dir.join("src/registry.rs"),
    ];
    // The SDK's whole `src` tree, WALKED rather than named. `ctx/` was already enumerated for this
    // reason — one submodule per WIT import interface, "so a later submodule cannot fall outside
    // this lint by being added and not listed here" — and the argument is the same one level up.
    // Naming `api.rs` by hand is what left the rest of the SDK invisible to both lint bodies:
    // `rg -c -e 'types\.ts:' -e 'loader\.ts:' -e 'runner\.ts:' -e '@v0\.83\.0' \
    // crates/cyrup-ext-sdk/src` puts 69 cited lines in `events.rs` against 45 in `api.rs`, and
    // EXT-073's `session_info_changed … subscribed at :1203` — corrected in `api.rs`, `event.rs`
    // and both `world.wit` copies — survived in `events.rs` alone precisely because that one file
    // was not on the list.
    let sdk_src = crate_dir.join("../cyrup-ext-sdk/src");
    let mut sdk: Vec<PathBuf> = Vec::new();
    collect_rs(&sdk_src, &mut sdk);
    // Non-vacuity as a COUNT floor, the shape `cyrup-ext-sdk/src/tests/world_import_coverage.rs`
    // uses (`scanned >= 13`): a walk that comes back empty or truncated — a moved directory, a
    // `read_dir` that yields no `.rs` — satisfies both scans below trivially, so this count is the
    // only evidence the lint looked at anything at all. The walk found 29 `.rs` files when this was
    // written — 13 under `ctx/`, 11 at the top level, 5 under `tests/` — and the floor sits one
    // under that, low enough that a single test module coming or going is not a false red, high
    // enough that losing any DIRECTORY of the SDK's citation surface lands well below it.
    assert!(
        sdk.len() >= 28,
        "the `cyrup-ext-sdk/src` walk found only {} `.rs` file(s) under {} — the citation lint \
         would be blind to most of the SDK's citation surface",
        sdk.len(),
        sdk_src.display()
    );
    sdk.sort();
    files.append(&mut sdk);
    files
}

/// Every `*.rs` under `dir`, recursively: `cyrup-ext-sdk/src` nests (`ctx/`, `tests/`), so a
/// single-level `read_dir` would re-create the blind spot one directory down.
fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
            .path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().is_some_and(|x| x == "rs") {
            out.push(path);
        }
    }
}

/// A gap-analysis id on the SAME line marks a deliberate quotation of a struck value — the
/// "do-not-restore" note. Anything else carrying one of these numbers is a live citation.
const CORRECTIVE_MARKERS: &[&str] = &[
    "EXT-036", "EXT-048", "EXT-060", "EXT-072", "EXT-073", "EXT-074",
];

/// Values re-derived against `v0.83.0` this pass and found to resolve to an unrelated symbol. Each
/// entry is `(the citation text, what that line actually is upstream)`; the second half is not
/// asserted on, it is the reason, kept here so a reader who hits this test learns why rather than
/// deleting the entry.
const STRUCK_CITATIONS: &[(&str, &str)] = &[
    (
        "types.ts:1145",
        "blank line (tool_call subscribes at :1228)",
    ),
    (
        "types.ts:1146",
        "`MessageRenderer` (tool_result subscribes at :1229)",
    ),
    (
        "types.ts:1144",
        "a closing brace (context subscribes at :1207)",
    ),
    (
        "types.ts:1143",
        "`expanded: boolean` (message_end subscribes at :1222)",
    ),
    (
        "types.ts:1135",
        "blank line (before_agent_start subscribes at :1214)",
    ),
    (
        "types.ts:1158",
        "the Command Registration banner (input subscribes at :1231)",
    ),
    (
        "types.ts:1159",
        "the Command Registration banner (user_bash subscribes at :1230)",
    ),
    (
        "types.ts:1160",
        "a banner rule (before_provider_request subscribes at :1209)",
    ),
    (
        "types.ts:1161",
        "blank line (after_provider_response subscribes at :1213)",
    ),
    (
        "types.ts:1108",
        "`cancel?: boolean` (getArgumentCompletions is :1166)",
    ),
    (
        "types.ts:1109",
        "`skipConversationRestore` (RegisteredCommand.handler is :1167)",
    ),
    (
        "types.ts:1105",
        "a closing brace (registerCommand is :1247)",
    ),
    (
        "types.ts:218",
        "`getEditorText`'s doc line (addAutocompleteProvider is :225)",
    ),
    (
        "types.ts:1373",
        "an `@example` JSDoc line inside `registerProvider`",
    ),
    (
        "types.ts:1257",
        "wrong surface; getActiveTools/getAllTools/setActiveTools/getCommands are :1320-:1329",
    ),
    (
        "types.ts:117",
        "a `WorkingIndicatorOptions` doc line (AutocompleteProviderFactory is :124)",
    ),
    (
        "types.ts:1337",
        "blank line (registerProvider(name, config) is :1401)",
    ),
    // EXT-074's own citations, struck by the verification pass that closed it. `:342-345` and
    // `:352-354` correspond to NO tag: at v0.83.0 they are the `getAllTools` and `getCommands`
    // bindings, at v0.82.1 `setModel`/`setThinkingLevel` are :359/:369 and at v0.84.x they are
    // :383/:393. The claim they support (pi gates neither) is correct; only the lines were wrong.
    (
        "loader.ts:342-345",
        "the `getAllTools` binding (`setModel` is :359-362 @v0.83.0)",
    ),
    (
        "loader.ts:352-354",
        "the `getCommands` binding (`setThinkingLevel` is :369-372 @v0.83.0)",
    ),
];

/// EXT-072/EXT-073: a struck citation may appear ONLY on a line that also names the item that
/// struck it — i.e. as a do-not-restore note, never as a live citation.
///
/// Pre-fix this is RED at nine sites in each `world.wit` copy alone: `world.wit:296` read
/// "`tool_call [block/mutate] (types.ts:1145), tool_result [mutate] (types.ts:1146)`" with no
/// marker on the line, and `:1145` is a blank line at `v0.83.0` — the fabricated `:1135-1161` band
/// EXT-073 filed, which lies entirely outside pi's `on(event: …)` overload block.
#[test]
fn no_struck_pi_citation_is_restored_as_a_live_citation() {
    let mut live: Vec<String> = Vec::new();
    for path in cited_files() {
        for (n, line) in read(&path).lines().enumerate() {
            for (cite, actual) in STRUCK_CITATIONS {
                if line.contains(cite) && !CORRECTIVE_MARKERS.iter().any(|m| line.contains(m)) {
                    live.push(format!(
                        "{}:{} cites `{cite}`, which at v0.83.0 is {actual}\n    {}",
                        path.display(),
                        n + 1,
                        line.trim(),
                    ));
                }
            }
        }
    }
    assert!(
        live.is_empty(),
        "{} struck pi citation(s) are live again — re-derive against the tag before citing it, and \
         if you are quoting a struck value on purpose, name the item that struck it on the same \
         line (EXT-072/EXT-073):\n{}",
        live.len(),
        live.join("\n"),
    );
}

/// EXT-073, the class rather than the instances. pi's `on(event: "…")` overload block occupies
/// `extensions/types.ts:1190-1231` @v0.83.0 — 33 overloads, the count the header test pins — and
/// every "subscribed at" citation in the tree names one of them. So the citation is checkable
/// WITHOUT a pi checkout: the line number determines which event upstream subscribes there, and that
/// event must be the one the comment is about.
///
/// The map below was re-derived line by line at `v0.83.0` this pass (EXT-073 records the same
/// derivation). It is the whole reason a fabricated subscription line is a different defect from a
/// stale one: staleness moves every citation by one offset, so the map still resolves in order,
/// while `agent_settled … subscribed at :1225` resolves to `tool_execution_end` — a real overload,
/// eight events away, which is why it read as plausible through nine sweeps.
///
/// Pre-fix this is RED at eight sites (measured), all of them INSIDE the band and therefore
/// invisible to a plain range check: `agent_settled … :1225` — upstream `tool_execution_end` — in
/// both `world.wit` copies, `api.rs:51` and `event.rs:50`; and `session_info_changed … :1203` —
/// upstream `session_compact` — in both copies, `api.rs:58` and `event.rs:61`.
const PI_EVENT_SUBSCRIPTION_LINES: &[(&str, u32)] = &[
    ("project_trust", 1190),
    ("resources_discover", 1191),
    ("session_start", 1192),
    ("session_info_changed", 1193),
    ("session_before_switch", 1195),
    ("session_before_fork", 1198),
    ("session_before_compact", 1200),
    ("session_compact", 1203),
    ("session_shutdown", 1204),
    ("session_before_tree", 1205),
    ("session_tree", 1206),
    ("context", 1207),
    ("before_provider_request", 1209),
    ("before_provider_headers", 1212),
    ("after_provider_response", 1213),
    ("before_agent_start", 1214),
    ("agent_start", 1215),
    ("agent_end", 1216),
    ("agent_settled", 1217),
    ("turn_start", 1218),
    ("turn_end", 1219),
    ("message_start", 1220),
    ("message_update", 1221),
    ("message_end", 1222),
    ("tool_execution_start", 1223),
    ("tool_execution_update", 1224),
    ("tool_execution_end", 1225),
    ("model_select", 1226),
    ("thinking_level_select", 1227),
    ("tool_call", 1228),
    ("tool_result", 1229),
    ("user_bash", 1230),
    ("input", 1231),
];

#[test]
fn every_subscribed_at_citation_names_the_event_pi_subscribes_on_that_line() {
    let mut wrong: Vec<String> = Vec::new();
    for path in cited_files() {
        let src = read(&path);
        let lines: Vec<&str> = src.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let Some(rest) = line.split("subscribed at").nth(1) else {
                continue;
            };
            let next = lines.get(i + 1).copied().unwrap_or_default();
            // A wrapped citation puts the number on the continuation line.
            let joined = if rest.trim().is_empty() || !rest.contains(':') {
                format!("{rest} {next}")
            } else {
                rest.to_string()
            };
            let Some(num) = joined
                .split(':')
                .nth(1)
                .map(|t| {
                    t.chars()
                        .take_while(char::is_ascii_digit)
                        .collect::<String>()
                })
                .and_then(|d| d.parse::<u32>().ok())
            else {
                continue;
            };
            // The comment the citation belongs to: the line itself plus one either side, which is as
            // far as any of these wrap.
            let window = format!(
                "{} {line} {next}",
                lines.get(i.saturating_sub(1)).copied().unwrap_or_default()
            );
            match PI_EVENT_SUBSCRIPTION_LINES.iter().find(|(_, l)| *l == num) {
                Some((event, _)) if window.contains(event) => {}
                Some((event, _)) => wrong.push(format!(
                    "{}:{} cites `:{num}`, where pi subscribes `{event}` — which this comment is not about\n    {}",
                    path.display(),
                    i + 1,
                    line.trim(),
                )),
                None => wrong.push(format!(
                    "{}:{} cites `:{num}`, which is not one of pi's 33 `on(event: …)` overload lines \
                     (extensions/types.ts:1190-1231 @v0.83.0)\n    {}",
                    path.display(),
                    i + 1,
                    line.trim(),
                )),
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "{} `subscribed at` citation(s) name the wrong pi overload. Re-derive against the tag — a \
         subscription line that resolves to a DIFFERENT event is the fabrication class EXT-073 \
         filed, not a stale offset:\n{}",
        wrong.len(),
        wrong.join("\n"),
    );
}

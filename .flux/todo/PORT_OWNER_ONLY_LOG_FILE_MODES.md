---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Restrict the log file and logs directory to owner-only mode

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | high |
| **Kind** | partial |
| **Upstream area** | logging, redaction and log hygiene |
| **Verification** | **SINGLE-SOURCE — not adversarially verified.** The verifier for this area died on a session limit mid-run. Re-check the port before starting work: the claim may be a false positive if the capability exists under another name. |

## What upstream does that the port does not

Upstream creates every log line with mode 0600, chmods an inherited log file to 0600 once per
session, and mkdirs the logs directory with 0700 plus an explicit tighten; the port creates both
with default umask permissions (typically 0644 / 0755).

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

/home/user/cyrup/tmp/pi-packages/packages/pi-permission-system/src/log-file-permissions.ts:12-33
(OWNER_ONLY_FILE_MODE = 0o600, OWNER_ONLY_DIRECTORY_MODE = 0o700, restrictExistingPathToOwner);
applied at logging.ts:76-83 (`appendFileSync(path, line, {mode: OWNER_ONLY_FILE_MODE})` plus the
per-session `hardened` set calling `restrictExistingPathToOwner`) and at extension-
config.ts:110-124 (`mkdirSync(logsDir, {recursive: true, mode: OWNER_ONLY_DIRECTORY_MODE})` +
explicit tighten)

**Port** (`crates/cyrup-permission-system`):

/home/user/cyrup/crates/cyrup-permission-system/src/logging.rs:223-227 opens the log with
`OpenOptions::new().create(true).append(true)` — no `.mode(...)` and no chmod, and no `hardened`
set exists; /home/user/cyrup/crates/cyrup-permission-system/src/logging.rs:89-97
`ensure_logs_directory` is a bare `std::fs::create_dir_all(logs_dir)` with no mode. `rg -n
"OWNER_ONLY_FILE_MODE|OWNER_ONLY_DIRECTORY_MODE|restrict_existing_path_to_owner" <src>` returns
nothing. The primitive exists but is only used for the forwarding spool (forwarding.rs:222-234
`set_restrictive_mode`, forwarding.rs:264 0o700, forwarding.rs:402/412/414 0o600), so the omission
on the log path is silent rather than a design choice.

## Why it matters

On a multi-user host the permission trail — which records full bash command strings, file paths
and raw tool input — is world-readable, and an installation that predates any future hardening
keeps its loose mode because there is no chmod path. This is exactly the exposure the port already
defends against for forwarding files, left open on the file that accumulates far more sensitive
text.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.

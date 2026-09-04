#!/usr/bin/env python3
"""
count_open_items.py — mechanical census of docs/gap-analysis's area files.

Implements README.md's "If you need a number, derive it with a committed
script" rule and PARITY-GAPS.md §7's counting method: "every row of every
`## Open items` table tallied by `Kind` and `Severity`, with `tracker` rows
excluded by construction."

WHAT IT PARSES
    Each area file's markdown table under its single `## Open items` heading
    (the only current table per file — the "prior analyses" table some files
    also carry is explicitly non-current and is never touched by this
    script, because it is never inside the `## Open items` .. next `## `
    span). Table format, uniform across areas 01-12/14:

        | ID | Severity | Kind | Effort | Title |
        |---|---|---|---|---|
        | PROV-014 | medium | parity-bug | M | ... |
        | ~~PROV-030~~ | ~~high~~ **CLOSED 2026-08-14** | not-ported | L | ... |

    Area 12 carries one extra `Dedup` column between Kind and Effort; the
    header row is read to find each column by name, not by fixed index.

    Area 09a (`09a-cyrup-ext-subagents-v0.57-drift.md`) has no `## Open
    items` heading and no `Kind` column — it is a supplement to area 09 with
    its own ids, structured as a `## Summary — confirmed items` table
    (`ID | Sev | Eff | Subsystem | Title`) plus a `## Carried — NOT
    adversarially verified` prose list. It is parsed by a dedicated,
    smaller routine; every 09a open item is classified `upstream-drift`
    (Version lag) by convention, because the file's whole premise is drift
    against a later `pi-subagents` tag than the one cyrup ported — recorded
    here so the classification is not re-derived by hand next time.

CLOSED-ROW DETECTION
    A row counts as CLOSED if its Severity cell contains a struck span
    (`~~...~~`) — this is the one reliable signal: some rows strike only
    the ID, some strike only the Severity, a few (inconsistently) strike
    both, but every closed row in every file strikes the severity word.
    A row whose Severity cell narrates "PARTIALLY CLOSED" or "RESIDUAL"
    without actually striking the severity token stays OPEN — the item
    still has real remaining work, which is exactly what that annotation
    means.

TRACKER / EXCLUDED-ROW DETECTION
    A row is excluded from every tally (open AND closed, per README's
    "Reading the area tables") when its Severity cell, once struck-through
    markup is stripped, is the literal marker `tracker` (any case, with or
    without surrounding `**`) or the literal parenthetical
    `*(partially-closed)*`. These rows keep their id and body in the area
    file; they are not part of the arithmetic in this script's output
    either.

OUTPUT
    Prints a per-area table (open / crit / high / med / low / trackers) and
    a gap-class table (Port bug / Version lag / Reverse lag / Test defect /
    Invented surface / Tooling), each restricted to OPEN, non-tracker rows,
    plus a flat list of every open critical/high row for §0a. Kind values
    this script does not recognise are reported under UNCLASSIFIED rather
    than silently folded into a bucket, so a new Kind string does not
    silently corrupt the census.

USAGE
    python3 scripts/count_open_items.py            # human-readable report
    python3 scripts/count_open_items.py --json      # machine-readable dump
Run from docs/gap-analysis/, or pass --dir to point elsewhere.
"""
import argparse
import json
import os
import re
import sys

STANDARD_AREAS = [
    ("01", "01-cyrup-core-and-provider.md"),
    ("02", "02-cyrup-agent.md"),
    ("03", "03-cyrup-session.md"),
    ("04", "04-cyrup-tools.md"),
    ("05", "05-cyrup-config-and-resources.md"),
    ("06", "06-cyrup-ext.md"),
    ("07", "07-cyrup-tui.md"),
    ("08", "08-cyrup-session-svc-and-modes.md"),
    ("09", "09-cyrup-ext-subagents.md"),
    ("10", "10-cyrup-permission-system.md"),
    ("11", "11-cyrup-intercom.md"),
    ("12", "12-upstream-drift-pi-core.md"),
    ("14", "14-cyrup-flux.md"),
]
AREA_09A = ("09a", "09a-cyrup-ext-subagents-v0.57-drift.md")

SEVERITIES = ["critical", "high", "medium", "low"]

KIND_TO_CLASS = {
    "not-ported": "Port bug",
    "parity-bug": "Port bug",
    "port-divergence": "Port bug",
    "port-bug": "Port bug",  # typo variant seen in the wild (e.g. area 01)
    "upstream-drift": "Version lag",
    "stale-port": "Reverse lag",
    "test-defect": "Test defect",
    "cyrup-original": "Invented surface",
    "tooling": "Tooling",
}

# A handful of rows carry a Kind cell that is not one of the seven-value
# `Kind` enum README's "Item format" section defines at all — they read as a
# scheduling/process note rather than a defect classification (an item
# proposing a human decision, a still-unclassified live-use lead, or a
# test-infrastructure gap rather than a `test-defect`). Rather than silently
# forcing them into one of the six census buckets — which would misrepresent
# what they are — they are reported separately as "Unclassified" by the
# normalizer below, matching how PARITY-GAPS §6 already treats open
# questions as a class of their own outside the six-bucket Kind tally.
KNOWN_NON_TAXONOMY_KINDS = {"product-decision", "test-gap"}


def normalize_kind(raw_kind):
    """Strip bold/annotation noise from a Kind cell and return the bare
    lowercase token this ledger's Kind taxonomy actually uses.

    Handles two attested forms: a plain value (`parity-bug`), and a
    reclassified value carrying its old classification in a trailing
    parenthetical (`**not-ported** *(was upstream-drift)*` — DRIFT-015,
    DRIFT-019) or after em-dash prose (`*unclassified — lead*` — PERM-032).
    The CURRENT classification (before the parenthetical/dash) is what is
    returned; the old one is deliberately discarded, since the row's own
    text is what corrected it.
    """
    if raw_kind is None:
        return None
    s = raw_kind.strip()
    s = re.sub(r"\(was[^)]*\)", "", s)          # drop "(was upstream-drift)"
    s = re.sub(r"\*\([^)]*\)\*", "", s)          # drop "*(...)*" forms
    s = s.split("—")[0]                          # drop "— lead" style suffixes
    s = s.replace("*", "").strip().lower()
    return s

STRIKE_RE = re.compile(r"~~(.+?)~~")
PIPE_PLACEHOLDER = "\x00ESC_PIPE\x00"


def split_row(line):
    """Split a markdown table row on unescaped '|', trimming the outer empties."""
    tmp = line.replace("\\|", PIPE_PLACEHOLDER)
    cells = tmp.split("|")
    cells = [c.replace(PIPE_PLACEHOLDER, "\\|").strip() for c in cells]
    if cells and cells[0] == "":
        cells = cells[1:]
    if cells and cells[-1] == "":
        cells = cells[:-1]
    return cells


def extract_id(cell):
    m = re.search(r"[A-Za-z]+-[A-Za-z0-9.]+", cell.replace("~~", ""))
    return m.group(0) if m else cell.strip()


def clean_severity_token(cell):
    """Strip strike/bold markup and return the first recognised severity word,
    or 'tracker' / 'excluded-marker' / None."""
    stripped = STRIKE_RE.sub(r"\1", cell)
    stripped = stripped.replace("*", "").strip()
    low = stripped.lower()
    low_noparens = low.strip("() ").strip()
    if low_noparens.startswith("partially-closed"):
        return "excluded-marker"
    if low_noparens.startswith("tracker"):
        # both this repo's two attested spellings: bold "**tracker**" and
        # italic-parenthetical "*(tracker)*" (AGENT-028, SESS-038).
        return "tracker"
    for sev in SEVERITIES:
        if low.startswith(sev):
            return sev
    # fall back: search anywhere in the (unstruck) text for a severity word,
    # in case of a leading annotation before the token
    for sev in SEVERITIES:
        if re.search(r"\b" + sev + r"\b", low):
            return sev
    return None


def analyze_severity_cell(raw_sev_cell, id_struck):
    """Return (closed, severity_token).

    The strike-through convention this ledger uses is NOT one signal, it is
    two, and they must be told apart:

      1. CLOSURE — the whole severity value is struck and what follows is a
         bold status marker with nothing else: `~~high~~ **CLOSED 2026-08-14**`,
         `~~medium~~ **CLOSED …, REFUTED**`, `~~high~~ **PARTIALLY CLOSED …**`
         (this ledger's own convention treats "PARTIALLY CLOSED" *with the
         severity struck* as closed — see the module docstring's note on
         SUBA-081 for the one place this reads oddly against its own prose).

      2. RE-RATING — the OLD severity is struck and a bare (non-bold) new
         severity word follows, because the item's severity changed but the
         item itself is still open: `~~medium~~ low — **PARTIALLY CLOSED
         2026-08-14**` (area 08's SEAM-020), `~~low~~ → medium` before its own
         later closure. These rows are OPEN, at the NEW severity.

    `id_struck` (the ID cell itself carrying `~~..~~`) overrides both: a few
    rows in the wild strike only the ID (or, inconsistently, only the
    severity) rather than both as the repo's convention asks — an ID strike
    is treated as authoritative for closure either way.
    """
    if id_struck:
        return True, None

    has_strike = bool(STRIKE_RE.search(raw_sev_cell))
    if not has_strike:
        if re.match(r"^\**FIXED\b", raw_sev_cell.strip()):
            return True, None
        return False, clean_severity_token(raw_sev_cell)

    remainder = STRIKE_RE.sub("", raw_sev_cell)
    remainder = remainder.strip().lstrip("—-→").strip()
    remainder_plain = remainder.replace("*", "").strip()
    low = remainder_plain.lower()
    for sev in SEVERITIES:
        if low.startswith(sev):
            return False, sev  # re-rate: still open, at the new severity
    return True, None


def parse_standard_area(path):
    with open(path, encoding="utf-8") as f:
        lines = f.read().split("\n")

    start = None
    for i, l in enumerate(lines):
        if l.strip() == "## Open items":
            start = i
            break
    if start is None:
        raise ValueError(f"{path}: no '## Open items' heading found")

    end = len(lines)
    for i in range(start + 1, len(lines)):
        if lines[i].startswith("## "):
            end = i
            break

    header_idx = None
    header_cells = None
    rows = []
    for i in range(start, end):
        l = lines[i]
        if header_idx is None and l.startswith("| ID"):
            header_idx = i
            header_cells = [c.strip().lower() for c in split_row(l)]
            continue
        if header_idx is not None and i == header_idx + 1 and re.match(r"^\|[\s:-]+\|", l):
            continue
        if header_idx is not None and l.startswith("|"):
            rows.append((i + 1, split_row(l)))

    if header_cells is None:
        raise ValueError(f"{path}: '## Open items' heading found but no table header in range")

    id_idx = header_cells.index("id")
    sev_idx = header_cells.index("severity")
    kind_idx = header_cells.index("kind") if "kind" in header_cells else None
    title_idx = header_cells.index("title") if "title" in header_cells else None

    parsed = []
    for lineno, cells in rows:
        if len(cells) <= max(id_idx, sev_idx):
            continue  # malformed/short row, skip rather than crash
        raw_id_cell = cells[id_idx]
        raw_sev_cell = cells[sev_idx]
        item_id = extract_id(raw_id_cell)
        id_struck = bool(STRIKE_RE.search(raw_id_cell))
        closed, sev_token = analyze_severity_cell(raw_sev_cell, id_struck)
        # Area 07's own convention for a subset of its rows: the Severity
        # cell is the bare marker "**FIXED <date>**" with no strike-through
        # at all (handled inside analyze_severity_cell), and the
        # strike-through instead wraps the whole Title cell as a second,
        # redundant signal — checked here defensively.
        if not closed and title_idx is not None and len(cells) > title_idx:
            title_cell = cells[title_idx].strip()
            if re.match(r"^~~.*~~$", title_cell):
                closed = True
        if sev_token is None and closed:
            # A closed row whose severity cell carries no plain severity word
            # (e.g. area 07's bare "**FIXED ...**"). Harmless for the tally —
            # closed=True short-circuits before sev is read in tally() — but
            # recorded plainly rather than silently.
            sev_token = "closed-no-severity-token"
        kind = None
        if kind_idx is not None and len(cells) > kind_idx:
            kind = cells[kind_idx].strip().lower()
        parsed.append(
            {
                "id": item_id,
                "line": lineno,
                "closed": closed,
                "severity": sev_token,
                "kind": kind,
                "raw_severity_cell": raw_sev_cell,
            }
        )
    return parsed


def parse_09a(path):
    """09a has no '## Open items' heading, no Kind column, and a second class
    of open item recorded only as a prose list ('## Carried — NOT
    adversarially verified'). Both are parsed and every open row is
    classified upstream-drift by the file-level convention documented in
    this script's module docstring."""
    with open(path, encoding="utf-8") as f:
        text = f.read()
        lines = text.split("\n")

    start = None
    for i, l in enumerate(lines):
        if l.strip() == "## Summary — confirmed items":
            start = i
            break
    if start is None:
        raise ValueError(f"{path}: no '## Summary — confirmed items' heading found")
    end = len(lines)
    for i in range(start + 1, len(lines)):
        if lines[i].startswith("## "):
            end = i
            break

    header_idx = None
    header_cells = None
    rows = []
    for i in range(start, end):
        l = lines[i]
        if header_idx is None and l.startswith("| ID"):
            header_idx = i
            header_cells = [c.strip().lower() for c in split_row(l)]
            continue
        if header_idx is not None and i == header_idx + 1 and re.match(r"^\|[\s:-]+\|", l):
            continue
        if header_idx is not None and l.startswith("|"):
            rows.append((i + 1, split_row(l)))

    id_idx = header_cells.index("id")
    sev_idx = header_cells.index("sev")

    parsed = []
    for lineno, cells in rows:
        if len(cells) <= max(id_idx, sev_idx):
            continue
        raw_id_cell = cells[id_idx]
        raw_sev_cell = cells[sev_idx]
        item_id = extract_id(raw_id_cell)
        id_struck = bool(STRIKE_RE.search(raw_id_cell))
        closed, sev_token = analyze_severity_cell(raw_sev_cell, id_struck)
        parsed.append(
            {
                "id": item_id,
                "line": lineno,
                "closed": closed,
                "severity": sev_token,
                "kind": "upstream-drift",  # see module docstring
                "raw_severity_cell": raw_sev_cell,
            }
        )

    # The "Carried — NOT adversarially verified" rows: no table, listed in
    # prose in the '## Summary — confirmed items' block's own trailing
    # paragraph and in the '## Carried — NOT adversarially verified' section.
    # Hand-enumerated here (ids + severities are stable, cited in-file) rather
    # than prose-parsed, since the prose format is not table-shaped.
    #
    # 2026-09-04: the three carried highs (SUBA-082, SUBA-084, SUBA-086) were
    # promoted into the '## Summary — confirmed items' table as CLOSED rows
    # (each now has a full '## ~~SUBA-0xx~~' section in the confirmed set).
    # They are therefore counted from the table above, and MUST NOT also be
    # listed here, or each would count a second time as an open carried row.
    # Only the five carried mediums remain prose-only.
    carried_high = []
    carried_medium = ["SUBA-087", "SUBA-088", "SUBA-089", "SUBA-090", "SUBA-091"]
    for cid in carried_high:
        parsed.append({"id": cid, "line": None, "closed": False, "severity": "high",
                        "kind": "upstream-drift", "raw_severity_cell": "(carried, not adversarially verified)"})
    for cid in carried_medium:
        parsed.append({"id": cid, "line": None, "closed": False, "severity": "medium",
                        "kind": "upstream-drift", "raw_severity_cell": "(carried, not adversarially verified)"})

    return parsed


def tally(all_items):
    per_area = {}
    class_totals = {}
    unclassified = {}
    above_medium = []

    for area_id, items in all_items.items():
        counts = {"open": 0, "critical": 0, "high": 0, "medium": 0, "low": 0,
                  "trackers": 0, "closed": 0, "excluded_other": 0, "unclassified_sev": 0}
        for it in items:
            sev = it["severity"]
            if sev == "tracker":
                counts["trackers"] += 1
                continue
            if sev == "excluded-marker":
                counts["excluded_other"] += 1
                continue
            if it["closed"]:
                counts["closed"] += 1
                continue
            if sev is None:
                counts["unclassified_sev"] += 1
                continue
            counts["open"] += 1
            counts[sev] += 1
            if sev in ("critical", "high"):
                above_medium.append({"area": area_id, "id": it["id"], "severity": sev,
                                      "kind": it["kind"], "line": it["line"]})
            kind = normalize_kind(it["kind"])
            cls = KIND_TO_CLASS.get(kind)
            if cls is None:
                unclassified.setdefault(area_id, []).append((it["id"], it["kind"]))
            else:
                class_totals[cls] = class_totals.get(cls, 0) + 1
        per_area[area_id] = counts

    return per_area, class_totals, unclassified, above_medium


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dir", default=os.path.dirname(os.path.abspath(__file__)) + "/..",
                     help="docs/gap-analysis directory (default: parent of this script)")
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args()

    all_items = {}
    for area_id, fname in STANDARD_AREAS:
        path = os.path.join(args.dir, fname)
        all_items[area_id] = parse_standard_area(path)
    path_09a = os.path.join(args.dir, AREA_09A[1])
    all_items["09a"] = parse_09a(path_09a)

    per_area, class_totals, unclassified, above_medium = tally(all_items)

    if args.json:
        print(json.dumps({
            "per_area": per_area,
            "class_totals": class_totals,
            "unclassified": unclassified,
            "above_medium": above_medium,
        }, indent=2, sort_keys=True))
        return

    order = [a for a, _ in STANDARD_AREAS] + ["09a"]
    total = {"open": 0, "critical": 0, "high": 0, "medium": 0, "low": 0, "trackers": 0, "closed": 0}
    print(f"{'area':<5}{'open':>6}{'crit':>6}{'high':>6}{'med':>6}{'low':>6}{'trackers':>10}{'closed':>8}")
    for a in order:
        c = per_area[a]
        print(f"{a:<5}{c['open']:>6}{c['critical']:>6}{c['high']:>6}{c['medium']:>6}{c['low']:>6}{c['trackers']:>10}{c['closed']:>8}")
        for k in total:
            total[k] += c[k]
    print(f"{'TOTAL':<5}{total['open']:>6}{total['critical']:>6}{total['high']:>6}{total['medium']:>6}{total['low']:>6}{total['trackers']:>10}{total['closed']:>8}")

    print()
    print("Gap class (open, non-tracker rows only):")
    grand = 0
    for cls in ["Port bug", "Version lag", "Reverse lag", "Test defect", "Invented surface", "Tooling"]:
        n = class_totals.get(cls, 0)
        grand += n
        print(f"  {cls:<20}{n:>5}")
    print(f"  {'TOTAL':<20}{grand:>5}")

    if unclassified:
        print()
        print("UNCLASSIFIED kind values (need a manual look / a KIND_TO_CLASS entry):")
        for area_id, pairs in unclassified.items():
            for iid, kind in pairs:
                print(f"  {area_id} {iid}: kind={kind!r}")

    print()
    print(f"Above-medium open rows ({len(above_medium)}):")
    for row in sorted(above_medium, key=lambda r: (r["severity"] != "critical", r["area"], r["id"])):
        print(f"  {row['area']:<4} {row['id']:<12} {row['severity']:<9} kind={row['kind']}")


if __name__ == "__main__":
    main()

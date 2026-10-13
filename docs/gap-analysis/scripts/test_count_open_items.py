#!/usr/bin/env python3
"""Tests for count_open_items.py.

    python3 -I docs/gap-analysis/scripts/test_count_open_items.py

The first test is the one that matters: every `*.md` in docs/gap-analysis/ is
either counted or excluded with a reason, so an area file added later cannot
fall out of the census unnoticed, as area 13's and 15's did.
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
LEDGER = os.path.dirname(HERE)
SCRIPT = os.path.join(HERE, "count_open_items.py")
sys.path.insert(0, HERE)

import count_open_items as census


def run(directory, *extra):
    return subprocess.run(
        [sys.executable, "-I", SCRIPT, "--dir", directory, *extra],
        capture_output=True, text=True, check=False,
    )


class Inventory(unittest.TestCase):
    def test_every_ledger_file_is_counted_or_excluded(self):
        _, _, unaccounted, stale = census.inventory(LEDGER)
        self.assertEqual(unaccounted, [], "add each to the census or to EXCLUDED_FILES")
        self.assertEqual(stale, [], "listed files that no longer exist")

    def test_an_unlisted_file_fails_the_run(self):
        with tempfile.TemporaryDirectory() as tmp:
            copy = os.path.join(tmp, "gap-analysis")
            shutil.copytree(LEDGER, copy, ignore=shutil.ignore_patterns("scripts"))
            with open(os.path.join(copy, "19-new-area.md"), "w", encoding="utf-8") as f:
                f.write("# 19\n\n## Open items\n\n| ID | Severity | Kind | Effort | Title |\n"
                        "|---|---|---|---|---|\n| NEW-001 | high | not-ported | S | x |\n")
            result = run(copy)
        self.assertEqual(result.returncode, 1)
        self.assertIn("UNACCOUNTED: 19-new-area.md", result.stderr)


class AreaCounts(unittest.TestCase):
    def per_area(self):
        result = run(LEDGER, "--json")
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)["per_area"]

    def test_area_13_reads_only_its_standard_item_tables(self):
        # 53 rows across eight files; a table with `id | severity` but no
        # `kind` (13-cyrup-mcp.md's "why it is a unit" table) is not an item
        # table and must not be counted.
        items = census.parse_area_13(LEDGER)
        self.assertEqual(len(items), 53)
        self.assertEqual(sorted({i["file"] for i in items}), sorted(census.AREA_13_FILES))

    def test_area_15_skips_the_rerating_table(self):
        # The `id | from | to | why` table above the items is severity changes,
        # not items; only the 157-row `sev | id | ...` table is read.
        items = census.parse_area_15(os.path.join(LEDGER, census.AREA_15[1]))
        self.assertEqual(len(items), 157)
        ids = {i["id"] for i in items}
        self.assertIn("ACP-299", ids)


class History(unittest.TestCase):
    """MCP-638 and MCP-639 closed between these two commits (PR #251); the
    census must see it. Skipped where the history is not available."""

    BEFORE, AFTER = "4f522804", "f9e106a4"

    def open_total_at(self, sha):
        with tempfile.TemporaryDirectory() as tmp:
            root = subprocess.run(
                ["git", "-C", LEDGER, "rev-parse", "--show-toplevel"],
                capture_output=True, text=True, check=False,
            ).stdout.strip()
            archive = subprocess.run(
                ["git", "-C", root, "archive", sha, "docs/gap-analysis"],
                capture_output=True, check=False,
            )
            if archive.returncode != 0:
                self.skipTest(f"{sha} not available")
            subprocess.run(["tar", "-x", "-C", tmp], input=archive.stdout, check=True)
            result = run(os.path.join(tmp, "docs", "gap-analysis"), "--json")
            self.assertEqual(result.returncode, 0, result.stderr)
            return sum(a["open"] for a in json.loads(result.stdout)["per_area"].values())

    def test_two_mcp_closures_move_the_total_by_two(self):
        self.assertEqual(self.open_total_at(self.BEFORE) - self.open_total_at(self.AFTER), 2)


if __name__ == "__main__":
    unittest.main(verbosity=2)

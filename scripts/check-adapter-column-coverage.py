#!/usr/bin/env python3
"""Verify both adapter docs map every usage_record column.

Reads the authoritative DDL LIVE from crates/agentlens-core/src/archive.rs (never a
hardcoded column list) so that adding a column to the archive immediately fails this
check until both adapter docs gain a mapping row for it.

Run from the repository root:

    python3 scripts/check-adapter-column-coverage.py

Optional first argument overrides the repository root.

Exit codes:
    0  every column is mapped in every adapter doc
    1  at least one column has no mapping row (missing columns printed by name)
    2  a required input file is missing, empty, or has no parsable DDL
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

DDL_RELATIVE = Path("crates/agentlens-core/src/archive.rs")
DOC_RELATIVE = [
    Path("docs/adapters/codex.md"),
    Path("docs/adapters/claude-code.md"),
]

TABLE_START = re.compile(r'"CREATE TABLE usage_record \(')
COLUMN = re.compile(r"^([a-z_][a-z0-9_]*)\s+(TEXT|INTEGER|REAL)\b")


def fail(message: str, code: int) -> None:
    print(f"error: {message}", file=sys.stderr)
    raise SystemExit(code)


def read_text(path: Path) -> str:
    if not path.exists():
        fail(f"required file not found: {path}", 2)
    if not path.is_file():
        fail(f"expected a file, found something else: {path}", 2)
    text = path.read_text(encoding="utf-8", errors="replace")
    if not text.strip():
        fail(f"required file is empty: {path}", 2)
    return text


def parse_columns(ddl_path: Path) -> list[str]:
    """Extract usage_record column names from the live CREATE TABLE statement."""
    text = read_text(ddl_path)
    match = TABLE_START.search(text)
    if match is None:
        fail(f"no `CREATE TABLE usage_record (` statement found in {ddl_path}", 2)

    columns: list[str] = []
    for line in text[match.end():].splitlines():
        stripped = line.strip()
        if stripped.startswith(");") or stripped.startswith("UNIQUE("):
            break
        column = COLUMN.match(stripped)
        if column is not None:
            columns.append(column.group(1))

    if not columns:
        fail(f"usage_record DDL in {ddl_path} yielded zero columns", 2)
    return columns


def mapped_columns(doc_path: Path) -> set[str]:
    """Collect column names appearing as `col` in the first cell of a table row."""
    text = read_text(doc_path)
    found: set[str] = set()
    for line in text.splitlines():
        stripped = line.strip()
        if not stripped.startswith("|"):
            continue
        cells = [cell.strip() for cell in stripped.strip("|").split("|")]
        if not cells:
            continue
        for name in re.findall(r"`([a-z_][a-z0-9_]*)`", cells[0]):
            found.add(name)
    return found


def main(argv: list[str]) -> int:
    root = Path(argv[1]).resolve() if len(argv) > 1 else Path.cwd()
    if not root.is_dir():
        fail(f"repository root is not a directory: {root}", 2)

    columns = parse_columns(root / DDL_RELATIVE)
    print(f"usage_record columns parsed live from {DDL_RELATIVE}: {len(columns)}")

    failed = False
    for relative in DOC_RELATIVE:
        doc_path = root / relative
        mapped = mapped_columns(doc_path)
        missing = [column for column in columns if column not in mapped]
        if missing:
            failed = True
            print(f"FAIL {relative}: {len(missing)} column(s) without a mapping row")
            for column in missing:
                print(f"  missing column: {column}")
        else:
            print(f"OK   {relative}: all {len(columns)} columns mapped")

    if failed:
        print("column coverage check FAILED", file=sys.stderr)
        return 1
    print("column coverage check PASSED")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))

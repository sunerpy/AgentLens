#!/usr/bin/env python3
"""Diff the wire DTO field names in docs/remote-source-api.md against the code.

Sources of truth:
  * meta / source_meta / scan_window  -> CollectorMetaV1, CollectorSourceMetaV1,
    CollectorScanWindowV1 under crates/agentlens-collector/ (serde field names are the
    Rust field names; these structs carry no rename_all attribute).
  * record -> NormalizedUsageRecord in crates/agentlens-core/src/archive.rs, converted
    to camelCase because that type carries #[serde(rename_all = "camelCase")].

The doc declares its field lists in machine-readable marker comments:

    <!-- wire-dto:meta = protocol_version, machine_id_hash, ... -->

Run from the repository root:

    python3 scripts/check-wire-dto-fields.py

Optional first argument overrides the repository root.

Exit codes:
    0  doc and code agree on every field name and order
    1  a field-name or ordering mismatch was found
    2  a required input file is missing, empty, or has no parsable marker/struct
    3  the collector wire DTO source is absent (run after todo 10)
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

DOC_RELATIVE = Path("docs/remote-source-api.md")
CORE_RELATIVE = Path("crates/agentlens-core/src/archive.rs")
COLLECTOR_DIR = Path("crates/agentlens-collector")

COLLECTOR_STRUCTS = {
    "meta": "CollectorMetaV1",
    "source_meta": "CollectorSourceMetaV1",
    "scan_window": "CollectorScanWindowV1",
}
FIELD = re.compile(r"^\s*pub(?:\(\w+\))?\s+(\w+)\s*:", re.MULTILINE)


def fail(message: str, code: int) -> None:
    print(f"error: {message}", file=sys.stderr)
    raise SystemExit(code)


def read_text(path: Path, missing_code: int = 2) -> str:
    if not path.is_file():
        fail(f"required file not found: {path}", missing_code)
    text = path.read_text(encoding="utf-8", errors="replace")
    if not text.strip():
        fail(f"required file is empty: {path}", missing_code)
    return text


def struct_fields(text: str, name: str, origin: Path) -> list[str]:
    marker = f"struct {name} {{"
    index = text.find(marker)
    if index < 0:
        fail(f"struct {name} not found in {origin}", 2)
    body = text[index + len(marker):]
    end = body.find("\n}")
    if end < 0:
        fail(f"struct {name} in {origin} is not terminated", 2)
    fields = FIELD.findall(body[:end])
    if not fields:
        fail(f"struct {name} in {origin} yielded zero fields", 2)
    return fields


def to_camel(name: str) -> str:
    head, *rest = name.split("_")
    return head + "".join(part[:1].upper() + part[1:] for part in rest)


def find_collector_source(root: Path) -> tuple[Path, str]:
    """Locate the file under crates/agentlens-collector/ declaring CollectorMetaV1."""
    directory = root / COLLECTOR_DIR
    if not directory.is_dir():
        fail("collector wire DTO not found — run after todo 10", 3)
    for candidate in sorted(directory.rglob("*.rs")):
        text = candidate.read_text(encoding="utf-8", errors="replace")
        if "struct CollectorMetaV1 {" in text:
            return candidate, text
    fail("collector wire DTO not found — run after todo 10", 3)
    raise AssertionError("unreachable")


def doc_lists(doc_path: Path) -> dict[str, list[str]]:
    text = read_text(doc_path)
    found: dict[str, list[str]] = {}
    pattern = re.compile(r"<!--\s*wire-dto:(\w+)\s*=\s*([^>]*?)\s*-->")
    for key, raw in pattern.findall(text):
        found[key] = [item.strip() for item in raw.split(",") if item.strip()]
    if not found:
        fail(f"no `wire-dto:` marker comments found in {doc_path}", 2)
    return found


def compare(label: str, expected: list[str], actual: list[str]) -> bool:
    if expected == actual:
        print(f"OK   {label}: {len(expected)} field(s) match")
        return True
    print(f"FAIL {label}: doc and code disagree")
    for name in expected:
        if name not in actual:
            print(f"  in code but missing from doc: {name}")
    for name in actual:
        if name not in expected:
            print(f"  in doc but missing from code: {name}")
    if sorted(expected) == sorted(actual):
        print(f"  same field set, different order")
        print(f"  code order: {expected}")
        print(f"  doc order:  {actual}")
    return False


def main(argv: list[str]) -> int:
    root = Path(argv[1]).resolve() if len(argv) > 1 else Path.cwd()
    if not root.is_dir():
        fail(f"repository root is not a directory: {root}", 2)

    collector_path, collector_text = find_collector_source(root)
    print(f"collector wire DTO source: {collector_path.relative_to(root)}")

    code: dict[str, list[str]] = {}
    for key, struct_name in COLLECTOR_STRUCTS.items():
        code[key] = struct_fields(collector_text, struct_name, collector_path)

    core_path = root / CORE_RELATIVE
    core_text = read_text(core_path)
    code["record"] = [
        to_camel(name)
        for name in struct_fields(core_text, "NormalizedUsageRecord", core_path)
    ]

    doc = doc_lists(root / DOC_RELATIVE)

    ok = True
    for key in ("meta", "source_meta", "scan_window", "record"):
        if key not in doc:
            print(f"FAIL {key}: no `wire-dto:{key}` marker in {DOC_RELATIVE}")
            ok = False
            continue
        ok = compare(key, code[key], doc[key]) and ok

    if not ok:
        print("wire DTO field diff FAILED", file=sys.stderr)
        return 1
    print("wire DTO field diff PASSED")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))

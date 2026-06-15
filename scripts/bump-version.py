#!/usr/bin/env python3
"""Bump the workspace version across the repository.

Updates, precisely and only:
  1. Root Cargo.toml — [workspace.package] version and the internal
     path-dependency pins (adk-*, awp-*, cargo-adk).
  2. Dependency snippets in docs, READMEs, and Rust doc comments —
     lines like `adk-rust = "1.0.0"` or `adk-graph = { version = "1.0.0", ... }`.

Deliberately never touched:
  - CHANGELOG.md (history must not be rewritten)
  - Lock files (Cargo.lock is synced separately via `cargo update --workspace`,
    which only re-pins workspace members, never third-party deps)
  - reference/, learning/, tmp/, output/, target/, docs/podcast/ (historical
    or third-party content)
  - Prose mentions like "v1.0.0 Released!" — only dependency-snippet
    patterns are rewritten, so announcements and history stay intact.

Usage:
  python3 scripts/bump-version.py <new-version> [--dry-run]
"""

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

SKIP_PARTS = {"reference", "learning", "tmp", "output", "target", ".git", "proptest-regressions"}
SKIP_SUFFIXES = (".lock",)


def fail(msg: str) -> None:
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


def current_version(cargo_toml: str) -> str:
    in_pkg = False
    for line in cargo_toml.splitlines():
        if line.strip().startswith("["):
            in_pkg = line.strip() == "[workspace.package]"
        elif in_pkg:
            m = re.match(r'version = "([^"]+)"', line.strip())
            if m:
                return m.group(1)
    fail("could not find [workspace.package] version in Cargo.toml")
    raise AssertionError  # unreachable


def skip(path: Path) -> bool:
    rel = path.relative_to(ROOT)
    if any(part in SKIP_PARTS for part in rel.parts):
        return True
    if rel.parts[:2] == ("docs", "podcast"):
        return True
    if path.name == "CHANGELOG.md":
        return True
    if path.suffix in SKIP_SUFFIXES:
        return True
    return False


def main() -> None:
    args = [a for a in sys.argv[1:] if a != "--dry-run"]
    dry_run = "--dry-run" in sys.argv[1:]
    if len(args) != 1:
        fail(f"usage: {sys.argv[0]} <new-version> [--dry-run]")
    new = args[0]
    if not re.fullmatch(r"\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?", new):
        fail(f"'{new}' is not a valid semver version")

    root_cargo = ROOT / "Cargo.toml"
    cargo_text = root_cargo.read_text()
    cur = current_version(cargo_text)
    if cur == new:
        fail(f"workspace is already at {cur}")
    print(f"Bumping {cur} -> {new}\n")
    cur_re = re.escape(cur)

    # --- 1. Root Cargo.toml ------------------------------------------------
    # [workspace.package] version — exactly one bare `version = "<cur>"` line
    # exists (member crates use `version.workspace = true`).
    pkg_pattern = re.compile(rf'^version = "{cur_re}"$', re.M)
    if len(pkg_pattern.findall(cargo_text)) != 1:
        fail("expected exactly one bare workspace `version` line in Cargo.toml")
    cargo_text = pkg_pattern.sub(f'version = "{new}"', cargo_text)

    # Internal path-dependency pins: `name = { path = "...", version = "<cur>" ... }`
    pin_pattern = re.compile(
        rf'^((?:adk|awp|cargo)-[a-z0-9-]+ = \{{ path = "[^"]+", version = "){cur_re}(")',
        re.M,
    )
    cargo_text, pin_count = pin_pattern.subn(rf"\g<1>{new}\g<2>", cargo_text)
    print(f"Cargo.toml: workspace version + {pin_count} internal dependency pins")
    if not dry_run:
        root_cargo.write_text(cargo_text)

    # --- 2. Dependency snippets in docs / READMEs / Rust doc comments ------
    snippet_patterns = [
        # adk-rust = "1.0.0"
        re.compile(rf'\b((?:adk|awp|cargo)-[a-z0-9-]+\s*=\s*"){cur_re}(")'),
        # adk-graph = { version = "1.0.0", features = [...] }
        re.compile(rf'\b((?:adk|awp|cargo)-[a-z0-9-]+\s*=\s*\{{\s*version\s*=\s*"){cur_re}(")'),
    ]

    tracked = subprocess.run(
        ["git", "ls-files", "*.md", "*.rs"],
        cwd=ROOT, capture_output=True, text=True, check=True,
    ).stdout.splitlines()

    total = 0
    changed_files = 0
    for rel in tracked:
        path = ROOT / rel
        if skip(path) or not path.exists():
            continue
        text = path.read_text()
        count = 0
        for pattern in snippet_patterns:
            text, n = pattern.subn(rf"\g<1>{new}\g<2>", text)
            count += n
        if count:
            changed_files += 1
            total += count
            print(f"{rel}: {count} snippet(s)")
            if not dry_run:
                path.write_text(text)

    print(f"\n{'[dry-run] would update' if dry_run else 'Updated'} "
          f"{changed_files} doc/source files ({total} snippets) + Cargo.toml")
    if not dry_run:
        print("\nNext steps:")
        print("  cargo update --workspace   # re-pin workspace members in Cargo.lock (third-party deps untouched)")
        print("  bash scripts/check-doc-versions.sh")


if __name__ == "__main__":
    main()

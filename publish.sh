#!/usr/bin/env bash
# Thin wrapper for muscle memory — the publish logic lives in the shell-agnostic
# task runner so it also works on Windows (PowerShell/cmd): `cargo xtask publish`.
#
# Usage:
#   ./publish.sh             # cargo publish --workspace (cargo computes the order)
#   ./publish.sh --dry-run   # native publish, no upload
#   ./publish.sh --resume    # sequential per-crate publish in computed dependency
#                            # order, skipping already-published versions
set -euo pipefail
exec cargo xtask publish "$@"

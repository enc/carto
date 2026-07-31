#!/usr/bin/env bash
# Local stand-in for spec §9.5's CI security gates. Runs everything that
# doesn't require Linux/strace or a network fetch of the RUSTSEC advisory
# database — see docs/adr/0004-deferred-security-gates.md for exactly what's
# deferred and why. Intended to be green before every commit.
#
# Of spec §9.5's 5 gates, this script now runs 1, 3, and 4:
#   1. cargo deny check bans                       — below.
#   3. test_determinism (double-index byte-compare) — crates/carto-cli/tests/cli.rs,
#      via `cargo test --workspace` below, since M1.b.1 added `carto index`.
#   4. test_pathguard (denylisted write -> exit 3)   — same file, same reason.
# 2 (strace no-network) and 5 (M5 WASM/Landlock) remain deferred; see the ADR.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

echo "==> cargo fmt --check"
cargo fmt --all --check

echo "==> cargo clippy"
cargo clippy --all-targets --all-features -- -D warnings

echo "==> cargo deny check bans licenses sources (advisories deferred, needs network)"
cargo deny check bans licenses sources

echo "==> cargo test --workspace (includes gates 3 & 4, see above)"
cargo test --workspace --all-features

echo "==> all gates passed"

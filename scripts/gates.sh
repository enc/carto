#!/usr/bin/env bash
# Local stand-in for spec §9.5's CI security gates. Runs everything that
# doesn't require Linux/strace or a network fetch of the RUSTSEC advisory
# database — see docs/adr/0004-deferred-security-gates.md for exactly what's
# deferred and why. Intended to be green before every commit in M1.a.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

echo "==> cargo fmt --check"
cargo fmt --all --check

echo "==> cargo clippy"
cargo clippy --all-targets --all-features -- -D warnings

echo "==> cargo deny check bans licenses sources (advisories deferred, needs network)"
cargo deny check bans licenses sources

echo "==> cargo test --workspace"
cargo test --workspace --all-features

echo "==> all gates passed"

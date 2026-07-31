# 0003 — `build.rs` reads `.git/HEAD` via `std::fs`, no `git` subprocess

**Status:** accepted · **Date:** 2026-07-31 · **Milestone:** M1.a

## Context

`carto selfcheck`/`--version` needs to report the git SHA carto itself was
built from (spec §9.1: "`carto --version` prints version + git SHA + …").
The obvious implementation is `git rev-parse HEAD` from `build.rs`.

INV-2 ("no foreign code execution... external process execution is limited
to an allowlist: none in v1") is stated about the *shipped binary's runtime
behavior*, and `build.rs` runs at compile time on the build host, not at
`carto` runtime — so a `git` subprocess in `build.rs` would not technically
violate INV-2 as scoped. It would, however, undercut the spirit of the
invariant (a build environment without `git` on `PATH`, or a sandboxed CI
runner, would break the build for a cosmetic version string) and set a
precedent that "build.rs can shell out" the next time someone's tempted to
add something less cosmetic there.

## Decision

`crates/carto-cli/build.rs` reads `.git/HEAD` directly via `std::fs`,
follows the `ref: refs/heads/<branch>` indirection by reading that ref file
too, and falls back to the literal `"unknown"` — never failing the build —
when `.git` is absent entirely (e.g. building from a source tarball with no
VCS metadata). No `std::process::Command` anywhere in `build.rs`.

## Consequences

- Doesn't handle git worktrees or submodules (where `.git` is a *file*
  pointing elsewhere, not a directory) — `find_git_dir()` only recognizes
  the directory form. Acceptable: carto's own repo is a normal checkout,
  and a wrong/missing git SHA in `selfcheck` output is cosmetic, not
  correctness-affecting.
- Doesn't handle packed refs (`.git/packed-refs`) — a repo with no loose
  ref for the current branch (e.g. right after a `git gc`) would fall
  through to `"unknown"` rather than the real SHA. Same cosmetic-only
  impact; not worth the added parsing for a version string.
- `clippy.toml`'s `disallowed-methods` lint is scoped to `crates/*/src/`
  and does not cover `build.rs`, so this constraint is enforced by this
  ADR and code review, not by tooling. If a future build.rs need actually
  requires shelling out, that's a new ADR, not a silent addition.

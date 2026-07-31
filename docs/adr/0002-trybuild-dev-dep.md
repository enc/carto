# 0002 — `trybuild` as a dev-dependency

**Status:** accepted · **Date:** 2026-07-31 · **Milestone:** M1.a

## Context

INV-5 requires that "constructing report/MCP output from raw repo text MUST
NOT compile." A claim like that is only actually verified by a test that
tries to write the disallowed code and asserts the compiler rejects it —
ordinary `#[test]` functions can't express "this should fail to compile."

## Decision

Added `trybuild` (`crates/carto-core/Cargo.toml`, `[dev-dependencies]`
only — never reaches a shipped binary) to run a `compile_fail` suite over
`crates/carto-core/tests/ui/*.rs`. Each case is a tiny program attempting
one specific escape from `TaintedString` (`Display`, `Into<String>`,
`AsRef<str>`, `Deref`, direct private-field access); trybuild compiles it
in a scratch crate and diffs the compiler's stderr against a checked-in
`.stderr` snapshot.

The snapshot (not just "did it fail") matters: without it, a case that
fails to compile for an unrelated reason (a typo, an unrelated API change)
would still show up as a passing test, silently no longer testing what its
name claims.

## Consequences

- Dev-only dependency; `deny.toml`'s bans apply to it like any other crate,
  and it does not affect the shipped binary's dependency graph or INV-1
  posture.
- Adding a new escape hatch to guard against means: write the `tests/ui/*.rs`
  case, run the suite once to generate the `.stderr` into `wip/` (gitignored),
  move it into `tests/ui/`, commit both.
- If `TaintedString`'s public API changes, every `.stderr` snapshot is a
  potential source of churn (compiler error message wording can shift
  between rustc versions) — accepted trade-off, since the alternative is not
  testing the invariant at all.

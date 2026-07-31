# 0001 — Hand-rolled `Error` and `AtomicFile` instead of `thiserror`/`tempfile`

**Status:** accepted · **Date:** 2026-07-31 · **Milestone:** M1.a

## Context

Spec §13 requires an ADR justifying any dependency outside the starting
allowlist. `thiserror` (error-derive macro) and `tempfile` (secure temp
files) are the conventional choices for `crates/carto-core/src/error.rs`
and `crates/carto-core/src/pathguard/atomic.rs` respectively, and neither
is banned by `deny.toml`.

## Decision

Wrote both by hand instead of adding the dependency.

- **`Error`**: four `ErrorKind` variants with a fixed spec-mandated
  exit-code mapping (§9.2) — a closed, small problem. `thiserror`'s value is
  amortizing derive-macro boilerplate across many error variants with
  `#[from]` conversions; carto's error surface doesn't grow that way (kinds
  are fixed by the exit-code contract, not by how many failure sources
  exist).
- **`AtomicFile`**: tmp-file-then-rename with an explicit `commit()` and a
  `Drop` cleanup. `tempfile`'s value is portable secure temp-file creation
  (permissions, race-free unique naming) across more scenarios than carto
  needs — carto only ever writes inside a `carto`-owned out-dir it already
  controls, and INV-7 (determinism) actively wants a *non-random* temp name
  (pid + sequence number) rather than `tempfile`'s random suffix.

## Consequences

- Smaller dependency graph, which is the explicit point of INV-1: every
  crate in the graph is something `cargo deny check bans` had to reason
  about, and fewer crates means fewer things that could later gain a
  networking feature undetected.
- Both types are covered by unit tests in the same commit they were
  introduced in (`error.rs`, `pathguard/atomic.rs`) rather than trusting an
  external crate's test suite.
- If carto's error surface grows enough that hand-written `Error`
  construction becomes repetitive boilerplate, revisit; that hasn't
  happened as of M1.a.

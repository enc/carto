# 0006 — `manifest.json`'s commit SHA read by hand; `dirty` left `null`

**Status:** accepted · **Date:** 2026-07-31 · **Milestone:** M1.b.1

## Context

Spec §4.4 says `manifest.json` records "indexed commit SHA + dirty flag."
INV-2 forbids a `git` subprocess to get either. Spec §7.1 anticipates this
for the *other* place carto touches git (`--diff <rev>`): "obtain WITHOUT
running git... `gix` (pure-Rust, no subprocess... features pinned)" — but
that's scoped to M1.b.3+ (`impact --diff`), not this slice, and pulling in
`gix` now would be front-loading a real dependency (with network features
that must be disabled and pinned, per §7.1's own caveat) for two manifest
fields that a hand-rolled reader can mostly cover.

`crates/carto-cli/build.rs` already reads `.git/HEAD` by hand for a
cosmetic version string (ADR-0003), deliberately skipping packed-refs and
worktrees as acceptable-for-cosmetic gaps.

## Decision

New `crates/carto-core/src/gitinfo.rs`, runtime code (not a build script),
duplicating build.rs's shape rather than sharing it (a build script can't
cleanly depend on the crate it builds for) but going further:

- Handles loose refs, `packed-refs`, detached HEAD, and worktrees
  (`.git`-as-file + `commondir`) — build.rs's ADR-0003 skips the first two
  as not worth it for a cosmetic string; `commit_sha` in a persisted
  manifest is closer to data, so the extra parsing earns its keep here.
- Does **not** attempt a `dirty` flag. An honest one needs comparing the
  index/worktree against `HEAD` — reading the index format and diffing
  file content/mtimes by hand is real scope, not a small extension of
  "read a ref file." `manifest.json`'s `dirty` field is `Option<bool>`,
  always `None` this slice, with a doc comment explaining why — not
  defaulted to `false`, which would claim a clean tree never actually
  checked (that would be worse than omitting the field).
- Returns `None` rather than erroring whenever anything is missing or
  doesn't parse (not a git repo, unreadable ref, malformed HEAD) — same
  honesty principle: an absent commit SHA is `null`, not a build failure
  or a guess.

## Consequences

- `gix` is not added as a dependency in M1.b.1, despite spec §7.1 pre-
  clearing it for `--diff`. That work item is unaffected: when `impact
  --diff <rev>` is implemented, `gix` arrives then, on its own merits
  (needs actual commit range / diff traversal, which `gitinfo.rs` doesn't
  attempt).
- `dirty` stays `null` in every `manifest.json` produced until a milestone
  actually implements the comparison — tracked as spec debt, not silently
  dropped: this ADR is the record of the gap and its trigger condition
  (implementing a real index/worktree diff, most naturally alongside
  whatever milestone adds `gix`).
- `gitinfo.rs`'s test suite exercises packed-refs and worktree resolution
  against hand-built fake `.git` directories (not the real carto repo),
  keeping the tests hermetic and fast rather than coupled to this
  repository's actual git state.

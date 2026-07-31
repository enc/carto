# 0007 — `carto-grammars`: the designated `unsafe_code` carve-out, empty in practice

**Status:** accepted · **Date:** 2026-07-31 · **Milestone:** M1.b.2a

## Context

Spec §3.1: `unsafe_code = "forbid"` workspace-wide "except
`carto-grammars` (tree-sitter FFI), which isolates and documents each
[unsafe] block." Historically, converting a tree-sitter grammar crate's
raw C `LANGUAGE` function pointer into a `tree_sitter::Language` needed
an `unsafe` block at the call site, because the ABI compatibility between
a grammar's compiled parse tables and the linked `tree-sitter` runtime
can't be checked by the type system.

Spec §0/§13 explicitly ask the implementer to verify crate API surfaces
at implementation time rather than trust the spec's own illustrative
description. `docs.rs` isn't reachable from this sandbox (not on the
network allowlist), so this was resolved empirically: write
`carto_grammars::rust_language()` using `tree_sitter_rust::LANGUAGE.into()`
with no `unsafe` block, and see whether `cargo build` demands one.

## Decision

It compiled clean — `tree-sitter` 0.26.11 / `tree-sitter-rust` 0.24.2 /
`tree-sitter-language` 0.1.7's `LanguageFn -> Language` conversion is
safe Rust. `crates/carto-grammars/src/lib.rs` contains **zero** `unsafe`
blocks as of this commit.

The crate keeps its role as the designated exception anyway
(`Cargo.toml`'s `[lints.rust] unsafe_code = "allow"`, deliberately not
inheriting `[lints] workspace = true` — Cargo's workspace-lint
inheritance is all-or-nothing per package, and `forbid` can't be
downgraded locally even if it could be inherited partially):

- The exception is architectural (spec §3.1 names this crate as *the*
  seam for grammar loading), not contingent on today's tree-sitter
  version happening to need `unsafe`. A grammar crate version bump, or
  the M5 `wasm-grammars` path (which touches `wasmtime`), could
  reintroduce a need for it — better to have the lint boundary already
  in the right place than to redraw it under time pressure later.
- `crates/carto-grammars/src/lib.rs`'s module doc comment records this
  finding directly at the point a future reader would look for the
  `unsafe` block and not find one, rather than leaving them to wonder if
  it's missing by mistake.

## Consequences

- If a future grammar (this crate, or the TS/JS/Python/Go extractors'
  slice) needs a real `unsafe` block, add it there with the isolation +
  documentation spec §3.1 asks for — this ADR is not blanket permission
  to scatter `unsafe` anywhere in the crate, just the record that the
  lint boundary is intentionally where it is.
- `cargo deny check bans` was re-run after adding `tree-sitter`/
  `tree-sitter-rust`/`tree-sitter-language`/`regex`/`streaming-iterator`
  to the dependency graph: no `multiple-versions` collision with
  `ignore`'s dependency tree (the risk flagged going in) — clean.

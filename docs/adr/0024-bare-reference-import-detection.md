# 0024 — Bare fully-qualified-path references as imports (Rust)

**Status:** accepted · **Date:** 2026-08-03 · **Milestone:** post-S-1
improvement plan follow-up, L3

## Context

Two consecutive real benchmark batches graded L3 ("Which files in this
repo import anything from the `carto_core` crate?") `partial` for
`cli`. The mistake changed shape between batches — first: a grep-style
sweep picking up 2 known doc-comment false positives; second: `cli`
correctly *excluded* those 2 using carto's own structural data, but
then mis-bucketed `crates/carto-cli/src/main.rs` as "transitive, not
direct" and dropped it, landing on 18/19 instead of 19/19.

Checked directly: `main.rs` has **zero** `use carto_core::...;`
statements. Its only two references are `#[command(name =
carto_core::consts::BIN_NAME, ...)]` (an attribute argument) and `->
carto_core::Result<u8>` (a function return type, twice) — both
fully-qualified-path references, valid Rust since 2018 with no `use`
needed at all. `imports.scm` only captured `use_declaration`/
`mod_item` nodes — a bare `scoped_identifier`/`scoped_type_identifier`
reference outside a `use` statement was **structurally invisible** to
carto's import graph. So `deps carto_core --dir in` was not actually
19/19 even after §1.2/§1.3 landed — it was permanently missing any file
whose only reference to an external crate is a bare qualified path.
This was a real gap, not the `cli` session's own confused reasoning
about transitivity.

## Why this can't be "just capture every `scoped_identifier`"

That node shape is identical for `carto_core::Result` (root = a crate
name) and `Order::new()`/`PathGuard::new()` (root = a *local type
name*, called via `Type::method()`) — tree-sitter can't syntactically
tell these apart, the exact ambiguity ADR-0008 already excludes from
*call* resolution for this reason. Capturing every such node naively
would fabricate a spurious external `Module` node for every locally-
defined type used in `Type::method()` style anywhere in the repo
(hundreds of false positives on carto's own code alone) — a real INV-8
violation, not an improvement.

## Decisions

- **Two cheap filters, both checked in Rust code, not the query.**
  `imports.scm` now also captures every `(scoped_identifier)` and
  `(scoped_type_identifier)` node — deliberately broad, the same
  precedent this file's own header comment already sets for use-tree
  shapes. Precision comes from `rust.rs`:
  1. **Exclude a `call_expression`'s own callee position.**
     `Type::method()`/`module::func()` call shapes are exactly
     ADR-0008's existing exclusion — this must not re-capture what
     `calls.scm` already deliberately leaves alone.
  2. **Root must start lowercase.** Rust crate names are
     conventionally snake_case; types/traits/generic parameters are
     conventionally PascalCase (`Order`, `PathGuard`, `T`, `Self`,
     `Item`) — a near-universal, clippy-enforced convention in real
     Rust code. This eliminates the residual ambiguity for non-call
     positions too (`Self::Output`, `T::Item`, associated-const paths)
     without cross-referencing the whole symbol table. `crate`/`self`/
     `super` stay excluded exactly as `RawImport::Absolute` already
     does.
- **New `RawImport::BareReference { root }` variant, honestly labeled
  heuristic.** Unlike every other `RawImport` variant, this is *not* a
  verified declaration — a doc comment states this plainly. Routed
  through the *same* internal/external classification `Absolute`
  already has in `resolve.rs` (a root matching `known_modules`/
  `crate`/`self`/`super` is internal; a new external root reuses the
  existing dedup-by-root `Module` node logic) — no new resolve.rs
  classification logic, only a new variant feeding the existing
  pipeline.
- **`Confidence::Certain`, distinct evidence string
  `"external-package-bare-reference"`.** Certain because valid Rust
  genuinely cannot reference an external crate without it being a real
  dependency — the compiler enforces this, same footing as
  `Absolute`'s `"external-package"`. The evidence string stays distinct
  so a careful caller can tell a verified `use` declaration apart from
  this heuristic, per INV-8's "evidence explains how" contract. When a
  file has both a real `use` statement and a separate bare reference to
  the same crate, the existing edge-dedup-with-evidence-concatenation
  mechanism (spec §4.3) merges both strings onto one edge — confirmed
  directly against `crates/carto-mcp/src/tools/deps_tool.rs`, which
  carries `["external-package", "external-package-bare-reference"]`.
- **No `alias_to_declared` contribution.** This shape doesn't bind a
  local name the way `use` does, so it can never feed call-resolution
  tier (b) — the same reasoning `PackagePath`/`NamespaceImport` already
  documented for their own no-tier-(b) cases.
- **Rust-only for now.** Scoped to the demonstrated case; other
  languages' import models differ enough (and are typically
  declaration-only already) that revisiting is only warranted if a
  concrete instance shows up — same precedent §1.3 set for scoping
  grouped-use extraction to Rust first.
- **Residual false-negative risk, accepted.** A crate referenced only
  via a bare path whose leftmost segment happens to be capitalized
  (unconventional but not impossible) still won't be captured. INV-8
  favors missing over guessing wrong; this stays a known, accepted gap
  rather than a reason to widen the heuristic.

## Consequences

- Verified against carto's own repo: `carto deps carto_core . --dir in
  --json` now reports **exactly 19** files, matching `bench/tasks.md`'s
  L3 ground truth precisely. `crates/carto-cli/src/main.rs` appears
  with evidence `"external-package-bare-reference"`; all other 18 files
  retain their original `"external-package"` evidence, unchanged. A
  full external-module sweep over the repo's own `graph.json` found 32
  external modules, all genuine external packages across every
  language — no spurious `Module` node was fabricated from a local
  PascalCase type.
- MCP surface verified: a hand-fed `tools/call` for `deps` returns the
  same `main.rs` entry with the same evidence string in
  `structuredContent`.
- This closes the *data*-side gap completely: `deps carto_core --dir
  in` is now deterministically 19/19 regardless of which front end
  asks. Whether a given agent session *chooses* to run that query
  instead of (or in addition to, uncritically) its own grep sweep
  remains a real agent-strategy decision no data fix can force — spec
  §9.3 forbids imperative skill-file language that would try to force
  it either.
- `docs/STATUS.md`, `skill/carto.skill.md`, and `bench/cli-arm-prompt.md`
  each gain a factual note about this capability.

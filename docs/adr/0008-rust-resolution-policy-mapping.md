# 0008 — Rust-specific mapping of spec §5.3's resolution policy

**Status:** accepted · **Date:** 2026-07-31 · **Milestone:** M1.b.2a

## Context

Spec §5.3's resolution rules are written language-agnostically:

1. Imports: "resolve relative paths exactly (certain); package imports
   become `Module` nodes with `external: true`."
2. Calls: match against (a) same-file symbols, (b) symbols imported into
   the file, (c) exported symbols of same-package files — first match
   wins, `inferred`, evidence records which rule fired; multiple
   candidates ⇒ no edge, recorded under `unresolved_calls`.

Rust has no file-relative `import './x'` syntax and no single obvious
notion of "package" at the level carto operates (no `Cargo.toml`/
workspace parsing in v1). Implementing the Rust extractor required
turning these generic rules into concrete decisions. Spec §0 anticipates
this class of gap explicitly ("if a task appears to require... " — this
isn't an invariant conflict, just underspecified language mapping) and
asks for durable records of implementer judgment calls; this ADR is that
record, and the reference point for TS/Python/Go's extractors when they
need their own analogous mapping.

## Decisions

- **`mod foo;` → `imports` edge, `certain`.** Resolved to `<dir>/foo.rs`
  or `<dir>/foo/mod.rs` next to the declaring file — the common case.
  `#[path]` overrides and other edition-2018+ corner cases are out of
  scope (`crates/carto-core/src/lang/resolve.rs::resolve_mod_decl`).
  Unresolvable `mod` declarations (target file not walked/found) produce
  no edge and no error — same honesty principle as call resolution,
  applied to imports.
- **Inline `mod foo { .. }` is not extracted.** It's a namespace, not a
  file reference; spec's `imports` edge is about file/module
  relationships, and an inline module doesn't have one.
- **`use <path>;` root classification.** The path's leftmost segment is
  "internal" (feeds call-resolution tier b, no separate `imports` edge)
  if it's `crate`/`self`/`super`, or matches any top-level `mod` name
  declared anywhere in the walked repo; otherwise "external" → a
  `Module{external: true}` node (deduped globally by path), `imports`
  edge from the file, `certain` (spec §5.3: "certain that it's imported,
  target unresolved").
- **Only single-target `use` paths are extracted.** `use_list`
  (`use a::{b, c}`), `use_wildcard` (`use a::*`), and `use_as_clause`
  (`use a::b as c`) are not walked this slice — `resolve_use_tree`
  returns `None` for them, extracting nothing rather than guessing at a
  partial interpretation. Only `use crate::a::b::c;`-shaped paths (and
  bare `use crate_name;`) are handled.
- **Calls match by bare name, not qualified path.** `Type::method(...)`
  and `module::func(...)` (path-qualified calls — a `call_expression`
  whose `function` is a `scoped_identifier`) are not captured as
  call-sites at all this slice. Resolving them correctly needs
  qualifier-aware matching against `qualified_name`, not just `name` —
  a meaningfully bigger step than the plain-identifier/`receiver.method()`
  matching implemented now. `impl Order { fn summary(&self) }` is still
  extracted as a `Symbol` (proving impl-block extraction works); nothing
  in the fixture calls it via `Order::summary(...)` syntax, only
  `order.summary()` (a `field_expression` callee, which *is* captured).
- **"Same-package" (§5.3 rule 2c) = "same walked repo."** v1 doesn't
  parse `Cargo.toml`/workspace structure, so there's no narrower notion
  of "package" available. A repo indexed as one `carto index` invocation
  is the whole candidate pool for tier (c).
- **Tier (b) "imported into the file" doesn't verify the `use` path's
  full chain resolves to the matched symbol's actual file.** It checks
  two independent facts: (1) this file's `use` declarations name the
  callee's bare identifier, and (2) exactly one `pub` symbol anywhere in
  the repo has that name. If a repo had two same-named `pub` symbols in
  different modules and only one was genuinely imported, resolution
  fails (falls through to tier (c), which also requires uniqueness
  excluding the caller's own file, and if that's still ambiguous,
  produces no edge) rather than picking the wrong one. Deliberately
  conservative — consistent with INV-8 ("missing honestly beats
  guessing"): the cost of this simplification is a false negative
  (unresolved call), never a false positive (wrong edge).
- **Visibility (`pub`) gates tiers (b)/(c), not tier (a).** A private
  same-file call is ordinary; Rust itself wouldn't compile a private
  cross-file call, so tiers (b)/(c) only ever consider `pub` symbols
  (`RawSymbol.is_pub`, detected by a `visibility_modifier` child on the
  item node — not exposed as a named field in tree-sitter-rust's grammar).
- **`LangExtractor::extract`'s spec-literal `file: &FileCtx` parameter is
  simplified to `relpath: &str`.** Nothing implemented so far needs more
  file context than the path — Rust's `mod`/`use` resolution is a
  whole-repo concern handled by `resolve.rs`, which already has full
  file context independent of what `extract` receives. A richer
  `FileCtx` can replace this if a later language extractor (e.g. one
  that needs the file's own module identity, common in ESM/TS) actually
  needs one — avoids inventing an abstraction ahead of a real need.

## Consequences

- These decisions are Rust-specific but the *shape* of each decision
  (what counts as internal vs. external, what "same-package" means,
  which call syntaxes are in/out of scope) is exactly what TS/JS,
  Python, and Go's extractors will each need to decide for themselves —
  this ADR is the template, not a rule that transfers verbatim (e.g. TS's
  `import './x'` genuinely is file-relative, unlike anything in Rust's
  `mod`/`use` system).
- `fixtures/rust-crate` is deliberately built to exercise every tier
  (same-file, imported, same-package, unresolved) plus the impl-block/
  qualified-name path, so this mapping has real end-to-end test coverage,
  not just unit tests against hand-built `RawSymbol`/`RawImport` data.
- If a future need requires resolving `use_list`/`use_wildcard`/
  path-qualified calls, extend `resolve_use_tree`/`calls.scm`
  incrementally rather than redesigning — the "return `None`/don't
  capture, extract nothing" fallback already in place means adding
  support for one more shape doesn't change behavior for shapes still
  unhandled.

# 0011 — Python-specific mapping of spec §5.3's resolution policy

**Status:** accepted · **Date:** 2026-08-01 · **Milestone:** M1.b.2b

## Context

[ADR-0008](0008-rust-resolution-policy-mapping.md) mapped spec §5.3's
language-agnostic resolution rules onto Rust and named itself "the
template, not a rule that transfers verbatim." Python is the second
language extractor (M1.b.2b's first slice, chosen specifically because
it has real dotted-relative imports — `from ..pkg import x` — that
nothing in Rust's `mod`/`use` system has, forcing the right
generalization now rather than guessing it from Rust alone). This ADR
is that mapping, and records where Python's answer to the same question
genuinely differs from Rust's, not just restates it.

## Decisions

- **`RawImport` generalized to `Relative`/`Absolute`, not a third
  Python-specific variant.** (`crates/carto-core/src/lang/
  extractor.rs`.) Rust's `mod foo;` and `use` paths were renamed into
  this shape with no behavior change (proven by the full existing Rust
  test suite passing unchanged before any Python code was written).
  `imported_names` became `Vec<String>` (was `Option<String>`) because
  Python's `from pkg import a, b` — multiple names in one statement —
  is common enough that excluding it (the way Rust's rarer `use a::{b,
  c}` stays out of scope) would miss a large fraction of real Python
  tier-(b) opportunities.
- **`from . import pkg` / `from .pkg import a, b` levels_up
  convention: a single leading dot means "the current package" —
  `levels_up: 0`, zero directory levels up from the declaring file's
  own directory.** Each additional dot adds one more level (`from
  ..pkg import x` → `levels_up: 1`). This is `dot_count - 1`, not
  `dot_count` — an off-by-one I got wrong on the first pass (encoded
  into both the extractor and its own unit tests) and caught by
  checking it against Python's actual semantics before the fixture
  locked it in; `resolve_relative_import`'s directory-walk loop needed
  no change, only the dot-counting formula that feeds it. Regression
  tests in `resolve.rs` walk multiple real directory levels, not just
  unit-test the extractor's own dot count in isolation.
- **No visibility keyword in Python; a leading underscore is the
  is_pub proxy for tiers (b)/(c)**, exactly the role Rust's `pub` plays
  (tier (a), same-file, still doesn't care about it either). Applies
  uniformly to functions, classes, *and* methods — mirroring Rust,
  where a `pub fn` inside an `impl` block is tier-(c)-eligible the same
  way a bare `pub fn` is (this is exactly what makes `order.summary()`
  resolve via tier (c) in both the Rust and Python fixtures). Dunder
  methods (`__init__`) fall under "private" by this rule too — accepted
  as a minor over-restriction, since they're conventionally invoked
  implicitly (`Order(1)`, not `order.__init__()`) and this resolver
  would never match that call shape anyway.
- **Wildcard imports (`from x import *`) are not extracted** — same
  honesty-over-guessing precedent as Rust's `use_wildcard`. Comes for
  free from the query shape: only `name:` field children are captured,
  and `wildcard_import` is a distinct, unnamed node tree-sitter-python
  never gives a `name:` field, so it produces zero name captures rather
  than needing an explicit exclusion rule.
- **Module-level "constant" assignments (`X = 5`) are not extracted.**
  Python has no `const` keyword; treating an uppercase module-level
  name as a constant is a heuristic with no clear win for v1. Leaves
  `SymKind::Const` unused for Python, same as several Rust-only variants
  stay unused for other languages.
- **Decorated definitions keep their decorator line(s) in their
  symbol's range** — `@symbol.function`/`@symbol.class`/`@symbol.method`
  bind to the whole `decorated_definition` node, not just the inner
  `def`/`class`. This produced a real tree-sitter query gotcha (the
  same *class* of bug Rust's impl-block double-match was, but not the
  same fix): the bare, undecorated `function_definition`/
  `class_definition` patterns match a decorated def's *inner* node too
  (tree-sitter's generic patterns match anywhere, decoration or not),
  giving it a genuinely different byte range than the decorated-specific
  pattern's outer node — so the existing byte-range dedup (which
  assumes colliding matches share an exact range, true for Rust's
  impl-block case) can't merge them. Fixed by checking the bare match's
  own parent node kind directly (`crates/carto-core/src/lang/
  python.rs`'s `extract_symbols`): if a bare `function_definition`/
  `class_definition`'s immediate parent is `decorated_definition`, drop
  it — the decorated-specific pattern already has it covered, correctly
  ranged.
- **Module-qualified calls (`pkg.func()`) are *not* excluded the way
  Rust excludes `Type::method()`/`module::func()`.** This is a genuine
  difference from ADR-0008's precedent, not the same exclusion
  reapplied — worth stating plainly since the original plan for this
  slice assumed symmetry with Rust here before implementation showed
  otherwise. Rust's grammar has a distinct `scoped_identifier` node for
  a path-qualified call, syntactically different from `field_expression`
  (instance/self calls) — `calls.scm` simply never captures the former.
  Python's grammar has no such distinction: `pkg.func()` and
  `order.summary()` are both `attribute` expressions, indistinguishable
  without semantic (type) information carto doesn't have in v1. So
  every `X.name(...)` call captures `name` as a candidate regardless of
  what `X` is, and resolves through the same tiers as any other
  attribute call — an honest consequence of the grammar, not a decision
  to special-case away.
- **Every Python absolute import is classified `external`, even one
  that actually names a local top-level package.** `known_modules` (the
  set that lets Rust's `use foo::bar` recognize `foo` as an internal,
  locally-`mod`-declared name) is populated only from Rust's `mod`
  shape — Python has no explicit module declaration to collect the same
  way, so it's always empty for a pure-Python repo, and every `import
  x`/`from x import y` becomes a `Module{external: true}` node. A known
  v1 simplification, analogous to but distinct from Rust's own
  "same-package = same walked repo" one; revisit only if false
  "external" classifications on genuinely local packages show up in
  practice.
- **The `contains` edge's evidence now names the actual producing
  extractor** (`fe.origin`, e.g. `"lang-python@1"`) instead of a
  hard-coded `"extractor:rust"` — a real bug this slice's registry
  change surfaced (a Python symbol's `contains` edge was claiming
  `"extractor:rust"` before the fix) and corrected alongside it, not a
  new decision specific to Python.

## Consequences

- `fixtures/py-lib` is built to exercise every tier (same-file,
  imported, same-package, unresolved) plus the class/method
  qualified-name path and a two-level relative import, the same
  end-to-end proof standard `fixtures/rust-crate` set for Rust — see
  its own README for the exact scenario table.
- The query layer (`crates/carto-core/src/query/`) needed zero changes
  for Python — `carto where`/`deps`/`map` work against
  `fixtures/py-lib` unmodified, confirming M1.b.3 (the query layer) was
  built genuinely language-agnostic, not accidentally Rust-shaped.
- TS/TSX, JS, and Go each still need their own version of this ADR —
  this one is now the second data point (alongside ADR-0008) for what
  varies per language: relative-import semantics, what "exported"
  means, whether path-qualified calls are syntactically distinguishable
  at all, and what "same-package" should mean given that language's own
  build/dependency-manifest conventions.

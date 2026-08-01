# 0012 — PHP added to the v1 language set; PHP-specific mapping of spec §5.3's resolution policy

**Status:** accepted · **Date:** 2026-08-01 · **Milestone:** M1.b.2b

## Context

[ADR-0008](0008-rust-resolution-policy-mapping.md) (Rust) and
[ADR-0011](0011-python-resolution-policy-mapping.md) (Python) each map
spec §5.3's language-agnostic resolution rules onto one language, and
each names itself a template rather than a rule that transfers
verbatim. PHP is the third data point, added ahead of TS/TSX, JS, and Go
(spec §10's stated M1.b.2b order) on direct request. This ADR is both
the scope decision that required and the resolution-policy mapping
itself.

### Scope: PHP is added to the v1 language set

Spec §5.2 lists TypeScript/TSX, JavaScript, Python, Rust, Go, HCL,
YAML, JSON as the v1 set; §1.3 explicitly names "languages beyond the
v1 set (§5.2)" as a non-goal — "the design must allow adding grammars,
but do not add them." PHP was outside that set. This is a §1.3 scope
statement, not a §2 invariant — amendable, unlike an INV violation,
which this repo's conventions require surfacing instead of working
around. The spec itself is amended alongside this ADR (§5.2 gains PHP,
§1.3's non-goal wording is adjusted) rather than left to silently
contradict the code — the spec stays the normative, truthful
description of what carto does.

### Why PHP is a genuinely new data point, not mechanical repetition

PHP's import model is neither Rust's nor Python's. `use
App\Orders\Order;` names a fully-qualified symbol resolved at runtime
by composer autoloading (PSR-4) — there is no file-relative path
arithmetic (unlike Rust's `mod`/Python's `from .pkg import x`) and no
`Cargo.toml`/`node_modules`-style manifest to consult even if carto
parsed manifests (it doesn't, v1). But PHP files declare their
namespace explicitly (`namespace App\Orders;`), which is enough to
build a fully-offline, **exact** fully-qualified-name → file index
from the walked repo alone. That index, not a directory walk, is
`RawImport`'s third variant and this slice's central addition.

## Decisions

- **`RawImport` gains a third variant, `Qualified { fqn, bound_name }`,
  not a reinterpretation of `Relative`/`Absolute`.**
  (`crates/carto-core/src/lang/extractor.rs`.) Neither existing variant
  fits: `Relative` is path arithmetic from the declaring file's own
  directory (meaningless for a namespace); `Absolute` classifies
  internal/external by a bare root string with no repo-wide index
  behind it (would produce zero internal file→file `imports` edges for
  a pure-PHP repo — `deps`/`map` would show no file-level structure at
  all). `bound_name` is the alias if the `use` has one, else the FQN's
  last segment — the identifier tier (b) call resolution matches
  against.
- **Internal `use` resolves via a repo-wide FQN index
  (`resolve::resolve`'s `fqn_to_file`), built from every file's own
  `namespace` declaration plus its top-level declared symbol names**
  (methods excluded — a `use` never names a method). Three outcomes, in
  order: (1) exact FQN match → `imports` edge, `certain`, evidence
  `namespace-import`; (2) the FQN's root matches a namespace declared
  somewhere in the repo but no file declares that exact FQN → no edge,
  no node — internal but unresolvable, the same "missing honestly beats
  guessing" (INV-8) treatment Rust's unresolvable `mod` gets; (3) unknown
  root → external `Module{external: true}` node, deduped by root the
  same way `Absolute` imports already are, evidence `external-package`.
  A colliding FQN (two files illegally declaring the same one) keeps
  whichever file is encountered first rather than erroring — carto only
  reads source, it doesn't enforce PHP's own uniqueness rules.
- **All four call shapes are captured, including static calls
  (`Foo::bar()`, `scoped_call_expression`)** — a deliberate divergence
  from ADR-0008's Rust precedent, not the same exclusion re-applied.
  PHP's grammar *can* distinguish a static call from a member call the
  way Rust's `scoped_identifier` can (unlike Python, which has no such
  distinction at all — ADR-0011). But static calls are pervasive in real
  PHP (`self::helper()`, factory methods, `parent::__construct()`);
  excluding them the way Rust excludes `Type::method()` would drop a
  large fraction of real call edges into invisibility — not even
  `unresolved_calls`, since the call site itself would never be
  extracted. The other three shapes (`foo()`, namespace-qualified
  `App\Orders\foo()`, `$o->m()`/`$o?->m()`) are captured the same way
  Rust's and Python's plain/member calls are.
- **`require`/`include` are not extracted.** Their argument is an
  arbitrary runtime expression (string concatenation, a variable,
  `__DIR__ . '/x.php'`), not a static literal in the general case; the
  FQN index already covers the real dependency structure of any
  autoloaded (i.e. modern) PHP codebase. Deliberately modest, spec §5.3.
- **Real visibility keywords, not Python's underscore-prefix proxy.**
  PHP has `public`/`protected`/`private`; `is_pub` for a method is
  `false` iff a `visibility_modifier` child's text is `private` or
  `protected`, `true` otherwise (no modifier defaults to `public`,
  PHP's own rule). Top-level functions/classes/interfaces/traits/enums
  have no visibility keyword at all and are always `is_pub: true` —
  same role Python's non-underscore-prefixed top-level names play, but
  reached without a heuristic since PHP simply has no way to make a
  top-level declaration non-exported.
- **Methods are captured only via their class/interface/trait/enum
  parent** (four query patterns in `symbols.scm`, one per container
  kind), not by a generic `method_declaration` pattern plus byte-range
  dedup the way Rust's impl-block methods and Python's decorated
  definitions need. `method_declaration` is a distinct node kind from
  `function_definition` in PHP's grammar and only ever appears nested in
  one of those four container kinds, so each declaration node matches
  exactly one pattern — no double-match, no dedup map needed. The
  trade-off: an anonymous class's methods are never captured (anonymous
  classes have no `name:` field, so they never match any of the four
  container patterns) — a documented exclusion, not a bug.
- **`namespace_use_declaration`'s plain, aliased, `function`/`const`,
  and grouped forms are all handled** (`use App\Orders\Order;`, `use
  ... as X;`, `use function App\Orders\parseOrder;`, `use
  App\Orders\{Order, Handler as H};`) — decomposed in `php.rs`, same
  "handled more legibly in plain Rust" choice Rust's `use`-tree walk and
  Python's relative-import decomposition each made for their own import
  shapes. The `function`/`const` keyword itself is ignored: the FQN
  index doesn't distinguish import kinds. A class body's `use
  SomeTrait;` (trait-use) is a distinct node kind (`use_declaration`,
  not `namespace_use_declaration`) and is never matched by the imports
  query at all — not an exclusion that needed deliberate handling.
- **An aliased `use ... as X` import does not make a call written as
  `X(...)` resolve via tier (b).** `imported_names`/`pub_by_name`
  matching is bare-name-based throughout `resolve.rs`: the call site's
  own identifier (`X`) is checked against `pub_by_name`, which is keyed
  by the symbol's *declared* name (`parseOrder`), not its alias. This
  is a real, inherited limitation of the shared bare-name matching
  architecture — Python's own aliased `from x import y as z` has the
  exact same gap, just never exercised by `fixtures/py-lib` (no aliased
  import there calls the alias). Not fixed by this slice: doing so would
  mean resolving `imported_names` to a target identity up front rather
  than a flat name set, a bigger, cross-language change out of scope
  here. `parseOrder`'s own, unaliased `use function App\Orders\parseOrder;`
  does resolve via tier (b) normally — `fixtures/php-app` exercises that
  path, not the aliased one.

## A shared bug this slice's fixture caught, fixed for all three languages

`resolve.rs`'s call-to-symbol attribution used to check, independently
per symbol, whether a call's line fell inside that symbol's
`start_line..end_line` — with no check that no *smaller* symbol also
claimed it. Rust never surfaces this (an `impl` block is never itself a
`Symbol`, so no symbol's range ever nests inside another's). Python's
`class` *is* a `Symbol` whose range spans its methods' bodies, so the
bug was always latent there too — but `fixtures/py-lib`'s two methods
(`__init__`, `summary`) happen to contain no call sites, so it was never
exercised or caught. `fixtures/php-app`'s `Handler::handle()` does call
things, and real `carto index` output showed every one of `handle`'s
calls (and its `unresolved_calls`) duplicated onto the enclosing
`Handler` class symbol as well — caught by eyeballing real output
against a fixture, exactly the class of bug CLAUDE.md's own "gotchas"
section describes (the impl-block-method and decorated-definition
duplicates were caught the same way).

Fixed in `resolve.rs` by `assign_calls_to_innermost_symbol`: each call
site is assigned to the *smallest* (by line-span) symbol whose range
contains it, computed once per file before the per-symbol loop, rather
than checked independently per symbol. Language-agnostic, and verified
behavior-preserving for Rust and Python — the full pre-existing test
suite (153 `carto-core` tests, all CLI acceptance tests, both golden
files) passes unchanged, since neither fixture has a call site nested
inside a symbol that's itself nested inside another.

## Consequences

- `fixtures/php-app` is built to exercise every reachable tier
  (same-file, imported, same-package, unresolved) plus the class/method
  qualified-name path (`Order::summary`), the FQN-index hit/miss/unknown-
  root three-way split, and a static call — the same end-to-end proof
  standard `fixtures/rust-crate` and `fixtures/py-lib` set. See its own
  README for the exact scenario table.
- The query layer (`crates/carto-core/src/query/`) needed zero changes
  for PHP — `carto where`/`deps`/`map` work against `fixtures/php-app`
  unmodified, the same confirmation ADR-0011 recorded for Python.
- TS/TSX, JS, and Go each still need their own version of this ADR —
  PHP is now the third data point (alongside Rust and Python) for what
  varies per language, and specifically the first to show that "does
  this language's grammar distinguish path-qualified calls" and
  "should carto capture them anyway" are independent questions (Rust:
  distinguishable, excluded; Python: not distinguishable at all;
  PHP: distinguishable, captured anyway).

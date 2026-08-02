# 0016 — C# added beyond the v1 language set; C# mapping of spec §5.3's resolution policy

**Status:** accepted · **Date:** 2026-08-02 · **Milestone:** post-M1.b.2b (first language past the v1 set)

## Context

[ADR-0008](0008-rust-resolution-policy-mapping.md) (Rust),
[ADR-0011](0011-python-resolution-policy-mapping.md) (Python),
[ADR-0012](0012-php-resolution-policy-mapping.md) (PHP),
[ADR-0013](0013-typescript-javascript-resolution-policy-mapping.md)
(TypeScript/TSX/JavaScript), and
[ADR-0015](0015-go-resolution-policy-mapping.md) (Go) map spec §5.3's
language-agnostic resolution rules onto spec §5.2's v1 set, which
closed with Go. C# is the eighth extractor and the **first addition
beyond the v1 set**, added on direct user request (the user works in
C# daily) — the same kind of on-request scope amendment ADR-0012 made
for PHP, and one spec §12 explicitly anticipated ("additional
languages (Java/C# next)").

### Scope: C# is added to spec §5.2

Like PHP's addition, this is a §1.3 scope statement being amended, not
a §2 invariant being violated. The spec is amended alongside this ADR
(§5.2 gains C#, §1.3's wording now cites this ADR too, §12's
future-language line drops C#) rather than left to silently contradict
the code.

### Why C# is a genuinely new data point

C# is namespace-based like PHP — files declare `namespace Acme.Orders;`
explicitly, so PHP's offline FQN-index machinery
(`declared_namespace`/`fqn_to_file`, ADR-0012) transfers — but its
*import* model matches nothing prior: a plain `using Acme.Orders;`
imports a **namespace**, i.e. every file declaring it, not one
declaration (PHP's `use` names exactly one FQN), not a directory (Go's
import unit — C# namespaces have no required relationship to the
directory tree), with no path arithmetic (TS/JS/Python/Rust relative
shapes) and a meaningless bare root (`System` vs `System.Text.Json`
are different packages). And C#'s namespace separator is `.` where
PHP's is `\` — the shared FQN index needs to know which it's
composing with.

## Decisions

- **New `RawImport::NamespaceImport { path }` variant**
  (`crates/carto-core/src/lang/extractor.rs`), resolved in `resolve.rs`
  by **exact match against every file's own `declared_namespace`**, no
  project-file parsing (`.csproj` is never read — the same "no manifest
  parsing" precedent as `Cargo.toml`/`package.json`/`go.mod`). A hit
  fans out to **one `imports` edge per declaring file** (Go's
  per-package-file fan-out shape, keyed by namespace instead of
  directory), evidence `namespace-import`, `certain`. A miss whose
  root segment is a namespace root some file declares ⇒ **no edge, no
  node** (internal-but-unresolvable, INV-8 — PHP's outcome 2). Unknown
  root ⇒ external `Module` node keyed by the **full** namespace string
  (`System.Text.Json`, not a truncated `System` — Go's full-identity
  reasoning, ADR-0015, not npm's truncate-to-package convention).

  Like Go's `PackagePath`, `NamespaceImport` carries no
  `bound_name`/`ImportedName` and **never feeds tier (b)**: a
  namespace `using` binds no symbol name a call site could be matched
  against (it makes *all* of the namespace's types visible). Every
  resolving cross-namespace C# call therefore lands on tier (c) with
  `same-package` evidence even when reached through a real `using` —
  the same accepted cosmetic quirk ADR-0015 records for Go.

- **`LangExtractor::namespace_separator() -> &'static str`, default
  `"\\"`** (PHP's — the only namespace-based language before C#), C#
  overrides to `"."`. `resolve` composes and splits the shared FQN
  index with each file's own separator, so `App\Orders` (PHP) and
  `App.Orders` (C#) coexist without ever falsely matching each other —
  the separator spelling makes cross-language keys unequal by
  construction (pinned by a resolve unit test). Same opt-in-with-
  benign-default pattern as ADR-0015's `package_scope_is_directory()`.

- **The other two `using` shapes map to existing variants.**
  `using P = Acme.Orders.OrderParser;` (alias) is `Qualified { fqn,
  bound_name: alias }` — PHP's exact shape, resolved through
  `fqn_to_file` and feeding alias-aware tier (b) (ADR-0013 machinery,
  unchanged): `new P()` resolves to `OrderParser` with `imported`
  evidence. `using static Acme.Orders.Order;` names a *type*, so it's
  also `Qualified` (bound to the FQN's last segment); its
  member-binding behavior is not modeled. `global using` parses as a
  plain using in its own file; its repo-wide scope effect is not
  modeled. Both deliberately modest (§5.3).

- **`internal` counts as exported (`is_pub`)** — confirmed with the
  user. carto's "same package" already means "same walked repo"
  (ADR-0008's simplification), which approximates one
  assembly/solution — exactly the visibility scope `internal` grants.
  Treating it as non-exported would push most real app-internal call
  edges into `unresolved_calls`. Only `private`/`protected`/`private
  protected`/`file` are non-exported. No modifier at all follows C#'s
  own contextual defaults: namespace-level types are internal
  (exported here), interface members are public, everything else
  nested in a type is private.

- **`new Foo(...)` object creation is a call site, resolving to the
  constructed *type*'s symbol** — confirmed with the user; the same
  pervasiveness argument that made PHP capture static calls (ADR-0012)
  where Rust excludes path-qualified calls (ADR-0008): constructor
  invocation is a large share of real C# dependency structure.
  Captured shapes otherwise mirror Go/PHP/TS precedent:
  `invocation_expression` with bare, member-access (rightmost
  identifier; qualifier discarded, ADR-0015's consequence),
  conditional-access (`o?.M()`), and generic (`Helper<int>()`) callees.

- **Constructors are deliberately NOT extracted as symbols** — the one
  place implementation contradicted this ADR's own first draft, caught
  while designing the fixture, *before* the first real run: a C#
  constructor shares its type's name, so a constructor symbol would
  make every `new Foo()` (and same-file `Foo` reference) **ambiguous**
  between type and constructor under bare-name resolution — INV-8
  would then honestly drop exactly the construction edges capturing
  `new` exists to produce. PHP's `__construct` and Python's `__init__`
  are safely extractable only because they have their own distinct
  names; C#'s doesn't. A constructor body's calls are attributed to
  the enclosing *class* symbol instead (innermost containment —
  `fixtures/csharp-app`'s `Order -> Stamp` edge pins this), and `new
  Order(...)` resolves to the class even when an explicit constructor
  exists (`Parse -> Order` pins that).

- **Both namespace declaration forms are read** (file-scoped
  `namespace X;` and the block form); a file with multiple namespace
  blocks keeps the first-declared one (`ExtractOut` carries one
  `declared_namespace`; multi-namespace files are rare and
  discouraged in modern C#). Symbols: classes, interfaces, structs,
  enums, records (`record`/`record class` → Class, `record struct` →
  Struct, decided from the node's own anonymous `struct` token),
  delegates (→ Type), methods, and `const` fields (one symbol per
  declarator, `Type.Member` qualified names — C#'s own member-access
  spelling, like Go's `Order.Summary`).

## Not extracted, deliberately (documented exclusions, not bugs)

- **Properties** — accessors, not invocables; property access isn't an
  `invocation_expression`, so there'd be no call edges to them anyway.
  Same modesty as Python's module-level assignments (ADR-0011).
- **Events, indexers, operators, destructors, enum members,
  non-`const` fields, local functions** (a distinct node kind,
  `local_function_statement` — like Go's function-local declarations).
- **Partial classes' cross-file private visibility** — a `private`
  member called from the same class's other partial file lands in
  `unresolved_calls`. The one C# case tier (c)'s exported-only rule
  misses; honest omission.
- **`.csx` scripts** — only `.cs` maps to `Lang::CSharp`.

## What building `fixtures/csharp-app` caught

The constructor-ambiguity defect above — notably caught by *designing*
the fixture (tracing which resolution outcome each planned call site
must produce) rather than by running it, one step earlier than the
eyeball-real-output gotcha usually bites. After that fix, the first
real `carto index` run matched the design exactly: every symbol and
`sym_kind`, all four evidence labels, the two-edge namespace fan-out,
the full-string external module identities, the `Acme.Reports`
known-root omission, and the three honest `unresolved_calls`
(`WriteLine`, `NewGuid`, `Trim`).

## Post-merge fix: cross-language index collisions

A `/code-review` pass over this slice found that the three indices this
ADR introduced (`known_namespace_roots`, `fqn_to_file`,
`namespace_to_files`) were keyed by bare namespace/FQN strings, not by
language. This ADR's own claim that "the separator spelling makes
cross-language keys unequal by construction" is true for multi-segment
names but false for a **root segment or a single-segment name**, which
contains no separator at all: a PHP `namespace App;` and a C# `using
App;` collided exactly, and a PHP `namespace System\Legacy;` (root
`System`) could silently absorb an unrelated C# `using System.Text;`
into "known internal root, no exact match" (no edge) instead of the
correct external-module outcome. Separately, `RawImport::Qualified`'s
external fallback always truncated to the FQN's root segment — correct
for PHP's Composer-style dedup, but wrong for C#, where it contradicted
this ADR's own full-identity policy for `NamespaceImport`.

Fixed by keying all three indices on `(fe.origin, ...)` — the actual
per-language discriminant already on every `FileExtraction` — and by
adding `LangExtractor::qualified_external_is_full_fqn()` (default
`false`, PHP unchanged; `true` for C#) so the `Qualified` external
fallback follows the same full-FQN policy `NamespaceImport` already
had. Neither `fixtures/php-app` nor `fixtures/csharp-app` exercises the
collision path (both are single-language), which is exactly how this
went unnoticed until reviewed; three new `resolve.rs` unit tests pin
the cross-language cases directly. See the commit fixing this for the
full detail; not worth a separate ADR since no new decision was made,
only a bug in this one's own stated policy.

## Consequences (query layer)

`crates/carto-core/src/query/` needed zero changes — the same
confirmation every prior language's ADR has recorded, now true for
eight languages. `carto where`/`deps`/`map` work against
`fixtures/csharp-app` unmodified.

`tree-sitter-c-sharp` 0.23 joins `carto-grammars` behind the same
`native-grammars` feature (spec §13 dependency addition, verified
against crates.io at implementation time: its runtime deps are only
`tree-sitter-language ^0.1` and `cc ^1.1`, both already in-tree —
cargo-deny's `multiple-versions = "deny"` passes with no new skip).

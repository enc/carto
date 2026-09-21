# 0030 — Type-position capture, per language (amends ADR-0029)

**Status:** accepted · **Date:** 2026-08-05 · **Milestone:** post-S-1
field feedback

## Context

ADR-0029 records the cross-cutting mechanism (`RawTypeRef`,
`EdgeKind::References`, resolution reuse, why an unresolved ref is
dropped rather than counted). This ADR is the per-language survey: six
grammars' worth of "which shapes are captured, which are deliberately
excluded, and what broke the first time it was tried" — kept in one
document rather than six near-identical ones (unlike ADR-0008/0011/
0012/0013/0015/0016's one-ADR-per-language precedent, which record
§5.3 **resolution-policy** mappings; resolution is unchanged here, this
is purely a capture-surface decision, and six copies of the same
"here's the query file, here's the descent function" shape would
obscure that it's one decision, not six).

Every language's implementation follows the same split, established
by `imports.scm`'s own precedent for PHP/C#: a `.scm` query file
captures only the *type-position container node* (anchored on a
distinct field, or on a node kind that cannot collide with anything
else), and a small plain-Rust `collect_type_names()` walks that
captured subtree down to head identifiers. Two judgment calls recur
across all six and are recorded once, here, rather than per language:

- **Every `field: (_)` capture uses the tree-sitter wildcard, never a
  literal supertype name** (`type`, `_type`, `_simple_type`, ...).
  C#/Rust/TS/PHP's own "type" grammar rule is a *hidden* supertype —
  verified against each grammar crate's own `node-types.json`: it has
  `subtypes`, not `children`, meaning the concrete subtype is what
  actually appears in the parse tree, never a node of that supertype's
  own name. A query pattern naming a supertype directly depends on
  query-engine supertype-alias support this crate hasn't otherwise
  relied on; `(_)` sidesteps the question entirely. (Python's own
  `type` node is the one exception — a genuine concrete wrapper,
  confirmed empirically — but the wildcard is still used there too,
  for reasoning uniformity across all six files rather than because
  it's required.)
- **A node whose children are already captured by a separate,
  unanchored top-level pattern must never be recursed into a second
  time from elsewhere.** This is CLAUDE.md's tree-sitter double-match
  gotcha, generalized from "the same query pattern matches a node
  twice" to "two different, individually-correct capture paths reach
  the same node." It bit twice during implementation, both caught by
  running real extraction and inspecting counts rather than trusting
  the query compiling cleanly: Go's named multiple-return-value list
  (`func f() (a int, b string)`) is a `parameter_list` of
  `parameter_declaration`s, each already reached by the file-wide
  unanchored `parameter_declaration` pattern — recursing into a
  `result:`-captured `parameter_list` too would have double-counted
  every named return type. TS's tuple-type named member
  (`[x: Foo, y: Foo]`) is a `required_parameter`/`optional_parameter`
  node whose own `type:` field is a `type_annotation`, already reached
  by the file-wide unanchored `type_annotation` pattern — recursing
  into it from `tuple_type`'s own descent would have double-counted
  the same way. Both are now regression-tested by name
  (`named_multiple_return_values_are_captured_exactly_once`,
  `named_tuple_member_is_not_double_counted`).

## Decisions, per language

**C#** (`queries/csharp/types.scm`, `csharp.rs::collect_type_names`) —
the language that motivated this work; see ADR-0029's Context and
`fixtures/csharp-app/Ports/`+`Services/` for the exact reproduction.
Captures: `base_list` (whole node, not per-child — its one
record-primary-constructor shape, `primary_constructor_base_type`,
is walked by the same descent rather than a second pattern that would
double-match a `base_list` containing one), `variable_declaration`/
`parameter`/`property_declaration`/`delegate_declaration`/
`type_parameter_constraint`/`catch_declaration`'s `type:` field,
`method_declaration`'s `returns:` field, `cast_expression`/
`typeof_expression`/`as_expression`'s operand, and generic type
arguments on invocations/constructions (`services.AddSingleton<
IQueryJobStore, X>()`, `new Dictionary<K, V>()`) via the concrete
`type_argument_list` node — anchored there specifically, not on the
surrounding `invocation_expression`/`object_creation_expression`, so
it can't collide with `calls.scm`'s own patterns on those same outer
nodes. **Deliberately excluded:** `object_creation_expression`'s own
non-generic `type:` head (`new Order()`) — `calls.scm` already turns
that into a `calls` edge (ADR-0016); capturing it here too would
double-represent one call site as both a call and a reference.

**Rust** (`queries/rust/types.scm`, `rust.rs::collect_type_names`) —
the first thing in this crate that makes `deps SomeTrait --dir in`
work at all; orthogonal to ADR-0008's `Type::method()` call exclusion,
which is unchanged. Captures: struct/tuple-struct fields, return
types, parameters, `let` annotations, `impl`'s trait and Self type,
const/static types, generic bounds (`<T: Bound>` and `where T: Bound`
both parse to one `trait_bounds` node — a single unanchored pattern
covers both without per-context anchoring), and turbofish generic
arguments — which turned out to be **two distinct parse shapes**,
confirmed against a real parse tree after an initial single-pattern
implementation silently missed one: a plain-call turbofish
(`parse::<Order>(...)`) is a `generic_function` node, but a turbofish
on a path *segment* before a final associated-item access
(`Vec::<Order>::new()`) instead puts a `generic_type` inside the
outer `scoped_identifier`'s own `path:` field — needs its own second
pattern. A qualified path (`crate::orders::Order`) yields only its
rightmost segment. Additive to, not a replacement for, ADR-0024's
existing `path.ref`/`RawImport::BareReference` capture — genuinely
different questions (module-level "this file references an unimported
crate path" vs. symbol-level "this declaration names that type")
answered from the same source text via two different `ExtractOut`
fields with no interaction.

**Go** (`queries/go/types.scm`, `go.rs::collect_type_names`) —
`field_declaration`/`parameter_declaration`'s `type:` field is
captured **unanchored** (matches anywhere in the file, not just at
top-level declarations), which is exactly what makes a second pattern
unnecessary for named multiple returns (see the double-match section
above). Also captures function/method results, `type` specs, type
assertions/conversions, `var`/`const` types, composite literal types,
and an interface's own embedded-interface list (`type Foo interface {
Bar }`) — anchored specifically on `interface_type`'s own direct
`type_elem` children, *not* a bare unanchored `(type_elem)` pattern,
since a generic type's own arguments (`Container[Order]`) are *also*
wrapped in `type_elem` nodes, reached instead through `generic_type`'s
own `type_arguments` field — a global pattern here would have
double-captured those too. **Deliberately not recursed into at all:**
`struct_type`/`interface_type`/`function_type`, anywhere they appear
(a `type_spec`'s type, a composite literal's type) — their own
`field_declaration`/`type_elem` children are already independently,
globally captured, so recursing into them a second time from any other
entry point would double-count. Go's builtins (`int`, `string`, ...)
have no distinguishing node kind in this grammar — unlike every other
language here, they're captured the same as any other name, not
filtered out.

**TypeScript/TSX/JavaScript** (`queries/ecma/types.scm` shared +
`types_ts.scm` TS/TSX-only + `types_js.scm` JS-only,
`ecma.rs::collect_type_names`) — mirrors the existing `symbols.scm`/
`symbols_ts.scm` split, but needed a **third** file this time: `class X
extends Y` cannot be a single shared pattern the way it first looked.
TS's grammar wraps the superclass in an intervening `extends_clause`
node (so `class_heritage` can also hold a sibling `implements_clause`);
plain JS's grammar holds the superclass `expression` directly under
`class_heritage` with no such wrapper at all. Neither pattern compiles
against the other grammar (`tree_sitter::Query::new` fails on a node
kind the target grammar doesn't define) — confirmed empirically after
the first, single-file attempt failed to compile against plain JS.
`new Foo(...)` *is* safely shared (both grammars name the field
`constructor:` identically) and is genuinely new signal in all three
languages, not a double-representation: `calls.scm` only ever matches
`call_expression`, never `new_expression`, so object construction
produced zero edges before this in TS, TSX, *and* JS alike. TS-only:
the single broad `type_annotation` pattern (covers every parameter/
property/variable/return annotation in one unanchored pattern),
`implements_clause`, interface `extends`, generic arguments on calls
and `new`, and `as`/`satisfies` (no named field for their own `type`
child in this grammar — `children: [expression, type]`, both
positional — so the *last* named child is taken instead of a
field-name match).

**PHP** (`queries/php/types.scm`, `php.rs::collect_type_names`) — two
shapes captured here have **zero** signal anywhere else in this
extractor, verified against `calls.scm`/`imports.scm` directly rather
than assumed: `object_creation_expression` (`new Foo()`; PHP's
`calls.scm` only matches `function_call_expression`/
`member_call_expression`/`nullsafe_member_call_expression`/
`scoped_call_expression`, the same gap TS/JS's `new_expression` had),
and trait `use SomeTrait;` inside a class body — a genuinely different
node kind (`use_declaration`) from namespace `use`
(`namespace_use_declaration`), which `imports.scm`'s own comment
already noted was never matched by anything. `object_creation_expression`
has no named field for the constructed type at all (`name`/
`qualified_name`/`relative_name` are unnamed alternatives among many
possible children including the constructor arguments) — each matched
by its own concrete node kind directly; a query pattern matches only
*direct* children by default, so this can't reach into a constructor
argument's own nested names. Also captures property/parameter/
promoted-constructor-property types, return types, `catch` clause
types, and `extends`/`implements` lists. PHP 8.2's disjunctive-normal-
form type (`(A&B)|C`) is a rare enough shape that its own nested unions
aren't walked this slice — documented exclusion.

**Python** (`queries/python/types.scm`, `python.rs::collect_type_names`)
— every pattern anchors on a field local to a `typed_parameter`/
`typed_default_parameter`/`function_definition`/`assignment`/
`class_definition`, so `collect_type_names`'s recursion into
`attribute`/`generic_type`/`argument_list` is reached only from an
already-anchored subtree, never from an unanchored global pattern —
unlike Go, there's no risk here of an ordinary runtime subscript or
call expression elsewhere in the file being mistaken for a type
reference. The one genuine surprise, caught by inspecting a real parse
tree after an initial implementation silently captured nothing: a
subscripted generic in annotation position (`dict[str, Foo]`) does
**not** parse as `subscript` the way the same-looking runtime
expression would — it parses as `generic_type (identifier)
(type_parameter (type ...) (type ...))`, a shape specific to
annotation context in this grammar version. The `subscript` descent
arm stays anyway as a defensive fallback for any annotation shape that
does resolve to it. A bare string annotation (`"Foo"`, a forward
reference) is **silently skipped, not resolved** — user-confirmed:
Python's grammar gives it no distinguishing shape from any other
string literal, and guessing that a given string is a forward
reference rather than ordinary data would be exactly the kind of guess
INV-8 exists to avoid. Like Go, Python's builtins (`str`, `int`,
`dict`, ...) have no distinguishing node kind and are captured the
same as any other name.

**HCL** — no type system; unchanged, no `types.scm` file.

## Consequences

- The skill doc's known-gaps list needed a new entry: an unresolved
  type ref is dropped (not counted, unlike calls); `var`/`:=`-inferred
  types (C#, Go), Python string forward references, and PHP's DNF
  types are not attempted.
- Each language's own fixture gained at least one type-reference case
  and a README row documenting it, so the fixture's own
  file→behavior map (per `CLAUDE.md`'s fixture convention) stays
  complete.
- Any future language extractor should expect this same two-step
  shape (query file + `collect_type_names`) and should verify its
  generic/turbofish/subscript-in-annotation shapes against a real
  parse tree before writing the descent function — three separate
  wrong assumptions were caught exactly that way while implementing
  this ADR, not by the query failing to compile or a type error, but
  by a passing-looking test asserting the wrong count.

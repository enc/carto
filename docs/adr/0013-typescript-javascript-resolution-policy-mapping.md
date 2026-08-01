# 0013 — TypeScript/TSX/JavaScript mapping of spec §5.3's resolution policy; alias-aware tier (b)

**Status:** accepted · **Date:** 2026-08-01 · **Milestone:** M1.b.2b

## Context

[ADR-0008](0008-rust-resolution-policy-mapping.md) (Rust),
[ADR-0011](0011-python-resolution-policy-mapping.md) (Python), and
[ADR-0012](0012-php-resolution-policy-mapping.md) (PHP, which also
added PHP to spec §5.2's v1 set) each map spec §5.3's language-agnostic
resolution rules onto one language. TypeScript, TSX, and JavaScript —
already in §5.2's v1 set, STATUS.md's "Next" slice after PHP — are the
fourth, fifth, and sixth data points, and the first case where three
`Lang` variants share one extractor module instead of getting one
each. This ADR is that mapping, plus a cross-cutting fix
(alias-aware tier (b) resolution) this slice's real-world import shape
made necessary to do properly rather than defer a third time.

## Part 1: the alias-resolution fix

ADR-0012 documented, but deliberately didn't fix, a real gap: an
aliased import (`from x import y as z`, `use Foo as X;`) never made a
call written as the alias resolve via tier (b), because `resolve.rs`
matched a call site's own identifier against both "is this name
imported" *and* `pub_by_name` using the same string — correct only
when the alias and the declared name coincide. TypeScript/JavaScript's
`import { foo as bar } from './x'` is common enough that shipping this
extractor with that gap unfixed would have made it meaningfully less
useful than Rust/Python/PHP's own extractors, so this slice fixes it
generically instead of adding a fourth documented instance of the same
gap.

- **`RawImport::Relative`/`Absolute`'s `imported_names: Vec<String>`
  becomes `Vec<ImportedName>`**, a `{ bound_name, declared_name }` pair
  (`crates/carto-core/src/lang/extractor.rs`) — equal for an unaliased
  import, different for an aliased one. `RawImport::Qualified` (PHP)
  needed no struct change: its `fqn`'s own last segment already *is*
  the declared name.
- **`resolve.rs`'s tier (b) collapses to one `alias_to_declared: Vec<
  BTreeMap<&str, &str>>` lookup** (bound → declared, per file) instead
  of a separate "is this name imported" set plus a same-string
  `pub_by_name.get`. A call site's identifier is looked up once; the
  map's *value*, not the call site's own spelling, is what's checked
  against `pub_by_name`.
- **Per-language impact was small everywhere except Python**: Rust's
  `use` never aliases (`use_as_clause` excluded, ADR-0008), so
  `bound_name == declared_name` unconditionally there — no behavior
  change, proven by the full pre-existing suite passing unchanged.
  Python's whole-module `import x as y` has no separate declared
  symbol name to speak of either. Python's `from x import y as z` is
  where the real, previously-broken case lived — now reads both the
  `aliased_import` node's `name` (declared) and `alias` (bound) fields
  instead of discarding the former.
- **Still a known limit, not claimed fixed**: `alias_to_declared` only
  helps when the call site spells the *local* alias — it doesn't (and
  structurally can't) help a caller resolve a symbol whose target file
  itself never declared the name under any string the caller could
  match against `pub_by_name`. This isn't a new limitation; it's just
  now scoped precisely to "no declared name exists to match," not "the
  matching logic can't handle aliases at all."

## Part 2: TypeScript/TSX/JavaScript extraction

### One module, mostly-shared queries — and where that breaks down

`crates/carto-core/src/lang/ecma.rs` houses three `LangExtractor`
impls sharing extraction functions, parameterized by which
`tree_sitter::Language` to use. Verified TypeScript, TSX, and
JavaScript's `function_declaration`/`class_declaration`/
`import_statement`/`call_expression`/`method_definition` node shapes
are structurally identical — a deliberate deviation from the "one
`queries/<lang>/` dir per language" convention, since these three are
dialects of one grammar family, unlike Rust vs. Python vs. PHP, which
are genuinely distinct languages.

That sharing has a real, non-cosmetic limit:
`tree_sitter::Query::new` **fails to compile** a query referencing a
node kind the target grammar doesn't define *at all* — even inside an
unused alternation branch. Two consequences, both empirically
discovered while implementing (not predicted in the plan up front):

- TS-only symbol kinds (`interface_declaration`, `enum_declaration`,
  `type_alias_declaration`) live in `symbols_ts.scm`, run as a second
  query pass only for TypeScript/TSX — plain JavaScript's grammar has
  none of these three node kinds, so a single fully-shared
  `symbols.scm` genuinely cannot include them.
- A class's own `name` field is `identifier` in JavaScript but
  `type_identifier` in TypeScript/TSX — and JavaScript's grammar has
  no `type_identifier` node kind whatsoever, so even
  `name: [(identifier) (type_identifier)] @symbol.name` (an
  alternation, which works fine when *both* branches' node kinds exist
  in the grammar being compiled against) fails to compile for
  JavaScript specifically. Class and method patterns split into
  `symbols_class_identifier.scm` (JavaScript) and
  `symbols_class_type_identifier.scm` (TypeScript/TSX) instead of one
  shared file.

`imports.scm` and `calls.scm` stay fully shared, one file each — every
node kind they reference exists in all three grammars.

### Symbols

Function/class/method declarations, plus TS-only interface/enum/
type-alias declarations, plus (confirmed with the user before
implementing, given how much idiomatic exported TS/JS code is written
this way) **top-level `const`/`let`-bound arrow-function and
function-expression assignments** (`export const foo = () => {...}`).
The query pattern nests under `(program ...)` directly — a plain
top-level statement, or one `export_statement` deep — so a callback
passed to some other call three levels down is never captured; a
structural constraint, not a runtime check.

**`is_pub` is a genuine departure from every prior language.** PHP has
no way to make a top-level declaration non-exported at all, so it was
unconditionally `true` there. TypeScript/JavaScript *does* have real
non-exported module-private top-level declarations — deciding `is_pub`
needs (a) walking each matched declaration's own ancestry for a direct
`export_statement` wrapping (`function foo(){}` vs. `export function
foo(){}`, and for the const-arrow case one hop further up through its
`lexical_declaration` parent, handled uniformly by the same walk
without special-casing which item kind started it), plus (b) a
file-wide scan of bare `export { foo, bar as baz };` clauses, keyed by
each specifier's *local* `name` field, not `alias` (the external-facing
name a re-export can rename to — irrelevant to whether the local
declaration itself is reachable).

For methods: TS's `accessibility_modifier` (`private`/`protected`)
plays PHP's `visibility_modifier` role. Additionally — and this has no
PHP/Python equivalent — a method name captured as
`private_property_identifier` (real JS/TS `#foo(){}` private-field
syntax) is **never** `is_pub`, regardless of any modifier: a hard
language guarantee, not a convention the way Python's underscore
prefix is.

One minor, accepted cosmetic consequence: a symbol's `signature` spans
its own declaration node's byte range, which for `export function
foo(){}` does *not* include the `export` keyword (unlike Rust, where
`pub` is a child of the item node itself, so `pub fn foo()` is the
node's own text). `is_pub` is still computed correctly; the signature
text just doesn't visually echo it.

### Imports — no new `RawImport` variant

TS/JS relative specifiers (`./orders`, `../orders`,
`../../shared/orders/handlers`) decompose directly into the *existing*
`RawImport::Relative` shape: each leading `../` segment is one more
`levels_up` (the same convention Python's dot-counting already uses),
and whatever remains after stripping a single leading `./`/`../`
becomes `module_path` — which may itself contain internal slashes
(`"shared/utils"`), already tolerated as an opaque suffix by
`resolve::resolve_relative_import`, which needed only one new
extension-keyed candidate-list arm (`ts`/`tsx`/`js`/`jsx`/`mts`/`cts`/
`mjs`/`cjs` declaring files try, in order, `.ts`, `.tsx`, `.js`,
`.jsx`, `/index.ts`, `/index.tsx`, `/index.js`, `/index.jsx` — TS-first
priority, first match wins; real bundler/tsconfig-driven resolution
can differ, out of scope). Bare specifiers (no leading `.`) go through
`RawImport::Absolute`, with one real TS/JS-specific wrinkle: a scoped
package (`@scope/pkg`, `@scope/pkg/subpath`) roots at its first *two*
`/`-separated segments, not one — npm's own scoping convention, with
no equivalent in Rust's, Python's, or PHP's package namespaces. No
`package.json`/`node_modules` parsing in v1, so every `Absolute`-
classified TS/JS import is external — the same simplification Python's
and PHP's absolute imports already made, for the same reason.

**Default and namespace imports contribute no `ImportedName`.**
`import Foo from './x'` and `import * as ns from './x'` bind a local
name the *importer* chose, not a name the target file declares under
that exact spelling — there's nothing verifiable to pair with
`declared_name`. This is not a regression from the alias fix above;
it's an honest "nothing to check" rather than a wrong guess. A call to
`Foo(...)` still falls through to tier (c) or lands honestly in
`unresolved_calls` — and *will* resolve via tier (c) if the target
file's own top-level declaration happens to share that exact name,
which is not a special case, just tier (c) doing what it always does.

**Re-exports (`export { foo } from './x'`, confirmed in scope) also
contribute no `ImportedName`** — for a different reason: a re-export
never creates a binding usable by a call site in *this* file at all
(it only forwards the name onward to whoever imports from this file),
so there's no local identifier to feed tier (b) with in the first
place. Only the file-level `imports` edge is meaningful.

**A resulting, accepted evidence-label quirk**: `resolve.rs`'s
`Relative` branch picks its evidence string by checking whether
`imported_names` is empty (`"mod-declaration"`, Rust's `mod foo;`
shape) vs. non-empty (`"relative-import"`). Since default imports,
namespace imports, *and* re-exports all produce empty
`imported_names` for the reasons above, they all get
`"mod-declaration"` evidence — a label that reads Rust-specific even
though nothing Rust-related happened. Functionally correct in every
case (a `certain` file-to-file edge to the right target), just a
naming artifact of reusing the existing branch rather than threading a
fourth discriminant through `RawImport::Relative` for what would be a
purely cosmetic improvement. Not fixed this slice.

### Calls

Plain (`foo()`) and member (`x.foo()`, including both instance calls
and module-namespace calls like `ns.foo()` from `import * as ns` — the
grammar can't tell them apart, the same situation Python's attribute
calls are in, not Rust's/PHP's, since `member_expression` has no
separate "static/scoped" call node the way PHP's
`scoped_call_expression` does). Optional chaining (`x?.foo()`) needs
no separate pattern: `?.` is carried on an `optional_chain` field
alongside `object`/`property`, which doesn't change either field's own
shape, so the member-call pattern already matches it.

**Deliberately out of scope this slice** (confirmed with the user
before implementing): CommonJS (`require()`/`module.exports`) — ESM
`import`/`export` only, same "extract nothing rather than partially
interpret" precedent as every prior exclusion.

## What building `fixtures/ts-app` caught (not unit-tested in isolation)

Two real issues surfaced only by eyeballing actual `carto index`
output against the fixture, CLAUDE.md's documented gotcha class — the
same way the Rust impl-block, Python decorated-definition, and PHP
call-attribution issues were each caught:

- **A fixture design mistake, not an extractor bug**: the fixture
  originally imported a non-exported symbol under an alias
  (`validate as v`, where `validate` had no `export` keyword) — invalid
  in real TypeScript/JavaScript, since you cannot import something that
  was never exported. The call correctly landed in `unresolved_calls`
  for the *right* underlying reason (`validate` was never in
  `pub_by_name`), but for the wrong *intended* reason — it would have
  looked exactly like the alias fix hadn't worked. Fixed by exporting
  `validate` in the fixture, not by touching `resolve.rs`.
- **A naming collision, not an extractor bug**: the fixture had a
  function literally named `log` in the same file that also called
  `console.log(...)` — `calls.scm` captures member calls by property
  name only (`log`), so this produced a spurious same-file self-call
  edge purely by coincidence. The same category of grammar ambiguity
  Python's attribute calls already accept (ADR-0011); fixed by
  renaming the fixture's function to `logOrder`, not by trying to
  special-case `console.*` in the extractor (which would just move the
  same ambiguity to the next coincidental collision).

## Consequences

- `fixtures/ts-app` mixes `.ts`, `.tsx`, and `.js` deliberately —
  proving all three `Lang` variants work and cross-resolve imports
  against each other (a `.tsx` file importing a `.js` file), not just
  same-language relative imports. See its own README for the full
  scenario table, including the alias fix exercised end-to-end (not
  just unit-tested against `resolve.rs` in isolation).
- The query layer (`crates/carto-core/src/query/`) needed zero changes
  — the same confirmation every prior language's ADR has recorded.
- Go remains the only unimplemented language from spec §5.2's original
  v1 set; STATUS.md's "Next" section is updated accordingly.

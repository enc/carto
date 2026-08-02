# carto — status / handoff

**Milestone:** M1.b.2b done (Rust, Python, PHP, TS/TSX/JS, Go — spec §5.2's full v1 language set), plus C# added post-v1 on user request (ADR-0016)

Where the implementation is in the milestone sequence, what's deliberately
absent, and what's next. Everything else lives elsewhere on purpose:

| For | Read |
|---|---|
| What the product is, normatively | [`docs/carto-design-spec.md`](carto-design-spec.md) (§2 invariants override everything) |
| Architecture, commands, conventions, gotchas | [`CLAUDE.md`](../CLAUDE.md) |
| Why a given decision was made | [`docs/adr/`](adr/) |
| What changed and when | `git log` |

## Milestone position

Spec §10's milestones proved too large to plan and execute as single units,
so they've been subdivided. Current state:

- **M1.a** — invariant substrate (consts, error, taint, pathguard, outdir). Done.
- **M1.b.1** — walk, graph store, persist choke-point, `carto index`. Done.
- **M1.b.2a** — Rust extractor end-to-end: symbols, imports, calls, full
  spec §5.3 resolution policy. Done.
- **M1.b.3** — `where`/`deps`/`map` commands, over the `QueryGraph`
  adjacency index (ADR-0009/0010). Done. Taken ahead of M1.b.2b — no new
  deps/grammar work, and it closes ADR-0005's traversal deferral against
  real requirements instead of speculatively.
- **M1.b.2b** — TS/TSX, JS, Python, Go, PHP extractors (PHP added to the
  v1 set post-M1.b.2a, ADR-0012). **Python done** (ADR-0011), **PHP
  done** (ADR-0012), **TS/TSX/JS done** (ADR-0013, which also fixed a
  cross-language alias-resolution gap ADR-0012 had documented but left
  unfixed), **Go done** (ADR-0015 — the closing slice; new
  `RawImport::PackagePath` for Go's directory-shaped, module-qualified
  import paths, and a new opt-in directory-scoped resolution tier for
  Go's file-independent package visibility). Spec §5.2's v1 language
  set is now fully implemented.
- **Post-v1-set: C#** (2026-08-02, ADR-0016) — the eighth language,
  added on direct user request ahead of the M2+ roadmap, the same
  on-request scope-amendment path PHP took (ADR-0012). New
  `RawImport::NamespaceImport` (a C# `using` imports a *namespace* —
  many files — fitting no prior variant) with Go-style per-file
  fan-out, and `LangExtractor::namespace_separator()` so PHP's `\` and
  C#'s `.` FQNs share one index without cross-matching. Two
  user-confirmed judgment calls: `internal` counts as exported, and
  `new Foo()` is a captured call site resolving to the *type* (which
  is exactly why constructors are deliberately not symbols — see
  ADR-0016's constructor-ambiguity section).
- **M2+** — infra graph, join, MCP, ingest, real redaction. Per spec §10. **Current head.**

Post-M1.b.2b hardening (2026-08-01): real-world PHP field-testing
against a TYPO3 codebase surfaced two query-layer gaps, both fixed —
`deps` now surfaces the root symbol's `unresolved_calls` (a large
static-utility-class root returning zero outbound edges was previously
indistinguishable from "calls nothing"), and `where`/`deps`/`map` gained
`--subpath <dir>` scoping (the positional `PATH` argument never scoped
an already-built `--out` index; there was no subtree-scoping mechanism
at all). See [ADR-0014](adr/0014-subpath-scoping.md).

## Deliberately absent — not gaps, not bugs

Do not "fix" these without checking the linked reasoning first:

- **Rust path-qualified call resolution** (`Type::method()`, `module::func()`).
  Call matching is bare-name-only; those call sites aren't captured at all.
  Spec §5.3's "deliberately modest" policy — [ADR-0008](adr/0008-rust-resolution-policy-mapping.md).
  **Python does not have this exclusion** — its grammar has no node type
  distinct from plain attribute access for a module-qualified call
  (`pkg.func()`) vs. an instance call (`order.summary()`), so both
  resolve through the same tiers. A genuine cross-language difference,
  not an inconsistency — [ADR-0011](adr/0011-python-resolution-policy-mapping.md).
- **`use_list` / `use_wildcard` / `use_as_clause`** Rust import shapes —
  extracted as nothing rather than partially interpreted. Same ADR.
  Python's `from x import *` is excluded the same way (ADR-0011).
- **Python has no visibility keyword; a leading underscore is the
  is_pub proxy** for call-resolution tiers (b)/(c), applied uniformly to
  functions/classes/methods (including dunder methods, e.g. `__init__`,
  which are therefore never tier-(b)/(c)-eligible) — ADR-0011.
- **Python module-level assignments aren't extracted as `Const`** — no
  `const` keyword, and "uppercase name = constant" is a heuristic with
  no clear v1 win. `SymKind::Const` stays unused for Python.
- **Every Python absolute import is classified `external`**, even one
  naming a local top-level package — no walked-repo equivalent of
  Rust's `known_modules` (no explicit module declaration to collect).
  ADR-0011; revisit only if false "external" classifications on
  genuinely local packages show up in practice.
- **Stdlib/external method calls land in `unresolved_calls`** (e.g.
  `input.is_empty()`). Correct behavior: there's no type information to
  resolve against, and INV-8 requires honest omission over a guessed edge.
- **`calls` edges are never `certain`** — always `inferred`, by spec.
- **`manifest.json`'s `dirty` is always `null`** — [ADR-0006](adr/0006-manifest-git-provenance.md).
- **No `petgraph`** — [ADR-0005](adr/0005-graph-store-sorted-vec-not-petgraph.md); revisit at M1.b.3 when real traversal exists.
- **No `.github/`**; §9.5 gates 1/3/4 run locally via `scripts/gates.sh`,
  gate 2 (strace) needs Linux CI — [ADR-0004](adr/0004-deferred-security-gates.md).
- **`carto-grammars` contains zero `unsafe`** despite being the designated
  carve-out — [ADR-0007](adr/0007-carto-grammars-unsafe-carveout.md).
- **"Same-package" means "same walked repo"** — no `Cargo.toml`/workspace
  parsing. Revisit only if multi-crate false positives show up in practice.
- **`where` matches `Symbol.name` only** — `File`/`Module` nodes aren't
  searched. Spec §7.1 names "symbol name" specifically; broadening this
  is a real decision, not an oversight.
- **`deps` reports a spanning-tree view, not every edge in the reachable
  subgraph** — each node listed once via the edge that first discovered
  it; a "cross edge" between two already-discovered nodes isn't shown
  separately. See `crates/carto-core/src/query/deps.rs`'s module doc.
- **`map`'s "entry points" is a structural heuristic** (files with no
  incoming `imports`, symbols named `main`), explicitly labeled as such —
  not real per-language entry-point analysis (e.g. a `Cargo.toml`
  `[[bin]]` table), which carto doesn't have in v1.
- **`map`'s infra/join sections are explicit "requires M2"/"requires M3"
  placeholders**, not omitted — an absent section would read as "no
  infra found," which isn't yet a true statement either way.
- **No stale-index detection** — `manifest.json` carries `commit_sha`/
  `file_sha256` but nothing compares them against the working tree.
  M5 incrementality territory.
- **`MCP_TEXT_CAP`'s 8 KiB byte cap isn't enforced anywhere yet** — no
  MCP text renderer exists until M4. Today's output is bounded by each
  command's own `--limit`/`--depth`/`--budget`.
- **Query-layer `--json` output never fences tainted text; human output
  fences once per section, not once per row** —
  [ADR-0010](adr/0010-tainted-text-in-query-output.md).
- **PHP `require`/`include` are not extracted** — the argument is an
  arbitrary runtime expression in the general case; the FQN index
  already covers real (autoloaded) dependency structure — ADR-0012.
- **PHP anonymous classes' methods are never extracted** — anonymous
  classes have no `name:` field, so they never match `symbols.scm`'s
  four class-like-container patterns. ADR-0012.
- ~~An aliased `use ... as X` import doesn't make a call written as
  `X(...)` resolve via tier (b)~~ — **fixed** (ADR-0013): `RawImport`'s
  `imported_names` now carries `ImportedName { bound_name,
  declared_name }` pairs instead of a single flat name, and
  `resolve.rs`'s tier (b) looks a call site's identifier up in a
  per-file `alias_to_declared` map before consulting `pub_by_name` —
  alias-aware for Rust, Python, PHP, and TS/JS alike.
- **A colliding FQN (two PHP files illegally declaring the same
  namespace+name) keeps whichever file `resolve` encounters first**,
  not an error — carto only reads source, it doesn't enforce PHP's own
  uniqueness rules. ADR-0012.
- **CommonJS (`require()`/`module.exports`) is not extracted** — ESM
  `import`/`export` only. Same "extract nothing rather than partially
  interpret" precedent as every other exclusion here — ADR-0013.
- **TS/JS default and namespace imports (`import Foo from './x'`,
  `import * as ns from './x'`) never feed tier (b)** — the local name
  is the importer's own choice, not a name the target file declares
  under that spelling, so there's nothing verifiable to check against.
  Re-exports (`export { foo } from './x'`) don't either, for a
  different reason: they never create a binding usable by a call site
  in the *declaring* file at all. ADR-0013.
- **Default/namespace-import and re-export `imports` edges get
  `"mod-declaration"` evidence**, the same label Rust's bare `mod
  foo;` gets — a naming artifact of `resolve.rs` picking the evidence
  string by whether `imported_names` is empty, not a discriminant
  specific to what actually happened. Functionally correct (`certain`,
  right target file) in every case; just reads oddly for TS/JS. Not
  fixed — ADR-0013.
- **No `package.json`/`tsconfig.json`/`node_modules` parsing** — every
  TS/JS bare package specifier is classified external, the same
  simplification Python's and PHP's absolute imports already made.
  Relative-import extension-guessing (`.ts`/`.tsx`/`.js`/`.jsx`/
  `/index.*`) is a fixed priority order, not real bundler/tsconfig
  resolution. ADR-0013.
- **`deps` surfaces `unresolved_calls` on the root symbol only, not
  per-hop** — directly answers "why is `--dir out` empty" without
  bloating every row of a possibly-large traversal. Broaden to
  per-node only if a concrete need shows up. ADR-0014 (recorded
  alongside `--subpath` since both came from the same round of
  real-world feedback).
- **`--subpath` restricts which rows get listed, never what a
  command's underlying computation is based on** — a file's fan-in/out
  in `map`'s ranking is always its real, whole-repo connectivity, not
  clipped at the subtree boundary; `deps`'s BFS traversal itself is
  never scoped, only which discovered nodes get reported. The one
  deliberate exception: `map`'s external-package fan-in *is* scoped
  (counts only edges from in-scope files) — the opposite rule from the
  file ranking, on purpose, not an inconsistency. ADR-0014.
- **`Module` nodes are never excluded by `--subpath`** — packages have
  no directory, so path-prefix filtering doesn't apply to them
  directly; they only disappear from a scoped `map` ranking by having
  zero edges with an in-scope endpoint. ADR-0014.
- **Go's cross-package calls always resolve via tier (c)
  (`"same-package"`), never tier (b) (`"imported"`), even when reached
  through a real `import`** — a Go import binds a *package* name, not
  a symbol name, so there's nothing for `RawImport::PackagePath` to
  offer tier (b): `resolve.rs`'s `alias_to_declared` map is never
  populated for Go files. The same class of cosmetic evidence-label
  quirk ADR-0013 already accepted for TS/JS's `"mod-declaration"`
  label. ADR-0015.
- **No `go.mod` parsing.** `RawImport::PackagePath` resolves a Go
  import path against every walked directory containing a Go file by
  **longest-suffix match**, not an exact prefix computed from
  `go.mod`'s `module` line — same "no manifest parsing" precedent as
  Rust's/Python's/PHP's/TS-JS's own absolute-import simplifications.
  Accepted false-positive: an external import whose path coincidentally
  ends with a local directory's path resolves as internal. `go.mod`
  is still committed in `fixtures/go-svc` for realism and indexed as a
  plain `File` node. ADR-0015; revisit only if this causes a real false
  positive in practice.
- **The package qualifier in a Go selector call (`pkg.Func()`) is
  discarded** — `RawCallSite` carries only the bare callee name
  (`Func`), the same grammar-level ambiguity Python's/TS-JS's
  attribute/member calls already have, not a Go-specific gap. ADR-0015.
- **C# constructors are never extracted as symbols** — a constructor
  shares its type's name, so a constructor symbol would make every
  `new Foo()` ambiguous between type and constructor and INV-8 would
  drop the very construction edges capturing `new` exists to produce.
  `new Foo()` resolves to the *type*; constructor-body calls attribute
  to the enclosing class (innermost containment). Not the same
  situation as PHP's `__construct`/Python's `__init__`, whose distinct
  names make them safe to extract. ADR-0016.
- **C# properties, events, indexers, operators, non-const fields, and
  local functions are not extracted**; `using static`'s member-binding
  and `global using`'s repo-wide scope are not modeled; partial
  classes' cross-file `private` visibility lands in
  `unresolved_calls`. ADR-0016.
- **C#'s cross-namespace calls always resolve via tier (c)
  (`"same-package"`), never tier (b), even through a real `using`** —
  a namespace `using` binds no symbol name (Go's exact shape, ADR-0015);
  only the *alias* form (`using P = X.Y.Z;`) feeds tier (b). And every
  C# `using` of a namespace no walked file declares becomes an external
  `Module` keyed by the full namespace string — no `.csproj` parsing,
  same "no manifest parsing" precedent as every other language.
  ADR-0016.

## Next: M2 (infra graph, join, MCP, ingest, real redaction)

M1.b.2b is done — spec §5.2's full v1 language set (TypeScript, TSX,
JavaScript, Python, Rust, Go, PHP) is implemented, plus C# post-v1
(ADR-0016). `carto index` extracts symbols/imports/calls for all eight
`Lang` variants, and `where`/`deps`/`map` work against every one of
them with zero language-specific code in
`crates/carto-core/src/query/` — reconfirmed for C# (ADR-0016), the
eighth data point for that claim.

Eight languages across seven ADRs (Rust/ADR-0008, Python/ADR-0011,
PHP/ADR-0012, TS-TSX-JS/ADR-0013, Go/ADR-0015, C#/ADR-0016) are enough
data points to say what varies per language with some confidence:
relative-import vs. FQN-based vs. literal-path vs. directory-path vs.
namespace-fan-out import semantics, what "exported" means (a keyword,
a convention, a hard first-letter-case rule — or C#'s
`internal`-is-repo-visible judgment call), whether path-qualified
calls are even syntactically distinguishable from plain ones (and,
PHP/TS/Go/C# all show, distinguishable ≠ excluded — an independent
call each language gets to make), and what "same-package" should mean
(same walked repo for Rust/Python/TS-JS/C#, same FQN-namespace for
PHP, same *directory* for Go — the one language so far where package
scope needed a genuinely new resolution tier, not just a new
`RawImport` variant).

Next per spec §10 is M2: the infrastructure graph (Terraform/CFN/CDK),
the code↔infra join, and real redaction (`redact::redact()` is
currently a no-op stub with the real signature). Read spec §6/§7 before
planning M2's first slice — it hasn't been sliced yet the way M1.b was.

Also worth knowing before touching `resolve.rs` again: building
`fixtures/php-app` surfaced a real, language-agnostic bug in call
attribution (a symbol whose range nests another symbol's — PHP/Python
classes containing methods — used to get every nested call's edges/
`unresolved_calls` double-counted onto itself too). Fixed by
`assign_calls_to_innermost_symbol`, verified behavior-preserving for
Rust/Python (full pre-existing suite green, both golden files
untouched) — see ADR-0012's "shared bug" section for the detail. And
building `fixtures/ts-app` caught two issues that turned out to be
fixture-design mistakes, not extractor bugs (an aliased import of a
non-exported symbol — invalid TS/JS — and a function named `log`
coincidentally colliding with `console.log`) — see ADR-0013's own
"what building the fixture caught" section. Both were only found by
eyeballing real `carto index` output, never by unit tests on isolated
snippets — keep doing that for every new milestone, per CLAUDE.md's own
documented gotcha. `fixtures/go-svc` (ADR-0015) broke that streak in a
good way: the fixture matched real output on the first run, with
nothing to fix — recorded in ADR-0015 rather than left unmentioned, so
"nothing was wrong" reads as verified, not skipped.
`fixtures/csharp-app` (ADR-0016) added a third variation: its one real
defect (constructor symbols making every `new Foo()` ambiguous) was
caught at fixture-*design* time — tracing which resolution outcome
each planned call site must produce, before the first run — and the
first real run then matched the corrected design exactly.

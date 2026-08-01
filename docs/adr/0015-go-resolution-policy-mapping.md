# 0015 — Go mapping of spec §5.3's resolution policy; directory-scoped resolution tier

**Status:** accepted · **Date:** 2026-08-01 · **Milestone:** M1.b.2b

## Context

[ADR-0008](0008-rust-resolution-policy-mapping.md) (Rust),
[ADR-0011](0011-python-resolution-policy-mapping.md) (Python),
[ADR-0012](0012-php-resolution-policy-mapping.md) (PHP), and
[ADR-0013](0013-typescript-javascript-resolution-policy-mapping.md)
(TypeScript/TSX/JavaScript) each map spec §5.3's language-agnostic
resolution rules onto a language or language family. Go is the seventh
extractor and the last language in spec §5.2's v1 set — closing
M1.b.2b. STATUS.md predicted "at least one real judgment call, not
mechanical repetition" for this slice; it turned out to be two, both
confirmed with the user before implementing:

1. **Go import paths are module-qualified and name a directory, not a
   file** (`import "github.com/acme/svc/internal/orders"`). None of
   the three existing `RawImport` variants fit: there's no path
   arithmetic from the declaring file (`Relative`), no meaningful root
   to classify by itself (`Absolute` — `github.com` is useless as a
   root), and no FQN-to-single-declaration index (`Qualified`, PHP's
   `\`-separated model, where the whole name identifies one
   declaration rather than a directory of them).
2. **A Go package's visibility boundary is the directory, not the
   file.** Files in one directory see each other's *unexported*
   symbols with no import at all — a shape spec §5.3's existing tiers
   (same-file, imported, same-repo-exported) have no room for.
   Without addressing this, unexported package-internal helpers —
   pervasive in real Go — would produce no edges at all.

## Decisions

- **New `RawImport::PackagePath { path: String }` variant**
  (`crates/carto-core/src/lang/extractor.rs`), resolved in `resolve.rs`
  by **longest-suffix match against every walked directory containing a
  Go file — no `go.mod` parsing.** `github.com/acme/svc/internal/orders`
  matches a walked `internal/orders` directory because the import path
  ends with `/internal/orders`; the longest matching directory wins so
  a deeper, specific package isn't shadowed by a shorter coincidental
  match. Same "no manifest parsing" precedent STATUS.md already records
  three times for `Cargo.toml`/`package.json`/`tsconfig.json`. A hit
  fans out to **one `imports` edge per Go file in the target
  directory**, since Go's import unit is the package (directory), not
  a single file the way every other language's import target is. No
  hit ⇒ an external `Module` node keyed by the import path's **full
  string**, not a truncated root — a Go import path is canonical
  package identity end to end (unlike npm's `@scope/pkg/subpath`
  convention, where truncation to `@scope/pkg` is correct); truncating
  a Go path would collapse genuinely distinct packages together.

  `PackagePath` carries no `bound_name`/`ImportedName` at all, unlike
  `Absolute`/`Qualified`: a Go import binds a *package* identifier
  (`orders`), never a symbol name, so there is nothing to pair with a
  call site's own identifier the way tier (b) needs. **Go therefore
  never uses tier (b)** — the same reasoning ADR-0013 already recorded
  for TS/JS default and namespace imports. A dot import (`import .
  "fmt"`) and a blank import (`import _ "x"`) decompose through this
  same variant with no special-casing: a blank import is still a real
  dependency edge, and neither import's local-binding behavior is
  consumed by anything downstream anyway.

- **New opt-in resolution tier (a′), directory-scoped visibility, gated
  per extractor.** `LangExtractor` gains
  `package_scope_is_directory() -> bool`, defaulting to `false` so
  every existing extractor (and every existing golden file) is
  unaffected — confirmed by the full pre-existing suite passing
  unchanged with zero golden-file drift. `GoExtractor` is the only
  override. `FileExtraction` carries the resulting `dir_scoped: bool`
  through to `resolve`, which builds a `(directory, bare name) ->
  candidates` index (all visibilities) from `dir_scoped` files only,
  and `resolve_call` consults it — for a `dir_scoped` caller only —
  between tier (a) and tier (b):

  ```
  tier (a)  same file,      any visibility    evidence "same-file"
  tier (a′) same directory, any visibility    evidence "same-directory"  ← new, Go-only
  tier (b)  imported into the file (exported) evidence "imported"
  tier (c)  same walked repo (exported)       evidence "same-package"
  ```

  Exactly-one-candidate-wins applies here exactly as everywhere else:
  ambiguous same-directory candidates produce no edge, recorded
  honestly in `unresolved_calls` (INV-8).

## Consequences

- **Every resolving cross-package Go call lands on tier (c), evidence
  `"same-package"`, even when it's genuinely reached through a real
  `import`** — because Go's import never feeds tier (b) (see above).
  This reads slightly oddly (a call written as `orders.ParseOrder()`,
  reached via a real import, gets an evidence label that implies "no
  import was involved") but is functionally correct: a `certain`-vs-
  `inferred`, correctly-targeted edge either way. The same class of
  cosmetic evidence-label quirk ADR-0013 already accepted for
  TS/JS's `"mod-declaration"` label on default/namespace imports and
  re-exports; not fixed there, not fixed here.
- **The package qualifier in `pkg.Func()` is discarded at the call
  site.** `RawCallSite` carries only `callee_name` + `line`; `calls.scm`
  captures the selector's `field:` (`Func`), not the `object:`
  (`pkg`). Threading a qualifier through `RawCallSite` would be a
  cross-language schema change made for exactly one language's
  benefit — out of scope for this slice, and not obviously a net
  improvement anyway (tier (c) already resolves the unqualified name
  correctly when it's unambiguous, which is the common case for
  exported Go identifiers).
- **A coincidental suffix match can misclassify an external import as
  internal** — importing `github.com/other/lib/internal/orders` in a
  repo that happens to have a local `internal/orders` directory would
  resolve internally. Accepted and documented, not fixed: the cost of
  not parsing `go.mod`'s module declaration to compute an exact prefix
  instead. Revisit only if this shows up as a real false positive in
  practice, the same "revisit only if" bar STATUS.md already applies
  to Rust's "same-package = same walked repo" simplification.
- **`ExtractOut::declared_namespace` stays `None` for Go.** PHP's own
  FQN-index machinery (ADR-0012) has no Go equivalent — package
  membership here is entirely directory-based, handled by
  `package_scope_is_directory` and `PackagePath`, not an FQN index.
- **`go.mod` is committed in `fixtures/go-svc` for realism but is
  never read** — indexed as a plain `File` node like any other
  non-source file, the same tolerance every other fixture's manifest
  file (`Cargo.toml`, `package.json` where present) already gets.

## What building `fixtures/go-svc` caught

Nothing — the fixture matched the real `carto index` output on the
first run: every expected symbol, `sym_kind`, resolution tier and its
evidence label, the two-edge package-import fan-out, and the two
full-path external `Module` nodes (`fmt`, `github.com/lib/pq`) were all
correct without adjustment, verified by eyeballing the actual JSON
before writing any test assertions (CLAUDE.md's documented gotcha,
followed the same way for every prior language). Recorded here for the
same reason the "nothing surprising happened" case matters as much as
the surprising ones: it confirms tree-sitter's repeated-field capture
behavior (`const A, B = 1, 2` producing two `symbol.name` captures, one
per name) was correctly assumed rather than guessed — checked directly
against tree-sitter-go's published `node-types.json` and then confirmed
by a passing unit test, not asserted from memory of how tree-sitter
"usually" behaves.

## Consequences (query layer)

`crates/carto-core/src/query/` needed zero changes — the same
confirmation every prior language's ADR has recorded, now true for all
seven `Lang` variants in the v1 set. `carto where`/`deps`/`map` work
against `fixtures/go-svc` unmodified.

This closes M1.b.2b. Spec §5.2's v1 language set (TypeScript, TSX,
JavaScript, Python, Rust, Go, PHP) is now fully implemented; M2+
(infra graph, join, MCP, ingest, real redaction) is next per spec §10.

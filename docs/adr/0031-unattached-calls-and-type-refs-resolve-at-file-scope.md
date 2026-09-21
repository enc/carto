# 0031 — Unattached calls/type-refs resolve at file scope (amends ADR-0029/0030)

**Status:** accepted · **Date:** 2026-08-06 · **Milestone:** post-S-1
field feedback, follow-up to ADR-0029/0030

## Context

Retest feedback on ADR-0029/0030 (the `references`-edge feature):
reproduced cleanly — 41 false positives collapsed to 14 correct matches
(grep's ground truth: 15) — but with one real miss. `deps
IQueryJobStore --dir in --depth 1` no longer found `Program.cs`'s own
`services.AddSingleton<IQueryJobStore, X>()` DI registration: exactly
the shape ADR-0029's own code comments call out as the motivating case,
and, per the reporter, the dominant style in modern ASP.NET Core
minimal-API code (a `Program.cs` with no `Main` method at all — valid
since C# 9's top-level statements), not an edge case.

Root cause, traced to `crates/carto-core/src/lang/resolve.rs`:
`assign_to_innermost_symbol` (the function attributing each call site
or type ref to its enclosing symbol, by line-range containment via
`smallest_containing_symbol`) silently dropped any item whose line fell
**outside every symbol's range**. C# top-level statements parse to
`global_statement` nodes directly under `compilation_unit`, with
nothing wrapping them — `symbols.scm` has no pattern that captures "the
top level of the file" as a container of anything, so
`smallest_containing_symbol` returned `None` for every such line, and
the item was never pushed anywhere. It wasn't recorded as unresolved,
wasn't counted, wasn't visible at all — a stricter silence than
`unresolved_calls` gives an ordinary same-symbol miss.

The extractor was not the bug: `csharp.rs`'s `extract_type_refs`/
`extract_call_sites` already emit a correctly-lined `RawTypeRef`/
`RawCallSite` for a top-level generic invocation regardless of position
— tree-sitter queries match anywhere in the tree, unconstrained by
containing scope (confirmed by a dedicated extractor-level test,
`top_level_statement_invocation_is_extracted_like_any_other`, added
alongside this fix). The gap was entirely in `resolve.rs`'s attachment
step — which is **language-agnostic and shared by both calls and type
refs**, so the same silent drop already affected ordinary top-level
`calls` too, in any language with real top-level executable code: not
just C#'s top-level statements, but TS/JS's common module-level setup
calls (`const app = express(); app.use(...)`) and Python's equally
common module-level pattern (`app = Flask(__name__)`). Not a bug
ADR-0029 introduced — a pre-existing gap it made newly consequential,
since the motivating DI-registration pattern lives exactly there.

## Decisions

- **User-confirmed scope: fix `calls` and `references` together, for
  the *resolved* case only.** An unattached call or type-ref that fails
  to resolve stays invisible, exactly as before this ADR —
  `SymbolNode.unresolved_calls` has no `FileNode` equivalent, and
  adding one is a separate, unscoped design decision (would it be a
  flat list? does it need the same tier-evidence shape? does `deps`
  need a new root-metadata field the way `root_unresolved_calls`/
  `root_uncaptured_*_calls` already are?) left for later, not bundled
  into this fix.
- **The fix reuses a precedent already in this codebase**, not a new
  mechanism: `resolve.rs`'s own contract-literal pass (ADR-0026)
  already falls back to the *file* when `smallest_containing_symbol`
  returns `None` (`.unwrap_or_else(|| fe.file_id.clone())`). This ADR
  applies the identical fallback to calls and type refs.
  `assign_to_innermost_symbol` now returns `(Vec<Vec<&T>>, Vec<&T>)` —
  the existing per-symbol assignment, plus every item that couldn't be
  placed. A new per-file block (after the existing per-symbol loop,
  still inside the per-file loop) resolves each unattached item through
  the *identical* tier ladder the symbol-scoped loops already use
  (`resolve_call` needed no changes — tier resolution never depended on
  the caller being a symbol) and, on success, pushes a `Calls`/
  `References` edge **from the `File` node** instead of a `Symbol`,
  same `Confidence::Inferred` and evidence-string policy as the
  symbol-scoped case. No self-reference suppression is needed the way
  the symbol-scoped type-ref loop needs one: a `File` ID and a `Symbol`
  ID are never equal.
- **No `SCHEMA_VERSION` bump.** `Edge`'s shape is unchanged — a `File`
  is already a valid edge endpoint (`imports`/`contains` both use one)
  — so this is a resolution-logic bug fix, not a schema change. An old
  reader already knows how to interpret a `calls`/`references` edge
  whose `from` is a File; nothing about the on-disk shape is new.
  `query/deps.rs`'s `owning_file()`/`summarize()` already handle a
  File-kind node generically (the prior slice's own file-target smoke
  test already exercised this) — no query-layer changes were needed.
- **New fixture, `fixtures/csharp-app/TopLevelRegistration.cs`** —
  genuine C# 9+ top-level statements calling a generic method
  (`Registrar.Register<IQueryJobStore, QueryJobService>()`) whose type
  arguments cross-reference the `Ports/`/`Services/` files ADR-0029's
  own fixture added. Exercises both halves of this fix in one file: the
  generic type arguments are a top-level type ref, cross-file
  (`references`, tier same-package); `Register` itself is a top-level
  *call* to a method declared later in the same file (`calls`, tier
  same-file) — the calls-side of the fix, not just the type-refs side
  the report was about. Noted honestly in the file's own comment: a
  real C# project allows top-level statements in only one file, and
  this fixture's `Program.cs` already holds that role — this file
  couldn't coexist with it in a project `csc` would actually compile.
  Irrelevant here, since carto only parses, never compiles.

## Consequences

- `deps IQueryJobStore --dir in --depth 1 --kinds references` on the
  reproduction now returns the file-sourced edge from
  `TopLevelRegistration.cs` alongside `QueryJobService`'s — the exact
  regression this ADR closes, without reopening the false-positive hole
  ADR-0029 closed (`S3PresignedUrlProvider` still absent).
- A `references`/`calls` row whose `node.kind` is `"file"` rather than
  `"symbol"` means the usage site is top-level-statement code with no
  enclosing method/class — worth a caller-facing note (skill doc), not
  just an internal implementation detail, since it changes how the
  result reads.
- The general TS/JS/Python top-level-call gap this ADR's root-cause
  section names is fixed by the same mechanism (the fallback block is
  language-agnostic), but has no dedicated fixture/test this slice —
  the retest report and its fixture were C#-specific; extending
  deliberate test coverage to a TS/JS or Python top-level-statement
  case is a reasonable follow-up, not done here.
- `crates/carto-cli/tests/cli_extraction_csharp.rs`'s
  `references_edges_are_precise_where_the_namespace_import_fan_out_was_not`
  needed deliberate updating (not just re-passing) — `refs_to("
  IQueryJobStore")` now legitimately includes `TopLevelRegistration.cs`
  alongside `QueryJobService`, sorted.

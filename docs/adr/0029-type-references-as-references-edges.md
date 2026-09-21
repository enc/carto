# 0029 — Type references become `EdgeKind::References` edges

**Status:** accepted · **Date:** 2026-08-05 · **Milestone:** post-S-1
field feedback

## Context

Field feedback from a real ~168-file C# repo (`bench/field-log.md`):
the query "find all usages/callers of `IQueryJobStore`" — an interface
consumed only by constructor injection — was answered better by one
`grep -rl "IQueryJobStore" --include="*.cs" .` (15 files, all correct,
0.02s) than by `carto deps IQueryJobStore --dir in --depth 2` (~41
files, ~26 false positives, requiring manual grep verification that
erased carto's token advantage). `--depth 1` returned almost nothing;
`--depth 2` was the only usable depth and over-collected roughly 2x.

The reporter's own diagnosis was that C#'s `imports` edge is
file→namespace granularity, so "who imports the file containing
`IQueryJobStore`" pulls in every file that `using`s
`Ticks.HistoryApi.Ports.Outbound` for any of the dozen unrelated
interfaces living in it. That's an accurate description of the
*observed* fan-out, but not the root cause. The actual cause sits one
level earlier: **no extractor in carto captured a type position at
all.** `queries/csharp/calls.scm` captured only
`invocation_expression` and `object_creation_expression` — an
interface referenced as a *type* (a field declaration, a constructor
parameter, a base clause, a generic type argument in
`services.AddSingleton<IQueryJobStore, X>()`) produced zero
`RawCallSite`s and therefore zero symbol-level edges of any kind. That
is why `--depth 1` was empty: the symbol's only inbound edge was the
`contains` edge from its own file, forcing the BFS through the file
node, whose inbound `imports` fan-out is the noise that was observed.
The same hole existed in every other typed language this crate
extracts (Rust trait bounds, Go interface fields, TS `implements`, PHP
type hints, Python annotations) — simply not yet reported.

Two consequences worth naming because they compound:

- The honest-absence machinery built exactly for this
  (ADR-0020/`uncaptured_inbound_calls`) reported **0**, because
  `csharp.rs` correctly has no *call shape* it deliberately excludes —
  true of call syntax, but the answer still looked confidently empty
  while being structurally blind to an entire node category.
- The reporter concluded, correctly given the build at the time, that
  "carto cannot answer 'who uses interface X' precisely in C# repos
  where X's namespace holds other interfaces." This ADR is the fix:
  `deps IQueryJobStore --dir in --depth 1 --kinds references` now
  returns exactly the real users.

`EdgeKind::References` already existed in the spec §4.2 vocabulary and
this crate's `EdgeKind` enum (`crates/carto-core/src/graph/edge.rs`) —
declared since the type was first introduced, never produced by
anything. This ADR gives it its first producer.

## Decisions

- **A new optional `ExtractOut` channel, `type_refs: Vec<RawTypeRef>`**
  (`crates/carto-core/src/lang/extractor.rs`), parallel to `literals`
  — an extractor that doesn't populate it (none did, before this)
  keeps prior behavior bit-for-bit. `RawTypeRef` is `{ name: String,
  line: u32 }`, deliberately as thin as `RawCallSite`.
- **Resolution reuses the existing call-resolution tier ladder
  verbatim** (`resolve_call` in `crates/carto-core/src/lang/
  resolve.rs`, refactored to take a bare `name: &str` instead of
  `&RawCallSite` so both call sites and type refs share one
  implementation) — same-file, same-directory (Go), imported/
  alias-aware, same-package, first-match-wins, ambiguous-or-zero
  produces nothing. "Which declaration does this bare identifier mean"
  is the identical question for a call's callee and a type ref's type
  name.
- **Emitted as `Confidence::Inferred`, never `Certain`** — a bare-name
  match, the same epistemic status as a `calls` edge (INV-8). Evidence
  is `type-reference:<tier>` (e.g. `type-reference:same-package`),
  distinguishing it from a `calls` edge's own tier evidence at a
  glance without a separate lookup.
- **Self-references are suppressed.** A field of a struct's own type,
  or a recursive generic (`struct Node { next: Option<Box<Node>> }`),
  resolves to the containing symbol itself; emitting that as an edge
  would be noise, not a dependency. Checked by comparing the resolved
  target's `NodeId` against the containing symbol's own ID before
  pushing the edge.
- **An unresolved type ref is silently dropped — no
  `unresolved_type_refs`-style list, no counter.** User-confirmed,
  deliberate asymmetry with `unresolved_calls`/`uncaptured_*_calls`:
  those exist because an unresolved *call* is usually a real, in-repo
  dependency carto merely couldn't verify. An unresolved *type* ref is
  overwhelmingly stdlib/BCL/framework noise (`Task`, `string`,
  `ILogger`, `CancellationToken` are typical inbound counts, not
  outliers) — a list would materially inflate `graph.json` on any real
  repo, and a bare count would be dominated by that noise to the point
  of being unusable as a signal. Recorded here as a conscious tradeoff,
  not an oversight; revisit if a concrete need for it shows up.
- **The `imports` edge's existing namespace/module fan-out is
  unchanged, on purpose.** `using Acme.Orders;` genuinely is a
  certain import of every file declaring that namespace; the edge is
  true, and INV-8 forbids weakening a verified edge because it turns
  out to be *irrelevant to one particular question*. The fix adds
  precision alongside the existing honest-but-coarse signal; it does
  not remove or narrow anything. `deps ... --kinds imports` and `deps
  ... --kinds references` now answer two different, both-honest
  questions ("what genuinely imports this file" vs. "what genuinely
  names this specific symbol"), and a caller who wants both still can.
- **`SCHEMA_VERSION` 4 → 5.** A v4 `graph.json` structurally cannot
  contain a `references` edge; `deps --kinds references` against one
  must be rejected with the existing "re-run `carto index`" message,
  not silently answer "nothing references this" from a graph that
  never looked.

Per-language capture positions (which grammar shapes feed `type_refs`,
and which are deliberately excluded) are recorded separately in
ADR-0030 — this ADR is the cross-cutting mechanism; that one is the
per-language survey, since six languages' worth of near-identical
capture-surface reasoning would otherwise obscure the one decision that
actually matters here.

## Consequences

- `deps <type> --dir in --depth 1 --kinds references` is now the
  precise "who uses this type" answer across all six typed languages
  this crate extracts, matching or beating a `grep -rl` on precision
  while keeping carto's structural/confidence advantages.
- Every existing `graph.json` must be re-indexed (schema bump) before
  `references` edges or `--kinds references` filtering are available;
  this is the same, already-established re-index contract every prior
  `SCHEMA_VERSION` bump has used.
- `fixtures/csharp-app` gained `Ports/`/`Services/` — a minimized,
  direct reproduction of the reported bug (`IQueryJobStore`/
  `IPresignedUrlProvider` sharing one namespace; one service uses only
  one of them) — serving as this feature's acceptance test at both the
  extractor-unit and CLI/JSON levels.
- The skill doc (`skill/carto/SKILL.md`) needed both a new recipe
  (`deps(..., kinds="references")` for "who uses this type/interface")
  and a rewritten gap entry — the old "carto cannot answer this
  precisely" language was accurate before this ADR and actively
  misleading after it.

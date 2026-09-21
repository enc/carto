# 0032 — Owner-type disambiguation for otherwise-ambiguous same-name candidates

**Status:** accepted · **Date:** 2026-08-05 · **Milestone:** follow-up to
ADR-0029/0030's type-reference work

## Context

ADR-0029/0030 gave every typed extractor a `type_refs` channel and a
first producer for `EdgeKind::References`. Building
`fixtures/csharp-app`'s own interface/implementation pair
(`Ports/IQueryJobStore.cs`'s `Save` and a second, unrelated `Save` on
`Services/InMemoryQueryJobStore.cs`) surfaced the next-door problem
spec §5.3's tier (c) already documents as a known trade-off: a bare-name
call through an interface-typed field
(`QueryJobService.CancelQuery`'s `_store.Save(jobId)`) resolves against
`pub_by_name`, finds **two** candidates repo-wide, and — correctly per
INV-8 — produces no edge at all. But the calling file already states,
syntactically, which one it means: `QueryJobService`'s constructor
parameter is typed `IQueryJobStore`, and the field-typed-by-interface
shape is idiomatic in every language with interfaces, not a C#
peculiarity. `type_refs` (ADR-0029) already captures that field/
parameter type position; nothing used it to narrow a same-name call
before this.

## Decision

**`CallResolver` gains one narrowing step, `disambiguate_by_owner`,
applied only when a bare-name tier would otherwise be ambiguous** — it
never changes which candidate an already-unambiguous tier picks, and it
can only turn a `None` into a `Some`, never the reverse.

- **Two new per-file indices**, built in lockstep with the existing
  `same_file_by_name`/`symbol_ids` shape: `sym_owner[fi][si]` is that
  symbol's `RawSymbol::owner` (`None` for a free function or a
  top-level type declaration itself); `type_ref_names[fi]` is the set
  of every type name that file's own `type_refs` names in a type
  position (field, parameter, base clause, generic argument — the same
  positions ADR-0030 surveyed per language).
- **`RawSymbol` gains `owner: Option<String>`** — the symbol's own
  enclosing/receiver type name where the language has one (a method's
  class, a Go method's receiver type via `receiver_type_name`), `None`
  for anything else (a free function, the type declaration itself).
  Every extractor that can name an owner populates it; extractors with
  no such position (nothing changes) leave it `None` throughout, and
  the tier below is then simply never eligible to fire for their
  symbols.
- **Applied at exactly the three tiers real field feedback motivated
  it for**: same-file (when ≥2 same-named declarations exist),
  imported, and same-package — each gaining an "and-owner-type-
  referenced" fallback that only runs once the plain tier already
  found ≥2 candidates. **Not** extended to tier (a′) (Go's
  same-directory tier, ADR-0015) — no real case has shown a need, and
  every added narrowing point is one more thing to keep correct.
- **The filter, precisely**: keep only candidates whose `owner` appears
  in the calling file's own `type_ref_names`. A candidate with no
  `owner` (a free function, or a candidate that *is* a type) never
  survives the filter — it's not disambiguable by this mechanism at
  all. Exactly one survivor resolves, with evidence suffixed
  `+owner-type-referenced` (`same-file+owner-type-referenced`,
  `imported+owner-type-referenced`, `same-package+owner-type-
  referenced`); zero or several survivors leave the ambiguity exactly
  as it was before this tier existed — INV-8 unchanged for every case
  this can't help with.
- **A real negative case is committed alongside the positive one**
  (`fixtures/csharp-app/TopLevelRegistration.cs`'s
  `RegisterQueryJobStore(IQueryJobStore primary, InMemoryQueryJobStore
  fallback)`): the calling method's own parameters name *both*
  candidates' owner types in the same file, so the filter has two
  survivors, not one, and `primary.Save(...)` stays exactly as
  ambiguous as it would without this tier at all — proving the
  mechanism doesn't overreach into "the first type mentioned nearby
  wins" territory.

## Consequences

- No `SCHEMA_VERSION` bump by itself — the change is confined to which
  edges `resolve()` produces (new `calls`/`references` edges where
  today's build produces none), not the on-disk shape. `RawSymbol` is
  extraction-internal, never persisted.
- `fixtures/csharp-app` gained `Ports/`, `Services/`, and the
  interface/two-implementation shape specifically to give this tier
  both its positive case (`QueryJobService.CancelQuery`) and its
  negative case (`RegisterQueryJobStore`) — see its README.
- `resolve.rs`'s own test suite gained
  `ambiguous_same_package_call_resolves_via_callers_owner_type_reference`
  alongside the pre-existing
  `ambiguous_same_package_candidates_produce_no_edge`, pinning both
  outcomes side by side so a future change can't silently widen the
  filter's reach.
- Interacts with ADR-0033 (same day): a candidate this tier can't
  disambiguate still needs its attempted-and-failed call site recorded
  honestly, not silently dropped — see that ADR.

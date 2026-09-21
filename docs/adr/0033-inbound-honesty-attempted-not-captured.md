# 0033 — Third honesty signal: attempted-but-unresolved inbound call sites

**Status:** accepted · **Date:** 2026-08-05 · **Milestone:** follow-up to
ADR-0020/0023/0032

## Context

Two honesty counters already existed on `SymbolNode` before this slice:
`uncaptured_inbound_calls` (ADR-0020) and `uncaptured_outbound_calls`
(ADR-0023) — both counting call sites whose *syntax* a language's
extractor deliberately never attempts to resolve at all (Rust's
`Type::method()`/`module::func()`). Neither one covers the case
ADR-0032's own negative fixture (`RegisterQueryJobStore`) pins: a call
site whose syntax *was* attempted, went through the full tier ladder
including the new owner-type narrowing step, and still resolved to
**two or more** candidates — the ordinary, common case of an interface
method and its implementation sharing a bare name. Today that call site
simply vanishes: no edge (correct, INV-8), and nothing on *either*
candidate symbol records that the attempt happened at all. A caller
looking at `deps IQueryJobStore::Save --dir in` sees zero inbound
edges and has no way to distinguish "genuinely uncalled" from "called,
but ambiguously."

## Decision

**A third, distinct field: `unresolved_inbound_calls` /
`unresolved_inbound_call_count`.** Distinct from both existing signals,
not a rename or a merge:

| Field | Question it answers | Keyed by |
|---|---|---|
| `uncaptured_inbound_calls` (ADR-0020) | how much syntax *elsewhere* spelling this name was never even attempted | `(origin, name)`, repo-wide |
| `uncaptured_outbound_calls` (ADR-0023) | how much syntax *inside this symbol* was never even attempted | per-symbol, by containment |
| `unresolved_inbound_calls` (this ADR) | which attempted call sites elsewhere spelling this name produced no edge | `(origin, name)`, repo-wide |

- **A second resolve pass, deliberately separate from the edge-building
  loop.** `CallResolver::resolve` is a pure `BTreeMap`-lookup function
  with no side effects, so re-running it over every `call_sites` entry
  (attached to a symbol or not — the ADR-0031 file-scope leftovers
  spell a name too) to record misses is a second, independent pass
  rather than a new output channel threaded through the existing
  per-symbol loop. Keeping the two honesty signals' bookkeeping
  independent was judged easier to verify than combining them, at the
  cost of one extra `BTreeMap` walk over what's already an
  O(call-sites) operation.
- **Keyed `(origin, name)`, exactly like `uncaptured_by_name`** — so a
  Rust call spelling `load` never inflates a same-named Python symbol's
  count, the same per-language partition every repo-wide name index in
  `resolve.rs` already uses.
- **Not a claim that any listed site calls this specific symbol** —
  stated explicitly in the field's own doc comment, the same
  non-claim `uncaptured_inbound_calls` already makes: the common cause
  is bare-name ambiguity (an interface method and its implementation
  both named `Save`), not that this particular symbol is the target.
  A caller wanting to know *which* candidate a site actually meant has
  no better answer than "ambiguous" — that's the honest state of a
  policy that refuses to guess.
- **`InboundCallSite { file, line }`, capped and counted separately**
  — `unresolved_inbound_calls` holds up to
  `consts::UNRESOLVED_INBOUND_SITES_CAP` (25) entries sorted by
  `(file, line)`; `unresolved_inbound_call_count` carries the uncapped
  total, the same capped-list-plus-uncapped-count pairing
  `unresolved_calls`/`uncaptured_*` already established, so a caller
  can tell "these are all of them" from "these are the first 25 of N."
  `file` is a plain `String` (repo-relative path), not `TaintedString`
  — matching `FileNode::path`'s own reasoning: an extractor-computed
  identifier, not captured source text.
- **Surfaced on `deps --dir in`/`both`** as `root_unresolved_inbound_calls`
  / `root_unresolved_inbound_call_count`, rendered as a text note in
  the same style ADR-0020's `root_uncaptured_inbound_calls` note
  already uses.
- **`SCHEMA_VERSION` bumped 5 → 6** — same justification as every prior
  bump in this family: `#[serde(default)]` lets a stale v5 `graph.json`
  parse cleanly, but the version check must still reject it before any
  query code can read a fabricated `0`/empty-list default for a repo
  that predates this pass entirely.

## Consequences

- `fixtures/csharp-app/TopLevelRegistration.cs`'s
  `RegisterQueryJobStore` doubles as this ADR's acceptance case as well
  as ADR-0032's negative case: both `Save` symbols (interface and
  `InMemoryQueryJobStore`'s implementation) must record this call site
  in their own `unresolved_inbound_calls`, not report a false
  all-clear.
- Every golden file touched by the schema bump was regenerated
  (`fixtures/mixed.graph.golden.json`,
  `fixtures/rust-crate.{deps,map}.golden.json`); `where`'s own golden
  is unaffected (`SymbolMatch` carries no honesty-signal fields).
- Deliberately not extended to a per-call-site "which candidates did
  this ambiguous site have" list — `unresolved_calls`
  (spec §5.3, the outbound-facing list on the *caller*) already records
  the bare name and line; cross-referencing which symbols shared that
  name repo-wide is answerable today via `where <name>`, and a denser
  structure here wasn't judged worth the size cost against
  `consts::MAX_GRAPH_BYTES` for a v1 slice.

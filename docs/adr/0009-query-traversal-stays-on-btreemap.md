# 0009 — Query-layer traversal stays on `BTreeMap`, not `petgraph`

**Status:** accepted · **Date:** 2026-07-31 · **Milestone:** M1.b.3

## Context

[ADR-0005](0005-graph-store-sorted-vec-not-petgraph.md) chose
`BTreeMap<NodeId, Node>` / `BTreeMap<EdgeId, Edge>` for the write-side
`Graph` store, and explicitly deferred the question for the read side:

> Revisit at M1.b.3 (`deps`/`map`/`where`), where real multi-hop
> traversal starts... Decide against actual traversal requirements then,
> not speculatively now.

M1.b.3 is that milestone: `QueryGraph` (`crates/carto-core/src/query/
mod.rs`) is the first thing in the codebase that walks the graph rather
than just building or persisting it.

## Decision

`QueryGraph` adds two more `BTreeMap`s (`NodeId -> Vec<EdgeId>` for
outgoing/incoming adjacency) on top of the same `nodes`/`edges` maps
`Graph` already uses. Still not `petgraph`.

- The actual traversal requirement, now that it's concrete, is a BFS
  bounded at `consts::MAX_DEPS_DEPTH` (5) for `deps`, plus a couple of
  single-pass scans (fan-in/out ranking, entry-point detection) for
  `map`. None of this needs `petgraph`'s indexed algorithms
  (`Dfs`/`Bfs` iterators, shortest-path, SCC) — a `VecDeque` and a
  visited `BTreeSet` is the whole BFS.
- carto's edges are identified by `EdgeId` and carry `confidence`/
  `evidence` that must survive lookups (`deps`'s output surfaces both on
  every row — this is INV-8's payoff, not incidental). `petgraph`
  addresses edges by `(NodeIndex, NodeIndex)` + an opaque `EdgeIndex`; a
  `NodeId`-keyed API on top of it needs a `NodeId <-> NodeIndex` side map
  maintained in lockstep with every insert, which is strictly more
  bookkeeping than the two `BTreeMap`s this ADR adds instead.
- Adjacency vectors are built by iterating `edges` (`BTreeMap`, so
  already ID-sorted) once at construction time, giving deterministic
  neighbor order for free — the same "sorted by construction, no
  separate sort step" property `Graph::into_sorted_parts` relies on for
  §4.4.

## Consequences

- `petgraph` remains unadded, now confirmed against real (not
  speculative) traversal code, not just deferred a second time.
- Next honest revisit point: `impact` (M3) — transitive dependents over
  the *joined* code+infra graph, which is a meaningfully bigger
  traversal (unbounded depth by default, per spec §7.1's row for
  `impact`) than `deps`'s depth-5 BFS. If that milestone's traversal
  needs turn out to want `petgraph`'s algorithms, decide then; this ADR
  is not a standing prohibition, just a second confirmation that today's
  actual requirements don't call for it.
- `QueryGraph` is deliberately a separate type from `Graph`
  (`crates/carto-core/src/graph/mod.rs`), not an extension of it —
  `Graph` is write-oriented (insert + merge-on-duplicate, consumed once
  by `persist`); `QueryGraph` is read-only and built fresh from a loaded
  `GraphDocument` per command invocation. Merging the two would couple
  the persist choke-point's invariants to query concerns that don't
  apply to it (there's no "insert" or "merge" on the read side).

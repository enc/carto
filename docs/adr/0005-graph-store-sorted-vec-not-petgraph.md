# 0005 — Graph store backed by `BTreeMap`, not `petgraph`

**Status:** accepted · **Date:** 2026-07-31 · **Milestone:** M1.b.1

## Context

Spec §3.1's crate layout names `petgraph + serde` for the `graph` module,
and §13 lists `petgraph` in the starting dependency allowlist. M1.b.1
introduces the graph store, stable IDs (§4.3), and the persist
choke-point (§6.5) — but produces no `Symbol`/`imports`/`calls` edges yet
(those need the language extractors, M1.b.2), and implements no traversal
command (`deps`/`map` land in M1.b.3).

## Decision

`crates/carto-core/src/graph::Graph` is `BTreeMap<NodeId, Node>` +
`BTreeMap<EdgeId, Edge>`, not a `petgraph` graph.

- Spec §4.4's on-disk format is already two sorted lists:
  `"nodes": [ … sorted by id … ], "edges": [ … sorted by id … ]"`.
  `BTreeMap` iteration is sorted by key for free, so
  `Graph::into_sorted_parts` satisfies INV-7 (determinism) without a
  separate sort step or an adjacency structure at all.
- Nothing in M1.b.1 traverses the graph (no `deps`, no `map`, no `impact`)
  — the entire value `petgraph` adds over a sorted map (fast adjacency
  walks, `Dfs`/`Bfs` iterators, algorithms) has no caller yet.
- The §4.3 edge-merge rule ("duplicates merge, keeping highest confidence
  and concatenating evidence, capped at 3") is a map upsert
  (`BTreeMap::get_mut` then merge-in-place) — exactly what a keyed map
  gives you directly; `petgraph` has no concept of "the edge with this ID"
  distinct from "an edge between these two node indices," so the same
  logic would need a side index into `petgraph` anyway.

## Consequences

- `petgraph` is not added as a dependency in M1.b.1, despite being
  pre-allowed by spec §13 — this ADR records that as a deliberate
  deferral, not an oversight, so a later milestone doesn't need to
  re-litigate it.
- Revisit at M1.b.3 (`deps`/`map`/`where`), where real multi-hop traversal
  starts: at that point a `BTreeMap`-based adjacency scan (build an
  index from `NodeId` to its outgoing/incoming edges on demand) may still
  be sufficient, or `petgraph` may earn its place. Decide against actual
  traversal requirements then, not speculatively now.
- `crates/carto-core/src/graph/mod.rs`'s module doc comment points here so
  the next reader sees the reasoning without needing to find this file
  first.

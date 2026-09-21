# 0038 — Descendant `--component` scoping

**Status:** accepted · **Date:** 2026-09-21 · **Milestone:** multi-root
support, follow-up slice 3 (amends ADR-0035's `--component` semantics)

## Context

The same review that produced ADR-0036/0037 found `--component <name>`
matches a node's own `component` field by exact name only
(`filter.contains(c)` in `QueryGraph::component_in_scope`,
`!filter.contains(&c.name)` in `map`'s `seed_component_counts`, and a
plain `BTreeSet::is_disjoint` in `orphans`). That's wrong whenever one
component is nested inside another's own directory — a shape that
already exists without any new capability: `components::tests::
innermost_component_wins_for_nested_markers` (ADR-0034) proves a
manifest marker inside another component's directory
(`services/api/go.mod` + `services/api/internal/go.mod`) produces two
separate `Component` rows, one nested inside the other, and
ADR-0034/0037's own component discovery already assigns every file to
its *innermost* enclosing component only — `services/api/internal/x.go`
belongs to `internal`, never to `api`, even though it's still, in the
ordinary sense, part of the `api` project.

Concretely verified before this fix: a synthetic repo with a fragmented
terraform tree (`infra/main.tf` + `infra/modules/vpc/main.tf`, an
earlier draft of the shape ADR-0037's own rollup now collapses)
produced separate `infra`/`vpc` components, and `contract <value>
--component infra` returned only `infra/main.tf`'s own site, silently
dropping `infra/modules/vpc/main.tf`'s — 1 of 5 real consumer sites in
the original over-fragmented repro. ADR-0037's rollup fixes the
terraform-specific over-fragmentation that produced *that* particular
case, but the underlying gap — `--component` not expanding to a
component's own nested components in general — is real independent of
terraform, and remains whenever nesting arises any other way (a
manifest marker, or a future declared root nested under another).

## Decision

**`--component <name>` now scopes to `name` and everything nested
under it**, not just nodes whose own `component` field equals `name`
exactly. `QueryGraph` gains `component_matches_filter(&self, name: &str,
filter: &BTreeSet<String>) -> bool`: `true` immediately if `filter`
contains `name` verbatim (preserves the exact-match case, and is the
only path taken whenever nesting doesn't exist — see Consequences);
otherwise, `true` if `name`'s own `Component::path` is strictly nested
(segment-boundary-safe, matching `is_at_or_under`'s rule already used
in `components/mod.rs`) under any filter entry's own path. Falls back
to the bare exact-match check for a `name` this index's `Component`
table has no entry for — should not happen for a filter value that
passed `validate_component_filter`, or for a name read from a real
`FileNode.component`, but stays conservative rather than assuming
consistency a hand-edited `graph.json` might not have.

This is deliberately one-directional: `--component internal` does
*not* pull in `api`'s own top-level files just because `internal` is
nested inside it — descendant scoping only ever widens *outward* from
the requested name toward its descendants, never inward toward its
ancestors. A request for the more specific name stays specific.

Three call sites updated to use it:

- `QueryGraph::component_in_scope` — the shared predicate `find`/
  `deps`/`contract` already route their per-node/per-edge filtering
  through, so this one change reaches all three with no further edits
  there.
- `map`'s `seed_component_counts` — a `--component api` request now
  also seeds `internal`'s own row (own file/symbol counts), not just
  `api`'s.
- `orphans` — replaced a direct `BTreeSet::is_disjoint(filter)` check
  (which only ever compared exact names) with an `.any(|c|
  qg.component_matches_filter(c, filter))` scan over a contract's
  touching components.

No `graph.json`/schema change — this is purely a query-layer read of
data the persisted format already carries (`Component::path`,
`FileNode.component`), verified by diffing `fixtures/mixed`/
`fixtures/rust-crate` `graph.json` before/after (byte-identical, as
expected for a change that touches no write-path code at all).

## Consequences

- `QueryGraph::component_matches_filter` (public — `map.rs`/
  `orphans.rs` are separate modules in the same crate) and a private
  `component_path` lookup helper.
- For a repo with no nested components anywhere (every existing
  fixture, before `fixtures/monorepo`'s own `services/api/internal`-
  shaped case would need adding), `component_matches_filter`'s
  ancestor-path branch is never reached — the first `filter.contains
  (name)` check alone decides every call, so behavior is byte-identical
  to before this ADR. All 469 pre-existing tests across `carto-core`
  pass unchanged.
- 6 new tests: `query::tests::
  component_in_scope_includes_a_component_nested_under_the_filtered_
  one` and its "not the reverse direction" counterpart, a
  `component_matches_filter` no-table-entry fallback test,
  `map::tests::component_filter_seeds_a_component_nested_under_the_
  filtered_one` and its nested-only counterpart, and `orphans::tests::
  component_filter_includes_a_site_whose_component_is_nested_under_the_
  filtered_one`.
- Verified end to end through the real binary against a synthetic
  nested-component repo (`services/api/go.mod` +
  `services/api/internal/go.mod`, the exact
  `innermost_component_wins_for_nested_markers` shape):
  `map --section components --component api` lists both `api` and
  `internal`; `--component internal` lists only `internal`.
- **Deliberately out of scope for this slice** — multi-directory
  components (one component spanning two *disjoint* subtrees, e.g.
  `services/orders` + `libs/orders-proto` as a single named component)
  is a separate capability (would need `Component::path: String` to
  become `paths: Vec<String>`, a breaking schema change requiring a
  `SCHEMA_VERSION` bump) — not attempted here, since descendant scoping
  needed no schema change at all and the two are independent problems.
  See `docs/STATUS.md`'s "deliberately absent" list.

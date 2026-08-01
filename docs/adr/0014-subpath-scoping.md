# 0014 — `--subpath` scoping for `where`/`deps`/`map`

**Status:** accepted · **Date:** 2026-08-01 · **Milestone:** M1.b.2b

## Context

Real-world PHP feedback (a TYPO3 codebase indexed alongside an
unrelated `typo3_v8_delete_me` directory) surfaced that `map`/`deps`/
`where`'s positional `PATH` argument doesn't scope an already-built
`--out` index — traced to `target::resolve()`
(`crates/carto-cli/src/target.rs`): once `--out` is given, `path` only
feeds repo-root canonicalization, never the query itself, so
`graph::load` always loads the entire `graph.json` regardless. There
was no subtree-scoping mechanism anywhere in the query layer. This ADR
adds one — a new `--subpath <DIR>` flag, independent of the existing
positional `PATH` (which keeps its current, unrelated meaning).

## Decisions

- **One principle applied uniformly across `where`/`deps`/`map`:
  restrict which rows get *listed*; never restrict what the underlying
  graph traversal/aggregation actually *computes*.** Three commands
  with three different jobs (a match list, a BFS, a set of aggregate
  rankings) would otherwise each need a bespoke scoping rule invented
  from scratch; this principle is a plain, restatable answer to "what
  does `--subpath` do here" for all three, so someone modifying the
  query layer later has a single mental model instead of three.
- **`QueryGraph::path_in_scope(id, subpath)`** (`query/mod.rs`) is the
  one shared predicate every command's `--subpath` filtering goes
  through. A `File` node is in scope iff its `path` equals `subpath` or
  starts with `subpath + "/"` — segment-boundary-safe, so
  `"src/handlers"` doesn't also match `"src/handlers2/x.ts"`. A
  `Symbol` node is in scope iff its *owning file* is. A `Module` node
  is **always** in scope: packages have no directory, so path-prefix
  filtering was never a meaningful question for them — they're either
  pulled in by an in-scope file's own edges or they aren't shown at
  all (see `map`'s treatment below), the same way they already worked
  before this ADR. `subpath: None` (or empty/whitespace-only, tolerated
  as equivalent) means no restriction.
- **`where`**: a straightforward filter on `FindResult.matches`.
  Incidental benefit: `deps`'s `resolve_target` (below) reuses this to
  disambiguate an otherwise-ambiguous symbol name by directory — not a
  new mechanism, just `find` called with a narrower `subpath`.
- **`deps`: the BFS traversal itself is unaffected by `--subpath`.** It
  still expands through out-of-scope nodes, so a dependency chain that
  dips outside the subtree and back is never silently broken — only
  which discovered nodes get *reported* is filtered. Two precise
  changes to the existing loop: `next_frontier` (not `edges_this_hop`)
  is `visited`/expanded unconditionally, decoupling "still worth
  exploring" from "worth showing"; the loop's early-exit condition
  became `next_frontier.is_empty()` (was `edges_this_hop.is_empty()`)
  — a hop can legitimately discover zero *visible* rows while still
  having real descendants reachable in a later hop, and the old
  condition would have stopped the whole traversal right there.
  `hit_depth_cap`/truncation logic needed no change — it already keyed
  off `frontier`, which stays scope-independent.
- **`resolve_target`'s `--subpath` is a tiebreaker, not an
  unconditional filter — this was a real bug caught by end-to-end
  testing, not a decision made up front.** The first implementation
  applied `subpath` to the *initial* name lookup unconditionally,
  which broke the single most common use case: `deps handle --subpath
  src/orders` to see which of `handle`'s dependencies land in
  `src/orders`, where `handle` itself lives entirely outside that
  directory. Fixed by resolving unscoped first; `subpath` is only
  applied as a second, narrowing attempt when the unscoped lookup is
  already ambiguous (more than one candidate) — so an unambiguous name
  always resolves regardless of where it lives, and `--subpath` still
  disambiguates a genuinely ambiguous one (the original motivating
  case: the same symbol name present in a live tree and in an unrelated
  vendored/dead-code directory).
- **`map`, applied per-section, each keeping its underlying computation
  whole-graph-accurate and only restricting which rows are listed**:
  - `counts.files`/`counts.symbols`: in-scope nodes only.
  - File fan-in/out ranking (`top_modules_lines`): lists only in-scope
    files, but each row's fan-in/out numbers are still the file's
    *real*, whole-repo connectivity — "this file has 40 real
    importers" stays informative even when only browsing a subdir, not
    clipped at the boundary into a smaller, less meaningful number.
  - External-package fan-in ranking: **deliberately the opposite
    choice** — recomputed counting only edges whose *source* file is in
    scope. "What does this subtree depend on externally" is the useful
    question there, unlike the file case above; worth stating plainly
    as an intentional asymmetry, not an inconsistency someone should
    "fix" later to match the file ranking's rule.
  - `entry_points_lines`: `has_incoming_import` stays computed
    whole-graph — a file imported only from *outside* the subtree must
    not look like a false entry point just because that importer isn't
    listed; only the *candidate list* (which files/symbols get
    considered) is restricted to in-scope ones.
  - `counts.modules`: recomputed as modules touched by at least one
    edge with an in-scope endpoint. Reduces to exactly today's
    pre-`--subpath` count when `subpath` is `None` (every Module node
    in the graph already has ≥1 edge by construction — `resolve.rs`
    never creates one without immediately adding the edge that
    references it) — verified, not assumed: the full pre-existing
    `map` test suite passes unchanged after this change.

## Consequences

- New tests at every layer: `query/find.rs`, `query/deps.rs` (including
  a dedicated test for the frontier/report-split BFS behavior — the one
  genuinely new piece of traversal logic, not just covered
  end-to-end), `query/map.rs` (all five section-specific scoping
  rules), and `crates/carto-cli/tests/cli_query.rs` (one real
  `carto`-binary test per command).
- `docs/carto-design-spec.md` §7.1's flag list for `deps`/`where`/`map`
  now includes `--subpath` — a new flag is a normative surface change,
  not just code + this ADR.
- `deps`'s truncation `next_call` hint now carries `--subpath` forward
  when set, so the suggested follow-up command reproduces the same
  scoped view instead of silently dropping the restriction.

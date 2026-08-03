# 0021 — `Module`/`File` discovery for `where`/`deps`, and `deps`'s
`same_file_as_root` flag

**Status:** accepted · **Date:** 2026-08-03 · **Milestone:** post-S-1
improvement plan, §1.2/§1.4

## Context

S-1's L3 task failed outright: `carto deps carto_core --dir in` errored
("no node ID or symbol named `carto_core` found in this index").
`where` only ever searched `Symbol` nodes (`docs/STATUS.md`'s own
"deliberately absent" list named this explicitly, calling broadening it
"a real decision, not an oversight"); node IDs are blake3 hashes with no
other reachable lookup path (spec §4.3), so a `Module`/`File` node was
*unreachable from the tool surface entirely*. The only signal reaching
`map`'s output was an aggregate fan-in count with no path from that
number to the underlying importer list. Every session in the benchmark
that answered L3 correctly did so by reading `graph.json` directly,
bypassing the tool surface — which an MCP client fundamentally cannot
do, since it only has the tools carto exposes.

Separately, T5's carto-arm session read `deps`'s own (independently
verified correct) output and still missed a real caller: a caller
symbol living in the same file as the callee was misdescribed as "the
method definition itself" rather than a distinct call site. Not a data
gap — the response simply doesn't distinguish "a call site in the
file where the callee is defined" from "the callee's own definition."

## Decisions

### `find`/`where`: `Module` nodes, kept as a separate list

- **A new `FindResult.module_matches: Vec<ModuleMatch>`, not a widened
  `SymbolMatch`.** A module has no `sym_kind`/`location`/`signature` to
  answer with; folding it into `SymbolMatch` would mean every consumer
  handles a bundle of optional fields instead of two small, honest
  shapes. `ModuleMatch` carries `id`/`path`/`external` — `external` is
  surfaced because it changes what a caller can do next (an internal
  module has real edges to traverse via `deps`; an external one is a
  leaf).
- **`limit`/truncation count `matches` and `module_matches` combined,
  symbols first, modules filling whatever's left** — not two
  independently-capped lists. A module match is rare in practice (only
  when the needle happens to equal a module path), so this rarely
  matters, but keeping one shared budget is what keeps the response's
  total size bounded the same way it always has, rather than doubling
  worst-case size to `2 × limit`.
- **`File` nodes are still not searched by `find`.** Spec §7.1 names
  "symbol name" specifically for `where`; a file is a traversal
  starting point more than a `where`-style name lookup, and `deps`'s
  own target resolution (below) is where it becomes reachable instead.
  `path_in_scope` already returns `true` unconditionally for a `Module`
  node (ADR-0014) — module matches are called through it anyway for
  symmetry with the symbol arm, not because it filters anything here.

### `deps`: target resolution also matches `Module.path` and `File.path`

- **`resolve_target`'s exact-match lookup now tries, in order: raw node
  ID (unchanged) → symbol name/module path (via the widened `find`,
  above) → a direct `File.path` scan** (`find` deliberately doesn't
  search files, so this is a separate loop in `deps.rs`, not a further
  widening of `find` itself).
- **A new `Candidate` enum** (`Symbol`/`Module`/`File`, each carrying an
  ID plus enough to describe itself) replaces the old symbol-only match
  list in the ambiguity path — a multi-match error can now legitimately
  mix all three kinds, and each needs a different description (a
  symbol's `file:start-end`, a module's `path`, a file's `path`). The
  `--subpath` tiebreaker behavior (ADR-0014) is unchanged; it applies
  uniformly across all three kinds' candidate lists.
- **The ambiguity error's wording changed from "matches multiple
  symbols" to "matches multiple nodes"** — accurate now that a match
  set isn't symbol-only.

### `deps`: `same_file_as_root`

- **A new `DepEdge.same_file_as_root: bool`**, computed by comparing
  each edge's node's owning file against the root's (`owning_file`:
  a `Symbol`'s own `file` field; a `File` node is its own answer; a
  `Module` has no file, so `false` unconditionally on either side of
  the comparison). Both the CLI and MCP text renderers append a short
  `[same file as root]` marker; the structured field is always present
  regardless of rendering.
- **No data model change was needed elsewhere** — this is a legibility
  fix, not a new fact the extractor has to compute. `graph.json`'s
  existing `Symbol.file` already carries everything needed.

## Consequences

- Verified against carto's own repo: `carto deps carto_core --dir in`
  now resolves and lists real importing files (previously an outright
  error); `carto deps TaintedString --dir in` marks the same-file
  `contains` edge from `taint.rs` with `[same file as root]`.
- `docs/STATUS.md`'s "`where` matches `Symbol.name` only" entry under
  "deliberately absent" is now false for modules specifically — updated
  to describe the new, narrower scope (files still excluded from
  `find`, by design, not by omission).
- `SCHEMA_VERSION` is unaffected — both changes are query-layer-only;
  nothing new is persisted to `graph.json`.

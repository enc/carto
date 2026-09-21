# 0036 — Cross-component import evidence, and `index`-time discoverability

**Status:** accepted · **Date:** 2026-09-21 · **Milestone:** multi-root
support, follow-up slice 1 (amends ADR-0035's evidence table; also fixes
two smaller ADR-0034 issues)

## Context

A review of ADR-0034/0035 against real binary output (not just the
fixture/unit-test suite) found two gaps in the capability those ADRs
shipped:

- **The `Certain`-confidence `Imports` edge — the kind ADR-0034's own
  Context calls out as "the most damaging" when it crosses a component
  boundary incorrectly — carried no record of the crossing at all.**
  `calls`/`references` edges get `"cross-component"`/`"imported-cross-
  component"` evidence (`CallResolver::bucket_evidence`, ADR-0035), but
  every file→file `Imports` edge (Rust `mod`, Python relative imports,
  PHP/C# `Qualified`/`NamespaceImport`, Go `PackagePath`) did not: a
  `using Acme.Shared;` between two `.csproj` components produced
  `imports certain ["namespace-import"]`, with nothing in `evidence`
  distinguishing it from an intra-component one. `deps` surfaces the
  crossing via `same_component_as_root` (a query-layer computation over
  `component_of`), but `graph.json` itself — the artifact every other
  consumer of this data reads — did not.
- **A monorepo with only a single top-level manifest gets zero
  components, silently.** `go.mod` at the repo root with `cmd/api/` and
  `cmd/worker/` each defining their own `Handler` produces
  `components: []` — the exact bare-name-collision failure ADR-0034
  exists to fix — with no signal anywhere in `index`'s own output that
  `.carto/roots.json` is the intended escape hatch for this shape
  (ADR-0034 deliberately never treats the walked root itself as a
  component candidate).

Two smaller issues surfaced in the same pass, fixed here since they're
in the same file and trivial: `.carto/roots.json` was read from disk
twice per index (once to parse, once for `config_digest`) — a narrow
but real TOCTOU — and `apply_declared_roots`' `any_file_under` allocated
a `format!("{path}/")` string per walked file per declared root instead
of the prefix-and-boundary check `component_of_path` already uses for
the identical rule.

## Decision

**Cross-component marking, extended to `Imports` edges.** A new free
function, `cross_component_marker(caller: Option<&str>, target:
Option<&str>) -> Option<&'static str>` in `lang/resolve.rs`, returns
`Some("cross-component")` whenever the two components differ (including
one `Some`/one `None` — a file under no recognized project root
importing one that is, or vice versa, is still a real crossing) and
`None` when they match (including both `None`, keeping every existing
evidence vector byte-identical for a repo with no components anywhere).
Applied at all five file→file `Certain` `Imports` edge sites
(`RawImport::Relative`'s two shapes, `Qualified`'s exact-FQN match,
`NamespaceImport`'s per-file fan-out, `PackagePath`'s per-file fan-out)
as an **additional** evidence entry, not a replacement — `Edge.evidence`
is already `Vec<String>` capped at `MAX_EVIDENCE_ENTRIES` (3), with room
to spare, unlike `CallResolver::bucket_evidence`'s label-substitution
approach (which had to suffix a single `&'static str`, since `calls`/
`references` evidence is chosen per-tier as one string). Edges into a
`Module` node (`"external-package"`, `"external-package-bare-
reference"`) are untouched — a module has no component to compare
against.

A new `file_id_to_component: BTreeMap<&NodeId, Option<&str>>` backs the
two sites (`Relative`, `Qualified`) whose target is resolved only as a
`&NodeId` with no extraction index in hand; the other three sites
(`NamespaceImport`, `PackagePath` fan-outs) already have a `usize`
target index and use the pre-existing `[fi]`-indexed `file_component`
vector directly.

**`map`'s cross-component edge summary gains confidence as a tally
key.** Previously `(from-component, to-component, kind) -> count`; now
`(from-component, to-component, (kind, confidence)) -> count`, so a
`## cross-component edges` line like `orders -> shared: 1
calls(inferred)` renders a `certain` crossing distinctly from an
`inferred` one rather than folding them into one number — the whole
reason evidence now records the crossing at all is defeated if the
summary immediately erases the distinction.

**`index` reports a `component_count`.** `IndexReport` (`indexer.rs`)
gains `component_count: usize`, read off `ComponentSet::components()`
before `graph::persist` consumes it. Both front ends print
`components: N`; when `N == 0`, an extra line points at
`.carto/roots.json` as the declaration path for a monorepo the
manifest-marker heuristic can't see (single top-level manifest, or no
manifest files carto recognizes at all). This is purely additive to
`IndexReport`'s serde shape (`--json`/MCP `structuredContent` gain one
field) and to both front ends' human text — no existing field changes
meaning.

**The two minor fixes**, `components/mod.rs`: `ComponentSet::discover`
now reads `.carto/roots.json`'s bytes exactly once (`read_config_bytes`
→ `parse_config(repo_root, bytes)` for the `ConfigDoc`, then the same
`bytes` feed `config_digest_for`), and `any_file_under` uses
`strip_prefix` + segment-boundary check instead of a per-file
allocation.

## Consequences

- New `lang::resolve::cross_component_marker` and its 5 call sites; new
  `file_id_to_component` index. All 458 pre-existing `carto-core` unit
  tests pass unchanged — none of them sets `FileExtraction.component`,
  so every comparison is `None == None` and no evidence vector grows.
- `fixtures/mixed.graph.golden.json` and
  `fixtures/rust-crate.*.golden.json` are byte-identical before/after,
  verified by diff, not assumed (both are single-component fixtures).
  `manifest.json` differs only in its timestamp fields, which were
  already unversioned/non-deterministic before this ADR.
- Double-index determinism (INV-7) reverified on `fixtures/rust-crate`
  and `fixtures/monorepo` after this change.
- The single-manifest-monorepo detection gap itself is **not** fixed by
  this ADR — `component_count`/the `.carto/roots.json` pointer only
  make that outcome visible at `index` time, they don't change what
  gets detected. See ADR-0037 for the detection-quality follow-up.

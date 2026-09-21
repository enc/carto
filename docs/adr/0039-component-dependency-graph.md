# 0039 — Component dependency graph

**Status:** accepted · **Date:** 2026-09-21 · **Milestone:** multi-root
support, follow-up slice 4 (extends ADR-0034's `Component`,
ADR-0035/0038's resolution/query layers) · **`SCHEMA_VERSION` 7 → 8**

## Context

ADR-0034 already reads each component's manifest file to detect
aggregator roots (`is_aggregator`, ADR-0037), but throws away everything
in it except the presence of a `[workspace]`/`"workspaces"` marker. The
manifest also says which *other* components a component actually
depends on — `go.mod`'s `require`, `package.json`'s `dependencies`,
`Cargo.toml`'s path deps, a `.csproj`'s `ProjectReference`s,
`composer.json`'s path repositories — a real signal this data model
had never used, requested directly by the user ("dependencies between
components shall be understood") as the fourth follow-up slice from
the ADR-0034/0035 review that also produced ADR-0036/0037/0038.

Without it, every cross-component edge looks the same regardless of
whether it crosses a boundary the two components' own manifests agree
should exist. `map`'s cross-component summary (ADR-0035) already
distinguishes an edge's *kind* and (ADR-0036) *confidence*, but has no
way to say "this crossing is expected" vs. "this crossing exists in
the code but neither manifest declares it" — the latter being exactly
the shape of an accidental coupling or an architecture-boundary
violation a monorepo owner would want surfaced.

## Decision

**Data model.** `Component` gains `depends_on: Vec<String>` (sorted
component names, deduplicated, self-references excluded) — a new
`crates/carto-core/src/components/deps.rs`, `deps::resolve(repo_root,
&mut Vec<Component>)`, called at the end of `ComponentSet::discover`
once the full component set (auto-detected and declared) is final.
Per-`kind` parsing, all dependency-free (no `toml`/XML crate — spec
§13 + `deny.toml`'s `multiple-versions = "deny"`; every format is
either JSON, already a `serde_json` dependency, or simple enough for a
line scan, the same discipline `is_aggregator` uses):

- **go** — `go.mod`'s `require` (single-line and `require (...)`
  block forms), matched against every other go component's own
  `module` declaration; a local `replace X => ../relative/path`
  directive resolved by path instead (a foreign-module replacement
  carries no component-dependency signal, so those are ignored).
- **node** — `package.json`'s `dependencies`/`devDependencies`: a
  `file:`/`workspace:` value with a relative path resolves by path;
  otherwise the dependency *key* is matched against another
  component's own declared `"name"`.
- **rust** — `Cargo.toml`'s `[dependencies]`/`[dev-dependencies]`/
  `[build-dependencies]` tables, both the inline-table
  (`foo = { path = "../foo" }`) and `[dependencies.foo]` sub-table
  forms, any entry naming a `path = "…"`.
- **dotnet** — every `*.csproj`/`*.fsproj` directly in the component's
  own directory, each `<ProjectReference Include="…">`.
- **php** — `composer.json`'s `repositories` entries of
  `"type": "path"` (resolved by path) plus `require` entries matching
  another component's own declared `"name"`.

`terraform`/`custom`-kind components have no manifest-identity concept
carto knows how to read here — `depends_on` stays empty for them, a
real, disclosed scope limit (see Consequences), not a claim of
"confirmed no dependencies." `crate::components::
DEPENDENCY_AWARE_KINDS` (`["go", "node", "rust", "dotnet", "php"]`)
names exactly the kinds this applies to, and is the one list both
`lang::resolve` and `query::map` consult — never duplicated.

**Use, three ways**, all additive (never a filter that can drop an
edge — INV-8: a manifest is evidence about intent, not proof a code
edge is false; dynamic loading and vendored copies are real):

1. **Evidence.** `lang::resolve::is_undeclared_dependency` — `true`
   only when the caller's own component is one of
   `DEPENDENCY_AWARE_KINDS` (so it has a real declared-dependency set
   to check, not merely an absent one — a terraform/custom-kind
   caller's crossing is *never* flagged, since there's no
   declared-dependency concept for it to have violated; conflating
   "never looked" with "confirmed undeclared" would misrepresent
   every contract-shaped cross-component edge from a terraform
   component as a violation, pure noise for the exact use case
   `orphans`/`contract` were built for) and the target's component
   isn't in it. Applied at all nine cross-component-marker-carrying
   edge sites (the five `Imports` sites from ADR-0036, plus the four
   `Calls`/`References` sites) as an additional evidence entry
   (`"undeclared-dependency"`), alongside `"cross-component"`, not
   replacing it.
2. **Resolution preference.** `resolve_fqn` (PHP `use`/C# `using X =
   ...`) and the `NamespaceImport` fan-out (C# `using Acme.Orders;`)
   each gain a tier between "same component" and the pre-existing
   arbitrary fallback (first-file-wins / full repo-wide fan-out):
   prefer a candidate in one of the caller's own declared
   dependencies. Same "prefer, fall through, never a new filter"
   discipline every other tier in this ladder uses — this can only
   turn one arbitrary choice into a better-justified one, never
   manufacture a match where none existed or override the
   same-component tier.
3. **`map --section components`** renders each component's own
   `depends_on` (`depends_on=billing` or `depends_on=(none)`) and
   marks a `## cross-component edges` row `[undeclared]` when the
   source component is dependency-aware and the target isn't in its
   `depends_on` — a component-level summary of the same per-edge
   evidence, not a second independent computation.

`SCHEMA_VERSION` bumped 7 → 8 for the new `Component::depends_on`
field (`#[serde(default)]`, the same absence-vs-zero discipline every
prior bump in this family follows — though `graph::load`'s exact-
version-match refusal means no live query ever actually observes that
default; it exists for direct `GraphDocument` construction in tests).

## Consequences

- New `crates/carto-core/src/components/deps.rs` (with its own
  `deps/tests.rs`, 14 tests covering all five kinds plus the
  self-dependency and dependency-unaware-kind exclusions) and
  `crate::components::DEPENDENCY_AWARE_KINDS`.
- `lang::resolve`'s `resolve()` and `extract_and_resolve()` both gain
  a `components: &[Component]` parameter — threaded from
  `indexer::build_and_persist` (which already has `ComponentSet` after
  `discover` runs, `depends_on` included). 4 new `resolve.rs` tests:
  the undeclared/declared evidence pair, the dependency-unaware-kind
  exclusion, and the `resolve_fqn` preference tier (a case
  deliberately constructed so a plain first-file-wins fallback would
  pick the *wrong* candidate, proving the tier changes the outcome,
  not just coincidentally agrees). All 489 pre-existing `carto-core`
  tests continued to pass unchanged throughout — every one of them
  passes `&[]` for the new parameter, so `component_depends_on` is
  always empty and `is_undeclared_dependency` always returns `false`.
- 3 new `map.rs` tests (`depends_on` rendering, the `[undeclared]`
  marker present/absent).
- `fixtures/monorepo/services/orders/go.mod` gains a real
  `require shared v0.0.0` line — verified end to end through the real
  binary: `map --section components` now shows `orders`'s own
  `depends_on=shared`, and the pre-existing `orders -> shared`
  cross-component `calls` edge (ADR-0035's own motivating case) no
  longer carries `"undeclared-dependency"`. A separate synthetic
  two-component repo with no manifest declaration confirmed the
  opposite: `evidence: ["cross-component", "undeclared-dependency"]`
  and a `[undeclared]`-marked summary row.
- Verified additive/backward-compatible the same way every prior
  ADR-0034-family bump was: `fixtures/mixed.graph.golden.json`'s only
  diff is `schema_version: 7 -> 8` (an empty `components: []` table
  has nowhere for a new per-component field to appear);
  `fixtures/rust-crate.*.golden.json` needed no regeneration at all
  (also zero components). Double-index determinism reverified on
  `fixtures/monorepo` with `depends_on` populated.
- **Deliberately out of scope for this slice**: `terraform`/`custom`
  components have no dependency resolution at all (no manifest-
  identity concept carto reads for either kind — a `.tf` file's own
  `module` blocks reference other `.tf` sources, not other
  components, and a declared/`"custom"`-kind root has no manifest
  format assumption to parse); a repo-local `.carto/deps.json`
  override for declaring dependencies a manifest can't express
  (mirroring `.carto/roots.json`'s own override shape) was considered
  and not built — no concrete need surfaced during this slice, unlike
  `.carto/roots.json`'s own origin story (ADR-0027's real finding).

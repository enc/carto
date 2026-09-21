# 0034 — Component discovery: multi-root support, the data model

**Status:** accepted · **Date:** 2026-09-21 · **Milestone:** multi-root
support, slice 1 (spec §4/§7 amendment, user request)

## Context

Real repos put several projects that belong together under one root:
microservices, frontends, lambdas, and the infrastructure code that
deploys them — a monorepo. carto's existing "same-package" tier (spec
§5.3 rule 2c) treats the *whole walked tree* as one package scope, an
assumption `docs/STATUS.md`'s "deliberately absent" list already named
explicitly: *"Same-package means same walked repo — no
`Cargo.toml`/workspace parsing. Revisit only if multi-crate false
positives show up in practice."* That revisit is this slice.

Two concrete failure modes follow from the single-scope assumption in a
monorepo:

- **Silent edge loss inside a component.** Two services each exporting
  `Handler` produce two bare-name candidates repo-wide; INV-8 correctly
  refuses to guess and emits no edge — including for a caller in the
  *same* service as its real target. A multi-component repo gets worse
  intra-component resolution than a single-component one would.
- **False edges across components**, where only one service happens to
  define a name and a caller in an unrelated service resolves to it
  with evidence claiming it's "same-package" — untrue in a monorepo,
  and for PHP `use`/C# `using` the false edge lands at `Certain`
  confidence, the most damaging kind.

This ADR covers the data model and discovery mechanism — a new
`components` module, the `component` field on `FileNode`, and the
persisted `graph.json` shape. [ADR-0035](0035-component-scoped-resolution-and-query-layer.md)
covers what changes in `resolve.rs` and the query layer once this
dimension exists.

## Decision

**One tree, N components** — not N filesystem roots. `carto index
<monorepo>` walks a single root exactly as before (no change to
repo-relative paths, node IDs, `PathGuard`, `outdir`, `gitinfo`, or
`target::resolve`); a **component** (`crates/carto-core/src/
components/mod.rs`) is a directory *inside* that tree recognized as the
root of one project. Every walked file belongs to at most one
component — the innermost one whose path is a prefix of the file's own
path — or to a real, meaningful `None` bucket (repo-root scripts,
top-level docs), not a missing value.

- **Discovery runs over `walk`'s own `File`-node output, not a second
  traversal** — `.gitignore`/the built-in denylist/`.cartoignore` are
  respected for free, the same reuse `resolve.rs` already gets from
  being handed `walk`'s nodes rather than re-reading the tree.
- **Auto-detection by manifest marker**, priority order (first match
  per directory wins): `go.mod` → go, `package.json` → node,
  `Cargo.toml` → rust, `pyproject.toml`/`setup.py` → python,
  `composer.json` → php, `*.csproj`/`*.fsproj` → dotnet, then — the
  weakest signal, checked last — ≥1 file whose `FileNode.lang` is
  `Hcl` → terraform (reusing ADR-0025's two-part-extension fix, so
  `locals.tf.simu` counts the same as `alarms.tf`).
- **Aggregator suppression**: a `Cargo.toml` containing `[workspace]`
  and no `[package]`, or a `package.json` containing `"workspaces"`,
  is a workspace root, not a component — checked by reading the marker
  file's own content (always small; never `MAX_FILE_BYTES`-relevant).
  A read failure (permissions, a `walk`-vs-this-pass race) is treated
  as "not an aggregator," best-effort, not a hard index failure.
- **The walked root itself (`dir == ""`) is never a candidate,
  deliberately** — a component's whole point is to name a project
  *nested inside* the tree; a marker at the root describes the repo as
  a whole, already today's "no narrower scope" (`component: None`)
  case for every single-project repo. This is also what keeps a
  pre-existing fixture's own top-level `Cargo.toml` from silently
  turning "no components" into "one component spanning everything" —
  the backward-compatibility guarantee below depends on it.
- **Name collisions resolved symmetrically.** A candidate's name starts
  as its directory's own basename, sanitized to
  `^[A-Za-z0-9][A-Za-z0-9_.-]*$` (non-conforming characters become
  `-`; a non-alphanumeric leading character gets a `c-` prefix). On a
  collision (`services/orders` and `lambdas/orders` both wanting
  `"orders"`), *every* colliding candidate is extended by one more
  trailing path segment at once and retried
  (`services-orders`/`lambdas-orders`) — which candidate keeps the
  short name never depends on processing order, only on each one's own
  path. Directory paths are unique by construction, so this always
  terminates.
- **`.carto/roots.json`** layers on top, following the exact
  built-ins-plus-repo-file shape ADR-0027 established for
  `.carto/contracts.json`: JSON (no new dependency; `serde_json` is
  already a `carto-core` dependency), closed-vocabulary fields
  (`name` charset, `path`) hard-error naming the file and the bad
  value, `kind` stays open (free-form, like a contract rule's
  `category`). `detect` defaults to `false` when `roots` is non-empty
  (declared roots *replace* auto-detection) and `true` when
  empty/absent; an explicit value always wins — so declaring roots is
  the common "I know better than the heuristic" case, and
  `"detect": true` makes declarations additive instead. Validation:
  `name` non-empty and charset-conforming (a repo-author-declared name
  is intent, not a best-effort guess the way marker detection is, so
  it hard-errors rather than auto-sanitizing); `path` repo-relative,
  non-empty, no `..`, and must contain at least one walked file (an
  empty match is a typo, not a legitimately-empty component); a
  declared root at an already-detected path *replaces* that entry.

## Data model and schema

- **`Component { name, path, kind }`** — plain `String` fields, not
  `TaintedString`: like `FileNode::path`, these are extractor-computed
  identifiers (a directory basename, or a repo-author-declared config
  value validated against a closed charset), never captured source
  text — INV-5 has nothing to say about them, the same reasoning
  `where_cmd.rs` already applies to `FileNode::path`.
- **`FileNode` gains `component: Option<String>`**
  (`#[serde(default)]`), set by `indexer::build_and_persist` between
  `walk` and `lang::extract_and_resolve` — `walk` itself has no
  component concept.
- **`GraphDocument` gains `components: Vec<Component>`**, sorted by
  `path` (INV-7) — a separate top-level table, not a new `NodeData`
  variant: a `Component` has no place in spec §4.1's node vocabulary
  and no edges of its own, and every `File`/`Symbol` node's own
  `component` field already carries the membership relationship. This
  is purely a lookup/rendering convenience for the query layer.
- **`InboundCallSite` (nested in `SymbolNode::unresolved_inbound_calls`,
  ADR-0033) gains `component` too** — without it, this honesty-signal
  row could silently mix components, listing a same-named call site
  from a *different* component as if it were plausibly this symbol's
  own caller.
- **`Manifest`/`PersistMeta` gain `roots_rule_digest: String`**
  (`#[serde(default)]`, unversioned — `Manifest` isn't schema-checked
  or read back by any query command, unlike `graph.json`), mirroring
  `ignore_rule_digest`'s provenance role for `.cartoignore` — closes,
  for `.carto/roots.json`, the exact gap ADR-0027 left open for
  `.carto/contracts.json` (`docs/STATUS.md`'s "deliberately absent"
  list).
- **`SCHEMA_VERSION` 6 → 7.** A v6 `graph.json` predates component
  discovery entirely; its absent `component`/`components` must not be
  misread as "confirmed not in any component" (a real, meaningful
  value this slice introduces) versus "never looked for one" — the
  same absence-vs-zero distinction every prior bump in this family
  (ADR-0020/0023/0026/0029/0033) protects.

## Backward compatibility, verified not assumed

A repo with no nested components anywhere (every existing fixture,
before this slice) produces a graph diff that is *exactly* the new
`"component": null` keys on every `File`/`Symbol` node plus the empty
`"components": []` table — checked by diffing `fixtures/mixed.graph.
golden.json` before and after, not merely argued. `where`/`deps`/`map`'s
own goldens against `fixtures/rust-crate` (a single Rust package, no
nested components) are unaffected by this ADR at all — the query-layer
`component` fields ADR-0035 adds there are `null`/empty for the same
reason.

## Consequences

- New module `crates/carto-core/src/components/mod.rs`, 22 unit tests
  against real temp-dir fixtures (not synthetic in-memory-only data) —
  marker detection per language, aggregator suppression (both
  directions: a workspace root correctly excluded, each component's
  *own* marker still recognized), name collision symmetry, innermost-
  match `component_of_path`, and the full `.carto/roots.json`
  validation matrix (missing file, malformed JSON, bad name charset,
  `..` escape, no walked file at the declared path, duplicate
  name/path pairs, `detect: true` additive vs. default replacing).
- Dogfooded against carto's own repo (a real 4-crate Cargo workspace):
  correctly detects `carto-cli`/`carto-core`/`carto-grammars`/
  `carto-mcp` as components, correctly suppresses the workspace-only
  root `Cargo.toml`, and — genuinely, not a bug — also picks up
  `fixtures/go-svc` (`go.mod`) and `fixtures/sid-like/infra` (`.tf`
  files) as their own components, since they really are nested project
  roots inside the walked tree.
- **Deliberately out of scope for this slice** (see `docs/STATUS.md`'s
  "deliberately absent" list): a `go.work`/npm-nested-workspace-aware
  multi-level component hierarchy beyond simple innermost-match
  nesting; per-component `.cartoignore`; joining `.carto/roots.json`'s
  digest into anything beyond `manifest.json` provenance (no query
  command reads `Manifest` back today, same reasoning ADR-0027 gave
  for its own config file).

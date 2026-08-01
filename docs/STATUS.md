# carto — status / handoff

**Milestone:** M1.b.2b in progress (Python done; TS/TSX, JS, Go not started)

Where the implementation is in the milestone sequence, what's deliberately
absent, and what's next. Everything else lives elsewhere on purpose:

| For | Read |
|---|---|
| What the product is, normatively | [`docs/carto-design-spec.md`](carto-design-spec.md) (§2 invariants override everything) |
| Architecture, commands, conventions, gotchas | [`CLAUDE.md`](../CLAUDE.md) |
| Why a given decision was made | [`docs/adr/`](adr/) |
| What changed and when | `git log` |

## Milestone position

Spec §10's milestones proved too large to plan and execute as single units,
so they've been subdivided. Current state:

- **M1.a** — invariant substrate (consts, error, taint, pathguard, outdir). Done.
- **M1.b.1** — walk, graph store, persist choke-point, `carto index`. Done.
- **M1.b.2a** — Rust extractor end-to-end: symbols, imports, calls, full
  spec §5.3 resolution policy. Done.
- **M1.b.3** — `where`/`deps`/`map` commands, over the `QueryGraph`
  adjacency index (ADR-0009/0010). Done. Taken ahead of M1.b.2b — no new
  deps/grammar work, and it closes ADR-0005's traversal deferral against
  real requirements instead of speculatively.
- **M1.b.2b** — TS/TSX, JS, Python, Go extractors. **Python done**
  (ADR-0011) — current head. TS/TSX, JS, Go not started.
- **M2+** — infra graph, join, MCP, ingest, real redaction. Per spec §10.

## Deliberately absent — not gaps, not bugs

Do not "fix" these without checking the linked reasoning first:

- **Rust path-qualified call resolution** (`Type::method()`, `module::func()`).
  Call matching is bare-name-only; those call sites aren't captured at all.
  Spec §5.3's "deliberately modest" policy — [ADR-0008](adr/0008-rust-resolution-policy-mapping.md).
  **Python does not have this exclusion** — its grammar has no node type
  distinct from plain attribute access for a module-qualified call
  (`pkg.func()`) vs. an instance call (`order.summary()`), so both
  resolve through the same tiers. A genuine cross-language difference,
  not an inconsistency — [ADR-0011](adr/0011-python-resolution-policy-mapping.md).
- **`use_list` / `use_wildcard` / `use_as_clause`** Rust import shapes —
  extracted as nothing rather than partially interpreted. Same ADR.
  Python's `from x import *` is excluded the same way (ADR-0011).
- **Python has no visibility keyword; a leading underscore is the
  is_pub proxy** for call-resolution tiers (b)/(c), applied uniformly to
  functions/classes/methods (including dunder methods, e.g. `__init__`,
  which are therefore never tier-(b)/(c)-eligible) — ADR-0011.
- **Python module-level assignments aren't extracted as `Const`** — no
  `const` keyword, and "uppercase name = constant" is a heuristic with
  no clear v1 win. `SymKind::Const` stays unused for Python.
- **Every Python absolute import is classified `external`**, even one
  naming a local top-level package — no walked-repo equivalent of
  Rust's `known_modules` (no explicit module declaration to collect).
  ADR-0011; revisit only if false "external" classifications on
  genuinely local packages show up in practice.
- **Stdlib/external method calls land in `unresolved_calls`** (e.g.
  `input.is_empty()`). Correct behavior: there's no type information to
  resolve against, and INV-8 requires honest omission over a guessed edge.
- **`calls` edges are never `certain`** — always `inferred`, by spec.
- **`manifest.json`'s `dirty` is always `null`** — [ADR-0006](adr/0006-manifest-git-provenance.md).
- **No `petgraph`** — [ADR-0005](adr/0005-graph-store-sorted-vec-not-petgraph.md); revisit at M1.b.3 when real traversal exists.
- **No `.github/`**; §9.5 gates 1/3/4 run locally via `scripts/gates.sh`,
  gate 2 (strace) needs Linux CI — [ADR-0004](adr/0004-deferred-security-gates.md).
- **`carto-grammars` contains zero `unsafe`** despite being the designated
  carve-out — [ADR-0007](adr/0007-carto-grammars-unsafe-carveout.md).
- **"Same-package" means "same walked repo"** — no `Cargo.toml`/workspace
  parsing. Revisit only if multi-crate false positives show up in practice.
- **`where` matches `Symbol.name` only** — `File`/`Module` nodes aren't
  searched. Spec §7.1 names "symbol name" specifically; broadening this
  is a real decision, not an oversight.
- **`deps` reports a spanning-tree view, not every edge in the reachable
  subgraph** — each node listed once via the edge that first discovered
  it; a "cross edge" between two already-discovered nodes isn't shown
  separately. See `crates/carto-core/src/query/deps.rs`'s module doc.
- **`map`'s "entry points" is a structural heuristic** (files with no
  incoming `imports`, symbols named `main`), explicitly labeled as such —
  not real per-language entry-point analysis (e.g. a `Cargo.toml`
  `[[bin]]` table), which carto doesn't have in v1.
- **`map`'s infra/join sections are explicit "requires M2"/"requires M3"
  placeholders**, not omitted — an absent section would read as "no
  infra found," which isn't yet a true statement either way.
- **No stale-index detection** — `manifest.json` carries `commit_sha`/
  `file_sha256` but nothing compares them against the working tree.
  M5 incrementality territory.
- **`MCP_TEXT_CAP`'s 8 KiB byte cap isn't enforced anywhere yet** — no
  MCP text renderer exists until M4. Today's output is bounded by each
  command's own `--limit`/`--depth`/`--budget`.
- **Query-layer `--json` output never fences tainted text; human output
  fences once per section, not once per row** —
  [ADR-0010](adr/0010-tainted-text-in-query-output.md).

## Next: M1.b.2b continued (TS/TSX, JS, Go)

Python (ADR-0011) is the second data point after Rust (ADR-0008) for
what varies per language: relative-import semantics, what "exported"
means, whether path-qualified calls are even syntactically
distinguishable, what "same-package" should mean. TS/TSX, JS, and Go
each still need their own version of that ADR — expect at least one
real judgment call per language, not mechanical repetition. TS's
`import './x'` is genuinely file-relative with extension-guessing
(`.ts`/`.tsx`/`.js`/`index.ts`) and `package.json`/`node_modules` for
external packages — meaningfully more resolution machinery than either
Rust's or Python's import model needed. `fixtures/ts-app`/`go-svc`
(spec §11.1) get built alongside the extractors that need them.

`carto where`/`deps`/`map` need no changes for any of this — confirmed
end-to-end against `fixtures/py-lib` with zero py-lib-specific code in
`crates/carto-core/src/query/`, and expected to hold for every future
language the same way.

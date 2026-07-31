# carto — status / handoff

**Milestone:** M1.b.3 complete · M1.b.2b not started

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
  adjacency index (ADR-0009/0010). Done — current head. Taken ahead of
  M1.b.2b — no new deps/grammar work, and it closes ADR-0005's traversal
  deferral against real requirements instead of speculatively.
- **M1.b.2b** — TS/TSX, JS, Python, Go extractors. Not started.
- **M2+** — infra graph, join, MCP, ingest, real redaction. Per spec §10.

## Deliberately absent — not gaps, not bugs

Do not "fix" these without checking the linked reasoning first:

- **Path-qualified call resolution** (`Type::method()`, `module::func()`).
  Call matching is bare-name-only; those call sites aren't captured at all.
  Spec §5.3's "deliberately modest" policy — [ADR-0008](adr/0008-rust-resolution-policy-mapping.md).
- **`use_list` / `use_wildcard` / `use_as_clause`** Rust import shapes —
  extracted as nothing rather than partially interpreted. Same ADR.
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

## Next: M1.b.2b

TS/TSX, JS, Python, Go `LangExtractor` impls, each needing its own
`.scm` query set and its own ADR-0008-style mapping of spec §5.3's
generic rules onto that language's actual import/call semantics. Expect
each language to require at least one real judgment call rather than
mechanical repetition — TS's `import './x'` is genuinely file-relative,
which nothing in Rust's `mod`/`use` system is. `fixtures/ts-app`/
`py-lib`/`go-svc` (spec §11.1) get built alongside the extractors that
need them.

`carto where`/`deps`/`map` all work end-to-end today, but only ever see
Rust repos until this lands — every language's `.scm` set feeds the same
`QueryGraph`/`find`/`deps`/`map` unchanged.

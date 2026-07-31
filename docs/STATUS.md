# carto — status / handoff

**Last updated:** 2026-07-31 · **Milestone:** M1.b.2a complete, M1.b.2b not started

Read this first if you're picking up this work cold. Full product spec:
[`docs/carto-design-spec.md`](carto-design-spec.md) (normative, MUST/SHOULD/MAY
per RFC 2119 — invariants in §2 override everything else). Decisions made
so far and why: [`docs/adr/`](adr/).

## Where things stand

`main` has 19 commits, working tree clean, `scripts/gates.sh` green
(86 unit tests + 12 CLI integration tests + 5 compile-fail cases):

```
5916dc4 feat(cli): wire extraction into carto index
cad41df feat(core): cross-file import/call resolution (spec §5.3)
c25d127 feat(core): LangExtractor trait + Rust extractor (raw symbols/imports/calls)
9df5ffb feat: carto-grammars crate (native tree-sitter, Rust grammar)
d19f978 feat(core): SymbolNode/ModuleNode/SymKind; TaintedString PartialEq
5e1d311 test: fixtures/mixed, determinism + pathguard CLI gates; docs update
c9d0827 feat(cli): carto index, --out resolution, exit-code plumbing
9a5ed4a feat(core): git HEAD provenance; pathguard checks out-root before creating it
f9d887a feat(core): walk with built-in denylist, sensitive/binary/size handling
b46c016 feat(core): graph data model — stable IDs, nodes, edges, persist choke-point
```

(plus one more commit landing alongside this file for
`fixtures/rust-crate` + extraction acceptance tests — see `git log` for
the exact current head.)

This is **M1.b.2a**: a complete Rust extractor — symbols, imports, calls,
and the full spec §5.3 resolution policy, one language end-to-end. TS/TSX,
JS, Python, Go extractors (spec §5.2's remaining v1 language set) are
**M1.b.2b, not started** — they reuse everything built here (the
`LangExtractor` trait, `Symbol`/`Module` node types, edge/resolution
machinery, `carto-grammars` crate structure); only new `.scm` queries and
a `LangExtractor` impl per language are new work.

### What exists

- `crates/carto-grammars`: new crate, the workspace's designated
  `unsafe_code` exception (spec §3.1) for tree-sitter FFI — though in
  practice, `tree-sitter` 0.26 / `tree-sitter-rust` 0.24's language
  loading turned out not to need an actual `unsafe` block (verified
  empirically; see ADR-0007). `rust_language()` behind the default-on
  `native-grammars` feature (spec §5.4).
- `crates/carto-core`:
  - `graph::node`: `Symbol`/`Module` node types alongside the existing
    `File` (spec §4.1), `SymKind` (spec's literal enum), `UnresolvedCall`.
    `graph::id` gained `sym_id`/`module_id` (spec §4.3's recipe, extended
    to a kind Module doesn't get an explicit format for).
  - `taint::TaintedString` gained `PartialEq`/`Eq` (needed for
    `SymbolNode.signature: Option<TaintedString>` to fit the existing
    `Node`/`NodeData` derive chain — doesn't weaken INV-5, since equality
    isn't one of the restricted accessors).
  - `lang::extractor`: the `LangExtractor` trait (spec §5.2, simplified
    to take `relpath: &str` instead of spec's literal `FileCtx` — nothing
    needs more yet) and raw pre-resolution types (`RawSymbol`,
    `RawImport`, `RawCallSite`).
  - `lang::rust`: `RustExtractor` — tree-sitter queries
    (`lang/queries/rust/{symbols,imports,calls}.scm`) for top-level/
    impl-block items, `mod`/`use` declarations, and plain/method call
    sites (never path-qualified calls — out of scope this slice).
  - `lang::resolve`: whole-repo, single-threaded cross-file resolution
    implementing spec §5.3's full policy — `mod` → sibling-file
    `imports` edges, `use`-root → internal/external classification
    (external → deduped `Module` node), the three-tier call-resolution
    (same-file / imported / same-package, first match wins,
    always `inferred`, never `certain`), and `unresolved_calls` for
    anything that doesn't resolve unambiguously (INV-8).
  - `lang::extract_and_resolve`: orchestrates the above over every walked
    `File` node whose `Lang` has a registered extractor.
- `crates/carto-cli`: `carto index` now inserts `Symbol`/`Module` nodes
  and `contains`/`imports`/`calls` edges alongside `File` nodes, before
  persisting.
- `fixtures/rust-crate/`: new, purpose-built 3-file fixture exercising
  every spec §5.3 resolution tier plus the impl-block/qualified-name
  path and the "no guessing" honesty path — see its README for the exact
  scenario-to-test-case mapping.
- `crates/carto-cli/tests/cli_extraction.rs`: 7 semantic-assertion tests
  against `fixtures/rust-crate`'s parsed `graph.json` (not a golden-file
  byte-compare — too fragile to hand-review for a symbol-rich fixture),
  plus its own determinism check.
- `fixtures/mixed.graph.golden.json` regenerated: `src/main.rs` now
  produces a `main` `Symbol` node + one `contains` edge.
- Tests: 86 unit tests in `carto-core` (up from 63), 12 CLI integration
  tests across `cli.rs` + `cli_extraction.rs` (up from 5), 1 unit test in
  `carto-grammars`, 5 trybuild compile-fail cases (unchanged).

### What's deliberately NOT here yet

TS/TSX, JS, Python, Go extractors (M1.b.2b). Path-qualified call
resolution (`Type::method()`, `module::func()` — same-name-only matching
this slice, spec §5.3 "deliberately modest"; see ADR-0008).
`use_list`/`use_wildcard`/`use_as_clause` Rust import shapes.
`where`/`deps`/`map` commands (M1.b.3 — needs real traversal, also when
ADR-0005's "why not petgraph yet" gets revisited). Infra graph, join,
MCP, ingest, real redaction. All later milestones.

## Decisions already made (don't re-litigate without new info)

Carried over from M1.b.1 (still true): name stays `carto`; no `.github/`
yet (ADR-0004, gates 1/3/4 run locally, 2/5 deferred); commit straight to
`main`; `cargo deny check advisories` excluded from the local gate;
graph store is `BTreeMap`-keyed, not `petgraph`, until real traversal
exists (ADR-0005); `manifest.json`'s `commit_sha` is hand-read, `dirty`
stays `null` (ADR-0006).

New this milestone:

- **M1.b.2 was split into M1.b.2a (Rust, full depth) / M1.b.2b (the
  remaining 4 languages)** rather than built as one unit — same reasoning
  as the M1.b.1/b.2/b.3 split: too large a single plan/execution unit.
- **`carto-grammars` keeps its `unsafe_code`-exception role even though
  zero `unsafe` code exists in it today** (ADR-0007) — the exception is
  architectural (spec §3.1 names this crate as the seam), not contingent
  on today's tree-sitter API surface.
- **Rust's mapping of spec §5.3's generic resolution rules onto Rust's
  actual module system is fully recorded in ADR-0008** — read this
  before implementing TS/Python/Go's extractors; it's the template
  (not a verbatim rule) for the same kind of decision each of those
  languages will need to make for itself.
- **"Same-package" (call resolution tier c) means "same walked repo,"
  v1-wide** — no `Cargo.toml`/workspace parsing. Revisit only if a real
  need surfaces (e.g. multi-crate workspace false-positives in practice).

## Known gotchas

Carried over from M1.b.1: sandbox denies direct `git` mutation without
`dangerouslyDisableSandbox: true`; this dev environment's write tooling
refuses to create `.env`/`*.pem`-shaped paths even for fake fixture
content (hit again — not needed this milestone, no new instance); the
`carto-cli` workspace-member historical wart in early commits; no
`strace`/`gh`/`cargo-nextest` locally.

New this milestone:

- **A tree-sitter query can match the same underlying node more than
  once** if two patterns both describe it at different specificity (an
  impl-block method's `function_item` matched both the generic
  `symbol.function` pattern and the specific `symbol.method` pattern).
  Caught via manual inspection of a real `carto index` run against
  `fixtures/rust-crate` (a call that should have resolved via
  same-package was showing up unresolved — traced back to a spurious
  duplicate `pub_by_name` entry making tier c look ambiguous when it
  wasn't). Fixed by deduping `extract_symbols`'s output by item byte
  range, preferring the more specific classification. Worth remembering
  when writing TS/Python/Go's queries too — check for this class of bug
  by eyeballing real extraction output, not just unit tests against
  synthetic snippets (the unit tests here didn't catch it, since none of
  them checked for *absence* of a duplicate).
- `docs.rs` is not on this sandbox's network allowlist (crates.io/
  static.crates.io/index.crates.io are) — API surface questions that
  would normally be a docs.rs lookup get resolved empirically instead
  (write the code, see if it compiles / behaves as expected).

## Verifying the current state works

```bash
bash scripts/gates.sh                              # fmt, clippy, deny, tests — must be green

cargo run -p carto-cli -- index fixtures/rust-crate --out /tmp/carto-rust --json 2>/dev/null | python3 -m json.tool
python3 -m json.tool /tmp/carto-rust/graph.json     # manual read-through against
                                                     # fixtures/rust-crate/README.md's table

cargo run -p carto-cli -- index fixtures/rust-crate --out /tmp/carto-rust-b
diff /tmp/carto-rust/graph.json /tmp/carto-rust-b/graph.json   # INV-7 (note: the --json
                                                     # run above used a different out
                                                     # dir; this line re-runs plain)

git log --oneline && git status                    # clean tree
```

## Next step: M1.b.2b

Not yet planned in detail. TS/TSX, JS, Python, Go `LangExtractor` impls,
each with their own `.scm` queries and their own ADR-0008-style mapping
of spec §5.3's generic rules onto that language's actual import/call
semantics (TS's `import './x'` genuinely is file-relative, unlike
anything in Rust's `mod`/`use` system — expect each language to need at
least one real judgment call, not just mechanical repetition of Rust's
approach). `fixtures/ts-app`/`py-lib`/`go-svc` (spec §11.1) will be
needed once there's something per-language to meaningfully test.

After that, M1.b.3: `where`/`deps`/`map` commands — needs `Symbol` nodes
and real edges to be useful (now available), and is also where ADR-0005's
"why not petgraph yet" decision gets revisited against actual traversal
requirements.

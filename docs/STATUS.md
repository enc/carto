# carto — status / handoff

**Milestone:** M1.b.2b done (Rust, Python, PHP, TS/TSX/JS, Go — spec §5.2's full v1 language set), plus C# added post-v1 on user request (ADR-0016). M2's first slice — real redaction (ADR-0017) — is also done. An MCP server slice (normally M4 scope) was pulled ahead on user request, to measure spec §11.4's S-1 benchmark before further capability work (ADR-0018). **The S-1 benchmark has now been run (ADR-0019): neither threshold is met on a one-trial measurement** (combined input tokens: carto uses 18.0% more, not ≥30% fewer; accuracy: carto is 6.2 points lower, not ≥20 higher) — full per-task result in [`bench/`](../bench/) and ADR-0019. **The post-S-1 improvement plan's slices 1–4 are now implemented** (`docs/post-s1-improvement-plan.md`, ADRs 0020–0022: the honest-absence signal, `map --section`, `Module`/`File` discovery plus the same-file caller flag, and Rust grouped-`use` extraction) — see the milestone entry below. **M3 still does not proceed automatically — spec §11.4 requires the user's decision, presented and pending; re-running S-1 against this slice (plan step 5) is a separate, not-yet-run step.**

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
- **M1.b.2b** — TS/TSX, JS, Python, Go, PHP extractors (PHP added to the
  v1 set post-M1.b.2a, ADR-0012). **Python done** (ADR-0011), **PHP
  done** (ADR-0012), **TS/TSX/JS done** (ADR-0013, which also fixed a
  cross-language alias-resolution gap ADR-0012 had documented but left
  unfixed), **Go done** (ADR-0015 — the closing slice; new
  `RawImport::PackagePath` for Go's directory-shaped, module-qualified
  import paths, and a new opt-in directory-scoped resolution tier for
  Go's file-independent package visibility). Spec §5.2's v1 language
  set is now fully implemented.
- **Post-v1-set: C#** (2026-08-02, ADR-0016) — the eighth language,
  added on direct user request ahead of the M2+ roadmap, the same
  on-request scope-amendment path PHP took (ADR-0012). New
  `RawImport::NamespaceImport` (a C# `using` imports a *namespace* —
  many files — fitting no prior variant) with Go-style per-file
  fan-out, and `LangExtractor::namespace_separator()` so PHP's `\` and
  C#'s `.` FQNs share one index without cross-matching. Two
  user-confirmed judgment calls: `internal` counts as exported, and
  `new Foo()` is a captured call site resolving to the *type* (which
  is exactly why constructors are deliberately not symbols — see
  ADR-0016's constructor-ambiguity section).
- **M2 — real redaction** (2026-08-02, ADR-0017) — `redact::redact()`'s
  M1.b.1 no-op stub is filled in: hand-rolled pattern matching (8
  categories, spec §7.5(a): AWS keys, secret-shaped base64 near
  "secret", PEM headers, GitHub/GitLab/Slack tokens, JWTs, credential
  connection strings) plus per-token Shannon entropy (§7.5(b)), scoped
  to every `TaintedString` field (today: `SymbolNode.signature`) —
  reached via `Graph`'s new `nodes_mut()`, with no changes to
  `taint.rs`'s INV-5 machinery. Dogfooding (`carto index .` against
  carto's own source) caught and fixed a real false positive before
  shipping: long, descriptive `snake_case` test names crossing the
  entropy threshold purely from underscore-joined lexical diversity —
  see ADR-0017's own section on it.
- **MCP server slice** (2026-08-03, ADR-0018) — normally M4 scope
  (`carto-mcp`, spec §7.3), pulled ahead of M2's remaining work and M3
  on user request: spec §10/§11.4 gate M3 on running the S-1 benchmark
  ("agent-with-carto answers structural questions with ≥30% fewer input
  tokens and ≥20% higher accuracy than agent-with-grep-only") before
  starting it, and that benchmark needs a real agent-facing surface, not
  the README's old CLAUDE.md-snippet-and-Bash-tool stopgap, to measure
  honestly. `carto serve` exposes `index`/`where`/`deps`/`map`/
  `selfcheck` — exactly M1's tool set, nothing from M2+ — over a
  hand-rolled JSON-RPC 2.0 stdio transport (not `rmcp`; ADR-0018 has the
  dependency-tree evidence). Every tool handler calls the same
  `carto_core::query::{find,deps,map}` / `carto_core::indexer::
  build_and_persist` function its CLI counterpart calls — no query logic
  duplicated. First real enforcement of `consts::MCP_TEXT_CAP` anywhere
  in the codebase. `skill/carto.skill.md` (spec §9.3) written against
  what's actually implemented today, not aspirationally against M4's
  full scope (no semantic-layer/`plan --semantic` section, since that
  isn't built). Verified against a real `claude --mcp-config` client
  session, not just this crate's own unit tests — see
  `bench/field-log.md`.
- **S-1 benchmark, run** (2026-08-03, ADR-0019) — 8 tasks (`bench/
  tasks.md`) over carto's own repo and `~/playground/zed`, real and
  deterministic-replay measurement arms (`bench/run.sh`, `bench/
  replay/`), scored and graded (`bench/score.py`, `bench/results/
  20260803T112447/`). **Result: neither S-1 threshold is met** on this
  one-trial measurement — combined input tokens 18.0% *higher* for
  carto (threshold: ≥30% lower), accuracy 6.2 points *lower*
  (threshold: ≥20 points higher). Two real harness bugs were found and
  fixed before this result could be trusted (a neutral-cwd setup that
  silently invalidated 5 of 8 tasks on the first attempt, and one
  isolated MCP-connection flake) — see ADR-0019 and `bench/field-log.md`
  for the full accounting, including real dollar cost of the mistakes.
  The per-task pattern is more informative than the aggregate: 3 of 8
  tasks are clean carto wins on tokens (one, whole-repo orientation, by
  74%); the sole accuracy loss traces to the agent misreading carto's
  own verified-correct tool output, not bad data; and the three hardest
  transitive-call tasks are all correct for carto specifically because
  the agent recognized carto's documented ADR-0008 limitation in the
  moment and fell back to grep rather than trusting an incomplete
  answer. **Per spec §11.4, this ADR does not decide whether M3
  proceeds — that's the user's call, presented and pending.**
- **Post-S-1 improvement plan, slices 1–4** (2026-08-03, ADRs 0020–0022;
  `docs/post-s1-improvement-plan.md`) — the four highest-confidence,
  most directly-evidenced fixes the S-1 result pointed at, landed ahead
  of the user's M3 go/no-go decision (which these slices don't
  determine, only prepare a better-measured foundation for):
  - **§1.1 honest absence** (ADR-0020): `SymbolNode.uncaptured_inbound_calls`
    counts (never resolves) call sites a language's extractor
    deliberately never attempts — today only Rust's path-qualified
    calls. Surfaced on `deps` as `root_uncaptured_inbound_calls`,
    rendered as a text note only on `--dir in`/`both`. `SCHEMA_VERSION`
    bumped 1 → 2 so a stale v1 `graph.json` is rejected with "re-run
    `carto index`" rather than silently reporting a fabricated `0`.
  - **§2.1 `map --section`** (`counts`/`modules`/`entry-points`/`infra`,
    repeatable): lets a caller render only the sections a question
    needs instead of paying for the full overview every time; `None`
    (the default) is unchanged full-overview behavior. The structured
    `counts` field is always exact regardless of this filter.
  - **§1.2/§1.4** (ADR-0021): `where`/`find` now also match `Module.path`
    (a separate `module_matches` list); `deps`'s target resolution now
    also matches `Module.path`/`File.path` exactly (previously
    symbol-name-only) — closing L3's "a package/file's blake3-hashed ID
    has no other reachable lookup path" gap. `deps`'s `DepEdge` also
    gained `same_file_as_root`, flagging when a caller and its callee
    share a file (T5's misread: a same-file caller read as "the
    definition itself").
  - **§1.3 Rust grouped-`use` extraction** (ADR-0022, amends ADR-0008):
    `use a::{b, c};` (including nested groups and a `self` member) is
    now extracted — previously **zero** import edges, not reduced
    precision, for any grouped `use`. `use_wildcard`/`use_as_clause`
    remain excluded, per the plan's scope. Checked (not deferred)
    whether Python/TS-JS/PHP/C# share this gap — they don't; it was
    Rust-specific.
  - **§3.1/§3.2/§2.3 skill-file wording** (no code): `skill/
    carto.skill.md` names these specific known gaps as actions rather
    than describing them generically, and frames carto+grep as
    complementary by default for transitive/blast-radius questions.
  - Every fixture golden regenerated where output changed
    (`fixtures/mixed.graph.golden.json`,
    `fixtures/rust-crate.{where,deps}.golden.json`; `map`'s golden is
    unaffected). `bash scripts/gates.sh` green at each of the six
    commits landing this work. **Not yet done: re-running S-1
    (the plan's step 5) against these changes** — that's the
    measurement round the user approves separately, per spec §11.4.
- **M2+ remaining** — infra graph, join, ingest. Per spec §10. **Paused
  at the S-1 gate, pending the user's M3 go/no-go decision above** —
  not proceeding automatically.

Post-M1.b.2b hardening (2026-08-01): real-world PHP field-testing
against a TYPO3 codebase surfaced two query-layer gaps, both fixed —
`deps` now surfaces the root symbol's `unresolved_calls` (a large
static-utility-class root returning zero outbound edges was previously
indistinguishable from "calls nothing"), and `where`/`deps`/`map` gained
`--subpath <dir>` scoping (the positional `PATH` argument never scoped
an already-built `--out` index; there was no subtree-scoping mechanism
at all). See [ADR-0014](adr/0014-subpath-scoping.md).

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
  **Since 2026-08-03 (ADR-0020), this exclusion is no longer silent**:
  every such call site is counted (not resolved) into
  `SymbolNode::uncaptured_inbound_calls`, surfaced on `deps --dir in`
  as `root_uncaptured_inbound_calls` — a nonzero count is a direct
  signal that a small/empty `--dir in` answer may be incomplete, not
  evidence the symbol has few callers.
- **`use_wildcard` / `use_as_clause`** Rust import shapes — extracted
  as nothing rather than partially interpreted (an aliased member
  *inside* a group is skipped individually, not the whole statement).
  Same ADR. Python's `from x import *` is excluded the same way
  (ADR-0011). **`use_list` (grouped imports, `use a::{b, c};`,
  including nested groups and a `self` member) is no longer in this
  list** — extracted since 2026-08-03
  ([ADR-0022](adr/0022-rust-grouped-use-extraction.md)), which amends
  ADR-0008's original blanket exclusion of all three shapes. The
  consequence of the old exclusion was silent, not just reduced,
  under-reporting on any Rust fan-in/fan-out question whose only
  relevant import happened to be grouped — see ADR-0022's Context.
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
- **`where` matches `Symbol.name` and, since 2026-08-03
  ([ADR-0021](adr/0021-module-file-discovery-and-same-file-caller-flag.md)),
  `Module.path` too** (a separate `module_matches` list, sharing a
  combined limit/truncation with symbol matches). `File` nodes still
  aren't searched by `where` — spec §7.1 names "symbol name"
  specifically, and a file is a traversal starting point more than a
  name-lookup target; `deps`'s own target resolution (same ADR) is
  where a `File.path` becomes reachable instead, alongside a
  `Module.path` — previously, a `Module`/`File` node's blake3-hashed ID
  had no other reachable lookup path at all, making it unreachable from
  the tool surface entirely (the motivating S-1/L3 finding).
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
- ~~`MCP_TEXT_CAP`'s 8 KiB byte cap isn't enforced anywhere yet~~ —
  **enforced** as of the MCP server slice below (`carto-mcp/src/
  render.rs::cap_text`, on a line boundary, with `structuredContent`
  staying uncapped). Each CLI command's own `--limit`/`--depth`/`--budget`
  remains the *only* bound on CLI output — the MCP cap is a second,
  independent dimension that only applies to `carto serve`'s text
  responses.
- **Redaction only scans `TaintedString` fields, not literally every
  `String` field** (spec §7.5's wording) — `FileNode.path`,
  `ModuleNode.path`, `SymbolNode.name`, and `UnresolvedCall.name` are
  all extractor-computed identifiers/paths, never free-form captured
  source text, so pattern/entropy-scanning them would be pure noise.
  ADR-0017.
- **`connection_string` redaction only covers the credential-bearing
  prefix**, not the trailing path (`postgres://user:pass@host/db` →
  `postgres://«redacted:...»/db`) — the path carries no credential.
  ADR-0017.
- **Query-layer `--json` output never fences tainted text; human output
  fences once per section, not once per row** —
  [ADR-0010](adr/0010-tainted-text-in-query-output.md).
- **PHP `require`/`include` are not extracted** — the argument is an
  arbitrary runtime expression in the general case; the FQN index
  already covers real (autoloaded) dependency structure — ADR-0012.
- **PHP anonymous classes' methods are never extracted** — anonymous
  classes have no `name:` field, so they never match `symbols.scm`'s
  four class-like-container patterns. ADR-0012.
- ~~An aliased `use ... as X` import doesn't make a call written as
  `X(...)` resolve via tier (b)~~ — **fixed** (ADR-0013): `RawImport`'s
  `imported_names` now carries `ImportedName { bound_name,
  declared_name }` pairs instead of a single flat name, and
  `resolve.rs`'s tier (b) looks a call site's identifier up in a
  per-file `alias_to_declared` map before consulting `pub_by_name` —
  alias-aware for Rust, Python, PHP, and TS/JS alike.
- **A colliding FQN (two PHP files illegally declaring the same
  namespace+name) keeps whichever file `resolve` encounters first**,
  not an error — carto only reads source, it doesn't enforce PHP's own
  uniqueness rules. ADR-0012.
- **CommonJS (`require()`/`module.exports`) is not extracted** — ESM
  `import`/`export` only. Same "extract nothing rather than partially
  interpret" precedent as every other exclusion here — ADR-0013.
- **TS/JS default and namespace imports (`import Foo from './x'`,
  `import * as ns from './x'`) never feed tier (b)** — the local name
  is the importer's own choice, not a name the target file declares
  under that spelling, so there's nothing verifiable to check against.
  Re-exports (`export { foo } from './x'`) don't either, for a
  different reason: they never create a binding usable by a call site
  in the *declaring* file at all. ADR-0013.
- **Default/namespace-import and re-export `imports` edges get
  `"mod-declaration"` evidence**, the same label Rust's bare `mod
  foo;` gets — a naming artifact of `resolve.rs` picking the evidence
  string by whether `imported_names` is empty, not a discriminant
  specific to what actually happened. Functionally correct (`certain`,
  right target file) in every case; just reads oddly for TS/JS. Not
  fixed — ADR-0013.
- **No `package.json`/`tsconfig.json`/`node_modules` parsing** — every
  TS/JS bare package specifier is classified external, the same
  simplification Python's and PHP's absolute imports already made.
  Relative-import extension-guessing (`.ts`/`.tsx`/`.js`/`.jsx`/
  `/index.*`) is a fixed priority order, not real bundler/tsconfig
  resolution. ADR-0013.
- **`deps` surfaces `unresolved_calls` on the root symbol only, not
  per-hop** — directly answers "why is `--dir out` empty" without
  bloating every row of a possibly-large traversal. Broaden to
  per-node only if a concrete need shows up. ADR-0014 (recorded
  alongside `--subpath` since both came from the same round of
  real-world feedback).
- **`--subpath` restricts which rows get listed, never what a
  command's underlying computation is based on** — a file's fan-in/out
  in `map`'s ranking is always its real, whole-repo connectivity, not
  clipped at the subtree boundary; `deps`'s BFS traversal itself is
  never scoped, only which discovered nodes get reported. The one
  deliberate exception: `map`'s external-package fan-in *is* scoped
  (counts only edges from in-scope files) — the opposite rule from the
  file ranking, on purpose, not an inconsistency. ADR-0014.
- **`Module` nodes are never excluded by `--subpath`** — packages have
  no directory, so path-prefix filtering doesn't apply to them
  directly; they only disappear from a scoped `map` ranking by having
  zero edges with an in-scope endpoint. ADR-0014.
- **Go's cross-package calls always resolve via tier (c)
  (`"same-package"`), never tier (b) (`"imported"`), even when reached
  through a real `import`** — a Go import binds a *package* name, not
  a symbol name, so there's nothing for `RawImport::PackagePath` to
  offer tier (b): `resolve.rs`'s `alias_to_declared` map is never
  populated for Go files. The same class of cosmetic evidence-label
  quirk ADR-0013 already accepted for TS/JS's `"mod-declaration"`
  label. ADR-0015.
- **No `go.mod` parsing.** `RawImport::PackagePath` resolves a Go
  import path against every walked directory containing a Go file by
  **longest-suffix match**, not an exact prefix computed from
  `go.mod`'s `module` line — same "no manifest parsing" precedent as
  Rust's/Python's/PHP's/TS-JS's own absolute-import simplifications.
  Accepted false-positive: an external import whose path coincidentally
  ends with a local directory's path resolves as internal. `go.mod`
  is still committed in `fixtures/go-svc` for realism and indexed as a
  plain `File` node. ADR-0015; revisit only if this causes a real false
  positive in practice.
- **The package qualifier in a Go selector call (`pkg.Func()`) is
  discarded** — `RawCallSite` carries only the bare callee name
  (`Func`), the same grammar-level ambiguity Python's/TS-JS's
  attribute/member calls already have, not a Go-specific gap. ADR-0015.
- **C# constructors are never extracted as symbols** — a constructor
  shares its type's name, so a constructor symbol would make every
  `new Foo()` ambiguous between type and constructor and INV-8 would
  drop the very construction edges capturing `new` exists to produce.
  `new Foo()` resolves to the *type*; constructor-body calls attribute
  to the enclosing class (innermost containment). Not the same
  situation as PHP's `__construct`/Python's `__init__`, whose distinct
  names make them safe to extract. ADR-0016.
- **C# properties, events, indexers, operators, non-const fields, and
  local functions are not extracted**; `using static`'s member-binding
  and `global using`'s repo-wide scope are not modeled; partial
  classes' cross-file `private` visibility lands in
  `unresolved_calls`. ADR-0016.
- **C#'s cross-namespace calls always resolve via tier (c)
  (`"same-package"`), never tier (b), even through a real `using`** —
  a namespace `using` binds no symbol name (Go's exact shape, ADR-0015);
  only the *alias* form (`using P = X.Y.Z;`) feeds tier (b). And every
  C# `using` of a namespace no walked file declares becomes an external
  `Module` keyed by the full namespace string — no `.csproj` parsing,
  same "no manifest parsing" precedent as every other language.
  ADR-0016.

## Next: the user's M3 go/no-go decision, then M2's remaining scope or M3

M1.b.2b is done — spec §5.2's full v1 language set (TypeScript, TSX,
JavaScript, Python, Rust, Go, PHP) is implemented, plus C# post-v1
(ADR-0016). `carto index` extracts symbols/imports/calls for all eight
`Lang` variants, and `where`/`deps`/`map` work against every one of
them with zero language-specific code in
`crates/carto-core/src/query/` — reconfirmed for C# (ADR-0016), the
eighth data point for that claim.

Eight languages across seven ADRs (Rust/ADR-0008, Python/ADR-0011,
PHP/ADR-0012, TS-TSX-JS/ADR-0013, Go/ADR-0015, C#/ADR-0016) are enough
data points to say what varies per language with some confidence:
relative-import vs. FQN-based vs. literal-path vs. directory-path vs.
namespace-fan-out import semantics, what "exported" means (a keyword,
a convention, a hard first-letter-case rule — or C#'s
`internal`-is-repo-visible judgment call), whether path-qualified
calls are even syntactically distinguishable from plain ones (and,
PHP/TS/Go/C# all show, distinguishable ≠ excluded — an independent
call each language gets to make), and what "same-package" should mean
(same walked repo for Rust/Python/TS-JS/C#, same FQN-namespace for
PHP, same *directory* for Go — the one language so far where package
scope needed a genuinely new resolution tier, not just a new
`RawImport` variant).

Real redaction (ADR-0017) is M2's first slice, done. An MCP server slice
(ADR-0018, `carto serve`) — normally M4 scope — was pulled ahead on user
request so spec §11.4's S-1 benchmark could be run against carto's real
agent-facing surface (an MCP server an agent actually calls as a tool)
rather than the README's old CLAUDE.md-snippet-and-Bash-tool stopgap.
**The benchmark has now been built and run** (ADR-0019, `bench/`) — 8
tasks, both a real-run and a deterministic-replay measurement arm,
every session's answer graded against source-derived ground truth.
**Result: neither S-1 threshold is met** on this one-trial measurement
(combined input tokens 18.0% higher for carto, not ≥30% lower; accuracy
6.2 points lower, not ≥20 higher) — though the per-task pattern
underneath is more nuanced than the headline (see ADR-0019's full
writeup: 3 clean carto wins, one accuracy loss traceable to a specific
misread rather than bad data, and a genuinely encouraging pattern where
the agent correctly fell back to grep on every task where it recognized
carto's known ADR-0008 limitation). Spec §11.4 requires presenting this
to the user and waiting for the M3 go/no-go decision, not deciding it
here — that decision is **pending**. Until it lands, M2's remaining
scope (the infrastructure graph, Terraform/CFN/CDK, and the code↔infra
join — spec §6, not sliced yet the way M1.b was) and M3 are both paused,
not proceeding by default.

**Since then, `docs/post-s1-improvement-plan.md`'s slices 1–4 have been
implemented** (ADRs 0020–0022; the milestone entry above has the full
list) — the concrete, evidenced fixes the S-1 result itself pointed at:
an honest absence signal for calls a language's extractor never
attempts, `map --section`, `Module`/`File` discovery in `where`/`deps`
plus a same-file caller flag, and Rust grouped-`use` extraction. This
does **not** decide the M3 go/no-go question either — spec §11.4 still
requires the user's decision, and the plan's own step 5 (re-running
`bench/run.sh` as a full three-arm batch against these changes, then
comparing against `bench/results/20260803T112447/`) is a separate,
not-yet-run measurement round the user approves independently before
that decision is made.

Also worth knowing before touching `resolve.rs` again: building
`fixtures/php-app` surfaced a real, language-agnostic bug in call
attribution (a symbol whose range nests another symbol's — PHP/Python
classes containing methods — used to get every nested call's edges/
`unresolved_calls` double-counted onto itself too). Fixed by
`assign_calls_to_innermost_symbol`, verified behavior-preserving for
Rust/Python (full pre-existing suite green, both golden files
untouched) — see ADR-0012's "shared bug" section for the detail. And
building `fixtures/ts-app` caught two issues that turned out to be
fixture-design mistakes, not extractor bugs (an aliased import of a
non-exported symbol — invalid TS/JS — and a function named `log`
coincidentally colliding with `console.log`) — see ADR-0013's own
"what building the fixture caught" section. Both were only found by
eyeballing real `carto index` output, never by unit tests on isolated
snippets — keep doing that for every new milestone, per CLAUDE.md's own
documented gotcha. `fixtures/go-svc` (ADR-0015) broke that streak in a
good way: the fixture matched real output on the first run, with
nothing to fix — recorded in ADR-0015 rather than left unmentioned, so
"nothing was wrong" reads as verified, not skipped.
`fixtures/csharp-app` (ADR-0016) added a third variation: its one real
defect (constructor symbols making every `new Foo()` ambiguous) was
caught at fixture-*design* time — tracing which resolution outcome
each planned call site must produce, before the first run — and the
first real run then matched the corrected design exactly.

# carto — status / handoff

**Milestone:** M1.b.2b done (Rust, Python, PHP, TS/TSX/JS, Go — spec §5.2's full v1 language set), plus C# added post-v1 on user request (ADR-0016). M2's first slice — real redaction (ADR-0017) — is also done. An MCP server slice (normally M4 scope) was pulled ahead on user request, to measure spec §11.4's S-1 benchmark before further capability work (ADR-0018). **The S-1 benchmark has now been run (ADR-0019): neither threshold is met on a one-trial measurement** (combined input tokens: carto uses 18.0% more, not ≥30% fewer; accuracy: carto is 6.2 points lower, not ≥20 higher) — full per-task result in [`bench/`](../bench/) and ADR-0019. **The post-S-1 improvement plan's slices 1–4 are now implemented** (`docs/post-s1-improvement-plan.md`, ADRs 0020–0022: the honest-absence signal, `map --section`, `Module`/`File` discovery plus the same-file caller flag, and Rust grouped-`use` extraction) — see the milestone entry below. **M3 still does not proceed automatically — spec §11.4 requires the user's decision, presented and pending; re-running S-1 against this slice (plan step 5) was a separate step, now done — see the end of this paragraph.** **Since then, a genuinely new capability — cross-language string-literal contracts (ADRs 0025–0027) — was pulled ahead on user request, independent of the M3 gate**, the same "pulled ahead on request" precedent MCP/C# already set; see the milestone entry below. **Most recently, real C# field feedback (`bench/field-log.md`) drove a fix (ADRs 0029–0030): type positions (fields, parameters, base clauses, generic arguments) were never captured by any extractor, so `deps --dir in` on a type/interface could only ever answer from the coarser `imports` edge — a new `EdgeKind::References` producer, across all six typed languages, closes that gap. A same-day retest then caught a real follow-on miss — top-level-statement code (modern ASP.NET Core `Program.cs`) has no enclosing symbol, so calls/type-refs inside it were silently dropped — fixed by ADR-0031's file-scope fallback; see the milestone entry below.** **Next (ADRs 0032–0033), a direct follow-up narrowed same-name ambiguity using the same `type_refs` channel (owner-type disambiguation) and added a third honesty signal, `unresolved_inbound_calls`, for call sites that were attempted and still produced no edge — see the milestone entry below.** **Most recently, another genuinely new capability — multi-root/component-aware indexing (ADRs 0034–0035) — was pulled ahead on user request, independent of the M3 gate: real monorepos put several projects (microservices, frontends, lambdas, infra) under one root, which carto's original single-scope resolution policy handled poorly (silently dropping real intra-component edges, producing false cross-component edges at up to `Certain` confidence); `resolve.rs` now recognizes component boundaries and prefers same-component matches before falling back to repo-wide behavior, and `--component` reaches every query command — see the milestone entry below.** **A follow-up review of that capability (2026-09-21, ADR-0036) found the `Certain` `Imports` edge — the kind ADR-0034 itself calls the most damaging when a component crossing is wrong — carried no crossing marker at all; that's fixed, along with `index`-time visibility for the single-manifest-monorepo detection gap. The same review's detection-quality slice (ADR-0037) is also now done: the terraform marker's plain "≥1 HCL file" rule fragmented a normal `infra/modules/*`/`infra/envs/*` tree into one component per subdirectory — a rollup pass now collapses that to one; `is_aggregator` now really parses `Cargo.toml`/`package.json` instead of substring-matching, and recognizes pnpm/lerna/turborepo/nx/rush workspace roots; `.carto/roots.json` gains `exclude`. `--component` matched a node's own component by exact name only, missing a real, already-possible case: one component nested inside another's directory (a second manifest marker inside the first's own tree, `components::tests::innermost_component_wins_for_nested_markers`) — `--component api` silently excluded `internal`'s own files. Fixed (ADR-0038): `--component <name>` now scopes to `name` and everything nested under it, one-directional (never the reverse). **The fourth and final follow-up slice (2026-09-21, ADR-0039), requested directly by the user ("dependencies between components shall be understood"), is also now done: `Component` gains `depends_on`, resolved from each component's own manifest (`go.mod`/`package.json`/`Cargo.toml`/`.csproj`/`composer.json`, no new dependency) against every other component in the tree — `SCHEMA_VERSION` bumped 7 → 8. A cross-component edge whose target isn't in the caller's declared dependencies now carries an `"undeclared-dependency"` evidence entry (never for a `terraform`/`custom`-kind caller, which has no manifest-identity concept to have violated); two resolution tiers (PHP/C# FQN, C# namespace fan-out) prefer a declared dependency over the pre-existing arbitrary fallback; `map --section components` renders each component's `depends_on` and marks an undeclared crossing `[undeclared]`. This closes the ADR-0034/0035 review's full four-slice follow-up (ADR-0036/0037/0038/0039) — see the milestone entry below.** **Most recently (2026-09-22, ADR-0040), the post-S-1 improvement plan's own "step 5" finally ran: S-1 re-measured against current capabilities, ground truth for all 8 tasks re-verified first (not assumed — one task's recorded answer was provably wrong, its target function having been directly modified in the intervening six weeks). Neither S-1 threshold is met, same headline as ADR-0019, but the shape changed substantially: accuracy is a full reversal (carto 87.5%→100%, +12.5 points over grep, was −6.2) — the specific tasks the post-S-1 work targeted (honest-absence signals, grouped-`use` extraction, Module/File discovery) are the ones that flipped. Token cost barely moved (18.0%→15.8% more than grep) — correctness is no longer S-1's binding constraint, token footprint is. Full result, re-verified ground truth, and grading rationale in `bench/results/20260922T173830/` and ADR-0040. **M3's go/no-go call remains the user's per spec §11.4 — presented on current data now, still not decided here.**

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
    **Extended 2026-08-03 (ADR-0023) with the outbound half**,
    `uncaptured_outbound_calls`/`root_uncaptured_outbound_calls` — a
    symbol's own uncaptured calls (by line-range containment, reusing
    `assign_calls_to_innermost_symbol`) rather than uncaptured calls
    elsewhere that name it (by bare name, repo-wide). Rendered only on
    `--dir out`/`both`. `SCHEMA_VERSION` bumped again, 2 → 3. Motivated
    by T8 grading `partial` in a real batch: `deps build_and_persist
    --dir out` gave no signal that 6 real path-qualified calls (the
    entire substance of what the function does) were invisible.
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
  - **Bare fully-qualified-path import detection, Rust** (ADR-0024,
    2026-08-03, amends ADR-0008): `carto_core::Result<u8>` needs no
    `use carto_core;` at all (valid since Rust 2018) — previously
    structurally invisible to the import graph, since `imports.scm`
    only captured `use_declaration`/`mod_item` nodes. New
    `RawImport::BareReference` captures this shape, gated by two
    filters (excludes a `call_expression`'s own callee position;
    root must start lowercase, the snake_case-crate/PascalCase-type
    convention) to avoid fabricating an external `Module` node for
    every local `Type::method()` call site. Honestly labeled
    heuristic: distinct evidence string
    `"external-package-bare-reference"`, not `"external-package"`.
    Motivated by L3 grading `partial` twice: `deps carto_core --dir
    in` was 18/19, permanently missing `crates/carto-cli/src/main.rs`
    (whose only references to `carto_core` are a bare path in an
    attribute argument and a bare-path return type). Now
    deterministically 19/19.
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
- **Cross-language string-literal contracts, slice 1** (2026-08-04,
  ADRs 0025–0027) — a capability the design spec doesn't name at all,
  scoped against a real large monorepo (services in C#/TS/Go/Rust,
  infra in Terraform) whose costliest dependency edges are
  string-keyed contracts no compiler checks: a CloudWatch alarm's
  `metric_name` referencing a name no service emits, an env var whose
  Terraform-set and C#-read spellings differ by compute-type
  convention, a DynamoDB attribute name duplicated as literals across
  services with no shared type. Pulled ahead on user request,
  independent of the M3 go/no-go gate above — the same "pulled ahead"
  precedent MCP (ADR-0018) and C# (ADR-0016) already set.
  - **New `Contract` node kind, `produces`/`consumes` edges** —
    ADR-0026, a spec §4.1/§4.2 amendment. `value`/`qualifier` are
    `TaintedString` (a string literal is captured source text, INV-6
    must scan it); node IDs are built from the raw pre-redaction value
    so a `produces`/`consumes` join survives redaction. `qualifier` is
    part of node identity (not display-only) — guards exactly the
    "two same-spelled values in different scopes must not
    cross-match" failure mode the motivating repo's own env-var
    naming-convention difference warns about.
  - **New `HclExtractor`** (ADR-0025) — `Lang::Hcl` existed in the
    enum with no extractor registered; filled in by parsing `.tf`
    source directly (spec §6.2 assumes `terraform show -json`, which
    this slice's own use cases — unused locals, undefined `local.X` —
    can't be answered from a resolved plan). New dependency
    `tree-sitter-hcl = "1.1.0"` (verified via the crates.io sparse
    index: no second `tree-sitter` version, `deny.toml`'s
    `multiple-versions = "deny"` needs no `[[bans.skip]]`). Also fixes
    a real, general `walk` gap: `Path::extension()` alone never sees a
    two-part extension (`locals.tf.simu`), which a real Terraform
    per-environment-file pattern in the motivating repo relies on.
  - **`.carto/contracts.json`** (ADR-0027) — built-in classification
    rules (position → category/role/confidence) plus an optional
    repo-local override file extending the category vocabulary. `role`/
    `lang` are validated against a closed set (hard error on an unknown
    spelling); `category` is intentionally open.
  - **Two new commands, `carto contract <value>` / `carto orphans`**
    (neither in spec §7.1) — `orphans --category metric_name` is the
    literal acceptance-test command: against `fixtures/sid-like` (new,
    synthetic) it returns exactly the fixture's one dead alarm and one
    unwatched emission, the fixture-scale reproduction of the
    motivating repo's real finding. Both got MCP tools
    (`contract_tool.rs`/`orphans_tool.rs`) calling the identical core
    function, per spec §7.1's "commands = MCP tools, same core
    functions" even though these two commands are themselves outside
    §7.1's named list.
  - `SCHEMA_VERSION` bumped 3 → 4 — a v3 `graph.json` has no `Contract`
    nodes; `orphans`/`contract` against one is rejected with "re-run
    `carto index`," same reasoning ADR-0020/0023 used for their bumps.
  - **Scoped to one category (`metric_name`) end-to-end**, deliberately
    — the motivating repo's spec ranks nine more (env vars, DynamoDB
    attributes, Kafka topics, WS wire-protocol fields, Parquet/Glue
    columns, subtype IDs, doc-mention edges, Terraform reachability,
    the `.csproj`/`implements` graph), none built this slice; the
    node/edge vocabulary is designed to need no new node or edge kind
    for any of them, only new `ContractRule`s and per-language literal
    capture. See ADR-0026's own "deliberately absent" list below for
    what's cut even within the one category built.
  - `bash scripts/gates.sh` green (fmt, clippy -D warnings, `cargo deny
    check bans licenses sources`, full workspace test suite).
- **Skill file made discoverable and current** (2026-08-04, ADR-0028) —
  moving from the S-1 benchmark harness (which pastes the skill file's
  text directly into a system prompt, `bench/run.sh:145`) toward real-
  world use surfaced that `skill/carto.skill.md`'s flat-file shape,
  named literally by spec §9.3, is never actually discovered by Claude
  Code's own skill loading (`<skills-dir>/<name>/SKILL.md`, a directory
  per skill) — confirmed empirically against a pre-existing, unrelated
  flat skill file on the test machine that was silently absent from a
  live session's available-skills list. Repackaged as `skill/carto/
  SKILL.md` (`git mv`, ADR-0028 records the spec deviation and why the
  spec's underlying intent is still met). Separately, the file's
  *content* had also drifted: it documented five tools while
  `schema.rs::tool_list()` had advertised seven since the contract slice
  above (`contract`/`orphans` landed with no matching skill-file
  update) — rewritten to cover both, plus `subpath`/`sections`/
  `module_matches`/`same_file_as_root` and the contract-specific known
  gaps (interpolated/computed literals aren't extracted; `qualifier` is
  part of contract identity). Added a new non-imperative section asking
  the agent to surface a carto answer that looks wrong to the user
  rather than silently routing around it — carto is still under active
  development and a surprising result is data, not just an obstacle.
  New mechanical guard against the content drift recurring:
  `crates/carto-mcp/src/schema.rs::skill_file_names_every_advertised_tool`
  `include_str!`s the skill file and asserts every advertised tool name
  appears in it, the same drift-prevention idea `tools/mod.rs`'s
  existing schema/handler test already uses. `target/release/carto`
  rebuilt (the pre-existing binary predated `SCHEMA_VERSION` 4 and the
  contract/orphans commands entirely).
- **Type references as `references` edges** (2026-08-05, ADRs
  0029–0030) — field feedback from a real C# repo (`bench/field-log.md`):
  `deps IQueryJobStore --dir in` was worse than `grep -rl` for "who
  uses this interface" — ~41 files at depth 2, ~26 false positives,
  when only 15 files actually used it. The reported diagnosis
  (`imports`'s file→namespace granularity) was accurate as a
  description but not the root cause: **no extractor captured a type
  position at all** — only invocations and `object_creation_expression`
  — so a field/parameter/base-clause reference produced zero edges of
  any kind, forcing every `--dir in` query through the file node's
  coarse `imports` fan-in. `EdgeKind::References` (declared in the
  vocabulary since the start, never produced) now has a first
  producer: a new `ExtractOut::type_refs` channel, populated by all six
  typed extractors (Rust/Go/TS+TSX+JS/PHP/Python/C#), resolved through
  the exact same tier ladder `calls` edges already use
  (`resolve_call` generalized to take a bare name). `deps <type> --dir
  in --depth 1 --kinds references` now gives the precise answer the
  field report needed; `imports`'s own fan-out is unchanged and still
  available for "what genuinely imports this module" — see ADR-0029
  for why weakening it would have been the wrong fix. `SCHEMA_VERSION`
  bumped 4 → 5. `fixtures/csharp-app/Ports/`+`Services/` reproduces the
  reported bug directly as this feature's acceptance test, at both the
  extractor-unit and CLI/JSON levels. ADR-0030 is the six-language
  capture-position survey (which grammar shapes feed `type_refs`, three
  wrong assumptions caught empirically along the way — Rust's turbofish
  is two different parse shapes, TS/JS's `class_heritage` needed a
  third query file, Python's subscripted generics parse to
  `generic_type` in annotation position, not `subscript`). Skill doc
  gained the `kinds="references"` recipe and a new known-gap entry
  (unresolved type refs are silently dropped, unlike unresolved calls —
  user-confirmed: overwhelmingly stdlib/BCL noise, not a signal worth a
  counter).
  - **Follow-up fix, same day (ADR-0031)**: retest feedback on the slice
    above found one real miss — `Program.cs`'s own
    `services.AddSingleton<IQueryJobStore, X>()` DI registration wasn't
    found, exactly the shape the feature's own comments call out as
    motivating and, per the reporter, the dominant modern ASP.NET Core
    style (`Program.cs` with no `Main` method — C# 9's top-level
    statements). Root cause: `resolve.rs`'s `assign_to_innermost_symbol`
    silently dropped any call/type-ref whose line fell outside every
    symbol's range — top-level statements have no enclosing method/class
    for `symbols.scm` to capture. Not a new bug this feature introduced,
    a pre-existing, language-agnostic gap (would equally affect TS/JS's
    module-level setup calls, Python's module-level pattern) this
    feature made newly consequential. Fixed by reusing the file-level
    fallback the contract-literal pass already had (ADR-0026): an
    unattached call/type-ref now resolves through the same tier ladder
    and gets a `Calls`/`References` edge from the **File** node instead
    of being dropped — resolved case only, user-confirmed scope; an
    unattached miss still stays invisible (no `FileNode.unresolved_calls`
    equivalent exists, a separate decision left for later). No
    `SCHEMA_VERSION` bump (`Edge`'s shape is unchanged — `imports`
    already proves File is a valid endpoint). New fixture,
    `TopLevelRegistration.cs`, reproduces the exact retest scenario.
- **Owner-type disambiguation and a third inbound honesty signal**
  (2026-08-05, ADRs 0032–0033) — a direct follow-up to the
  references-edges slice above, building `fixtures/csharp-app`'s own
  interface/implementation pair (`Ports/IQueryJobStore.cs`'s `Save`,
  `Services/InMemoryQueryJobStore.cs`'s own `Save`) surfaced the
  next-door case spec §5.3 already flags as a known trade-off: a
  bare-name call through an interface-typed field is genuinely
  ambiguous by name alone, but the calling file's own `type_refs`
  (ADR-0029) already states which candidate it means.
  - **ADR-0032**: `CallResolver` gains an owner-type narrowing step,
    applied only when same-file/imported/same-package would otherwise
    be ambiguous — never changes which candidate an already-
    unambiguous tier picks, can only turn a `None` into a `Some`.
    `RawSymbol` gains `owner: Option<String>` (a method's enclosing/
    receiver type; `None` for anything else); a candidate survives only
    if its owner appears among the type names the calling file's own
    `type_refs` names. A real negative case is committed alongside the
    positive one (`TopLevelRegistration.cs`'s `RegisterQueryJobStore`,
    whose parameters name *both* candidates' owners) proving the
    mechanism doesn't overreach into "first type mentioned nearby
    wins." No `SCHEMA_VERSION` bump — confined to which edges
    `resolve()` produces, not the on-disk shape.
  - **ADR-0033**: a third honesty signal, `unresolved_inbound_calls`/
    `unresolved_inbound_call_count`, distinct from both
    `uncaptured_inbound_calls` (ADR-0020, syntax never attempted) and
    `uncaptured_outbound_calls` (ADR-0023, same but outbound) — this
    one counts call sites elsewhere that spell a symbol's bare name,
    *were* attempted through the full tier ladder including ADR-0032's
    new step, and still produced no edge (most often bare-name
    ambiguity). Capped at `UNRESOLVED_INBOUND_SITES_CAP` (25) with an
    uncapped count alongside, surfaced on `deps --dir in`/`both` as
    `root_unresolved_inbound_calls`. `SCHEMA_VERSION` bumped 5 → 6.
    `RegisterQueryJobStore` doubles as this ADR's acceptance case too:
    both `Save` symbols must record the ambiguous call site honestly
    rather than reporting a false all-clear.
  - Every golden file touched by the schema bump regenerated
    (`fixtures/mixed.graph.golden.json`,
    `fixtures/rust-crate.{deps,map}.golden.json`); `bash scripts/
    gates.sh` green.
- **Multi-root support (component-aware indexing)** (2026-09-21, ADRs
  0034–0035) — another capability outside spec §4/§7's original scope,
  pulled ahead on user request, the same "pulled ahead" precedent MCP
  (ADR-0018), C# (ADR-0016), and the contracts slice (ADR-0025–0027)
  already set. Real repos put several projects under one root
  (microservices, frontends, lambdas, infra) — carto's original
  "same-package = same walked repo" resolution policy (spec §5.3 rule
  2c) treated the whole tree as one scope, silently dropping real
  intra-component edges (two services each defining `Handler` made
  either one unresolvable) and producing false cross-component edges
  at up to `Certain` confidence. **One tree, N components, not N
  filesystem roots.**
  - **ADR-0034**: new `crates/carto-core/src/components/` module —
    auto-detects project roots inside the walked tree from manifest
    markers (`go.mod`/`package.json`/`Cargo.toml`/`pyproject.toml`/
    `setup.py`/`composer.json`/`*.csproj`/`*.fsproj`, plus ≥1 HCL file
    as the weakest signal for terraform), suppresses workspace/
    monorepo aggregators by reading the marker file's own content, and
    layers an optional `.carto/roots.json` on top (the exact
    built-ins-plus-repo-file shape ADR-0027 established). `FileNode`
    gains `component: Option<String>`; `GraphDocument` gains a
    `components: Vec<Component>` table; `Manifest` gains
    `roots_rule_digest` (closing, for this config file, the same
    provenance gap ADR-0027 left open for `.carto/contracts.json`).
    `SCHEMA_VERSION` bumped 6 → 7. Verified additive: a repo with no
    nested components produces a graph diff that is exactly the new
    `null`/empty-table fields, checked by diffing
    `fixtures/mixed.graph.golden.json`, not assumed. Dogfooded against
    carto's own 4-crate workspace — correctly detects all four crates,
    correctly suppresses the workspace-only root `Cargo.toml`.
  - **ADR-0035**: `resolve.rs`'s `CallResolver` gains two new
    component-scoped tiers (imported/exported, caller's own
    component), tried before their repo-wide counterparts and falling
    through to them on ambiguity — provably safe, since a
    component-scoped candidate set is always a subset of the repo-wide
    one. The four `Certain`-confidence import indices each get their
    own treatment rather than one uniform rule: `known_modules` (Rust
    `mod` names) is a strict per-component partition (semantically
    required — a `mod` in one crate is never visible to another);
    `fqn_to_file`/`namespace_to_files` (PHP `use`/C# `using`) prefer
    same-component with a repo-wide fallback; `known_namespace_roots`
    is deliberately left unchanged (narrowing it risks misclassifying
    a genuinely-internal-but-cross-component import as external). One
    disclosed non-additive case, tested explicitly: component scoping
    can retarget a call that used to resolve via cross-component
    owner-type disambiguation (ADR-0032) to a different, same-component
    candidate instead — the more precise answer, not silently folded
    into "purely additive." `--component <name>` (repeatable) reaches
    `where`/`deps`/`map`/`contract`/`orphans` on both front ends
    (ADR-0014's "restricts what's listed" principle, a separate
    concern from the resolution narrowing above); `deps`'s
    `resolve_target` treats it as a tiebreaker only, exactly like
    `--subpath`, not an unconditional filter (ADR-0014's own
    real-bug-caught lesson, not re-made here — verified end to end:
    `deps Handler --component billing` resolves cleanly on a
    synthetic two-component fixture where the unscoped name alone is
    ambiguous). `map` gains a `components` section. 11 new
    `resolve.rs` tests (84 total) plus 13 new `QueryGraph` tests; all
    429 pre-existing `resolve.rs` tests pass completely unchanged.
    `fixtures/rust-crate.{where,deps,map}.golden.json` regenerated —
    diffs are exactly the new fields.
  - `bash scripts/gates.sh` green at each commit landing this work.
  - **Follow-up review (2026-09-21, ADR-0036)**: exercising the shipped
    capability against real binary output (not just the fixture/unit
    suite) found the `Certain`-confidence `Imports` edge — the kind
    ADR-0034's own Context calls "the most damaging" when it crosses a
    component boundary wrongly — carried no record of the crossing at
    all; `calls`/`references` got `cross-component`/`imported-cross-
    component` evidence, `imports` did not. Fixed: all five file→file
    `Certain` `Imports` sites now push an additional `"cross-component"`
    evidence entry when their endpoints' components differ; `map`'s
    cross-component summary now tallies by confidence too, so a
    `certain` crossing renders distinctly from an `inferred` one
    instead of one merged count. Also fixed: a monorepo with only a
    single top-level manifest got zero components with no signal
    anywhere that `.carto/roots.json` was the intended escape hatch —
    `index` now reports `components: N` and, at `N == 0`, points at
    that file. Two minor fixes in the same pass: `.carto/roots.json`
    was read from disk twice per index (parse + digest, a narrow
    TOCTOU), and `any_file_under` allocated a string per walked file
    per declared root instead of the prefix-and-boundary check
    `component_of_path` already uses. All 458 pre-existing tests pass
    unchanged; `fixtures/mixed`/`fixtures/rust-crate` graphs verified
    byte-identical before/after by diff.
  - **Follow-up review, slice 2 (2026-09-21, ADR-0037)**: the terraform
    marker's plain "≥1 HCL file in a directory" rule fragmented a
    normal `infra/modules/*`/`infra/envs/*` tree into one component per
    subdirectory (`infra`/`vpc`/`rds`/`prod`/`dev` instead of one
    `infra`) — generic names colliding with real service names, and
    defeating `--component`'s whole purpose for the `orphans
    --category metric_name` use case (ADR-0026) an infra project's own
    resources are meant to be scoped by. Fixed: `detect_components`
    drops a terraform-only directory at or under an exact-marker
    component's own directory (`services/orders/infra` belongs to
    `orders`), then repeatedly merges surviving terraform directories
    to their lowest common ancestor unless that ancestor is the repo
    root or under a strong component — collapsing a genuinely nested
    `.tf` tree to its own topmost directory, and scattered
    environment/module directories with no shared `.tf`-bearing
    ancestor up to the one directory that represents the project,
    while leaving two genuinely unrelated terraform trees unmerged.
    Also fixed: `is_aggregator` (previously `content.contains(
    "[workspace]")`/`content.contains("\"workspaces\"")`, which could
    false-positive on a comment, a `[workspace.metadata]` nested table,
    or an unrelated string value) now line-scans `Cargo.toml` for an
    exact `[workspace]`/`[package]` header and real-parses
    `package.json` as JSON for a top-level `"workspaces"` key, and
    additionally recognizes a sibling `pnpm-workspace.yaml`/
    `lerna.json`/`turbo.json`/`nx.json`/`rush.json` as an aggregator
    signal `package.json`'s own content can't see.
    `.carto/roots.json` gains `exclude: Vec<String>` (repo-relative
    paths dropped from auto-detection, applied before `roots` so a
    declared root can still re-add an excluded path; unlike `roots`'
    own `path`, an exclude matching nothing is not an error). 11 new
    `components` tests (35 total); `fixtures/monorepo` gains an
    `infra/envs/{prod,dev}` + `infra/modules/vpc` subtree exercising
    the rollup end to end (`cli_multiroot.rs` updated: six components,
    not five). `fixtures/sid-like/infra` (single, non-fragmented
    terraform directory) reverified unaffected.
  - **Follow-up review, slice 3 (2026-09-21, ADR-0038)**: `--component
    <name>` matched a node's own `component` field by exact name only
    — wrong whenever a component nests inside another's own directory,
    a shape ADR-0034's own innermost-match rule already permits
    (`services/api/go.mod` + `services/api/internal/go.mod` producing
    two components, one nested in the other, `components::tests::
    innermost_component_wins_for_nested_markers`). Fixed: new
    `QueryGraph::component_matches_filter` treats `--component api` as
    scoping to `api` *and* everything nested under it (one-directional
    — `--component internal` does not pull in `api`'s own files);
    reaches `find`/`deps`/`contract` via the shared
    `component_in_scope` predicate, plus direct updates to `map`'s
    `seed_component_counts` and `orphans`' touching-components check.
    No schema change — purely a query-layer read of data already
    persisted. 6 new tests; verified end to end against a synthetic
    nested-component repo through the real binary (`map --section
    components --component api` lists both `api` and `internal`).
    Multi-directory components (one component spanning two *disjoint*
    subtrees) is a separate, larger capability — would need
    `Component::path: String` to become `paths: Vec<String>`, a
    breaking schema change — deliberately not attempted in this slice;
    see "deliberately absent" below.
  - **Follow-up review, slice 4 (2026-09-21, ADR-0039)**: a component
    dependency graph, requested directly by the user. `Component`
    gains `depends_on: Vec<String>`, resolved by a new
    `crates/carto-core/src/components/deps.rs` at the end of
    `ComponentSet::discover` — per-`kind` manifest parsing, dependency-
    free: go's `require`/local `replace`, node's `file:`/`workspace:`/
    name-match `dependencies`, Rust's `path =` entries (inline-table
    and sub-table forms), dotnet's `<ProjectReference>`, PHP's path
    `repositories`/name-match `require`. `terraform`/`custom`-kind
    components get no dependency resolution (no manifest-identity
    concept for either) — `crate::components::DEPENDENCY_AWARE_KINDS`
    names the five kinds that do, and gates every use of the data so a
    dependency-unaware component's crossings are never misread as
    "confirmed undeclared." Used three ways, all additive: a
    cross-component `Imports`/`Calls`/`References` edge whose target
    isn't in the caller's `depends_on` gains an
    `"undeclared-dependency"` evidence entry (`lang::resolve::
    is_undeclared_dependency`, applied alongside `"cross-component"`
    at all nine cross-component-marker sites); `resolve_fqn` and the
    `NamespaceImport` fan-out each gain a tier preferring a declared
    dependency over the pre-existing arbitrary fallback, between
    same-component and that fallback; `map --section components`
    renders `depends_on` and marks an undeclared crossing
    `[undeclared]`. `SCHEMA_VERSION` bumped 7 → 8. 14 new
    `components::deps` tests, 4 new `resolve.rs` tests (one
    deliberately constructed so the dependency-preference tier changes
    the outcome, not just coincidentally agrees with first-file-wins),
    3 new `map.rs` tests; all 489 pre-existing tests pass unchanged
    (every one passes `&[]` for the new `components` parameter).
    `fixtures/monorepo/services/orders/go.mod` gains a real
    `require shared v0.0.0` — verified end to end: `orders`'s
    pre-existing cross-component call to `shared` (ADR-0035's own
    motivating case) no longer carries `"undeclared-dependency"`,
    while a separate synthetic two-component repo with no declaration
    confirmed the opposite. `fixtures/mixed.graph.golden.json`'s only
    diff is the schema-version bump (its `components: []` table has
    nowhere for the new field to appear); `fixtures/rust-crate.*`
    needed no regeneration.
- **S-1 re-measured (2026-09-22, ADR-0040)** —
  `docs/post-s1-improvement-plan.md`'s own "step 5," pending since
  ADR-0019. Ground truth for every one of the 8 tasks was re-verified
  against current state first, not assumed valid — a real, not
  theoretical, precaution: T8's recorded answer was provably wrong
  (its target function, `indexer::build_and_persist`, was directly
  modified by the multi-root work in the intervening six weeks), and
  L3's entire premise (a documented Rust grouped-`use` extraction gap)
  no longer held, having been fixed by ADR-0022. Re-verifying also
  caught `bench/replay/replay.sh`'s T6 command failing outright
  (`load` became ambiguous once contracts added a second symbol by
  that name) before it could abort the paid batch. `bench/tasks.md`
  carries dated addenda recording every changed number, kept visible
  per the file's own house style, not silently edited over.
  - The re-run itself: 3 arms (`grep`/`cli`/`carto`-MCP) × 8 tasks, 24
    real sessions, $4.74. Neither S-1 threshold met, same headline as
    ADR-0019, but the shape changed substantially. **Accuracy is a
    full reversal**: carto 87.5%→100% (+12.5 points over grep-only,
    was −6.2). The specific tasks the post-S-1 work targeted are the
    ones that flipped — T6 (ADR-0019's own "sharpest, most instructive
    result," carto silently returning a plausible-but-beside-the-point
    answer) is now correct, the agent explicitly noticing and acting
    on `root_uncaptured_inbound_calls` before answering; L3's "11 of
    18, missing 7" *and* its separate "unreachable via the MCP tool
    surface" finding are both now clean wins (24 of 24, `where`/`deps`
    resolve `carto_core` by name directly); T8's "invisible by
    construction" third bucket is now three visible numbers in carto's
    own output. **Token cost barely moved** (18.0%→15.8% more than
    grep) — correctness is no longer S-1's binding constraint, token
    footprint is; `map`'s whole-payload-per-call shape (flagged as a
    real cost driver in ADR-0019's own replay-arm finding) remains the
    likely largest un-pulled lever. grep-only's own accuracy dropped
    (93.8%→87.5%, missing one real T7 caller) — a reminder the
    baseline carries its own one-trial noise. **A genuinely new
    finding, independent of any threshold**: both carto-backed arms on
    T7 independently surfaced a bare call
    (`agent_ui/agent_diff.rs:686`) carto's extractor silently misses
    with *no* signal at all — outside the documented ADR-0008
    exclusion, likely because it sits inside a `format!(...)` macro
    argument — a real extraction gap worth its own follow-up, separate
    from this ADR. The `cli` arm matches MCP's 100% accuracy while
    using 7.7% fewer tokens — real evidence MCP's own transport
    overhead costs something independent of data quality.
  - Full per-task grading rationale, re-verified ground truth, and
    every raw session file: `bench/results/20260922T173830/`
    (`GRADES.md`, `GRADES.json`, `transcripts/`), spot-checked for
    secrets before committing (none found). Per spec §11.4, this ADR
    presents the result and does not make the M3 go/no-go call —
    **that decision remains the user's, now on current data.**
- **Terraform source support, slice 1 of 4 (2026-09-29, ADR-0041)** —
  pulled ahead of the M3 gate on user request (source HCL first; plan/
  state-JSON ingestion stays a later slice). `.tf`/`.tf.<variant>` files
  now yield one symbol per top-level `variable`/`output`/`resource`/
  `data`/`module` block and per `locals` attribute, **named by Terraform
  address** (`var.region`, `local.prefix`, `aws_s3_bucket.logs`) so
  `where`/`deps` work on them directly; seven new `SymKind`s. References
  (`var.*`, `local.*`, `module.*`, `data.*`, `<type>.<name>`) resolve in
  a **dedicated directory-scoped pass** (`lang/terraform.rs`), never the
  generic tier ladder: same-module → `inferred` `references`; definitions
  only in other env-variant files (`locals.tf.simu`/`.prod`) → one edge
  per variant; `override.tf` yields to the base; a miss or a real
  duplicate → no edge, recorded in `unresolved_calls`. Symbols are never
  `is_pub` (else a Terraform `variable "timeout"` would make a Python
  `timeout()` call ambiguous). References are read from source text
  after each `variable_expr` because the grammar's sibling runs are
  unreliable inside binary operations (verified on real parse trees).
  `SCHEMA_VERSION` 8 → 9. `fixtures/tf-modules/` +
  `cli_terraform.rs` (7 tests). Contract literals in HCL now attach to
  the resource symbol instead of the `File`. **Next slices:** ADR-0042
  module calls/cross-module edges, ADR-0043 Terragrunt, ADR-0044 the
  tfvars implication (variables may be set outside the code; `.tfvars`
  contents stay unread). Note: `carto-core`'s `trybuild` compile-fail
  suite mismatches its snapshots on rustc 1.94.1 (extra `println!`
  macro note) — pre-existing, identical on untouched `origin/main`.
- **Terraform source support, slice 2 of 4 (2026-09-29, ADR-0042)** —
  module calls. A local `source` (`./`, `../`) → file-level `imports`
  fan-out to every `.tf` file of the target directory, `certain`
  (`tf-module-source`); any other source → an external `Module` node
  whose key has **credentials and `?query` stripped before it exists**
  (`ModuleNode::path` is never redacted); `module.m.out` → the *called*
  directory's `output`, module arguments → the called directory's
  `variable`s (both `inferred`); missing target dir / argument /
  output / dynamic source → `unresolved_calls`, never guessed.
  `map --section infra` now summarizes source-level Terraform (counts
  per kind, module-call graph, remote modules) and still says the
  resolved graph needs M2; non-Terraform repos keep the placeholder.
  No `SCHEMA_VERSION` change. `fixtures/tf-modules/.../modules.tf`,
  +5 `cli_terraform.rs` tests, 19 `terraform.rs` tests. **Next:**
  ADR-0043 Terragrunt, ADR-0044 tfvars caveat.
- **M2+ remaining (infra graph proper, join, ingest)** — spec §10. The
  contract slice above is adjacent to, not a substitute for, this: no
  `IacResource`/`depends_on`/IAM extraction/§6.6 attribute allowlist
  exists yet. **Still paused at the S-1 gate, pending the user's M3
  go/no-go decision above** — not proceeding automatically.

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

- **`known_namespace_roots` (C#/PHP) is not component-scoped**, unlike
  its sibling indices `fqn_to_file`/`namespace_to_files` — a namespace
  root declared anywhere in the repo is still evidence it's the
  repo's own, regardless of which component declared it; narrowing
  this risks the opposite failure (a genuinely-internal-but-cross-
  component import misclassified as external). ADR-0035.
- **No `go.work`/npm-nested-workspace-aware multi-level component
  hierarchy** beyond simple innermost-directory-match nesting (though
  `--component` does scope *across* that nesting since ADR-0038 — see
  below); no per-component `.cartoignore`; `.carto/roots.json`'s digest
  goes into `manifest.json` provenance but nothing reads it back (no
  query command reads `Manifest` at all today — same reasoning
  ADR-0027 gave for `.carto/contracts.json`'s own digest). ADR-0034.
- **`Component` is one directory, not a set of directories** — a
  component spanning two *disjoint* subtrees (`services/orders` +
  `libs/orders-proto` as one named component) has no representation;
  `Component::path` would need to become `paths: Vec<String>`, a
  breaking `graph.json` schema change, to support it. Descendant
  scoping (`--component <name>` also matching a nested component,
  ADR-0038) is a different, already-solved problem — it needed no
  schema change, since ADR-0034's own innermost-match nesting already
  produces multiple `Component` rows for that shape.
- **`Component::depends_on` is only resolved for
  `go`/`node`/`rust`/`dotnet`/`php`** (`crate::components::
  DEPENDENCY_AWARE_KINDS`) — a `terraform`/`custom`-kind component's
  `depends_on` stays empty always, not because it provably has no
  dependencies but because carto has no manifest-identity concept to
  read for either kind (a `.tf` file's own module blocks reference
  other `.tf` sources, not components; a declared/`"custom"` root has
  no assumed manifest format). No `.carto/deps.json` override exists
  to declare a dependency a manifest can't express — considered,
  not built; no concrete need surfaced yet, unlike `.carto/roots.json`'s
  own origin story. ADR-0039.
- **`Contract` categories beyond `metric_name`** — env vars, DynamoDB
  attributes, Kafka topics, WS wire-protocol fields, Parquet/Glue
  columns, SID-style subtype IDs, doc-mention edges, Terraform
  reachability (unused locals, undefined `local.X`), the `.csproj`/
  `implements` graph. Named as follow-up work in
  [ADR-0026](adr/0026-contract-node-and-produces-consumes-edges.md)'s
  Consequences and the motivating repo's own ranked list; none built
  yet. The node/edge vocabulary needs no new kind for any of them —
  only new `ContractRule`s (ADR-0027) and per-language literal capture.
- **`uncaptured_contract_sites` (a per-category count of interpolated/
  computed literals a `Contract`-producing extractor recognized but
  couldn't reduce to a plain value)** — not surfaced anywhere yet,
  unlike calls' own `uncaptured_inbound_calls`/`uncaptured_outbound_calls`
  (ADR-0020/0023). An interpolated HCL `metric_name` or a computed C#
  `Name` value is currently silently dropped, indistinguishable from
  "this position was never looked at" — ADR-0026's own "slice 1
  narrowing" note explains why (no clean home in `graph.json`'s
  current shape was found within this slice's scope; `Manifest` is
  never read back by any query command today).
- **`.carto/contracts.json`'s digest is not joined into
  `manifest.json`'s provenance tracking** the way `walk`'s
  `ignore_rule_digest` is — two indexes built under different
  classification rules aren't distinguishable from `manifest.json`
  alone yet. ADR-0027.
- **No `IacResource`/`depends_on`/IAM-extraction/§6.6 attribute
  allowlist** — the contract slice (ADR-0025/0026/0027) is adjacent to
  spec §6's infra graph, not a substitute for it. HCL literals attach
  to the enclosing resource *symbol* (ADR-0041; the `File` node only
  when no symbol contains them) — never to an infra-graph node, because
  that node doesn't exist yet.
- **Terraform source support is source-level and approximate** (ADR-0041):
  references are always `inferred`; scope is "this directory's `*.tf` +
  env-variant files", so a gitignored generated file (`locals_env.tf`), a
  `*.tf.json`, terragrunt `generate` blocks are invisible — an
  `unresolved_calls` entry means "not found among parsed files". No
  `count`/`for_each` instance expansion, no provider aliases, no
  `moved`/`import`/`check`/`provider`/`terraform` block symbols, nested
  blocks are not symbols. A token embedded in a remote module
  source's *path* is not detected (userinfo and `?query` are
  stripped, ADR-0042). Terragrunt and the tfvars caveat are
  ADR-0043/0044 (not yet built); `*.hcl` files still yield contract
  literals only.
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
  evidence the symbol has few callers. **Same day, ADR-0023 added the
  outbound counterpart**: `SymbolNode::uncaptured_outbound_calls`/
  `deps --dir out`'s `root_uncaptured_outbound_calls` — a symbol's own
  path-qualified calls, counted by which symbol's body they're inside,
  not by name.
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
  **Rust import detection is no longer `use`-declaration-only**:
  since 2026-08-03 ([ADR-0024](adr/0024-bare-reference-import-detection.md)),
  a bare fully-qualified-path reference with no `use`/`mod` at all
  (`carto_core::Result<u8>`, valid since Rust 2018) is also captured,
  heuristically (lowercase-root convention, excludes a call's own
  callee), with its own evidence string
  (`"external-package-bare-reference"`) distinguishing it from a
  verified `use` declaration.
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

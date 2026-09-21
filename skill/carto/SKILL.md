---
name: carto
description: Structural code navigation for a repository — symbol lookup, dependency/caller traversal, cross-language string-literal contract checks, and a token-bounded orientation overview, backed by a deterministic graph rather than lexical search.
---

# carto

carto builds a deterministic structural graph of a repository — files,
symbols, modules, imports, best-effort call and type-reference edges,
and cross-language string-literal contracts — using tree-sitter,
entirely offline. It
answers structural questions a grep/read exploration can also answer,
but by traversing a pre-built graph instead of reading files, which is
usually fewer tokens for the same answer, and gives an answer with an
explicit confidence attached rather than an implicit guess.

A repo that's actually several projects under one root — microservices,
frontends, lambdas, infra — gets that structure recognized
automatically: carto detects each project's own root (a `go.mod`,
`package.json`, `Cargo.toml`, …, or one declared in
`.carto/roots.json`) as a **component**, and every tool below accepts a
repeatable `component` param to scope its answer to one or more of
them.

carto is a separate MCP server (`carto serve`), not a bash tool. If it's
connected for this session, its tools appear alongside the built-in
ones, named `index`/`where`/`deps`/`map`/`contract`/`orphans`/
`selfcheck`.

## Six questions carto answers

**"Where is X defined?"** — `where` finds a symbol by name (substring or
exact match) and returns its file:line and signature. Since a repo's
modules are also indexed, it can also match a `Module.path`
(`module_matches`, a separate list from symbol matches) — a package/
namespace, not just a function or type.

    where(repo_path=".", needle="parseOrder")
    where(repo_path=".", needle="parseOrder", exact=true, limit=10, subpath="src/orders")
    where(repo_path=".", needle="Handler", component="orders")

**"What does this depend on, or what depends on it?"** — `deps` walks the
call/import graph from a symbol, file, or module, in either direction, to
a bounded depth, with confidence on every edge. `target` can be a node
ID, an exact symbol name, or a `Module.path`/`File.path`.

    deps(repo_path=".", target="parseOrder", dir="in", depth=2)
    deps(repo_path=".", target="src/orders/mod.rs", dir="both", depth=1, kinds="imports,calls", subpath="src")

In a multi-component repo, a same-named symbol in two different
components (two services each defining `Handler`) is genuinely
ambiguous by name alone — `target` becomes unresolvable without
narrowing. `component` on a `deps` call is a *tiebreaker* the same way
`subpath` already is: it only kicks in once the plain name is already
ambiguous, so it never overrides an otherwise-unambiguous match:

    deps(repo_path=".", target="Handler", component="billing", dir="out")

Each row in the response also carries its own `component` (via `node`),
and `same_component_as_root` flags whether a given edge crosses a
component boundary — rendered as `[cross-component]` next to the
existing `[same file as root]` marker.

**"What imports this module?"** — same `deps` tool, `dir="in"` on a file
or module target instead of a symbol.

**"Who actually uses this type/interface?"** — a field/parameter/base-
clause reference to a type is its own edge kind, `references`, distinct
from `imports` (which only ever proves "this file imports the *module*
the type lives in", not "this file names the type"). When a target
type's namespace/module/package holds more than one exported name,
`--kinds imports` on the containing file over-collects — every file
importing that module for *any* reason, not just this type — while
`--kinds references` on the type itself is exact:

    deps(repo_path=".", target="IQueryJobStore", dir="in", depth=1, kinds="references")

`--depth 1` is enough here on purpose: a symbol's `references`/`calls`
edges land directly on it, without the forced `contains`-hop through
its file that makes depth 1 look empty for an `imports`-only query.
`kinds="calls,references"` combines both signal types (who invokes it,
who names it as a type) while still skipping that file-level hop
entirely. One exception: a result row whose `node.kind` is `"file"`
rather than `"symbol"` is real, not noise — it means the usage site is
top-level-statement code with no enclosing method/class at all (C#'s
`Program.cs` in ASP.NET Core's minimal-API style is the common case;
TS/JS/Python module-level setup code the same shape). carto resolves
those at file scope rather than dropping them.

**"Give me an orientation of this repo"** — `map` returns a budget-capped
overview: top modules by import fan-in/out, structural entry points,
component structure, and counts by node/edge kind — meant to answer
"what is this repo, roughly" in far fewer tokens than reading a
directory tree and a handful of files.
`sections` (repeatable: `counts`, `modules`, `entry-points`, `infra`,
`components`) lets a question that only needs one section skip paying
for the rest; the structured `counts` field is always exact regardless
of this filter.

    map(repo_path=".", budget=100)
    map(repo_path=".", sections="entry-points", subpath="crates/carto-core")
    map(repo_path=".", sections="components")

The `components` section is the fastest way to learn a monorepo's
top-level shape: one row per detected project (name, path, kind,
file/symbol counts) plus a summary of edges crossing between two
different components — e.g. `orders -> shared: 12 calls, 3 imports`.
`sections="components"` alone, before reading anything else, is often
enough to know whether a question needs one component or several.

**"Do this repo's string-literal contracts line up?"** — a capability
outside the original design (ADR-0026), for values no compiler checks
because they cross a language boundary as a bare string: a CloudWatch
metric name emitted by one service (a C# `object-init:Name`) and
referenced by an alarm in another file's Terraform
(`aws_cloudwatch_metric_alarm.metric_name`). Today's built-in coverage is
scoped to one category, `metric_name`; a repo can extend the category
vocabulary via `.carto/contracts.json` without any code change.
`orphans` answers "what's missing" — contracts consumed but never
produced (a dead alarm that can never fire) and the mirror, produced but
never consumed:

    orphans(repo_path=".", category="metric_name")
    orphans(repo_path=".", category="metric_name", component="ingest")

`contract` answers "who else touches this exact value" — every producer
and consumer site for one literal:

    contract(repo_path=".", value="QuoteDropsPerSecond")

Each repo needs indexing once per session (or after a large change)
before these can answer anything:

    index(repo_path=".")

A repo already indexed into a given out-dir (the default, or whatever
`--out`/`out` you passed) keeps that graph available for every later
call in the same session — re-indexing the same repo before every
`where`/`deps`/`map`/`contract`/`orphans` call in a multi-question
session repeats a real, non-trivial cost for no new information.
Re-index only after a large change to the repo, not once per question.
An index built by an older carto version is rejected outright (with an
instruction to re-run `index`) rather than answered from stale data —
seeing that error means exactly that, not that something else is wrong.

## The honesty contract

Every `deps` edge carries a `confidence`: `certain` means the connection
was verified against a resolved reference (e.g. `mod foo;`); `inferred`
means a best-effort name match that could be wrong — worth a second look
before acting on it, especially for anything security- or correctness-
sensitive. A missing edge is not a claim that no connection exists — it
usually means carto couldn't verify the connection unambiguously and
chose to say nothing rather than guess (see `deps`'s
`root_unresolved_calls` field for what a symbol calls that carto couldn't
resolve, `root_uncaptured_inbound_calls` for call sites elsewhere in
the repo that spell this symbol's name in a shape carto never even
attempts to resolve, `root_uncaptured_outbound_calls` for the same
gap in the other direction — calls *this* symbol makes that carto never
attempts — and `same_file_as_root` on each row, flagging when a caller
and its callee share a file, so that isn't misread as the definition
itself; see the next section for what the uncaptured-calls gap means in
practice).

Symbol IDs are derived from file path, kind, name, and line number — they
change when code moves. Re-run `where`/`deps` after an edit rather than
reusing an ID from before it.

## Known gaps worth acting on

Capabilities aside, a few specific, current limitations are worth
knowing before trusting a result at face value:

- A `deps --dir in` answer can look smaller than reality for a symbol
  you know is used — Rust calls written as `Type::method()` or
  `module::func()` are never attempted for resolution (by design, not
  a bug). When this applies, the same response's
  `root_uncaptured_inbound_calls` will be nonzero — that's a direct
  signal the answer is incomplete, not evidence the symbol has few
  callers. Worth a targeted grep too when the count is nonzero and the
  question matters. The same applies in the other direction: a `deps
  --dir out` answer that looks like a symbol calls little can have a
  nonzero `root_uncaptured_outbound_calls` instead — its own calls
  written in that same unresolved shape, not evidence it does less
  than it looks like.
- A file's absence from an import/dependency answer can mean "no
  import" or "the only import is `use a::*;` or an aliased `use a::b
  as c;`" (Rust) — those two shapes are extracted as nothing rather
  than partially interpreted. Treat a narrow miss on an import-fan-out
  question as plausible, not certain, until confirmed another way.
  (Grouped `use path::{a, b};` is extracted normally, not a gap.) A
  Rust file can also import something with no `use`/`mod` statement at
  all, via a bare fully-qualified path like `carto_core::Result<u8>`
  (valid since Rust 2018) — carto captures this too, heuristically,
  with its own evidence string `"external-package-bare-reference"` on
  the `deps` edge so it's distinguishable from a verified `use`
  declaration.
- `map`'s entry-points list is a structural heuristic (zero incoming
  imports, or a symbol named `main`) — it will include non-code files
  (READMEs, config, fixtures) alongside real entry points. Filtering
  the list by judgment, rather than treating every row as a genuine
  entry point, tends to give a better answer.
- `orphans`/`contract` only see a literal that's spelled as a plain
  string in the source — an interpolated or computed value (an HCL
  `"${local.prefix}Drops"`, a C# value built at runtime rather than a
  string constant) is silently not extracted at all, and unlike calls'
  `uncaptured_inbound_calls`/`uncaptured_outbound_calls`, there's no
  counter yet surfacing how many such sites exist. A metric/contract
  that's genuinely there in the source but built dynamically won't show
  up in either producer or consumer lists — worth a grep for the
  category's known-dynamic patterns before concluding a value truly has
  no producer.
- A contract value is scoped by its `qualifier` (e.g. a CloudWatch
  namespace) as part of its identity, deliberately — the same bare name
  under two different qualifiers is two separate contracts, not one, so
  `contract(value="X")` can return multiple unrelated matches rather
  than one merged answer.
- A `references` edge that fails to resolve is silently dropped —
  unlike an unresolved *call*, there's no `unresolved_type_refs`-style
  list or counter surfacing how many such sites exist. This is
  deliberate (an unresolved type ref is overwhelmingly stdlib/BCL/
  framework noise — `Task`, `string`, `ILogger` — not a signal worth a
  counter), but it does mean a `references` answer of zero can still
  mean "this type is used, just via a shape not attempted" rather than
  "genuinely unused" — a fully inferred-type variable (`var`/`:=`), a
  Python string forward-reference annotation (`"Foo"`), and PHP's
  disjunctive-normal-form types (`(A&B)|C`) are all shapes no extractor
  attempts. Worth a grep when the answer looks surprisingly empty and
  the question matters.
- `component` filters *what's listed*, never what a traversal computes
  — `deps`'s BFS still crosses component boundaries and back, `map`'s
  fan-in/out numbers stay whole-repo-accurate. An unknown component
  name is a hard error listing the real ones (unlike `subpath`, which
  has no enumerable list to check against and just returns fewer
  rows). `contract`'s `component` filters producer/consumer *sites*
  within a match — a match can come back with empty lists rather than
  disappearing, which is itself the answer for a cross-component
  question. `orphans`'s `component` filters the *report* instead — an
  orphan's defining fact (no producer, or no consumer, anywhere) is
  repo-wide, not a per-site thing, so a contract with no site in the
  requested component is dropped entirely rather than shown empty.
- A call or type reference in top-level-statement code (C#'s
  `Program.cs`-with-no-`Main` style, TS/JS/Python module-level setup
  code) resolves at file scope when it succeeds, but — unlike a
  symbol-scoped miss, which lands in `root_unresolved_calls` — a miss
  there is fully invisible: there's no `unresolved_calls`-equivalent
  for a `File` root yet. A `deps --dir out` on a *file* target
  returning few edges can mean "this file's top-level code genuinely
  calls little" or "several of its top-level calls/type-refs didn't
  resolve," and the two look identical from the response alone.

## What carto does not do (yet)

No infrastructure graph, no code↔infrastructure join, no semantic/summary
layer, no `impact`/`infra_of`/`unused_permissions` tools — these are on
carto's roadmap but not built. `map`'s output says so explicitly rather
than silently omitting those sections. Call resolution is best-effort and
language-dependent: some call shapes (e.g. Rust's `Type::method()`,
module-qualified calls) aren't captured at all in the current version, by
design rather than oversight — `deps` on those call sites will show
nothing rather than a wrong answer, though it does count them (see
above). Type-reference resolution (`references` edges) goes through
the same best-effort tiers and the same first-match-wins/ambiguous-
produces-nothing policy — see the known-gaps entry above for which
type shapes aren't attempted at all. Contract coverage is one category
(`metric_name`) built in, with producers from C# and consumers from
Terraform/HCL — real, but narrow;
most string-keyed contracts in a typical repo (env vars, queue/topic
names, DB attribute names) aren't classified unless a repo adds its own
rules via `.carto/contracts.json`.

For anything transitive or blast-radius-shaped (what depends on this,
what does this affect), combining carto's structural answer with a
source read or a targeted grep tends to work better than treating either
one alone as the final word — carto is precise about what it verified
and honest about what it didn't attempt, but that's a reason to combine
it with other tools on a question that matters, not a reason to reach
for grep only after carto's answer looks incomplete.

## carto is under active development

When a carto answer looks wrong, empty, or contradicts what the source
plainly shows, that is worth surfacing to the user — with the exact tool
call and its raw output — rather than quietly working around it. A
surprising result is data about carto, not just an obstacle. The
distinction that matters: the gaps named above are *known* and
documented, so hitting one is expected behavior worth mentioning in
passing; a result that contradicts none of them is a candidate bug and
worth stopping on rather than silently falling back to grep.

## No API key, no config writes

carto never calls a network service and never needs credentials of any
kind. It only ever writes to its own output directory (default
`${XDG_CACHE_HOME:-~/.cache}/carto/<repo-hash>/`, or a directory you
choose) — never to `CLAUDE.md`, `.claude/`, git config, or anything
outside that one directory.

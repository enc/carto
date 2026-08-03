---
name: carto
description: Structural code navigation for a repository — symbol lookup, dependency/caller traversal, and a token-bounded orientation overview, backed by a deterministic graph rather than lexical search.
---

# carto

carto builds a deterministic structural graph of a repository — files,
symbols, modules, imports, and best-effort call edges — using tree-sitter,
entirely offline. It answers structural questions a grep/read exploration
can also answer, but by traversing a pre-built graph instead of reading
files, which is usually fewer tokens for the same answer, and gives an
answer with an explicit confidence attached rather than an implicit guess.

carto is a separate MCP server (`carto serve`), not a bash tool. If it's
connected for this session, its tools appear alongside the built-in ones,
named `index`/`where`/`deps`/`map`/`selfcheck`.

## Four questions carto answers

**"Where is X defined?"** — `where` finds a symbol by name (substring or
exact match) and returns its file:line and signature.

    where(repo_path=".", needle="parseOrder")

**"What does this depend on, or what depends on it?"** — `deps` walks the
call/import graph from a symbol or file, in either direction, to a bounded
depth, with confidence on every edge.

    deps(repo_path=".", target="parseOrder", dir="in", depth=2)

**"What imports this module?"** — same `deps` tool, `dir="in"` on a file
or module target instead of a symbol.

**"Give me an orientation of this repo"** — `map` returns a budget-capped
overview: top modules by import fan-in/out, structural entry points, and
counts by node/edge kind — meant to answer "what is this repo, roughly"
in far fewer tokens than reading a directory tree and a handful of files.

    map(repo_path=".", budget=100)

Each repo needs indexing once per session (or after a large change)
before these can answer anything:

    index(repo_path=".")

A repo already indexed into a given out-dir (the default, or whatever
`--out`/`out` you passed) keeps that graph available for every later
call in the same session — re-indexing the same repo before every
`where`/`deps`/`map` call in a multi-question session repeats a real,
non-trivial cost for no new information. Re-index only after a large
change to the repo, not once per question.

## The honesty contract

Every `deps` edge carries a `confidence`: `certain` means the connection
was verified against a resolved reference (e.g. `mod foo;`); `inferred`
means a best-effort name match that could be wrong — worth a second look
before acting on it, especially for anything security- or correctness-
sensitive. A missing edge is not a claim that no connection exists — it
usually means carto couldn't verify the connection unambiguously and
chose to say nothing rather than guess (see `deps`'s
`root_unresolved_calls` field for what a symbol calls that carto couldn't
resolve, and `root_uncaptured_inbound_calls` for call sites elsewhere in
the repo that spell this symbol's name in a shape carto never even
attempts to resolve — see the next section for what that means in
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
  question matters.
- A file's absence from an import/dependency answer can mean "no
  import" or "the only import is a grouped `use path::{a, b};`
  statement, `use a::*;`, or an aliased `use a::b as c;`" (Rust). Those
  three shapes are extracted as nothing rather than partially
  interpreted — treat a narrow miss on an import-fan-out question as
  plausible, not certain, until confirmed another way.
- `map`'s entry-points list is a structural heuristic (zero incoming
  imports, or a symbol named `main`) — it will include non-code files
  (READMEs, config, fixtures) alongside real entry points. Filtering
  the list by judgment, rather than treating every row as a genuine
  entry point, tends to give a better answer.

## What carto does not do (yet)

No infrastructure graph, no code↔infrastructure join, no semantic/summary
layer, no `impact`/`infra_of`/`unused_permissions` tools — these are on
carto's roadmap but not built. `map`'s output says so explicitly rather
than silently omitting those sections. Call resolution is best-effort and
language-dependent: some call shapes (e.g. Rust's `Type::method()`,
module-qualified calls) aren't captured at all in the current version, by
design rather than oversight — `deps` on those call sites will show
nothing rather than a wrong answer, though it does count them (see
above).

For anything transitive or blast-radius-shaped (what depends on this,
what does this affect), combining carto's structural answer with a
source read or a targeted grep tends to work better than treating either
one alone as the final word — carto is precise about what it verified
and honest about what it didn't attempt, but that's a reason to combine
it with other tools on a question that matters, not a reason to reach
for grep only after carto's answer looks incomplete.

## No API key, no config writes

carto never calls a network service and never needs credentials of any
kind. It only ever writes to its own output directory (default
`${XDG_CACHE_HOME:-~/.cache}/carto/<repo-hash>/`, or a directory you
choose) — never to `CLAUDE.md`, `.claude/`, git config, or anything
outside that one directory.

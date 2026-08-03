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

## The honesty contract

Every `deps` edge carries a `confidence`: `certain` means the connection
was verified against a resolved reference (e.g. `mod foo;`); `inferred`
means a best-effort name match that could be wrong — worth a second look
before acting on it, especially for anything security- or correctness-
sensitive. A missing edge is not a claim that no connection exists — it
usually means carto couldn't verify the connection unambiguously and
chose to say nothing rather than guess (see `deps`'s
`root_unresolved_calls` field for what a symbol calls that carto couldn't
resolve).

Symbol IDs are derived from file path, kind, name, and line number — they
change when code moves. Re-run `where`/`deps` after an edit rather than
reusing an ID from before it.

## What carto does not do (yet)

No infrastructure graph, no code↔infrastructure join, no semantic/summary
layer, no `impact`/`infra_of`/`unused_permissions` tools — these are on
carto's roadmap but not built. `map`'s output says so explicitly rather
than silently omitting those sections. Call resolution is best-effort and
language-dependent: some call shapes (e.g. Rust's `Type::method()`,
module-qualified calls) aren't captured at all in the current version, by
design rather than oversight — `deps` on those call sites will show
nothing rather than a wrong answer.

## No API key, no config writes

carto never calls a network service and never needs credentials of any
kind. It only ever writes to its own output directory (default
`${XDG_CACHE_HOME:-~/.cache}/carto/<repo-hash>/`, or a directory you
choose) — never to `CLAUDE.md`, `.claude/`, git config, or anything
outside that one directory.

# 0010 — Tainted text at the query-output boundary

**Status:** accepted · **Date:** 2026-07-31 · **Milestone:** M1.b.3

## Context

`carto where` (and, prospectively, `deps`/`map`) is the first place in
the codebase that renders repo-derived text — `SymbolNode.signature`, a
`TaintedString` — directly to a terminal. Before this, `TaintedString`'s
only consumer was `graph.json`, via its `Serialize` impl (capped,
unfenced, per `crate::taint`'s own doc comment: "matching what
`graph.json` stores"). Nothing before this slice called
`render_fenced()`. Spec §8.4 is explicit about the requirement:

> Wherever tainted text is rendered (report.md, MCP `map`/`deps`
> responses): [fence] … Renderers MUST route tainted text through
> `render_fenced()`; there is no other accessor (compile-time, INV-5).

Two decisions were needed that the spec text doesn't spell out: what
`--json` output does (the fence is markup meant for something reading
plain text — putting it inside a JSON string value is a different
question), and how many fences a single command's output gets when it
lists multiple signatures.

## Decisions

- **`--json` does not fence.** A query result's `signature` field
  serializes through `TaintedString`'s existing `Serialize` impl —
  sanitized, capped at `consts::SIGNATURE_CAP`, no fence markers —
  identical to what `graph.json` already stores for the same field.
  Consumers of `--json` output are parsers (scripts, another program's
  JSON decoder, eventually an MCP client reading structured content); a
  `⟦carto:data...⟧` string embedded as JSON string content is noise for
  that consumer, not a boundary — the JSON structure itself (a field
  scoped to exactly one symbol's signature) already delimits where the
  repo-derived text starts and ends. §8.4's fence is doing work that
  JSON's own structure already does.
- **Human-readable output fences once per section, not once per row.**
  `carto where`'s `print_human` wraps the *entire* match listing (all
  rows, however many) in one `FENCE_OPEN`/`FENCE_CLOSE` pair when at
  least one match has a signature, rather than fencing each signature
  individually. §8.4's own example (`report.md`, MCP `map`/`deps`
  responses) fences a whole response's data section, not each sentence
  within it — a fence's job is to mark the *boundary* of a region
  containing untrusted text, and a `where` listing with several matches
  is one region. Fencing per-row would spend three lines of boilerplate
  per match and, for `map`'s eventual budget-capped output, would burn
  real budget on repeated markers instead of content.
- Both are accessors [`crate::taint::TaintedString`] already exposes
  (`Serialize`/`render_capped` for `--json`, `render_fenced`-equivalent
  manual wrapping for human output) — no change to `taint.rs` or the
  `trybuild` compile-fail suite was needed. What's new is only the
  *policy* of when each is used, which lives in the CLI renderer
  (`crates/carto-cli/src/where_cmd.rs::print_human`), not in core.

## Consequences

- `deps`'s and `map`'s human renderers (this milestone, later commits)
  follow the same split: `--json` never fences, human output fences the
  whole listing once. Keeping this consistent across all three commands
  means an agent parsing `--json` never has to strip fence markers, and
  a human reading any command's default output sees exactly one region
  boundary regardless of how many rows are inside it.
- If a future renderer (report.md, the MCP text rendering §7.2
  describes) needs a different granularity — e.g. fencing per-file
  rather than per-command — that's a new, separately justified decision,
  not an extension of this one; this ADR covers the CLI's `--json` vs.
  human split specifically.
- Recorded here rather than only in code comments because the same
  question (JSON vs. human, one fence vs. many) will come up again for
  every subsequent renderer that touches `TaintedString`, and getting a
  different answer next time without a stated reason would be exactly
  the kind of silent re-decision `docs/adr/` exists to prevent.

# 0020 — Honest absence signal: `uncaptured_inbound_calls`

**Status:** accepted · **Date:** 2026-08-03 · **Milestone:** post-S-1
improvement plan, §1.1

## Context

S-1's benchmark (ADR-0019, `bench/`) found the sharpest gap in the
whole measurement on T6: `deps load --dir in` returned 5 real but
irrelevant edges (the function's own unit tests, which call it
same-file) and **zero** of the 6 real production callers — all
path-qualified `graph::load(...)` calls, excluded by ADR-0008's
"deliberately modest" v1 policy. The response contained no signal that
anything was excluded; `root_unresolved_calls` only ever covered the
query symbol's own *outbound* calls, never inbound calls the extractor
never attempted to capture at all. An agent that trusted this response
without falling back (as both the grep arm and, by choosing to verify,
the carto arm did) would ship a wrong answer with high apparent
confidence — the exact failure INV-8 exists to prevent, except INV-8 as
implemented only covered *presence* (every edge carries a confidence),
never *absence*.

`docs/post-s1-improvement-plan.md` §1.1 named this the highest-priority
item in the whole plan for exactly that reason: it's a direct extension
of a principle the product already claims to have, not a new one.

## Decisions

- **Count, don't resolve.** This does not attempt path-qualified call
  resolution (a real, larger project — see the plan's own scoping
  note). It only counts call sites the Rust extractor already walks
  past today (`Type::method()`/`module::func()`, a `call_expression`
  whose `function` is a `scoped_identifier`) — a new `calls.scm`
  pattern (`call.uncaptured`) captures them into
  `ExtractOut::uncaptured_call_sites`, a list carried *only* to be
  counted, never to produce an edge or feed any resolution tier.
- **Aggregated repo-wide by bare callee name, keyed by `(origin,
  name)`.** `resolve.rs` builds `uncaptured_by_name` the same way
  `fqn_to_file`/`known_namespace_roots` are already keyed by
  `(fe.origin, ...)` — so a Rust `graph::load()` call site never
  inflates a Python symbol also named `load`. Every symbol node with
  that bare name gets the *same* count; the field measures "how much
  unattempted syntax exists in this repo for this name", not "how many
  callers does this specific symbol have" — deliberately, since
  distinguishing which of several same-named symbols a path-qualified
  call actually targets is exactly the unsolved problem this item
  doesn't attempt to solve.
- **Per-symbol, not per-call-site.** Storing every individual
  uncaptured call site on every candidate symbol would be a much larger
  payload for no additional decision-relevant information (carto's own
  repo has hundreds of `Type::method()`-shaped call sites); a single
  `u32` answers the only question this item exists to answer: "is
  `--dir in`'s apparent silence real, or is it that carto didn't look
  here?"
- **Surfaced on `deps` only, not `where`/`map`.** `root_uncaptured_inbound_calls`
  mirrors `root_unresolved_calls`'s existing precedent exactly (root-only,
  not per-hop) — it answers the specific T6 pain point without bloating
  every row of a possibly-large traversal.
- **Rendered only when `dir` is `in`/`both`, never `out`.** The count is
  about inbound call sites; a `--dir out` question never consults it,
  so printing the note there would be pure noise. The structured
  (`--json`/MCP `structuredContent`) field always carries the true
  value regardless of `dir` — only the *rendered text* line is gated,
  the same "structured field exact, rendered text can economize" split
  `map`'s `counts` vs. `lines` already uses.
- **`SCHEMA_VERSION` bumped 1 → 2.** `SymbolNode::uncaptured_inbound_calls`
  carries `#[serde(default)]` so a v1 `graph.json` still *parses*
  cleanly (rather than a raw missing-field deserialization error) — but
  `graph::load`'s schema-version check runs immediately after parsing
  and rejects a v1 document with its existing "re-run `carto index`"
  message before any query code ever sees a document carrying this
  field. Without the version bump, a stale v1 index would silently
  report `0` for every symbol — precisely the dishonesty this item
  exists to remove; the bump is what makes the `#[serde(default)]`
  convenience safe rather than a second silent-absence bug introduced
  while fixing the first one.

## Consequences

- Extends to every language whose extractor declares an equivalent
  exclusion in the future — today only Rust's `RustExtractor` populates
  `uncaptured_call_sites`; every other extractor (`csharp.rs`, `ecma.rs`,
  `go.rs`, `php.rs`, `python.rs`) explicitly sets it to `Vec::new()`
  with a one-line note on *why* that language has nothing to count
  (PHP/TS-JS/Go/C# all capture their path-qualified/member/selector
  calls; Python's grammar has no node distinguishing a module-qualified
  call from an instance call at all, so there's no separate shape to
  exclude in the first place).
- Verified against carto's own repo: `carto deps load . --dir in` now
  reports "6 call sites... never attempts to resolve" alongside the
  same 5 same-file test callers — the count matches T6's own cited real
  production caller count exactly.
- `docs/STATUS.md`'s "Rust path-qualified call resolution" entry under
  "deliberately absent" gets a pointer to this field — the exclusion
  itself is unchanged; what's new is that its consequence is now
  visible rather than silent.

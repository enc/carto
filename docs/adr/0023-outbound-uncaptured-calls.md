# 0023 — Honest absence signal, outbound half: `uncaptured_outbound_calls`

**Status:** accepted · **Date:** 2026-08-03 · **Milestone:** post-S-1
improvement plan follow-up, T8

## Context

A real benchmark batch (`bench/results/20260803T220826/`) graded T8
("What does `carto_core::indexer::build_and_persist` call or depend on
directly?") `partial`: the `cli` session stopped at `deps
build_and_persist --dir out`'s answer — 4 resolved edges plus
`root_unresolved_calls: [len, clone, Ok, to_path_buf]` — and never went
to source, missing all 6 real path-qualified calls
(`pathguard::PathGuard::new`, `walk::walk`, `lang::extract_and_resolve`,
`Graph::new`, `gitinfo::head_sha`, `graph::persist` — confirmed against
`indexer.rs:36-77` directly; literally the entire substance of what the
function does).

Checked directly: the response carried **no signal at all** that
anything beyond the 4+4 calls existed. `root_uncaptured_inbound_calls`
(ADR-0020) was present but answered the wrong question — it counts
uncaptured call sites *elsewhere in the repo that call this symbol*
(inbound), not uncaptured call sites *inside this symbol's own body*
(outbound). ADR-0020 built the honest-absence mechanism for T6's
direction (inbound); this is its missing outbound half, and it was
always symmetric — `ExtractOut::uncaptured_call_sites` already carried
everything needed for both directions, `resolve.rs` just never
attributed it by containment, only by name.

## Decisions

- **Reuse `assign_calls_to_innermost_symbol`, not a new mechanism.**
  `resolve.rs` already calls this on `fe.extract.call_sites` (the
  *attempted* calls) to attribute each real call to the innermost
  symbol containing it — a call inside `Symbol::method` doesn't also
  land on `Symbol`'s own enclosing class (PHP/Python's contains-its-
  methods'-bodies shape, ADR-0012). Calling the exact same function on
  `fe.extract.uncaptured_call_sites` gives, per symbol, the uncaptured
  call sites *inside that symbol's own range* — no new extraction
  logic, no new `RawCallSite` field, and the same innermost-containment
  correctness guarantee real calls already have.
- **Outbound is containment-keyed; inbound stays name-keyed — genuinely
  different questions, not two views of one number.**
  `uncaptured_inbound_calls` (ADR-0020) asks "how much unattempted
  syntax in this repo spells *this bare name*" (repo-wide, `(origin,
  name)`-keyed — every same-named symbol shares one count).
  `uncaptured_outbound_calls` asks "how much unattempted syntax is
  *inside this symbol's own body*" (per-symbol, by line-range
  containment — no name lookup at all, and no risk of a same-named
  sibling symbol borrowing another's count the way the inbound field
  deliberately does). A recursive function calling itself via a
  qualified path can contribute to both counts on the same symbol —
  correct, not a double-count of the same fact.
- **Count, don't resolve — identical scope discipline to ADR-0020.**
  Still doesn't identify path-qualified calls' targets, only how many
  exist per symbol; resolving them is the same materially bigger,
  explicitly out-of-scope project ADR-0020 already deferred.
- **`SCHEMA_VERSION` bumped 2 → 3**, same justification as ADR-0020's
  1 → 2 bump: `#[serde(default)]` lets a stale v2 `graph.json` parse
  cleanly, and the bump is what stops that parse from ever reaching
  query code with a fabricated `0` — `graph::load`'s version check
  rejects it first, with "re-run `carto index`".
- **Rendered only when `dir` is `out`/`both`, mirroring the inbound
  field's `in`/`both` gate exactly** — `--dir in` never consults an
  outbound count, so printing it there would be pure noise. `--dir
  both` can and should show both notes together when both counts are
  nonzero; that's correct, not conflicting, since `both` genuinely
  traverses both directions.

## Consequences

- Verified against carto's own repo: `carto deps build_and_persist .
  --dir out --json` now reports `root_uncaptured_outbound_calls: 6` —
  the exact count of real path-qualified calls in `build_and_persist`'s
  body, matching T8's ground truth precisely — and the human-rendered
  form prints a note naming it, the same mechanism that made T6/T7 land
  on the fully correct answer once the inbound count existed.
- `docs/STATUS.md`'s ADR-0020 milestone entry and its "Rust
  path-qualified call resolution" deliberately-absent bullet both note
  the new outbound count alongside the existing inbound one.
  `skill/carto.skill.md` and `bench/cli-arm-prompt.md`'s existing
  "known gaps" bullet about `root_uncaptured_inbound_calls` gains a
  sibling sentence about the outbound count for `--dir out`/`both`
  questions.

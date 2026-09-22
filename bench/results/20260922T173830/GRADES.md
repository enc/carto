# Grading rationale — 2026-09-22 S-1 re-run

Graded against `bench/tasks.md`'s "2026-09-22 re-verification" section
(re-derived ground truth, not the original 2026-08-03 numbers), by
direct comparison — never by trusting carto's own claimed answer for
what the ground truth is. `correct` / `partial` / `wrong`, matching
`bench/tasks.md`'s own scoring definition.

## L1 — repo orientation (zed)

**grep/cli/carto: correct.** All three identify a large Rust workspace
(245 crates), name real top-level pieces (gpui, editor, agent stack,
collab, etc.) with materially correct detail. carto's MCP answer says
"~280" crates (a looser approximation than the exact 245 grep/cli
state precisely) but is still order-of-magnitude correct — the task's
own bar. The carto arm also self-reported a real carto rough edge
worth a follow-up: `map`'s entry-points section came back empty at
budgets 60/120/200 against this corpus.

## L2 — symbol lookup (zed)

**grep/cli/carto: correct.** All three find `util.rs:95`, exact
correct signature.

## L3 — import fan-out (carto's own repo)

**grep: partial.** Found all 24 real importers, but also listed
`lib.rs`/`tools/mod.rs` as importers — the exact doc-comment-only false
positive `bench/tasks.md`'s own 2026-08-03 write-up already warned is
a trap (both original sessions made this same mistake).

**cli: correct.** Exactly 24, zero false positives, zero misses,
correctly caveats the wildcard/alias-import gap.

**carto (MCP): correct.** Exactly 24, zero false positives, zero
misses. A complete reversal from the original "11 of 18, missing 7" —
directly attributable to ADR-0022 (grouped-`use` extraction) and
ADR-0021 (Module/File discovery, which also fixed the original
write-up's separate "unreachable via the actual MCP tool surface"
finding — `where`/`deps` now resolve `carto_core` by name with no
node-ID indirection).

## L4 — real entry points vs. carto's heuristic (carto's own repo)

**grep/cli/carto: correct.** All three identify the 4 real entry
points. Both carto-backed arms explicitly surface and correctly
explain the noise problem (~410 rows now, up from ~130) rather than
reading the heuristic list uncritically — the actual bar this task
sets.

## T5 — direct-caller impact (carto's own repo)

**grep/cli/carto: correct.** Ground truth is now 7 production callers
(was 4 — 3 new sites from the contracts feature). All three arms
identify exactly these 7 and correctly exclude all 9 test-only sites.
grep's answer is notably thorough here, independently flagging that
its own count differs from what's on record for the original benchmark
and correctly attributing why.

## T6 — direct-caller impact, the sharp negative case (carto's own repo)

**grep/cli/carto: correct.** Ground truth is now 10 real call sites
(was 6). All three name them all. The load-bearing change: both
carto-backed arms explicitly noticed `root_uncaptured_inbound_calls:
15`, explained *why* (path-qualified calls never attempted, per
ADR-0008), and independently verified via source grep before
answering — the exact failure this task was built to catch ("carto
silently returns a plausible-looking but beside-the-point answer") no
longer occurs, because the honest-absence signal (ADR-0020/0023) now
prompts the verification step the original write-up found missing.

## T7 — incoming calls with confidence (zed)

**grep: partial.** Missed one real caller
(`activity_indicator.rs:755`, inside `render`) despite a thorough,
independently-reasoned confidence analysis (grep interpreted
"confidence" as call-site safety against a `debug_assert!`, a
different but defensible reading of an ambiguous prompt — noted for
future task-wording, not held against the grade beyond the actual
recall miss).

**cli/carto: correct.** Both achieve complete 17/17 recall: 8 resolved
edges, 8 honestly-flagged via `root_uncaptured_inbound_calls: 8`
(path-qualified, matches grep exactly), plus 1 more
(`agent_ui/agent_diff.rs:686`) found via their own follow-up grep after
noticing the flagged count didn't fully reconcile. **This last one is a
genuine new finding, not previously known**: a bare, unqualified call
carto's extractor silently misses with *no* signal at all — outside
the documented ADR-0008 path-qualified exclusion. The grep arm's
diagnosis (the call sits inside a `format!(...)` macro argument,
plausibly unwalked by `calls.scm`) is more specific and more likely
correct than the carto arm's own guess (a same-name-collision
hypothesis). **Worth a real follow-up investigation as a possible
extraction gap**, independent of this benchmark.

## T8 — outgoing dependencies (carto's own repo, self-authored ground truth)

**grep/cli/carto: correct.** Ground truth changed the most of any
task — `build_and_persist` was directly modified by the multi-root
support work in the six weeks since the original write-up. All three
arms correctly reflect the function's *current* body, not stale
assumptions. Both carto-backed arms explicitly use the new
`root_uncaptured_outbound_calls: 7` signal to report the previously-
invisible third bucket (path-qualified calls, never attempted) as an
honest count rather than silence — the original "sharpest possible
three-way split... invisible by construction" finding no longer holds;
all three buckets (7 resolved / 9 honestly-unresolved / 7 honestly-
uncaptured) are now visible in carto's own output.

## Summary

| | grep | cli | carto (MCP) |
|---|---:|---:|---:|
| Accuracy | 87.5% (7/8) | 100% (8/8) | 100% (8/8) |
| Combined input tokens | 309,061 | 330,255 | 357,824 |

Accuracy is a full reversal from the original run (grep 93.8%, carto
87.5%) — carto-backed arms now *outscore* grep-only, by a wide margin,
having gone from carto's worst dimension to its best. Token cost moved
only slightly (carto 15.8% more than grep, was 18.0% more) — neither
S-1 threshold (≥30% fewer tokens, ≥20 points higher accuracy) is met on
this one-trial measurement, same as the original, but for a
substantively different reason this time: correctness is no longer the
binding constraint, token footprint is. The `cli` arm matches `carto`
(MCP)'s accuracy exactly while using 7.7% fewer tokens — real evidence
that MCP's own transport overhead, not carto's answers (identical
either way), costs something in practice.

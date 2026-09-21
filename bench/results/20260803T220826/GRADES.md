# Grading reasoning — first run after pre-indexing, 20260803T220826

Two-arm run (`grep`, `cli`), after porting `bench/replay/replay.sh`'s
pre-index convention into `bench/run.sh` (see `bench/field-log.md`'s
"Part E" entry). Graded the same way as every prior batch: read each
session's `result` text against `bench/tasks.md`'s ground truth.

## The pre-index fix worked completely

**Zero `cli`-arm denials, across all 8 tasks** — down from 5, 10, and 5
in the three prior batches. The only denial in this entire batch was
one `grep`-arm `for` loop (`for f in $(find crates -maxdepth 2 -name
Cargo.toml); do ...; done` on L4) — same residual pattern that's
persisted every batch, since `grep` only ever got a prompt suggestion,
never a structural tool restriction. Turn counts dropped sharply too
(e.g. L1.cli: 5 turns, down from 8–10; L3.cli: 2 turns, down from 4–11)
— every `cli` session went straight to `where`/`deps`/`map` against the
pre-built index, with `index` itself both off the tool list and never
attempted.

**Token/cost, cleanly comparable for the first time (no denial-driven
retries confounding either arm)**: cli uses 20.0% fewer combined input
tokens than grep (168,786 vs 210,937) and costs 9.5% less ($1.25 vs
$1.39). This is the result the whole `cli`-arm question was trying to
measure, now visible without the harness's own friction obscuring it.

## But accuracy regressed this trial — a real, disclosed miss, not spun away

| | correct | partial | wrong | score |
|---|---:|---:|---:|---:|
| grep | 7 | 1 | 0 | 7.5/8 = 93.75% |
| cli | 6 | 2 | 0 | 7.0/8 = 87.5% |

This is **worse** for `cli` than the immediately prior batch (100%),
and this time `cli` falls *below* `grep`, not level with or ahead of
it. Nothing about the pre-index change explains this — both regressions
trace to the agent choosing not to do a verification step it did in
the prior trial, exactly the "one trial per cell" variance
`bench/tasks.md` has flagged as a real limitation from the start.

### T8 — outgoing dependencies — the more material miss

Ground truth: 6 path-qualified calls (`pathguard::PathGuard::new`,
`walk::walk`, `lang::extract_and_resolve`, `Graph::new`,
`gitinfo::head_sha`, `graph::persist` — the actual substance of what
`build_and_persist` does) plus `Graph`'s own receiver-method calls.

**cli: partial.** Reported only the 4 low-signal receiver-method edges
`deps --dir out` resolved (`redact::total`, `pathguard::out_root`,
`graph::insert_edge`, `graph::insert_node`) and treated the rest of
`deps`'s `unresolved_calls` as fully explained by stdlib noise
(`len`/`clone`/`Ok`/`to_path_buf`). It never went to source to find the
6 real path-qualified calls — the prior batch's `cli` session did
exactly that (explicitly listed all 6 by name, sourced from
`indexer.rs` directly) on this same task. This time the session
stopped at carto's own incomplete `deps` answer without the follow-up
read `bench/tasks.md`'s own grading philosophy calls for ("a
technically-accurate-but-silent omission... is a worse outcome than an
honestly-flagged gap") — the six most structurally important calls a
function this central makes are simply missing from the answer, with
no signal that anything's missing on the *outbound* side specifically
(the session correctly dismissed `root_uncaptured_inbound_calls: 2` as
irrelevant — accurate, but doesn't substitute for checking the
outbound direction).

**grep: correct**, comprehensive, all 6 path-qualified calls plus the
`Graph` methods, matching every prior batch's grep answer for this
task.

### L3 — import fan-out — both arms partial this trial, a repeat of a known trap

Ground truth: **19** real importers. Both arms answered with the exact
same *kind* of over-count seen in the very first S-1 batch: including
`crates/carto-mcp/src/lib.rs` and `tools/mod.rs` (doc-comment-only
mentions of `carto_core`, never real imports) as real importers.

**grep: partial** (21 — 6 + 9-with-2-false-positives + 6).

**cli: partial** (22 — same 2 false positives as grep, *plus* a third:
`crates/carto-core/src/query/find.rs`, which `bench/tasks.md`'s ground
truth explicitly names as a string-literal test fixture, not an
import). This session's own text shows why: it explicitly says "grep
matched string carto_core anywhere, not confirmed use lines only" —
this trial's `cli` session ran a grep-style text search rather than
querying carto's own `imports` edge data (which, being extracted from
real syntax, can't be fooled by a doc comment or a string literal). The
immediately prior batch's `cli` session took the opposite approach on
this exact task and got 19/19 exactly by doing that cross-check —
confirming this is genuine per-trial strategy variance, not a
regression in carto's own data or a consequence of pre-indexing.

## The other 5 tasks — no notable change from prior batches

L1, L2, L4, T5, T6, T7: both arms correct, consistent with every prior
batch's grading for these tasks. T7 in particular repeated the same
strong pattern (`cli` using `root_uncaptured_inbound_calls: 9` then
grep-confirming the 7 missed path-qualified-caller files exactly).

## Takeaway

The pre-index fix (this session's actual deliverable) is validated: it
eliminated `index`-related friction entirely and produced the first
batch where cli's token/cost advantage is visible without denial noise
confounding it. Accuracy is a **separate, orthogonal finding** this
trial — a real dip driven by the agent skipping a verification step on
two tasks, not by anything the pre-index change touched. Consistent
with every batch so far: single-trial accuracy swings by more than the
effect any one fix is trying to measure, and that's exactly why
`bench/tasks.md` frames every number here as directional, not
conclusive, on a single trial.

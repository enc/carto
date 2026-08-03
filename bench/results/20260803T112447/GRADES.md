# Grading reasoning — S-1 real-run batch 20260803T112447

Graded by reading each session's actual `result` text
(`transcripts/<task>.<arm>.txt`) against `bench/tasks.md`'s ground truth,
after fixing two harness bugs discovered along the way (see
`bench/field-log.md`): the neutral-cwd failure (fixed by running each
session with cwd = the real corpus) and an isolated MCP-connection flake
on the first L1/carto attempt (fixed by a targeted single-session re-run,
disclosed in the field log).

`correct` = 1.0, `partial` = 0.5, `wrong` = 0 for the accuracy percentage
in the final writeup.

## L1 — repo orientation (zed)

**grep: correct.** 236 crates, ~1.34M LOC, correctly named all the major
architectural groupings (gpui, text/editing core, project model,
collaboration, agent/AI, extensibility, dev tooling).

**carto: correct** (after the isolated re-run — the first attempt never
connected to the MCP server at all and is discarded, not counted here).
236 crates, exact structural counts from `map` (3,754 files, 58,638
symbols... — matches Part 0's own independently-measured figures),
correctly named the same architectural groupings via a different lens
(import fan-out ranking rather than manual crate-by-crate reading).

## L2 — symbol lookup (zed)

**grep: correct.** Exact file:line, exact signature, matches ground
truth verbatim.

**carto: correct.** Same, via `where`.

## L3 — import fan-out (carto's own repo)

**Both: partial** — and this required correcting my own ground truth a
second time. My original count (18, via `rg -l "^use carto_core::"`) was
itself wrong: it missed `main.rs` (which references `carto_core::` via
fully-qualified inline paths, e.g. `carto_core::Result<u8>`, never a
`use` statement) and initially I mis-tallied the true corrected count as
21 to match both models' answers — but `carto-mcp/src/lib.rs` and
`carto-mcp/src/tools/mod.rs` were then found to reference `carto_core::`
**only inside `//!` doc comments**, not real code — the exact
prose-vs-code distinction the original (pre-correction) draft of this
task predicted as a risk. True count: **19** real importers. **Both
grep and carto's real-run answers said 21**, incorrectly including those
same 2 doc-comment-only files as real importers — an identical,
shared false-positive pattern in both arms, not a carto-specific
weakness. Graded partial for both: 19/19 real files correctly found, 2
shared false positives.

## L4 — real entry points vs. carto's heuristic (carto's own repo)

**grep: correct.** All 4 real entry points (1 bin, 3 lib roots),
correctly reasoned from `Cargo.toml` `[[bin]]` + `lib.rs` locations.

**carto: correct.** Also found all 4, and explicitly named and worked
around `map`'s documented heuristic-noise limitation ("carto's `map`
entry-point heuristic is noisy here... filtering to what's real") —
exactly the behavior `bench/tasks.md` said this task would grade on.

## T5 — direct-caller impact (carto's own repo)

**grep: correct.** All 4 real production callers (`where_cmd.rs`,
`where_tool.rs`, `redact::redact_tainted_string`, and `TaintedString`'s
own `Serialize` impl in `taint.rs`), correctly excluded the 9 test-only
call sites with a stated reason.

**carto: partial.** Found only 2 of 4 (`where_cmd.rs`, `where_tool.rs`)
and explicitly asserted "no other production call sites," missing
`redact_tainted_string` and the `Serialize` impl entirely — and
misdescribed the `Serialize` impl's call as "the method definition
itself" (a real misreading: `taint.rs` contains both the definition
*and* a separate caller in the same file). Notable because carto's own
`deps` tool, verified independently while writing `bench/tasks.md`,
*does* return all 9 real edges (including these 2) at `--depth 1` — this
looks like the agent's interpretation of carto's output falling short,
not a gap in carto's own data.

## T6 — direct-caller impact, the sharp negative case (carto's own repo)

**grep: correct.** All 6 real call sites, correctly distinguished from
doc-comment mentions.

**carto: correct** — and the most instructive result in the whole set.
The agent explicitly recognized carto's known limitation ("carto's own
`deps` graph... doesn't show these 6 sites because they're
module-qualified calls... a call shape the Rust extractor doesn't
capture") and used grep to verify instead of trusting carto's
(materially wrong, per `bench/tasks.md`'s own earlier verification)
answer. Exactly the honest fallback behavior the benchmark's "steel-man
the baseline, let carto fall back" design hoped an agent would exhibit.

## T7 — incoming calls with confidence (zed)

**grep: correct** (arguably exceeds the bar) — found all 18 real call
sites and additionally flagged one genuine latent bug (an unbounded
`health_str.len()` that can violate `truncate_and_trailoff`'s
`debug_assert!(max_chars >= 5)`), a level of insight beyond the task's
own ground truth.

**carto: correct.** The agent used grep for this entire task rather than
carto's `deps` tool (explicitly: "grep found all of them... this is as
close to ground truth as static analysis gets, carto or not") and found
all 18 real call sites. A second clean instance of the same fallback
pattern as T6.

## T8 — outgoing dependencies (carto's own repo, self-authored ground truth)

**grep: correct.** All 6 path-qualified dependencies plus the meaningful
`Graph` method calls, correctly reasoned as not depending on `query`/
`carto-cli`/`carto-mcp`.

**carto: correct.** Same substance, and again explicit about carto's own
tool limitation ("carto's `deps` query on this symbol only surfaced 4
low-confidence... edges and flagged [4 more] as unresolved... so I read
the source directly").

## Accuracy tally

| | correct | partial | wrong | score |
|---|---:|---:|---:|---:|
| grep | 7 | 1 | 0 | 7.5/8 = 93.75% |
| carto | 6 | 2 | 0 | 7.0/8 = 87.5% |

S-1's accuracy bar is "≥20% higher than grep-only." Measured here: carto
is **lower**, not higher — driven entirely by T5's partial miss (the one
task where the agent trusted an incomplete read of carto's own tool
output instead of falling back, unlike T6/T7/T8's clean fallback
pattern). One trial per cell; see the main writeup for how much weight
this single data point should carry.

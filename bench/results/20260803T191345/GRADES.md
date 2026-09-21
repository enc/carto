# Grading reasoning — clean re-run after Bash-invocation-style harness fix, 20260803T191345

Two-arm run (`grep`, `cli`), run from the user's own terminal (not
nested inside another Claude Code session) after fixing
`bench/run.sh`'s default `CARTO_REPO` path bug and adding
`BASH_STYLE_NOTE` guidance to all three arms (see `bench/field-log.md`).
Graded the same way as prior batches: read each session's `result`
text against `bench/tasks.md`'s ground truth.

## Accuracy tally

| | correct | partial | wrong | score |
|---|---:|---:|---:|---:|
| grep | 7 | 1 | 0 | 7.5/8 = 93.75% |
| cli | 8 | 0 | 0 | 8.0/8 = 100% |

**This is the first batch where cli/carto actually beats grep on
accuracy**, not just ties it. The divergence is entirely L3.

## L3 — import fan-out (carto's own repo) — the deciding task

Ground truth: **19** real importers (`bench/tasks.md`'s corrected
count).

**grep: partial.** Answered **21** — repeating the *exact* false-positive
pattern the original S-1 batch's `GRADES.md` already documented and
named: incorrectly counting `crates/carto-mcp/src/lib.rs` and
`crates/carto-mcp/src/tools/mod.rs` as real importers, when both only
mention `carto_core` inside `//!` doc comments, never in actual code.
Grep-based text search can't distinguish "the crate name appears in this
file" from "this file has a real import" without the same careful
secondary read the original task-authoring process needed — and this
session's `grep` arm didn't do that read, landing on the same wrong
answer a from-scratch grep search landed on before.

**cli: correct.** Exactly 19/19, and explicitly named and excluded the
same two doc-comment-only files by name ("Two extra grep hits —
`tools/mod.rs`, `lib.rs`, `query/find.rs` — no real import, just
`carto_core` name in doc-comment text"). This is carto's structural
`imports` edges working as designed: they're extracted from real `use`/
qualified-path syntax, never from a text scan of comments, so the exact
false positive that traps a grep-only approach doesn't exist in carto's
data at all. Direct evidence for `bench/tasks.md`'s own framing:
"L3's outcome says something about the skill file, not carto's data" —
in this run, having carto's data available (and the `cli` arm's prompt
prompting the agent to cross-check grep hits against it) is exactly
what avoided the mistake.

## The other 7 tasks — both arms correct, no notable divergence

- **L1** (zed orientation): both correct, 236 crates, comparable
  architectural summaries.
- **L2** (symbol lookup): both correct, trivial exact match.
- **L4** (entry points): both correct, all 4 real entry points; `cli`
  additionally named and filtered `map`'s heuristic noise explicitly.
- **T5** (direct callers, `render_capped`): both correct, all 4
  production callers, test-only sites correctly excluded by both.
- **T6** (sharp negative case, `graph::load`): both correct, all 6 real
  call sites; `cli` explicitly used `root_uncaptured_inbound_calls: 6`
  (matching ground truth exactly) before confirming with grep.
- **T7** (zed, confidence-labeled callers): both correct — comprehensive
  coverage of all 12 real external-caller files (5 bare + 7
  path-qualified, excluding `util.rs`'s own unit tests from the
  "who calls this in the app" framing, which both arms reasonably do);
  `cli` explicitly labeled which edges were carto-inferred vs.
  grep-confirmed, matching the prompt's own confidence framing.
- **T8** (outgoing dependencies): both correct, full call list; `cli`
  additionally surfaced `root_uncaptured_inbound_calls: 2` and correctly
  explained it as irrelevant to an outbound-dependency question.

## Token/cost: cli costs more this run, and it's mostly attributable to a new, identified friction source

`bash bench/score.py bench/results/20260803T191345/`: grep totals
209,361 combined input tokens / $1.45; cli totals 238,321 / $1.84 — cli
uses ~14% more tokens and ~27% more dollars overall, despite the
accuracy win.

**The Bash-invocation-style fix from the previous run worked for its
original target**: zero permission denials on L1, L2, L3, T6, T7's
`cli` sessions (previously 5+ denials across these same tasks in the
prior two batches, mostly piped/compound commands). But a **new**
denial pattern showed up on L4 (5 denials), T5 (2), and T8 (3) — every
one a **plain, single-command output redirection** (`carto ... --json >
/tmp/x.json`, then retried against `.carto-entry.json` in the repo, then
`"$TMPDIR/..."` — all three target paths denied). This is the harness's
own `cli-arm-prompt.md` fallback advice from the last fix ("redirect to
a file... and read that file") backfiring — `>` redirection is
apparently treated as a filesystem-write action needing its own
permission, regardless of target path, separately from whatever `Bash(
<program>:*)` pattern is declared. That advice is being removed (see
`bench/field-log.md`'s entry on this run) rather than patched further,
since after three real batches the pattern is now: **any shell
operator at all (`|`, `>`, `&&`, `;`, a loop) risks a denial in this
environment — the only reliably clean shape is a single, bare command
with no redirection of any kind.**

Excluding the 10 denial-driven retries (L4/T5/T8's extra turns), the
per-task cost picture would likely look closer to the earlier clean
batch's mixed pattern (cli cheaper on some tasks, grep cheaper on
others) — this batch's aggregate cli-cost premium is not strong
evidence about carto-via-CLI's inherent efficiency, the same caveat as
every batch so far.

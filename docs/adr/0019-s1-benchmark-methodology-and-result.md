# 0019 — S-1 benchmark: methodology and result (spec §1.5, §10, §11.4)

**Status:** accepted (result recorded; **M3 gate decision is the user's,
not made here** — spec §11.4: "the implementing agent MUST present
results and wait") · **Date:** 2026-08-03 · **Milestone:** the S-1
measurement spec §10 requires before M3 can start.

## Context

Spec §1.5 states S-1 as a measurable success criterion: *"agent-with-carto
answers structural questions with ≥30% fewer input tokens and ≥20% higher
accuracy than agent-with-grep-only. Measured before M3 is started."*
Spec §10 makes it a hard gate: *"gate: run §11.4 benchmark before
starting; if S-1 fails on M1+M2 capabilities, stop and reassess product
direction with the user rather than proceeding."*

Until this slice, the benchmark had never been built or run — eight
languages and eighteen prior ADRs in, the product's core value hypothesis
was unvalidated. This ADR records how it was finally measured, what the
measurement found, and — per spec §11.4's explicit instruction —
presents the result to the user rather than making the M3 go/no-go
decision unilaterally.

## What was built (ADR-0018's companion work)

Spec §11.4's benchmark needs a real agent-facing surface to measure
honestly, not the README's old CLAUDE.md-snippet-and-Bash-tool stopgap —
so this slice pulled the MCP server (normally M4 scope) ahead of M2's
remaining work and M3, built it (`crates/carto-mcp`, ADR-0018), then
built and ran the benchmark against it:

- **`bench/tasks.md`** — 8 tasks (spec calls for 10, over
  `fixtures/mixed` + `tf-app`; neither is large enough or exists yet —
  see the file's own reasoning for running fewer, more rigorously
  ground-truthed tasks over two real corpora instead: carto's own repo
  and `~/playground/zed`, agreed with the user). Split into Category L
  (lookup/orientation) and Category T (transitive/blast-radius), based on
  a real measurement (`bench/field-log.md`'s Part 0): carto's Rust
  call-resolution rate is ~22.5% on both corpora, a direct consequence of
  ADR-0008's deliberate exclusion of path-qualified calls.
- **`bench/replay/replay.sh`** — a free, deterministic byte-count arm
  (no LLM). Real result: carto used ~2x more bytes than a hand-authored
  grep baseline, driven substantially by `map --json` always returning
  its entire rendered overview with no way to request one section — a
  real API-shape cost, not just an artifact of an unrealistically lean
  grep baseline.
- **`bench/run.sh`** — the real-run arm, 8 tasks × 2 arms × 1 trial via
  `claude -p --output-format json`, capturing each session's exact
  `usage` block.
- **`bench/score.py`** — aggregates combined input tokens (fresh +
  cache-creation, reported separately from cache-read) and, given a
  `GRADES.json`, an accuracy percentage per S-1's own two-part
  definition.

## Two real harness bugs, found and fixed before trusting anything

Both are recorded in full in `bench/field-log.md`; summarized here
because they're load-bearing for how much confidence to place in the
result below.

1. **The first full 16-session batch was invalid for 5 of 8 tasks.**
   `bench/run.sh` originally ran every session from a neutral `mktemp`
   directory, pointing at the target repo only via prompt text — no
   `cd`, no `--add-dir`. This worked by accident for zed (the model
   happened to issue absolute-path commands that succeeded) and failed
   completely for every task using carto's own repo as corpus: both
   arms reported the working directory as empty, in 2–4 turns with zero
   exploration attempted. Cost of running this invalid batch: $1.51, on
   top of an earlier $0.68 spent discovering that `--safe-mode` (an
   attempted `--bare` substitute, since `--bare` requires
   `ANTHROPIC_API_KEY`, unset in this environment) disables MCP servers
   entirely. **Fix:** run each session with cwd set to the actual
   corpus directly. Confirmed with one cheap diagnostic before
   re-running the full paid batch. The user was informed of this cost
   and confirmed proceeding with a full re-run rather than a partial
   one, given the value of an apples-to-apples 16-session comparison.
2. **One isolated session in the fixed batch (L1's carto arm) never
   connected to the MCP server**, silently falling back to plain file
   reading and producing a plausible-looking answer anyway — caught only
   by reading every carto-arm transcript for "not connected" language
   before trusting the aggregate, and confirmed isolated (not systemic)
   before spending on a fix. Re-ran that one session alone ($0.16); the
   MCP tools connected correctly the second time and the answer cited
   exact figures straight from `map`'s real output.

Neither bug is subtle in retrospect, and neither was caught by watching
`score.py`'s aggregate number — both were caught only by reading the
actual session transcripts before trusting the numbers derived from
them. That discipline (verify the underlying evidence, not just the
summary statistic) is the same one `bench/tasks.md`'s own "methodology
correction" section and ADR-0017's dogfooding section already
established as this codebase's house style; this ADR is a third
instance of it, at a larger scale.

## Result

Full per-task numbers, reasoning, and every raw session transcript:
`bench/results/20260803T112447/` (`score.py`'s output, `GRADES.md`'s
per-task grading reasoning, `transcripts/*.txt` for the actual answer
text). Headline:

| Metric | grep-only | carto | S-1 threshold | Met? |
|---|---:|---:|---|:---:|
| Combined input tokens | 1,323,356 | 1,329,234 | carto ≥30% fewer | **No** — 18.0% *more* |
| Accuracy (8 tasks, correct=1/partial=0.5/wrong=0) | 93.8% | 87.5% | carto ≥20 points higher | **No** — 6.2 points *lower* |
| Real dollar cost | $1.43 | $1.20 | — | carto ~16% cheaper (cache-read pricing), despite more tokens |

**Neither S-1 threshold is met, on this one-trial measurement.** Per
`bench/tasks.md`'s own explicit caveat, one trial per task/arm cell
cannot separate a real effect from run-to-run variance — this is a
directional data point, not a confidence interval, and the per-task
pattern underneath the aggregate carries more information than the
headline numbers alone:

- **3 of 8 tasks are clean carto wins on tokens** — L1 (whole-repo
  orientation) by 74% fewer tokens (9,327 vs. 36,084), T5 and T7 more
  modestly. L1's margin alone is large enough that the aggregate's sign
  is sensitive to task mix, not a stable property of "carto vs. grep" in
  general.
- **The single accuracy loss (T5) has a specific, legible, and
  non-damning cause**: the agent read carto's own tool output
  *incompletely* — missed 2 of 4 real callers of
  `TaintedString::render_capped`, including misreading one call site as
  "the definition itself" — not a case where carto's underlying data was
  wrong. `bench/tasks.md`'s own independent verification confirms all 4
  real callers are present in carto's actual `deps` output at that exact
  query.
- **The three hardest transitive-call tasks (T6, T7, T8) are all correct
  for carto, via a real, repeatedly-observed fallback pattern**: every
  one of those three transcripts explicitly names carto's known,
  documented limitation on that call shape (Rust path-qualified calls,
  ADR-0008) and falls back to reading source directly — landing on the
  fully correct answer rather than reporting carto's incomplete data as
  if it were complete. This is the single most encouraging qualitative
  finding in the whole exercise: an agent with both tool sets available
  used carto when it helped and correctly stopped relying on it when it
  didn't.
- **L3's outcome implicates the skill file/task framing, not carto's
  data**: both arms made an identical doc-comment-vs.-real-import
  mistake, independent of which tools were available.

## What this ADR does *not* decide

Spec §11.4 is explicit: *"The M3 gate decision is made by the user on
this data — the implementing agent MUST present results and wait."*
This ADR records the measurement and its limitations; it does not
conclude "M3 is a go" or "M3 is a no-go." That decision, and what to do
about the token-cost pattern above (in particular `map`'s
whole-payload-every-time API shape, which the replay arm identified as a
real, fixable contributor independent of the LLM-side numbers), is the
user's to make.

## Consequences

- `docs/STATUS.md`'s milestone-position section is updated to record
  this result and that M2's remaining scope / M3 are paused pending the
  user's decision, not proceeding automatically.
- `bench/results/20260803T112447/` is committed in full (raw session
  JSON, extracted transcripts, grading reasoning) — spot-checked for
  secrets before committing (none found; both corpora are public OSS
  repos) — so the result is independently re-auditable, not just
  citable.
- If M3 does proceed, `map`'s all-sections-every-time payload shape (the
  replay arm's clearest, cheapest-to-fix finding) is a reasonable
  candidate for a follow-up slice regardless of the gate decision, since
  it's a real token-efficiency cost visible in both arms of this
  measurement.
- The two harness bugs and their fixes are preserved in `bench/run.sh`'s
  own comments and `bench/field-log.md`, not just this ADR, so a future
  re-run (e.g. with more trials, or after a `map` fix) starts from a
  known-working harness rather than rediscovering the same cwd/`--add-dir`
  pitfall.

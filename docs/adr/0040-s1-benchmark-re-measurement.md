# 0040 — S-1 benchmark re-measurement

**Status:** accepted (result recorded; **M3 gate decision is the user's,
not made here** — spec §11.4: "the implementing agent MUST present
results and wait", same as ADR-0019) · **Date:** 2026-09-22 ·
**Milestone:** `docs/post-s1-improvement-plan.md`'s own "step 5" —
re-running S-1 against the post-S-1 improvements before deciding
whether M3 proceeds.

## Context

ADR-0019 measured S-1 once, 2026-08-03, and found neither threshold
met (carto used 18.0% more combined input tokens, not ≥30% fewer;
scored 6.2 points lower accuracy, not ≥20 higher). Per spec §11.4 that
decision was presented to the user, not made, and has sat pending
since — `docs/STATUS.md` recorded it as such through six weeks and 20
ADRs (0020–0039) of subsequent work, most of it explicitly aimed at
S-1's own failure modes (`docs/post-s1-improvement-plan.md`'s slices
1–4: the honest-absence signal, `map --section`, Module/File discovery,
grouped-`use` extraction) plus later, independently-motivated
capability (contracts, C#, multi-root support). The plan's own step 5
— re-run the same 8 tasks before deciding — was never executed.

**A naive re-run would not have been valid.** Before touching
`bench/run.sh`, every task's ground truth was re-verified against
current state (see `bench/tasks.md`'s "2026-09-22 re-verification"
section and its 8 per-task addenda, committed separately). This was not
precautionary: T8's ground truth was *provably* wrong (its target
function had been directly modified by work in this same session), and
L3's entire premise (a documented extraction gap) had been fixed by
ADR-0022. Re-verifying also caught a real harness bug —
`bench/replay/replay.sh`'s T6 command failed outright (`load` became
ambiguous once the contracts feature added a second symbol by that
name) — fixed before it could have aborted the paid batch.

## What changed since ADR-0019

- **Corpora**: carto's own repo grew ~181 → 474 walked files. zed was
  re-cloned fresh (the original checkout no longer existed on this
  machine and was never pinned) — commit
  `62e5991dd0f0c8a3af8d5e7e9c4652490d468db8`, 2026-09-22, 245 crates
  (was 236).
- **A third arm.** `bench/run.sh` gained a `cli` arm (Bash access to
  the `carto` binary directly, no MCP server) after the original run,
  to answer whether MCP's own transport is worth its cost independent
  of carto's answers (which are identical either way — same underlying
  `query::{find,deps,map}` functions).
- **Environment note, disclosed but not new**: every session (both
  this run and, checked retroactively, the original) reports
  `Permission mode forced to default — CLAUDE_CODE_SUBPROCESS_ENV_SCRUB
  is set`, meaning `--permission-mode bypassPermissions` was not
  actually honored in either measurement. Checked for practical impact:
  `permission_denials` counts are small and comparable across both runs
  (2–3 sessions, 1–2 denials each, all the pre-existing "compound Bash
  command" pattern `run.sh`'s own notes already document), and
  `--allowedTools` was always the real functional gate regardless of
  permission mode. Not believed to bias the comparison, but recorded
  here since it was never disclosed in ADR-0019.

## Result

Full per-task numbers and grading rationale:
`bench/results/20260922T173830/` (`GRADES.md`, `GRADES.json`,
`transcripts/*.txt`, raw session JSON). 24 sessions, real cost $4.74.
Headline:

| Metric | grep-only | cli | carto (MCP) | S-1 threshold | Met? |
|---|---:|---:|---:|---|:---:|
| Combined input tokens | 309,061 | 330,255 | 357,824 | carto ≥30% fewer | **No** — 15.8% *more* (was 18.0% more) |
| Accuracy (8 tasks) | 87.5% | 100% | 100% | carto ≥20 points higher | **No** — +12.5 points (was −6.2 points) |
| Real dollar cost | — | — | — | — | $4.74 total, 3 arms |

**Neither S-1 threshold is met, on this one-trial measurement** — same
headline conclusion as ADR-0019. But the shape underneath it changed
substantially, in carto's favor on the dimension that was previously
the clearer problem:

- **Accuracy is a full reversal, not an incremental gain.** carto went
  from carto's worst dimension (6.2 points *below* grep) to its best
  (12.5 points *above*, at a perfect 8/8). The specific tasks that
  drove this are the same ones ADR-0020/0022/0023's post-S-1 work
  targeted: T6 (ADR-0019's own "sharpest, most instructive result" —
  carto silently returning a plausible-but-wrong answer) is now
  correct, with the agent explicitly noticing and acting on the new
  honest-absence signal before answering. L3 (carto's clean loss,
  "11 of 18, missing 7," plus a separate "unreachable via the actual
  MCP tool surface" finding) is now a clean win, 24 of 24 with zero
  false positives, both failures independently fixed. T8's own
  "sharpest possible three-way split... invisible by construction" is
  now visible as three real numbers in carto's own output.
- **Token cost barely moved** (18.0% more → 15.8% more) — correctness
  is no longer the binding constraint for S-1, token footprint is.
  `map`'s whole-payload-per-call shape (identified in ADR-0019's own
  replay-arm finding as a real, fixable cost driver) is still the
  likely largest lever un-pulled; `--section` exists since the post-S-1
  work but none of these 8 tasks' prompts happen to invoke it.
- **grep-only's own accuracy dropped** (93.8% → 87.5%) — not from any
  carto-side change, but from missing one real caller on T7
  (`activity_indicator.rs:755`) that a more careful read would have
  caught. A reminder that the "safe baseline" isn't perfectly reliable
  either, and that one-trial-per-cell noise cuts both directions.
- **A genuinely new finding, independent of any threshold**: both
  carto-backed arms on T7 surfaced a bare, unqualified call
  (`agent_ui/agent_diff.rs:686`) that carto's extractor silently
  misses with *no* signal at all — outside the documented ADR-0008
  path-qualified exclusion. Both arms caught it themselves, via their
  own follow-up verification after noticing carto's honest-absence
  count didn't fully reconcile — the exact "use carto when it helps,
  verify independently when something doesn't add up" behavior
  ADR-0019's own T6/T7/T8 discussion named as its most encouraging
  qualitative finding, now visibly still holding. The likely root
  cause (the call sits inside a `format!(...)` macro argument,
  plausibly unwalked by `calls.scm`) is a real, previously-unknown
  extraction gap worth its own follow-up investigation — separate from
  this ADR.
- **The `cli` arm matches MCP's accuracy exactly (100%) while using
  7.7% fewer combined input tokens.** Real, if modest, evidence that
  MCP's own transport overhead costs something independent of the data
  quality (identical either way, since both call the same core
  functions) — worth weighing if M4's MCP investment is reconsidered
  alongside the M3 decision.

## What this ADR does *not* decide

Same as ADR-0019: spec §11.4 requires the user to make the M3 go/no-go
call on presented data, not the implementing agent. This ADR records
the measurement, its methodology corrections, and what changed; it
concludes neither "M3 is a go" nor "M3 is a no-go."

## Consequences

- `docs/STATUS.md`'s milestone-position section updated to record this
  result; M3 remains paused pending the user's decision — now on
  current rather than six-week-old data.
- `docs/post-s1-improvement-plan.md`'s "step 5 pending" note closed out.
- `bench/results/20260922T173830/` committed in full (spot-checked for
  secrets before committing — none found; both corpora are public OSS,
  same precedent ADR-0019 set).
- `bench/tasks.md`'s re-verified ground truth and
  `bench/replay/replay.sh`'s T6 fix (committed separately, before this
  ADR) are durable artifacts of this slice independent of the M3
  decision — the next re-run, whenever it happens, starts from current
  ground truth rather than repeating this same re-verification work.

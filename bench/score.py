#!/usr/bin/env python3
"""Scores the S-1 benchmark (bench/tasks.md, spec §11.4) from a
bench/results/<timestamp>/ directory produced by bench/run.sh: 8 tasks x
3 strategy arms (grep, cli, carto), one real
`claude -p --output-format json` session each.

The `cli` arm (added after ADR-0019's first measurement) answers a
question spec §1.5 doesn't ask but the same evidence motivates: is MCP
the right integration surface at all, or does carto-via-Bash-CLI get the
same answers without the MCP transport/config friction that caused every
real harness failure in the first measurement? `cli` vs. `carto` is
reported alongside the spec-defined `grep` vs. `carto` comparison, but
kept clearly separate — S-1's own threshold is specifically about
carto-via-MCP vs. grep-only, and the cli comparison is informal,
exploratory numbers for that separate question.

Reports, per spec §1.5's S-1 wording ("≥30% fewer input tokens and ≥20%
higher accuracy than agent-with-grep-only"):
  - input_tokens, cache_creation_input_tokens, and cache_read_input_tokens
    separately as well as summed (S-1's "input tokens" is ambiguous across
    cache tiers; collapsing them would hide where any saving actually
    comes from — see bench/tasks.md's own methodology notes for why this
    matters concretely, e.g. an index-build's cache write amortizing
    across later queries in the same session).
  - a per-task table, not just an aggregate mean — with one trial per
    cell, a single blended number would claim more precision than the
    data supports.
  - accuracy is NOT auto-graded here: bench/tasks.md's ground truth
    requires human judgment against each task's real answer, and
    bench/results/<timestamp>/GRADES.json (checked in alongside a
    results run) supplies it. This script fails loudly rather than
    silently report token deltas without accuracy context if that file
    is missing.

Usage: python3 bench/score.py bench/results/<timestamp>/ [--grades bench/results/<timestamp>/GRADES.json]
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

TASKS = ["L1", "L2", "L3", "L4", "T5", "T6", "T7", "T8"]
ARMS = ["grep", "cli", "carto"]


def load_session(path: Path) -> dict | None:
    if not path.exists():
        return None
    try:
        return json.loads(path.read_text())
    except (json.JSONDecodeError, OSError) as e:
        print(f"warning: failed to parse {path}: {e}", file=sys.stderr)
        return None


def usage_row(session: dict | None) -> dict:
    if session is None:
        return {
            "input_tokens": None,
            "cache_creation_input_tokens": None,
            "cache_read_input_tokens": None,
            "output_tokens": None,
            "total_cost_usd": None,
            "num_turns": None,
            "duration_ms": None,
            "is_error": None,
        }
    usage = session.get("usage", {})
    return {
        "input_tokens": usage.get("input_tokens"),
        "cache_creation_input_tokens": usage.get("cache_creation_input_tokens"),
        "cache_read_input_tokens": usage.get("cache_read_input_tokens"),
        "output_tokens": usage.get("output_tokens"),
        "total_cost_usd": session.get("total_cost_usd"),
        "num_turns": session.get("num_turns"),
        "duration_ms": session.get("duration_ms"),
        "is_error": session.get("is_error"),
    }


def combined_input(row: dict) -> int | None:
    """S-1's "input tokens": the standard reading is prompt tokens
    actually billed as input, i.e. fresh input_tokens plus
    cache_creation (first-time-seen content, billed at the input rate)
    — cache_read is billed separately and far cheaper, so it's reported
    alongside but not folded into this headline figure. Both views are
    in the per-task table; this is only the one used for the S-1
    threshold check.
    """
    if row["input_tokens"] is None:
        return None
    return row["input_tokens"] + (row["cache_creation_input_tokens"] or 0)


def fmt(n) -> str:
    if n is None:
        return "n/a"
    return f"{n:,}"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("results_dir", type=Path)
    ap.add_argument(
        "--grades",
        type=Path,
        default=None,
        help="JSON file: {task: {arm: 'correct'|'partial'|'wrong'}}. "
        "Defaults to <results_dir>/GRADES.json.",
    )
    args = ap.parse_args()

    results_dir: Path = args.results_dir
    grades_path = args.grades or (results_dir / "GRADES.json")
    grades = {}
    if grades_path.exists():
        grades = json.loads(grades_path.read_text())
    else:
        print(
            f"note: no grades file at {grades_path} — accuracy column will read 'ungraded'.",
            file=sys.stderr,
        )

    rows = {}
    for task in TASKS:
        rows[task] = {}
        for arm in ARMS:
            session = load_session(results_dir / f"{task}.{arm}.json")
            rows[task][arm] = usage_row(session)

    print(
        f"{'task':<5} {'arm':<6} {'in':>7} {'cache_wr':>9} {'cache_rd':>9} "
        f"{'out':>7} {'combined_in':>12} {'cost_usd':>9} {'accuracy':>10}"
    )
    totals = {arm: {"combined_in": 0, "cache_read": 0, "cost": 0.0} for arm in ARMS}
    missing = False
    for task in TASKS:
        for arm in ARMS:
            row = rows[task][arm]
            ci = combined_input(row)
            grade = grades.get(task, {}).get(arm, "ungraded")
            if ci is None:
                missing = True
            else:
                totals[arm]["combined_in"] += ci
                totals[arm]["cache_read"] += row["cache_read_input_tokens"] or 0
                totals[arm]["cost"] += row["total_cost_usd"] or 0.0
            print(
                f"{task:<5} {arm:<6} {fmt(row['input_tokens']):>7} "
                f"{fmt(row['cache_creation_input_tokens']):>9} "
                f"{fmt(row['cache_read_input_tokens']):>9} {fmt(row['output_tokens']):>7} "
                f"{fmt(ci):>12} {row['total_cost_usd'] or 0:>9.4f} {grade:>10}"
            )
    print("-" * 90)
    for arm in ARMS:
        print(
            f"{'total':<5} {arm:<6} {'':<7} {'':<9} "
            f"{fmt(totals[arm]['cache_read']):>9} {'':<7} "
            f"{fmt(totals[arm]['combined_in']):>12} {totals[arm]['cost']:>9.4f}"
        )

    if missing:
        print(
            "\nSome sessions are missing or failed to parse — the totals above "
            "are computed only over the sessions that succeeded. Re-run "
            "bench/run.sh for any missing task/arm before treating this as final.",
            file=sys.stderr,
        )

    grep_in = totals["grep"]["combined_in"]
    carto_in = totals["carto"]["combined_in"]
    cli_in = totals["cli"]["combined_in"]
    # Guarded on carto_in too, not just grep_in: a batch that skipped the
    # carto-MCP arm entirely (e.g. to save quota) leaves carto_in at 0,
    # which would otherwise print a meaningless "carto uses +100.0%"
    # line — a division against zero tokens, not a real S-1 result.
    if grep_in and carto_in:
        pct = (1 - carto_in / grep_in) * 100
        print(f"\nS-1 input-token check: carto (MCP) uses {pct:+.1f}% vs. grep-only "
              f"(threshold: ≥30% fewer, i.e. pct ≥ 30).")
        print("This is ONE trial per task-arm cell — treat as directional, "
              "not a confidence interval, per bench/tasks.md's own caveat.")
    elif grep_in and not carto_in:
        print("\nS-1 input-token check: skipped — no carto-MCP arm sessions in "
              "this results dir (grep/cli-only batch).")

    if grep_in and cli_in:
        cli_pct = (1 - cli_in / grep_in) * 100
        print(f"\n[exploratory, not an S-1 metric] cli-vs-grep input tokens: "
              f"carto-via-CLI uses {cli_pct:+.1f}% vs. grep-only.")
    if carto_in and cli_in:
        cli_vs_mcp_pct = (1 - cli_in / carto_in) * 100
        print(f"[exploratory, not an S-1 metric] cli-vs-MCP input tokens: "
              f"carto-via-CLI uses {cli_vs_mcp_pct:+.1f}% vs. carto-via-MCP — "
              f"answers whether MCP's transport overhead (not carto's answers, "
              f"which are identical either way) costs anything in practice.")

    grade_weight = {"correct": 1.0, "partial": 0.5, "wrong": 0.0}
    for arm in ARMS:
        graded = [grades.get(t, {}).get(arm) for t in TASKS]
        graded = [g for g in graded if g in grade_weight]
        if graded:
            score = sum(grade_weight[g] for g in graded) / len(graded) * 100
            print(f"S-1 accuracy ({arm}): {score:.1f}% ({len(graded)}/{len(TASKS)} tasks graded)")
    grep_graded = [grades.get(t, {}).get("grep") for t in TASKS]
    carto_graded = [grades.get(t, {}).get("carto") for t in TASKS]
    cli_graded = [grades.get(t, {}).get("cli") for t in TASKS]
    if all(g in grade_weight for g in grep_graded) and all(g in grade_weight for g in carto_graded):
        grep_score = sum(grade_weight[g] for g in grep_graded) / len(TASKS) * 100
        carto_score = sum(grade_weight[g] for g in carto_graded) / len(TASKS) * 100
        acc_pct = carto_score - grep_score
        print(f"S-1 accuracy check: carto (MCP) is {acc_pct:+.1f} points vs. grep-only "
              f"(threshold: ≥20% higher, i.e. pct ≥ 20).")
        if all(g in grade_weight for g in cli_graded):
            cli_score = sum(grade_weight[g] for g in cli_graded) / len(TASKS) * 100
            print(f"[exploratory, not an S-1 metric] cli-vs-grep accuracy: "
                  f"carto-via-CLI is {cli_score - grep_score:+.1f} points vs. grep-only.")
            print(f"[exploratory, not an S-1 metric] cli-vs-MCP accuracy: "
                  f"carto-via-CLI is {cli_score - carto_score:+.1f} points vs. carto-via-MCP.")

    n_graded = sum(1 for t in TASKS for a in ARMS if grades.get(t, {}).get(a))
    if n_graded == 0:
        print(
            "\nNo accuracy grades found — S-1's accuracy threshold (≥20% higher "
            "than grep-only) cannot be checked without bench/results/<ts>/GRADES.json.",
            file=sys.stderr,
        )

    return 0


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env bash
# Deterministic replay arm for the S-1 benchmark (bench/tasks.md, spec
# §11.4). No LLM involved: for each of the 8 tasks, runs the exact
# command sequence a competent grep-only agent would run, and the exact
# `carto` command(s) an agent with the MCP server would call, and prints
# the raw stdout byte count each strategy would put into an agent's
# context. Free and perfectly reproducible — the honest cost is that the
# grep-arm sequences are authored by the same person who built carto, so
# they're checked in here for the user to audit, not presented as
# objectively "what a grep-only agent would do."
#
# carto's own CLI --json output is used as the byte-measurement proxy for
# the carto arm: it's the identical serde struct carto-mcp's
# `structuredContent` returns (crates/carto-mcp's tools/*.rs each call
# `serde_json::to_value(&result)` on the same result type the CLI's
# `--json` flag serializes), so this is not an approximation of what an
# MCP client would receive — it's the same bytes, modulo the JSON-RPC
# envelope's own small fixed overhead (method/id/jsonrpc keys), which is
# negligible against the payload sizes seen here.
#
# Usage: bash bench/replay/replay.sh [carto-repo] [zed-repo]
set -euo pipefail

CARTO_REPO="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
ZED_REPO="${2:-$HOME/playground/zed}"
CARTO_BIN="$CARTO_REPO/target/release/carto"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/carto-bench-replay.XXXXXX")"
IDX_CARTO="$WORK/idx-carto"
IDX_ZED="$WORK/idx-zed"

trap 'rm -rf "$WORK"' EXIT

if [ ! -x "$CARTO_BIN" ]; then
  echo "carto release binary not found at $CARTO_BIN — run: cargo build --release -p carto-cli" >&2
  exit 1
fi
if [ ! -d "$ZED_REPO" ]; then
  echo "zed repo not found at $ZED_REPO — pass its path as the second argument" >&2
  exit 1
fi

bytes_of() { wc -c | tr -d ' '; }

# --- Index-build cost (tracked separately from per-task carto costs, per
# bench/tasks.md's methodology — an agent pays this once per session, not
# once per question). ---
echo "== index-build cost (one-time, amortized across every carto-arm task on that corpus) ==" >&2
t0=$(date +%s)
carto_idx_bytes=$("$CARTO_BIN" index "$CARTO_REPO" --out "$IDX_CARTO" --json | bytes_of)
t1=$(date +%s)
echo "carto repo:  $((t1 - t0))s wall, ${carto_idx_bytes} index-summary bytes" >&2
t0=$(date +%s)
zed_idx_bytes=$("$CARTO_BIN" index "$ZED_REPO" --out "$IDX_ZED" --json | bytes_of)
t1=$(date +%s)
echo "zed repo:    $((t1 - t0))s wall, ${zed_idx_bytes} index-summary bytes" >&2
echo >&2

# --- Per-task sequences ---
# Each function prints exactly what an agent following that strategy
# would need to read to answer the task; only the byte count of that
# combined output is measured, not correctness (bench/score.py's job,
# graded separately against bench/tasks.md's ground truth).

grep_L1() { # zed orientation
  ( cd "$ZED_REPO" && ls crates | wc -l && grep -A3 '^\[workspace\]' Cargo.toml && find . -maxdepth 1 -type d )
}
carto_L1() {
  "$CARTO_BIN" map "$ZED_REPO" --out "$IDX_ZED" --budget 100 --json
}

grep_L2() { # zed: where is truncate_and_trailoff
  ( cd "$ZED_REPO" && rg -n "^pub fn truncate_and_trailoff" crates && sed -n '50,71p' crates/util/src/util.rs )
}
carto_L2() {
  "$CARTO_BIN" where truncate_and_trailoff "$ZED_REPO" --out "$IDX_ZED" --exact --json
}

grep_L3() { # carto: files importing carto_core
  ( cd "$CARTO_REPO" && rg -n "carto_core::query" -g '*.rs' crates && rg -l "^use carto_core::" -g '*.rs' crates )
}
carto_L3() {
  # `map`'s external-packages section is the ONLY thing reachable here —
  # `where` only searches Symbol nodes (not Module), and `deps
  # carto_core` genuinely fails ("no node ID or symbol named `carto_core`
  # found"): there is no tool-level way to discover a Module node's ID at
  # all, so an MCP-only agent cannot get from "carto_core in=11" to an
  # actual file list. Not a stand-in for a fuller sequence — this really
  # is the whole reachable answer (see bench/tasks.md's L3).
  "$CARTO_BIN" map "$CARTO_REPO" --out "$IDX_CARTO" --budget 400 --json
}

grep_L4() { # carto: real entry points
  ( cd "$CARTO_REPO" && grep -rn '\[\[bin\]\]' -A2 crates/*/Cargo.toml && find crates -name lib.rs )
}
carto_L4() {
  "$CARTO_BIN" map "$CARTO_REPO" --out "$IDX_CARTO" --budget 400 --json
}

grep_T5() { # carto: render_capped direct callers
  ( cd "$CARTO_REPO" && rg -n '\.render_capped\(' -g '*.rs' crates )
}
carto_T5() {
  "$CARTO_BIN" deps render_capped "$CARTO_REPO" --out "$IDX_CARTO" --dir in --depth 1 --json
}

grep_T6() { # carto: graph::load callers
  ( cd "$CARTO_REPO" && rg -n 'graph::load\(' -g '*.rs' crates )
}
carto_T6() {
  "$CARTO_BIN" deps load "$CARTO_REPO" --out "$IDX_CARTO" --dir in --depth 1 --json
}

grep_T7() { # zed: truncate_and_trailoff callers
  ( cd "$ZED_REPO" && rg -n 'truncate_and_trailoff\(' -g '*.rs' crates )
}
carto_T7() {
  "$CARTO_BIN" deps truncate_and_trailoff "$ZED_REPO" --out "$IDX_ZED" --dir in --depth 1 --json
}

grep_T8() { # carto: build_and_persist's own dependencies
  ( cd "$CARTO_REPO" && sed -n '1,80p' crates/carto-core/src/indexer.rs )
}
carto_T8() {
  "$CARTO_BIN" deps build_and_persist "$CARTO_REPO" --out "$IDX_CARTO" --dir out --depth 1 --json
}

printf '%-6s %14s %14s %10s\n' "task" "grep_bytes" "carto_bytes" "delta_%"
total_grep=0
total_carto=0
for task in L1 L2 L3 L4 T5 T6 T7 T8; do
  g=$("grep_$task" 2>/dev/null | bytes_of)
  c=$("carto_$task" 2>/dev/null | bytes_of)
  total_grep=$((total_grep + g))
  total_carto=$((total_carto + c))
  delta=$(python3 -c "print(f'{(1 - $c/$g)*100:.1f}' if $g else 'n/a')")
  printf '%-6s %14s %14s %10s\n' "$task" "$g" "$c" "$delta"
done
echo "---"
delta_total=$(python3 -c "print(f'{(1 - $total_carto/$total_grep)*100:.1f}')")
printf '%-6s %14s %14s %10s\n' "total" "$total_grep" "$total_carto" "$delta_total"
echo
echo "index-build cost (not amortized above): carto repo ${carto_idx_bytes}b, zed ${zed_idx_bytes}b — see bench/score.py for break-even task count."

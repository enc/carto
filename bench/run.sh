#!/usr/bin/env bash
# Real-run arm for the S-1 benchmark (bench/tasks.md, spec §11.4): 8
# tasks x 2 arms (grep-only vs. carto-MCP), one trial each, via real
# `claude -p` sessions. Saves each session's full JSON output (including
# the exact `usage` block: input/output/cache tokens, cost, duration) to
# bench/results/<timestamp>/<task>.<arm>.json.
#
# Auth note: `--bare` (the cleanest isolation — no CLAUDE.md/hook/plugin
# auto-discovery) requires ANTHROPIC_API_KEY, which isn't set in the
# environment this was built in. Runs instead under normal session auth,
# invoked from a neutral cwd (this script's own tempdir, not inside
# either target repo) specifically to avoid triggering carto's own
# elaborate CLAUDE.md via directory walk-up, and with `--strict-mcp-config`
# on BOTH arms (the grep arm gets an empty MCP config) so MCP-server
# exposure is at least cleanly controlled even though CLAUDE.md/hook
# isolation isn't as complete as --bare would give. Flagged here, and
# again in every results writeup, as a real methodology caveat — not
# silently treated as equivalent to --bare.
set -euo pipefail

CARTO_REPO="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
ZED_REPO="${2:-$HOME/playground/zed}"
CARTO_BIN="$CARTO_REPO/target/release/carto"
MODEL="${MODEL:-sonnet}"

RUN_DIR="$(mktemp -d "${TMPDIR:-/tmp}/carto-bench-run.XXXXXX")"
TS="$(date +%Y%m%dT%H%M%S)"
RESULTS_DIR="$CARTO_REPO/bench/results/$TS"
mkdir -p "$RESULTS_DIR"

if [ ! -x "$CARTO_BIN" ]; then
  echo "carto release binary not found at $CARTO_BIN — run: cargo build --release -p carto-cli" >&2
  exit 1
fi
if [ ! -d "$ZED_REPO" ]; then
  echo "zed repo not found at $ZED_REPO — pass its path as the second argument" >&2
  exit 1
fi

# Real MCP config for the carto arm.
cat > "$RUN_DIR/mcp-config.json" <<EOF
{"mcpServers": {"carto": {"command": "$CARTO_BIN", "args": ["serve"]}}}
EOF
# Empty MCP config for the grep arm — with --strict-mcp-config, this
# guarantees zero MCP tools are exposed, rather than relying on "nothing
# else happens to be configured."
echo '{"mcpServers": {}}' > "$RUN_DIR/empty-mcp-config.json"

READ_TOOLS="Read Glob Grep Bash(rg:*) Bash(grep:*) Bash(find:*) Bash(sed:*) Bash(wc:*) Bash(ls:*) Bash(cat:*)"
CARTO_TOOLS="mcp__carto__index mcp__carto__where mcp__carto__deps mcp__carto__map mcp__carto__selfcheck"
# --append-system-prompt-file doesn't actually exist as a standalone flag
# (checked against `claude --help`; only the inline-text
# --append-system-prompt does) — pass the skill file's content directly.
SKILL_TEXT="$(cat "$CARTO_REPO/skill/carto.skill.md")"

declare -A PROMPT
declare -A CORPUS
PROMPT[L1]="Give me a structural orientation of this repository: is it a single crate or a workspace, roughly how large is it, and what are its main architectural pieces?"
CORPUS[L1]="$ZED_REPO"
PROMPT[L2]="Where is \`truncate_and_trailoff\` defined, and what's its exact signature?"
CORPUS[L2]="$ZED_REPO"
PROMPT[L3]="Which files in this repo import anything from the \`carto_core\` crate?"
CORPUS[L3]="$CARTO_REPO"
PROMPT[L4]="What are this repo's real entry points — the binaries and library crate roots?"
CORPUS[L4]="$CARTO_REPO"
PROMPT[T5]="What directly calls \`TaintedString::render_capped\` in production code (not test code)?"
CORPUS[T5]="$CARTO_REPO"
PROMPT[T6]="If \`graph::load\`'s function signature changes, which files need updating?"
CORPUS[T6]="$CARTO_REPO"
PROMPT[T7]="What calls \`truncate_and_trailoff\`, and with what confidence should I trust each one?"
CORPUS[T7]="$ZED_REPO"
PROMPT[T8]="What does \`carto_core::indexer::build_and_persist\` call or depend on directly?"
CORPUS[T8]="$CARTO_REPO"

run_session() {
  local task="$1" arm="$2" corpus="$3" prompt="$4"
  local out="$RESULTS_DIR/$task.$arm.json"
  local full_prompt="Repository to answer this about: $corpus

$prompt"

  echo "== $task/$arm ==" >&2
  # Run from RUN_DIR (neutral cwd), not inside either target repo.
  (
    cd "$RUN_DIR"
    if [ "$arm" = "carto" ]; then
      claude -p \
        --mcp-config "$RUN_DIR/mcp-config.json" \
        --strict-mcp-config \
        --output-format json \
        --model "$MODEL" \
        --allowedTools "$READ_TOOLS $CARTO_TOOLS" \
        --permission-mode bypassPermissions \
        --append-system-prompt "$SKILL_TEXT" \
        "$full_prompt"
    else
      claude -p \
        --mcp-config "$RUN_DIR/empty-mcp-config.json" \
        --strict-mcp-config \
        --output-format json \
        --model "$MODEL" \
        --allowedTools "$READ_TOOLS" \
        --permission-mode bypassPermissions \
        "$full_prompt"
    fi
  ) > "$out" 2> "$RESULTS_DIR/$task.$arm.stderr"

  local cost tokens_in
  cost=$(python3 -c "import json;print(json.load(open('$out')).get('total_cost_usd','?'))" 2>/dev/null || echo "?")
  echo "   cost=\$$cost -> $out" >&2
}

for task in L1 L2 L3 L4 T5 T6 T7 T8; do
  run_session "$task" grep "${CORPUS[$task]}" "${PROMPT[$task]}"
  run_session "$task" carto "${CORPUS[$task]}" "${PROMPT[$task]}"
done

rm -rf "$RUN_DIR"
echo "Results written to $RESULTS_DIR" >&2

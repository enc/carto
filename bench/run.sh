#!/usr/bin/env bash
# Real-run arm for the S-1 benchmark (bench/tasks.md, spec §11.4): 8
# tasks x 3 strategy arms (grep-only, carto-via-CLI, carto-via-MCP), one
# trial each, via real `claude -p` sessions. Saves each session's full
# JSON output (including the exact `usage` block: input/output/cache
# tokens, cost, duration) to bench/results/<timestamp>/<task>.<arm>.json.
#
# The `cli` arm was added after the first S-1 measurement (ADR-0019)
# left an open question: is MCP actually the better integration surface
# for carto, or would the CLI invoked via Bash — no server handshake, no
# separate MCP config file, no connection state — be just as good or
# better? Every real failure the original two-arm batch hit (the
# neutral-cwd bug, --bare needing an unavailable API key, --safe-mode
# disabling MCP outright, one isolated MCP-connection flake) was friction
# *of the MCP transport/config layer*, never of carto's answers
# themselves — none of that is inherent to a CLI reached through a tool
# the agent already has unconditionally. `cli` and `carto` call the
# exact same underlying functions (query::{find,deps,map},
# indexer::build_and_persist) and answer identically when reached; this
# arm exists to measure whether the path to reach them differs in cost.
#
# Auth note: `--bare` (the cleanest isolation — no CLAUDE.md/hook/plugin
# auto-discovery) requires ANTHROPIC_API_KEY, which isn't set in the
# environment this was built in. Runs instead under normal session auth,
# with `--strict-mcp-config` on ALL THREE arms (grep and cli both get an
# empty MCP config) so MCP-server exposure is at least cleanly controlled
# even though CLAUDE.md/hook isolation isn't as complete as --bare would
# give. Flagged here, and again in every results writeup, as a real
# methodology caveat — not silently treated as equivalent to --bare.
#
# Working-directory note (fixed after a real, disclosed failure): the
# first version of this script ran every session from a neutral tempdir
# and pointed at the target repo only via prompt text. That's not enough
# — Claude Code's filesystem tools (Read/Glob/Grep, and even Bash's own
# access) are scoped to the session's cwd; a path outside it needs
# `--add-dir` (which has to be ordered carefully, since it's variadic and
# will otherwise swallow a following positional prompt argument — simpler
# to avoid entirely here) or the session has to actually run *from*
# that directory. It worked by accident for zed in the first run (the
# model happened to `cd` there inside Bash before reading) and failed
# completely for carto's own repo (both arms reported an empty
# directory, invalidating those results) — see bench/field-log.md's
# entry on this. Fixed by setting each session's cwd to the real corpus
# directly, confirmed working in a standalone diagnostic before
# re-running the full batch.
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

# Real MCP config for the carto (MCP) arm.
cat > "$RUN_DIR/mcp-config.json" <<EOF
{"mcpServers": {"carto": {"command": "$CARTO_BIN", "args": ["serve"]}}}
EOF
# Empty MCP config for the grep and cli arms — with --strict-mcp-config,
# this guarantees zero MCP tools are exposed, rather than relying on
# "nothing else happens to be configured."
echo '{"mcpServers": {}}' > "$RUN_DIR/empty-mcp-config.json"

READ_TOOLS="Read Glob Grep Bash(rg:*) Bash(grep:*) Bash(find:*) Bash(sed:*) Bash(wc:*) Bash(ls:*) Bash(cat:*)"
CARTO_MCP_TOOLS="mcp__carto__index mcp__carto__where mcp__carto__deps mcp__carto__map mcp__carto__selfcheck"
# The cli arm's only additional tool is Bash access to the carto binary
# itself, at its exact absolute path — Claude Code's Bash allowedTools
# patterns match on the command's leading word, so this has to be the
# literal resolved path, not a bare `carto` (which wouldn't be on PATH
# inside the spawned session anyway).
CARTO_CLI_TOOLS="Bash($CARTO_BIN:*)"
# --append-system-prompt-file doesn't actually exist as a standalone flag
# (checked against `claude --help`; only the inline-text
# --append-system-prompt does) — pass file content directly for both the
# MCP arm's skill file and the cli arm's CLI-usage guidance.
SKILL_TEXT="$(cat "$CARTO_REPO/skill/carto.skill.md")"
CLI_TEXT="$(sed "s|{{CARTO_BIN}}|$CARTO_BIN|g" "$CARTO_REPO/bench/cli-arm-prompt.md")"

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
  local full_prompt="Repository to answer this about: $corpus (this is also the session's current directory).

$prompt"

  echo "== $task/$arm ==" >&2
  # Run with cwd = the actual corpus (not RUN_DIR) — see this script's
  # own header comment for why a neutral cwd silently broke every
  # carto-repo task in the first version of this harness.
  (
    cd "$corpus"
    case "$arm" in
      carto)
        claude -p \
          --mcp-config "$RUN_DIR/mcp-config.json" \
          --strict-mcp-config \
          --output-format json \
          --model "$MODEL" \
          --allowedTools "$READ_TOOLS $CARTO_MCP_TOOLS" \
          --permission-mode bypassPermissions \
          --append-system-prompt "$SKILL_TEXT" \
          "$full_prompt"
        ;;
      cli)
        claude -p \
          --mcp-config "$RUN_DIR/empty-mcp-config.json" \
          --strict-mcp-config \
          --output-format json \
          --model "$MODEL" \
          --allowedTools "$READ_TOOLS $CARTO_CLI_TOOLS" \
          --permission-mode bypassPermissions \
          --append-system-prompt "$CLI_TEXT" \
          "$full_prompt"
        ;;
      *)
        claude -p \
          --mcp-config "$RUN_DIR/empty-mcp-config.json" \
          --strict-mcp-config \
          --output-format json \
          --model "$MODEL" \
          --allowedTools "$READ_TOOLS" \
          --permission-mode bypassPermissions \
          "$full_prompt"
        ;;
    esac
  ) > "$out" 2> "$RESULTS_DIR/$task.$arm.stderr"

  local cost tokens_in
  cost=$(python3 -c "import json;print(json.load(open('$out')).get('total_cost_usd','?'))" 2>/dev/null || echo "?")
  echo "   cost=\$$cost -> $out" >&2
}

for task in L1 L2 L3 L4 T5 T6 T7 T8; do
  run_session "$task" grep "${CORPUS[$task]}" "${PROMPT[$task]}"
  run_session "$task" cli "${CORPUS[$task]}" "${PROMPT[$task]}"
  run_session "$task" carto "${CORPUS[$task]}" "${PROMPT[$task]}"
done

rm -rf "$RUN_DIR"
echo "Results written to $RESULTS_DIR" >&2

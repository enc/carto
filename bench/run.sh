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

CARTO_REPO="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
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

IDX_CARTO="$RUN_DIR/idx-carto"
IDX_ZED="$RUN_DIR/idx-zed"

# Pre-build each corpus's index once, here, before any claude -p session
# launches — mirroring bench/replay/replay.sh's own established
# convention (its IDX_CARTO/IDX_ZED, built the exact same way, lines
# 27-56 there). This runs as a plain subprocess in this script's own
# unrestricted shell, never inside a permission-mediated graded session
# — which is where every real batch's `index`-related permission
# denials traced back to (indexing zed's 3,754 files produces enough
# output that an agent reaches for a pipe/redirect to manage it, and
# this environment's Bash permission matching denies that regardless of
# --allowedTools; L1.cli in bench/results/20260803T191345/ even fell
# back to raw ls/find/grep after hitting this, never reaching carto at
# all for that task — see bench/field-log.md). `set -euo pipefail`
# means a failed pre-index here aborts the whole batch loudly, before
# any paid session runs, rather than letting individual graded sessions
# silently degrade.
echo "== pre-indexing (once, outside any graded session) ==" >&2
t0=$(date +%s)
"$CARTO_BIN" index "$CARTO_REPO" --out "$IDX_CARTO" >&2
echo "   carto repo indexed in $(($(date +%s) - t0))s -> $IDX_CARTO" >&2
t0=$(date +%s)
"$CARTO_BIN" index "$ZED_REPO" --out "$IDX_ZED" >&2
echo "   zed repo indexed in $(($(date +%s) - t0))s -> $IDX_ZED" >&2
echo >&2

# Real MCP config for the carto (MCP) arm.
cat > "$RUN_DIR/mcp-config.json" <<EOF
{"mcpServers": {"carto": {"command": "$CARTO_BIN", "args": ["serve"]}}}
EOF
# Empty MCP config for the grep and cli arms — with --strict-mcp-config,
# this guarantees zero MCP tools are exposed, rather than relying on
# "nothing else happens to be configured."
echo '{"mcpServers": {}}' > "$RUN_DIR/empty-mcp-config.json"

# head/tail/python3/du added after a real batch showed denials on plain
# single invocations of these (repo-size estimation, JSON pretty-printing)
# — defense in depth only: a *compound* command (a pipe into one of
# these, a `for` loop, `&&`/`;` chains) still gets denied regardless of
# what's declared here, since this session's Bash permission matching
# checks the whole command's shape, not just its leading word. See
# BASH_STYLE_NOTE below for the actual fix for that case.
READ_TOOLS="Read Glob Grep Bash(rg:*) Bash(grep:*) Bash(find:*) Bash(sed:*) Bash(wc:*) Bash(ls:*) Bash(cat:*) Bash(head:*) Bash(tail:*) Bash(python3:*) Bash(du:*)"
# `index` deliberately excluded from both arms' tool sets below — both
# corpora are already pre-indexed (see above) before any graded session
# starts, so it's never needed, and excluding it makes that a structural
# guarantee (can't be re-invoked, can't hit an index-related permission
# denial) rather than something that relies on the agent following a
# prompt note alone.
CARTO_MCP_TOOLS="mcp__carto__where mcp__carto__deps mcp__carto__map mcp__carto__selfcheck"
# The cli arm's additional tools are Bash access to the carto binary
# itself, one pattern per subcommand (mirroring CARTO_MCP_TOOLS's own
# per-tool allowlist shape) rather than a blanket `Bash($CARTO_BIN:*)`
# — at its exact absolute path, since Claude Code's Bash allowedTools
# patterns match on the command's leading words, so this has to be the
# literal resolved path, not a bare `carto` (which wouldn't be on PATH
# inside the spawned session anyway).
CARTO_CLI_TOOLS="Bash($CARTO_BIN where:*) Bash($CARTO_BIN deps:*) Bash($CARTO_BIN map:*) Bash($CARTO_BIN selfcheck:*)"
# Two real batches refined this note. bench/results/20260803T185010/
# showed every permission_denials entry, on BOTH the grep and cli arms,
# was a compound/piped Bash command (`carto index x | tail -30`, a
# `for` loop piping into `grep`). After telling both arms to avoid
# piping and redirect large output to a file instead,
# bench/results/20260803T191345/ showed *that* advice also gets denied
# — a single plain command with its output redirected to a file
# (`carto ... --json > /tmp/x.json`, retried against three different
# target paths, all denied) is treated the same as a pipe. This
# session's permission matching checks a command's whole shape, not
# just its leading word, so widening READ_TOOLS/CARTO_CLI_TOOLS above
# only helps a genuinely bare invocation of those tools — never a
# pipe/redirect/loop/chain, no matter the target. The only reliable fix
# is a bare command with no trailing shell operator at all.
BASH_STYLE_NOTE="This session's tool permissions match a Bash command by its exact invocation shape: a compound command, or even a single command with output redirected to a file (pipes \`|\`, redirects \`>\`/\`>>\`, \`&&\`, \`;\`, a \`for\` loop, command substitution), can be denied even when the identical bare invocation (nothing after it) would be allowed. Prefer one simple, single-command Bash call per tool use, with no trailing shell operator — if you need to combine steps, use separate Bash calls rather than a shell pipeline, and if output is too large, bound it with the tool's own flags rather than piping or redirecting it elsewhere."
# --append-system-prompt-file doesn't actually exist as a standalone flag
# (checked against `claude --help`; only the inline-text
# --append-system-prompt does) — pass file content directly for both the
# MCP arm's skill file and the cli arm's CLI-usage guidance.
SKILL_TEXT="$(cat "$CARTO_REPO/skill/carto/SKILL.md")"
CLI_TEXT="$(sed "s|{{CARTO_BIN}}|$CARTO_BIN|g" "$CARTO_REPO/bench/cli-arm-prompt.md")

$BASH_STYLE_NOTE"

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

# Maps a corpus path to its pre-built index dir (both set up above) —
# only ever $CARTO_REPO or $ZED_REPO today, matching CORPUS[]'s own two
# hardcoded values; extend here if a third corpus is ever added.
idx_for_corpus() {
  case "$1" in
    "$CARTO_REPO") echo "$IDX_CARTO" ;;
    "$ZED_REPO") echo "$IDX_ZED" ;;
    *)
      echo "idx_for_corpus: no pre-built index for corpus '$1'" >&2
      exit 1
      ;;
  esac
}

run_session() {
  local task="$1" arm="$2" corpus="$3" prompt="$4"
  local out="$RESULTS_DIR/$task.$arm.json"
  local idx_dir
  idx_dir="$(idx_for_corpus "$corpus")"
  # Corpus-specific, so it belongs in the per-task prompt (built fresh
  # every call) rather than the static CLI_TEXT/SKILL_TEXT system
  # prompts (loaded once for the whole script run and shared across
  # every task, which can target either corpus). grep never touches
  # carto, so it gets no note — keeps its prompt minimal, unchanged.
  local idx_note=""
  if [ "$arm" = "cli" ] || [ "$arm" = "carto" ]; then
    idx_note="This repo has already been indexed by carto at --out $idx_dir — pass that exact path to every carto command (cli: --out $idx_dir; MCP: out=\"$idx_dir\" tool parameter) rather than omitting --out, and do not run index yourself; it isn't needed and isn't in your tool set.

"
  fi
  local full_prompt="${idx_note}Repository to answer this about: $corpus (this is also the session's current directory).

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
          --append-system-prompt "$SKILL_TEXT

$BASH_STYLE_NOTE" \
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
          --append-system-prompt "$BASH_STYLE_NOTE" \
          "$full_prompt"
        ;;
    esac
  ) > "$out" 2> "$RESULTS_DIR/$task.$arm.stderr"

  local cost tokens_in
  cost=$(python3 -c "import json;print(json.load(open('$out')).get('total_cost_usd','?'))" 2>/dev/null || echo "?")
  echo "   cost=\$$cost -> $out" >&2
}

# Which arms to run, space-separated — defaults to all three (unchanged
# default behavior). Override to spend less: e.g. `ARMS="grep cli"` skips
# the carto-MCP arm entirely when a run is only meant to re-check the
# grep/cli comparison and MCP quota isn't warranted that trip.
# bench/score.py already tolerates a missing arm's session files (reports
# that arm's columns as null) so a partial-arms run still scores cleanly.
ARMS="${ARMS:-grep cli carto}"

for task in L1 L2 L3 L4 T5 T6 T7 T8; do
  for arm in $ARMS; do
    run_session "$task" "$arm" "${CORPUS[$task]}" "${PROMPT[$task]}"
  done
done

rm -rf "$RUN_DIR"
echo "Results written to $RESULTS_DIR" >&2

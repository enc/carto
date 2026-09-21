# Grading reasoning — post-S-1-slices re-run, 20260803T153513

Two-arm run (`grep`, `cli` — the carto-MCP arm was deliberately skipped
this trip to save quota, per the user's direction; see this batch's own
note in `bench/field-log.md`). Graded by reading each session's actual
`result` text against `bench/tasks.md`'s ground truth, the same standard
`bench/results/20260803T112447/GRADES.md` used. `correct` = 1.0,
`partial` = 0.5, `wrong` = 0.

This run's purpose: check whether the four post-S-1 plan slices just
implemented (ADR-0020–0022 — the honest-absence signal, `map --section`,
`Module`/`File` discovery, Rust grouped-`use` extraction) actually move
the needle on the specific tasks they were built to fix, using the same
prompts/corpora/ground truth as the original S-1 batch. It is **not** a
new S-1 measurement (no MCP arm ran, so the spec §11.4 grep-vs-carto-MCP
comparison this batch can't speak to at all) — see the main writeup for
what it can and can't be used to conclude.

## L1 — repo orientation (zed)

**grep: correct.** 236 crates, correctly named the major architectural
groupings.

**cli: correct.** 236 crates via `carto map`'s exact structural counts
(3,750 files, ~54.5k symbols), same architectural groupings via a
different lens (import fan-out/fan-in ranking rather than manual
reading) — matches the original run's carto-MCP arm quality.

## L2 — symbol lookup (zed)

**grep: correct.** Exact file:line, exact signature.

**cli: correct.** Same, via `carto where --exact`.

## L3 — import fan-out (carto's own repo) — **the headline result**

Ground truth: **19** real importers (`bench/tasks.md`'s corrected
count — the original 18 from `rg -l "^use carto_core::"` plus
`main.rs`, which references `carto_core::` only via fully-qualified
inline paths).

**grep: correct.** Listed exactly the 19 files across `carto-cli` (6),
`carto-mcp` (7), and `carto-core`'s trybuild UI tests (6) — explicitly
excluded `carto-mcp/src/lib.rs` and `tools/mod.rs` (doc-comment-only
mentions) and `query/find.rs`'s test-fixture string literal. Verified
by counting: 6+7+6 = 19, zero extras, zero misses.

**cli: correct.** Same 19-file list, same exclusions, same reasoning
("just a string literal in a test fixture, not an import").

**This is the clearest evidence §1.3 (Rust grouped-`use` extraction)
and §1.2 (`Module`-node discovery) worked as intended.** The original
S-1 run's carto-MCP arm couldn't even attempt this task —
`deps carto_core --dir in` errored outright ("no node ID or symbol
named `carto_core` found"), because `where` only searched `Symbol`
nodes and a `Module`'s blake3-hashed ID had no other reachable lookup
path. Even reading `graph.json` directly (bypassing the tool surface,
which is what `bench/tasks.md`'s L3 section did to establish "carto's
actual answer") only found 11 of 18, because every file whose *only*
`carto_core` import was grouped (`use carto_core::{a, b};`) produced
zero import edges. This run's `cli` session used `carto deps carto_core
--dir in` successfully (per its own stderr trace) and got a complete,
correct 19/19 list — both the module-discovery gap and the group-import
gap are closed.

## L4 — real entry points vs. carto's heuristic (carto's own repo)

Ground truth: 4 real entry points (1 binary, 3 lib roots).

**grep: correct.** All 4, correct dependency-direction reasoning.

**cli: correct.** All 4, and explicitly named and filtered `map`'s
heuristic noise ("Everything else `map`'s entry-points heuristic
surfaced ... is heuristic noise") — exactly the behavior this task
grades on.

## T5 — direct-caller impact (carto's own repo)

Ground truth: 4 production callers (`where_cmd.rs::capped()`,
`where_tool.rs::capped()`, `redact::redact_tainted_string()`,
`TaintedString`'s own `Serialize` impl at `taint.rs:135`).

**grep: correct.** All 4, exact locations.

**cli: correct.** All 4, including the `Serialize` impl at
`taint.rs:135` — described accurately as a real call site inside
`TaintedString`'s own `impl Serialize` block, not (as the original
S-1 carto-MCP arm did) misdescribed as "the method definition itself."
**This is the direct target of §1.4's `same_file_as_root` flag** — the
original run's one accuracy loss traced exactly to this misread; this
run's `cli` answer gets full marks on the same task.

## T6 — direct-caller impact, the sharp negative case (carto's own repo)

Ground truth: 6 real call sites, all `graph::load(...)` module-qualified
calls.

**grep: correct.** All 6, correctly distinguished from doc-comment
mentions (including a mention of this very run's own new
`docs/adr/0020-...md`, correctly excluded as prose, not a call site).

**cli: correct.** All 6, and explicitly cited `root_uncaptured_inbound_calls:
6` — **the exact §1.1 signal this task was the primary evidence for** —
then confirmed with grep. In the original S-1 batch this was graded
"correct" too, but only because the agent recognized ADR-0008's
documented limitation from general knowledge and fell back blind; this
time carto's own tool output told the agent the precise count (6,
matching ground truth exactly) directly, without relying on the agent
already knowing about the gap. Line numbers in this run's answers
(e.g. `map_cmd.rs:70` vs. `bench/tasks.md`'s recorded `:41`) differ from
the ground-truth doc only because `map_cmd.rs`/`map_tool.rs` grew new
code (`--section`) between when the ground truth was written and this
run — same 6 files, same call sites, cosmetic line-number drift only.

## T7 — incoming calls with confidence (zed)

Ground truth: 17 bare (carto-attemptable) call sites across 10
files/functions, 9 path-qualified (excluded) across 7 files.

**grep: correct.** Thorough confidence analysis by argument
bounds-safety; explicitly flagged the one genuine risk
(`activity_indicator.rs`'s unbounded `saturating_sub`).

**cli: correct.** Used `carto deps --dir in`
(`root_uncaptured_inbound_calls: 9`), then grep-verified — landed on
exactly the 7 files / 9 path-qualified call sites ground truth lists
(`title_bar.rs` ×2, `project_search.rs`, `editor/items.rs` ×2,
`picker_prompt.rs`, `blame_ui.rs`, `update_title_tool.rs`,
`thread_view.rs`), framed by confidence exactly as the prompt asked
("Found by carto: confidence inferred" / "Missed by carto, confirmed by
grep: confidence certain"). A second clean instance of §1.1's signal
driving a complete, correctly-labeled answer.

## T8 — outgoing dependencies (carto's own repo, self-authored ground truth)

**grep: correct.** All 6 path-qualified calls plus the meaningful
`Graph` method calls.

**cli: correct.** Same call list, plus explicitly and correctly
explained `root_uncaptured_inbound_calls: 2` as being about *inbound*
callers of `build_and_persist` (irrelevant to "what does it call") —
demonstrating the new field doesn't confuse an outbound-dependency
question, only informs an inbound one.

## Accuracy tally

| | correct | partial | wrong | score |
|---|---:|---:|---:|---:|
| grep | 8 | 0 | 0 | 8.0/8 = 100% |
| cli | 8 | 0 | 0 | 8.0/8 = 100% |

Both arms score 100% on this 8-task set — up from the original run's
grep 93.75% / carto(MCP) 87.5%. cli's three previously-lossy or
partial tasks (L3: unreachable → correct; T5: partial miss → correct;
T6: correct-but-lucky → correct-with-real-signal) are exactly the three
this session's slices targeted. This is **not** evidence carto now
"beats" grep — with grep also at 100%, cli and grep are tied on
accuracy here, and S-1's own accuracy threshold is specifically
"≥20 points higher than grep," not "matches grep." What changed is that
carto-via-CLI no longer *loses* ground on any of these 8 tasks, which
the original run's carto-MCP arm did on 3 of 8.

## A real methodology caveat found in this run, not papered over

Every session in this batch (`grep` and `cli` alike) emitted a stderr
warning: `Permission mode forced to default — CLAUDE_CODE_SUBPROCESS_ENV_SCRUB
is set`. Checking `permission_denials` in each session's own JSON
output confirms this had a real, asymmetric effect: **7 tool-call
denials across 5 `cli`-arm sessions (L1, L4, T5, T6, T8), vs. 2 denials
in 1 `grep`-arm session (L4)** — despite `--allowedTools` being declared
explicitly for every arm (the warning's own suggested fix). All denied
calls were compound/piped Bash commands (e.g. `carto ... | python3 -m
json.tool | head -100`) that don't match the harness's simple
prefix-based `--allowedTools` patterns, or used commands (`python3`,
`head`, `du`) not present in `READ_TOOLS` at all — not a carto-CLI-
specific problem, a harness gap in how `bench/run.sh` declares its
Bash allowlist. Every affected session still recovered (higher
`num_turns`, real cost) and reached a fully correct answer — this cost
extra turns/tokens, not correctness, but it's a real, disclosable
friction source specifically disadvantaging the `cli` arm's *token
economy* (not its accuracy) that a future run should fix by widening
`READ_TOOLS`/`CARTO_CLI_TOOLS` before drawing a cli-vs-grep token
conclusion from this batch. See `bench/field-log.md`'s entry on this
run for the full accounting.

# S-1 benchmark task set (spec §11.4)

Spec §11.4 calls for 10 tasks over `fixtures/mixed` + `tf-app` with
ground-truth answers, run agent+grep-only vs. agent+carto-MCP, comparing
input tokens and correctness. `tf-app` doesn't exist yet (M2); the
fixtures are 15 files total, too small for a believable token delta (see
`bench/field-log.md`'s Part 0 for why). This set instead runs **8 tasks**
over two real corpora agreed with the user: carto's own repo (~181
walked files) and `~/playground/zed` (~3,754 walked files, a real,
unfamiliar-to-the-model production Rust codebase).

**8, not 10** — a deliberate scope call, not an oversight: each task
below needed hand-verified, non-circular ground truth (never derived from
carto's own output — spec's own requirement), and getting that right for
2 more tasks on an unfamiliar 236-crate codebase wasn't worth the
overhead of shrinking rigor on the other 8 to hit a round number.

## Why two categories, not one score

`bench/field-log.md`'s Part 0 measured carto's Rust call-resolution rate
directly: **~22.5%** of call sites become a resolved `calls` edge, both
on carto's own repo and on zed. ADR-0008 documents why: Rust
path-qualified calls (`Type::method()`, `module::func()`) are excluded
from extraction entirely, by design. This predicts carto should win
clearly on **lookup/orientation** (Category L: `where`, `map`, `imports`
edges — none of these depend on the sparse `calls` edge kind) and should
struggle on **transitive/blast-radius** questions that lean on `calls`
(Category T). Both categories turned out to have at least one
counter-example on closer inspection (T5's method-call resolution works
better than the `calls`-sparsity story predicts; L3's `Module`-node
discovery gap makes it a clean loss despite not touching `calls` at
all) — the category split is a prior about *where the risk concentrates*,
not a guarantee every task in a bucket lands the same way, and the task
set is designed so a reader can see exactly which predictions held.

## A methodology correction, kept visible rather than quietly fixed

Every task below was originally drafted with a *predicted* carto answer,
reasoned by hand from the spec/ADRs before running carto at all — per
this document's own rule that ground truth must never come from carto's
own output. But **predictions about carto's exact behavior are not
ground truth about the source code**, and nothing stops those
predictions from being checked against real `carto where`/`deps`/`map`
output before the benchmark runs — doing exactly that (spot-checking
carto against the source-derived ground truth below) caught three
outright wrong predictions:

- **L3** — assumed a same-workspace crate import (`use
  carto_core::query::...`) would resolve as an *internal* Rust import,
  then (after finding it doesn't) assumed the gap was external-import
  granularity ("carto knows *which file*, not *which item*"). Neither
  framing survived checking the actual mechanism: `carto_core` genuinely
  is classified `external` (a sibling workspace crate, indistinguishable
  from a true third-party dependency with no `Cargo.toml` parsing — same
  precedent already documented for Go/Python/PHP/C#), but the real,
  bigger effect traces to something already on record in
  `docs/STATUS.md` and never connected to this consequence before: Rust's
  grouped `use path::{a, b};` shape (`use_list` in tree-sitter's grammar)
  is **excluded from extraction entirely**, deliberately, per ADR-0008 —
  confirmed by reading `lang/rust.rs`'s own comment and its
  `use_list_and_wildcard_are_not_extracted` test, not by guessing.
  Concretely: a file whose *only* `use carto_core::...` statement is
  grouped produces **zero** import edges, full stop — not "less precise,"
  *invisible*. `crates/carto-mcp/src/tools/map_tool.rs` genuinely imports
  from `carto_core` (`use carto_core::query::{self, MapQuery,
  QueryGraph};`) but contributes nothing to `carto_core`'s fan-in count,
  simply because it never happens to also have a single-path
  `use carto_core::something;` statement the way its sibling tool files
  do. Isolated with a minimal repro before concluding this — see L3's own
  section below for the reproduction and the full accounting.
- **L4** — assumed carto's zero-fan-in entry-point heuristic would
  *miss* the three library crate roots (`lib.rs` files). It doesn't miss
  them — a crate root genuinely has zero incoming *file-level* `imports`
  edges in carto's model, for the same external-classification reason as
  L3 (a cross-crate reference points at the external module node, never
  at a specific file). The real problem is the opposite of what was
  predicted: the heuristic correctly flags all 4 real entry points, but
  drowns them under ~130 lines of non-code files (READMEs, `Cargo.toml`,
  `.scm` grammar files, ADRs, fixture JSON) that also trivially have zero
  incoming imports.
- **T5** — the first hand-count of `render_capped`'s callers accidentally
  excluded a real production caller (`TaintedString`'s own `Serialize`
  impl, in the same file as the definition) by over-broadly filtering
  out the whole defining file instead of just the definition line.

This section stays in the final document instead of being edited away,
for the same reason `docs/adr/0017`'s "a real false positive, found by
dogfooding" section wasn't quietly fixed either: a benchmark whose own
task-authoring mistakes are invisible is less trustworthy than one that
shows its work.

## Methodology for every task below

1. **Prompt** — the exact question given to both arms (`bench/run.sh`'s
   real-run arm; `bench/replay/`'s deterministic arm).
2. **Corpus** — which repo, and the absolute path used.
3. **Ground truth** — established by reading source and by `rg`/`grep`
   against the raw source, **never** by trusting carto's own answer for
   the *content* of the ground truth.
4. **Carto's actual answer** — since the predictions above needed
   correcting, every task below also records what `carto where`/`deps`/
   `map` genuinely returns (queried independently of either benchmark
   arm, purely to pin down the task's expected-correct-answer envelope
   before real/replay runs happen) rather than a hand-reasoned guess.

---

## 2026-09-22 re-verification, before re-running S-1

Six weeks and 20 ADRs (0020–0039) after the numbers below were recorded,
a prod-readiness review flagged that re-running `bench/run.sh` against
the *same* 8 tasks without first re-deriving their ground truth would
not be a valid comparison — a theoretical risk that turned out to have
concrete, confirmed instances, not just staleness in the abstract:

- **T8's ground truth is provably wrong, not just old.** Its target
  function, `indexer::build_and_persist`, was directly modified in the
  same work session that produced this correction (multi-root support,
  ADR-0034–0039 added `ComponentSet::discover`/`components()` calls to
  it) — the exact call list the original entry records as "known with
  certainty" no longer matches the function's actual current body.
- **L3's entire premise no longer holds.** The task existed to
  demonstrate that Rust's grouped `use a::{b, c};` imports are invisible
  to carto (ADR-0008). ADR-0022, part of the post-S-1 improvement plan
  *this benchmark's own first run motivated*, fixed exactly that gap.
  Re-verified fresh (`carto deps <carto_core-module-id> --dir in`
  against the current repo): carto now finds **24 of 24** real
  importers (`rg -l "^use carto_core::" crates` plus `main.rs`'s
  fully-qualified references) — zero missed, zero false positives.
  Compare the original "11 of 18, missing 7."
- **T6's failure mode is directly addressed by a feature that didn't
  exist yet.** `docs/post-s1-improvement-plan.md` names T6 as the
  motivating case for §1.1 (ADR-0020's honest-absence signal): the
  original grading's sharpest finding was that carto returns 5
  irrelevant test-only callers with *no indication anything real is
  missing*. Re-verified fresh: `carto deps <graph::load-id> --dir in`
  now explicitly reports `root_uncaptured_inbound_calls: 15` alongside
  the same 5 test-only edges — the exact silent-omission failure T6 was
  built to catch is now flagged, not silent. The 6 real production
  callers (still path-qualified `graph::load(...)`, still ADR-0008-
  excluded) are still not *resolved* — only now honestly disclosed as
  missing.
- **Every carto's-own-repo task's raw numbers drifted with repo growth**
  (~181 → 474 walked files): L3's importer count (18→24, see above), L4's
  noise-row count (~130→410), T5's production-caller count (4→7, three
  new sites from the contracts feature: `query/orphans.rs::run`,
  `query/contract.rs::run`, `query/deps.rs::summarize`), T6's real-caller
  count (6→10, contract/orphans command handlers added), T8's call-site
  count (14→~23 occurrences, see above). The zed-corpus tasks (L1, L2,
  T7) drifted less dramatically (zed's own upstream commits, not carto's
  capability changes) but every specific number/line reference needs a
  fresh source read, not the numbers below.

**Corpus note:** `~/playground/zed` no longer existed on the machine
this re-verification ran on and was re-cloned fresh
(`git clone --depth 1 https://github.com/zed-industries/zed`,
commit `62e5991dd0f0c8a3af8d5e7e9c4652490d468db8`, 2026-09-22) — the
original run never recorded a pinned commit, a reproducibility gap this
closes going forward. carto's own repo is used at whatever commit is
checked out when the benchmark actually runs, per its own nature.

**What follows below is the original 2026-08-03 write-up, unchanged** —
correction kept visible per this file's own house style (see L3's own
methodology-correction section above), not silently edited over. Each
task affected carries a dated pointer back to this section rather than
having its numbers rewritten in place; **the numbers actually used for
grading a fresh run are the ones in this section, not below.**

---

## Category L — lookup / orientation (carto predicted to win)

### L1 — repo orientation (zed)

**Prompt:** "Give me a structural orientation of this repository: is it
a single crate or a workspace, roughly how large is it, and what are its
main architectural pieces?"

**Corpus:** `~/playground/zed`

**Ground truth** (`ls crates | wc -l`; `grep -A3 '^\[workspace\]'
Cargo.toml`; top-level `find`): a Cargo workspace (`[workspace]`,
`resolver = "2"`) with **236 crates** under `crates/`; top-level dirs
include `crates/`, `extensions/`, `assets/`, `docs/`, `script/`,
`.github/`, `.cloudflare/`, `nix/`, `tooling/`. A correct answer
identifies it's a large, multi-crate Rust workspace (order of magnitude
~200+ crates), not a single-crate project, and names at least a few real
top-level pieces.

**Carto's actual answer** (`carto map --budget 100`): `files=3754
symbols=58638 modules=330`, `edges: calls=82911 contains=54554
imports=4789`, plus a top-modules-by-fan-in/out ranking and an
entry-points list — a materially different *kind* of answer than the
grep-only arm's (exact structural counts, not an approximation from
reading a handful of files), verified in Part 0 against zed's real scale
(3,754 walked files is the same figure `bench/field-log.md` recorded
independently via `carto index`'s own summary).

**2026-09-22 re-verification** (see the section above): re-cloned zed,
245 crates now (was 236, still "order of magnitude ~200+" — grading
criterion unaffected). Carto's fresh answer: `files=4050 symbols=68374
modules=788`, `edges: calls=150959 contains=68374 imports=15904
references=70947` — same kind of answer, larger numbers, plus a
`references` edge kind (ADR-0029) that didn't exist in the original run.

### L2 — symbol lookup (zed)

**Prompt:** "Where is `truncate_and_trailoff` defined, and what's its
exact signature?"

**Corpus:** `~/playground/zed`

**Ground truth** (`rg -n "^pub fn truncate_and_trailoff" crates`):

```
crates/util/src/util.rs:58:pub fn truncate_and_trailoff(s: &str, max_chars: usize) -> String {
```

**Carto's actual answer** (`carto where truncate_and_trailoff --exact`):
one exact match, `crates/util/src/util.rs:58-71`, signature `pub fn
truncate_and_trailoff(s: &str, max_chars: usize) -> String` — correct,
verified against the ground truth above.

**2026-09-22 re-verification**: same file, definition moved to
`crates/util/src/util.rs:95-108` (zed's own churn, not a carto-side
change) — signature unchanged. Carto's fresh answer still correct, and
now labels the match `[util]` (its own component, ADR-0034 multi-root
support — not built yet at the original run).

### L3 — import fan-out (carto's own repo)

**Prompt:** "Which files in this repo import anything from the
`carto_core` crate?"

(Originally scoped narrower — "import `carto_core::query`
specifically" — and rescoped after discovering carto can't answer
sub-crate-path questions at all, addressed below; asking a question
carto has no mechanism to answer even in principle would only measure
the grep-only arm.)

**Corpus:** `/Users/jonatan.reiners/playground/carto`

**Ground truth** (`rg -l "^use carto_core::" -g '*.rs' crates`):

```
crates/carto-cli/src/deps_cmd.rs
crates/carto-cli/src/index.rs
crates/carto-cli/src/map_cmd.rs
crates/carto-cli/src/selfcheck.rs
crates/carto-cli/src/where_cmd.rs
crates/carto-core/tests/ui/no_as_ref_str.rs
crates/carto-core/tests/ui/no_deref.rs
crates/carto-core/tests/ui/no_display.rs
crates/carto-core/tests/ui/no_display_via_query_result.rs
crates/carto-core/tests/ui/no_field_access.rs
crates/carto-core/tests/ui/no_into_string.rs
crates/carto-mcp/src/render.rs
crates/carto-mcp/src/server.rs
crates/carto-mcp/src/tools/deps_tool.rs
crates/carto-mcp/src/tools/index_tool.rs
crates/carto-mcp/src/tools/map_tool.rs
crates/carto-mcp/src/tools/selfcheck_tool.rs
crates/carto-mcp/src/tools/where_tool.rs
```

**18 files by this grep.** (**Further correction, found grading the
real-run batch, not repeated here**: this `^use` pattern itself
undercounts by one — `crates/carto-cli/src/main.rs` references
`carto_core::` only via fully-qualified inline paths, e.g.
`carto_core::Result<u8>`, never a `use` statement, and both real
sessions' answers correctly caught it. True total: 19. Full accounting
in `bench/results/20260803T112447/GRADES.md`'s L3 section, including a
second, opposite-direction correction: `carto-mcp/src/lib.rs` and
`carto-mcp/src/tools/mod.rs` mention `carto_core::` only inside `//!`
doc comments, not real code — both real sessions incorrectly counted
these as importers too, landing on 21, not 19. Left as originally
written below since the tool-reachability finding it documents is
unaffected by the exact total.)

**Carto's actual answer** (`carto deps <carto_core-module-id> --dir in`
— the external `Module` node for the whole crate; `where` can't find it
directly since it only searches `Symbol` nodes, a separately-documented
limitation): **11 of the 18** in this section's own (slightly
undercounted) list, missing exactly 7: `carto-cli/src/selfcheck.rs`, all
5 `tests/ui/*.rs` files, and `carto-mcp/src/tools/map_tool.rs`.

**Root cause, isolated with a minimal reproduction** (a one-file crate
with a single `use carto_core::query::{self, MapQuery, QueryGraph};`
statement and nothing else): produces **zero** import edges. Removing
the braces (`use carto_core::query::find;`) produces one, correctly.
This is not new or surprising behavior once traced to its source —
`crates/carto-core/src/lang/rust.rs`'s own comment at the relevant
function and its `use_list_and_wildcard_are_not_extracted` test confirm
Rust's grouped `use path::{a, b};` shape (`use_list` in tree-sitter's
grammar) is excluded from extraction **entirely**, deliberately, per
ADR-0008 and already listed in `docs/STATUS.md`'s "deliberately absent"
section. What hadn't been previously traced through: every one of the 7
missing files' *only* `carto_core` import happens to use this exact
grouped form (verified individually, e.g. `selfcheck.rs`:
`use carto_core::consts::{BIN_NAME, SCHEMA_VERSION}; use
carto_core::{outdir, pathguard};` — both grouped) — while the 11 files
carto *does* find each have at least one **non-grouped** `use
carto_core::single::path;` statement elsewhere in the same file (e.g.
`where_tool.rs`'s `use carto_core::taint::TaintedString;`) that happens
to create the same file→crate edge as a side effect. **The presence of
an edge is a coincidence of a file's *other*, unrelated imports, not a
signal about the import actually being asked about.** Graded on whether
an answer surfaces this — that carto's "11" is systematically biased
toward files with heterogeneous import styles, not a random or
representative sample of true importers — versus reporting 11 as if it
were exhaustive.

**A further correction, caught checking the tool surface an agent
actually has** (not just the graph.json I can read directly but a real
MCP client can't): `carto deps carto_core --dir in` — the call that
produces the 11-file list above — **fails**: `no node ID or symbol
named 'carto_core' found in this index`. `deps`'s `target` argument only
resolves via `where`'s search (Symbol nodes only, confirmed above) or a
literal raw node ID; there is no tool that lists `Module` nodes or
returns their IDs. **An agent restricted to the actual MCP tool surface
cannot reach the 11-file list at all** — the only thing genuinely
reachable is `map`'s aggregate line, `carto_core in=11`, a count with no
way to enumerate its members. The "11 vs. 18, partial credit" framing
above describes what's true of the *graph*; what's true of the *tools an
agent can actually call* is starker: this task is a clean loss for carto
as an MCP-only interaction — the correct answer (18 files, or even the
biased 11) is unreachable, and `map`'s count is the only signal
available, unlabeled as partial and with no path to the underlying list.
Kept in the task set anyway, specifically *because* it's a clean loss:
Category T already covers the `calls`-edge sparsity story; this is a
different, real gap (`where`/`deps` have no `Module`-node discovery
path) worth measuring on its own terms rather than folding into the same
narrative.

**2026-09-22 re-verification** (see the section above for the headline
number): both of this task's two documented failures are now fixed, not
merely improved. `rg -l "^use carto_core::" -g '*.rs' crates` plus
`main.rs`'s own fully-qualified references now total **24 files**
(5 new since Aug: `component_arg.rs`/`contract_cmd.rs`/`orphans_cmd.rs`,
`contract_tool.rs`/`orphans_tool.rs` — all from the contracts feature,
ADR-0025–0027, added after this write-up). `carto deps
7ceb829e0163fd98 --dir in` (still needing the module's node ID as of
this note, see below) returns **exactly those 24, zero missed, zero
false positives** — the "biased toward heterogeneous-import files" gap
is gone along with the underlying extraction bug (ADR-0022).
Separately, the MCP-unreachability finding is *also* independently
fixed: `where carto_core` (ADR-0021, Module/File discovery in
`where`/`deps`, landed as part of the same post-S-1 improvement plan)
now finds the `Module` node directly by name, and `carto deps
carto_core --dir in` resolves and returns the full correct list with no
node-ID indirection needed at all — the "unreachable via the actual MCP
tool surface" framing this task's whole second half was built around no
longer applies. This task, as originally scoped, is very likely now a
clean carto win on both of its own documented axes; grade against the
24-file list above, not the 18/11 numbers in the write-up below.

### L4 — real entry points vs. carto's heuristic (carto's own repo)

**Prompt:** "What are this repo's real entry points — the binaries and
library crate roots?"

**Corpus:** `/Users/jonatan.reiners/playground/carto`

**Ground truth** (`grep -rn "\[\[bin\]\]" -A2 crates/*/Cargo.toml`, plus
locating every crate's `src/lib.rs`): one binary, `carto`
(`crates/carto-cli/Cargo.toml`, `path = "src/main.rs"`), and three
library crate roots: `crates/carto-core/src/lib.rs`, `crates/carto-mcp/
src/lib.rs`, `crates/carto-grammars/src/lib.rs`. **4 real entry
points.**

**Carto's actual answer** (`carto map --budget 400`'s "## entry points"
section): all 4 real entry points **are present** — `main.rs` (both
zero-fan-in *and* named `main`) and all three `lib.rs` files (each
genuinely has zero incoming file-level `imports` edges, since nothing
else in the *same crate* imports its own crate root, and cross-crate
references point at the external module node, not the file — same
mechanism as L3). But they're listed alongside **~130 other files** —
every `README.md`, `Cargo.toml`, ADR, `.scm` grammar query file, golden
JSON fixture, and non-code fixture in the repo, all of which trivially
have zero incoming `imports` edges simply because nothing ever imports a
markdown file. Graded on whether the answer surfaces this signal-to-
noise problem (documented already in `docs/STATUS.md` as a known,
labeled heuristic limit) rather than reading off the raw list uncritically.

**2026-09-22 re-verification**: the 4 real entry points are unchanged
and still all present in carto's answer (still verified via
`crates/*/Cargo.toml` and each crate's `src/lib.rs`). The noise grew
roughly 3x — 410 non-header rows now (was ~130), tracking the repo's own
growth (more ADRs, fixtures, goldens). The task's actual point — does
the answer notice and surface the noise rather than reading the list
literally — is if anything a sharper test now than before, not a
weaker one.

---

## Category T — transitive / blast-radius (carto predicted to struggle)

### T5 — direct-caller impact (carto's own repo)

**Prompt:** "What directly calls `TaintedString::render_capped` in
production code (not test code)?"

**Corpus:** `/Users/jonatan.reiners/playground/carto`

**Ground truth** (`rg -n "\.render_capped\(" crates`, excluding only the
`pub fn render_capped` definition line itself, then classifying each
call site's enclosing function as production or `#[cfg(test)]`):

Production (4): `where_cmd.rs::capped()`, `where_tool.rs::capped()`,
`redact::redact_tainted_string()`, and `TaintedString`'s own `impl
Serialize` (`taint.rs:135`) — this last one was missed on the first pass
of this document (see the methodology-correction section above) by
over-broadly excluding the whole defining file instead of just the
definition line.

Test-only (9): 3 in `redact::mod::tests`, 6 in `taint::tests` — real
call sites, but not "production impact."

**Carto's actual answer** (`carto deps render_capped --dir in --depth
1`): resolves the target unambiguously (the name is unique repo-wide)
and returns **9 `calls` edges**, `inferred` confidence — mixing all 4
production callers with 5 of the 9 test callers undifferentiated (test
vs. production isn't a distinction carto's model makes at all). Since
`render_capped` is invoked via `receiver.render_capped(...)` (a
`field_expression`/method-call shape `calls.scm` *does* capture — only
`Type::method()`/`module::func()` path-qualified calls are ADR-0008-
excluded, not plain dot-method calls), this is the one Category T task
where carto's raw recall is genuinely strong; the honest grading
question is whether an answer correctly separates the 4 that matter from
the 5 that don't, since carto's own output doesn't do that separation.

**2026-09-22 re-verification**: 3 new production callers exist, all
from the contracts feature (ADR-0025–0027, added after this write-up):
`query/orphans.rs::run` (×2 call sites), `query/contract.rs::run` (×4),
`query/deps.rs::summarize` (×1) — **production is now 7, not 4**
(`capped`×2, `run`×2, `redact_tainted_string`, `serialize`,
`summarize`). Test-only is still 9 (unchanged). Carto's fresh answer
(`carto deps render_capped --dir in --depth 1`) returns **11 `calls`
edges** — still all 7 production callers plus a subset of the 9 test
callers, undifferentiated, the identical shape as before at larger
scale. The grading question is unchanged; the raw counts to grade
against are 7 production / 9 test, not 4 / 9.

### T6 — direct-caller impact, the sharp negative case (carto's own repo)

**Prompt:** "If `graph::load`'s function signature changes, which files
need updating?"

**Corpus:** `/Users/jonatan.reiners/playground/carto`

**Ground truth** (`rg -n "graph::load\(" crates`, confirmed no bare
imported `load(...)` call exists anywhere outside `load.rs` itself):

```
crates/carto-cli/src/map_cmd.rs:41
crates/carto-cli/src/deps_cmd.rs:72
crates/carto-cli/src/where_cmd.rs:47
crates/carto-mcp/src/tools/deps_tool.rs:66
crates/carto-mcp/src/tools/map_tool.rs:35
crates/carto-mcp/src/tools/where_tool.rs:43
```

**Exactly 6 real call sites** — one per read-path command handler (3
CLI + 3 MCP) — all path-qualified `graph::load(...)` calls.

**Carto's actual answer** (`carto deps load --dir in --depth 1`):
**zero** of the 6 real answers — confirmed, since every one is a
`module::func()` call, ADR-0008's canonical excluded shape, never even
attempted. But the answer isn't *empty*: it returns **5 unrelated
`calls` edges**, all from `load.rs`'s own unit tests (e.g.
`garbage_json_is_a_data_error`, `wrong_schema_version_is_a_data_error`),
which call `load(...)` *bare* (same-file, tier-a resolution, since
they're in the same module as the definition) and so resolve cleanly.
**This is the sharpest, most instructive result in the whole task set**:
carto doesn't fail by returning nothing — it returns a plausible-looking,
correctly-confident list of real callers that is *entirely beside the
point* of the question asked, silently omitting the one thing (the 6
production command handlers) that actually matters. An agent that trusts
this answer without independent verification would conclude the wrong
files need updating.

**2026-09-22 re-verification — this task's own documented failure mode
is directly targeted by a capability that didn't exist at the original
write-up.** `docs/post-s1-improvement-plan.md` names *this task* as
§1.1's motivating case. `bare load` is now ambiguous (a second `load`
exists, `contracts::mod::load`, added by the contracts feature) — use
`carto deps <graph::load's-own-node-id> --dir in --depth 1` instead.
Real call-site count is now **10**, not 6 (contract/orphans command
handlers added: `contract_cmd.rs`, `orphans_cmd.rs`,
`contract_tool.rs`, `orphans_tool.rs`), all still path-qualified and
still not resolved as `calls` edges — that part of the failure is
unchanged by design (ADR-0008). But the answer itself changed
materially: carto's fresh response now explicitly states
`root_uncaptured_inbound_calls: 15` alongside the same 5 test-only
edges — the *exact* silent-omission this task exists to demonstrate is
now flagged, not silent (ADR-0020/0023). Grading this fresh run against
the original "carto silently misleads, an agent would conclude the
wrong files need updating" framing would be scoring a bug that's fixed;
grade instead on whether the *current* answer (5 correct-but-beside-
the-point edges, explicitly flagged as incomplete, real 10-site list
still not resolved) is enough for the agent to avoid being misled —
materially better than the original, but the underlying `calls` edges
for the 10 real callers are still genuinely absent.

### T7 — incoming calls with confidence (zed)

**Prompt:** "What calls `truncate_and_trailoff`, and with what
confidence should I trust each one?"

**Corpus:** `~/playground/zed`

**Ground truth, raw call-site occurrences** (`rg -n
"truncate_and_trailoff\(" crates`, excluding the definition): **27
total** — **17 bare** (`truncate_and_trailoff(...)`, no `util::` prefix
— the shape carto's extractor can attempt) across
`activity_indicator.rs` (×2), `debugger_ui/dropdown_menus.rs` (×3),
`terminal.rs` (×3), `tasks_ui/modal.rs` (×1), `git_ui/commit_view.rs`
(×1), and `util.rs`'s own unit test (×7, same-file); **9 path-qualified**
(`util::truncate_and_trailoff(...)`, ADR-0008-excluded) across
`title_bar.rs` (×2), `search/project_search.rs` (×1), `agent/tools/
update_title_tool.rs` (×1), `agent_ui/conversation_view/thread_view.rs`
(×1), `editor/items.rs` (×2), `git_ui/blame_ui.rs` (×1), `git_ui/
picker_prompt.rs` (×1).

**Unit-of-measurement note:** `carto deps`'s edges are per *calling
symbol*, not per raw call-site occurrence — two call sites inside the
same function merge into one edge (spec §4.3's duplicate-merge rule).
Raw grep counts and carto's edge counts are therefore not directly
comparable 1:1; ground truth for grading is the **set of distinct
calling functions**, not the occurrence count above.

**Carto's actual answer** (`carto deps truncate_and_trailoff --dir in
--depth 1`, queried against zed's real index): **8 `calls` edges**, all
`inferred` — `terminal::title`, `debugger_ui::dropdown_menus::
{label_element, render_thread_dropdown, dropdown_label}`,
`tasks_ui::modal::render_match`, `activity_indicator::{render,
content_to_render}`, `git_ui::commit_view::tab_content_text` — spanning
6 of the 10 files with bare calls (only `util.rs`'s own test module and
one other bare-call site's enclosing function weren't independently
surfaced as separate edges, consistent with multiple call sites per
function merging). **All 9 path-qualified callers are absent** —
confirmed, not one `title_bar.rs`/`editor/items.rs`/etc. caller appears
anywhere in the response, and critically, **nothing in the response
indicates anything is missing** — no `unresolved_calls` entry, no low-
confidence marker, just silence. Every edge that *is* present carries
`inferred`, correctly signaling "verify before trusting," but the
9 entirely absent callers get no signal of any kind.

**2026-09-22 re-verification** (re-cloned zed, commit
`62e5991dd0f0c8a3af8d5e7e9c4652490d468db8`): raw occurrences shifted —
zed's own code changed a path-qualified call site (`agent/tools/
update_title_tool.rs` no longer contains this call) and added a new
bare one (`agent_ui/agent_diff.rs`) since Aug 3; re-derive the exact
per-file breakdown fresh at grading time rather than reusing the list
above. The structural finding is the same shape as T6's: carto's fresh
answer (`carto deps truncate_and_trailoff --dir in --depth 1` against
the re-cloned corpus) returns 8 `calls` edges, all `inferred`, **plus
now `root_uncaptured_inbound_calls: 8`** — the exact "nothing indicates
anything is missing" gap this task was built to demonstrate is fixed
the same way T6's is (ADR-0020/0023, landed after this write-up). The
9 path-qualified callers are still not resolved as edges — only the
*disclosure* that something's missing changed, not the underlying
extraction gap (ADR-0008, unchanged by design).

### T8 — outgoing dependencies (carto's own repo, self-authored ground truth)

**Prompt:** "What does `carto_core::indexer::build_and_persist` call or
depend on directly?"

**Corpus:** `/Users/jonatan.reiners/playground/carto`

**Ground truth:** this function was authored in this same work session
(`crates/carto-core/src/indexer.rs`), so its full call list is known
with certainty rather than derived by search:

- Path-qualified (6): `pathguard::PathGuard::new`, `walk::walk`,
  `lang::extract_and_resolve`, `Graph::new`, `gitinfo::head_sha`,
  `graph::persist`.
- Receiver-method calls (8 distinct call expressions, 6 distinct method
  names — `insert_node` appears twice, once per loop): `.len()`
  (`walked.nodes.len()`), `.insert_node()` ×2, `.insert_edge()`,
  `.clone()` (`commit_sha.clone()`), `.out_root()`, `.to_path_buf()`,
  `.total()` (`manifest.redaction.total()`).

**Carto's actual answer** (`carto deps build_and_persist --dir out
--depth 1`): **4 resolved `calls` edges** (`.total()`, `.out_root()`,
`.insert_edge()`, `.insert_node()`) plus an explicit
`root_unresolved_calls` list naming **4 more attempted-but-unresolved**
call sites (`len`, `clone`, `Ok`, `to_path_buf`) — `Ok(...)` parses as a
call to an unrecognized identifier (an enum-variant constructor, not a
locally-defined function). **All 6 path-qualified calls are completely
absent, not even in `unresolved_calls`** — confirmed, the sharpest
possible three-way split in the whole task set: resolved (4), attempted-
but-honestly-flagged-unresolved (4, INV-8's honest-omission design
visibly working), and silently never-attempted (6, the ADR-0008
exclusion, invisible by construction). Written and verified in the same
session the function itself was authored, before `deps` was run on this
symbol even once — no risk of the ground truth being reverse-engineered
from carto's own answer.

**2026-09-22 re-verification — this ground truth is not merely stale,
it is provably wrong as written**, confirmed by direct inspection of
`crates/carto-core/src/indexer.rs`'s current body:
`build_and_persist` was itself modified by the multi-root support work
(ADR-0034–0039) in the intervening six weeks, adding real new calls.
Current, re-derived from the actual source:

- Path-qualified (**7**, was 6): the original 6 plus
  `ComponentSet::discover`.
- Receiver-method calls (**~17 distinct call expressions, 13 distinct
  method names**, was 8/6): the original set plus
  `.component_of_path()`, `.map()`, `.components()` (×2 — once feeding
  `extract_and_resolve`, once for `.len()`), `.config_digest()`,
  `.to_string()`, `.components_sorted_by_path()`.

**Carto's fresh answer** (`carto deps build_and_persist --dir out
--depth 1`): **7 resolved `calls` edges** now (was 4) —
`insert_edge`, `config_digest`, `total`, `insert_node`, `out_root`,
`components_sorted_by_path`, `component_of_path`. `root_
unresolved_calls` lists **9** attempted-but-unresolved sites (was 4):
`len`×2, `map`, `components`×2, `clone`, `to_string`, `Ok`,
`to_path_buf`. And — the change that matters most for this task's own
point — **`root_uncaptured_outbound_calls: 7`** is now reported
explicitly, naming the count of the 7 path-qualified calls that are
still never attempted (ADR-0023, the outbound half of the same
honest-absence signal T6/T7 rely on, landed after this write-up). The
"sharpest possible three-way split" this task documents is now visible
*as three actual numbers in carto's own output* (7 resolved / 9
honestly-unresolved / 7 honestly-uncaptured) rather than the third
bucket being invisible by construction — grade against these three
numbers, not the original 4/4/6.

---

## What "correct" means for scoring (`bench/score.py`)

Each task is graded **correct / partial / wrong** against the ground
truth above, by direct comparison — not by carto's own claimed answer,
which is exactly the arm being measured. T6 and T8 in particular should
be graded on whether an answer surfaces *what's missing*, not only
whether it lists what carto found: a technically-accurate-but-silent
omission (T6's empty real-callers list backfilled with irrelevant test
callers; T8's six invisible path-qualified calls) is a worse outcome
than an honestly-flagged gap, and the grading should reflect that
distinction rather than scoring recall alone. Token counts come from
`bench/run.sh` (real `claude -p --output-format json` sessions, exact
`usage` block) and `bench/replay/` (deterministic byte counts, calibrated
to tokens using the real-run arm's own ratio — no external tokenizer is
available offline in this environment). Both arms pre-build each
corpus's index once, outside any per-task measurement, before their
per-task loop starts — `bench/replay/replay.sh` always did this
(`IDX_CARTO`/`IDX_ZED`, reported as a separate "index-build cost" line);
`bench/run.sh` adopted the same convention after real batches showed
graded sessions invoking `index` themselves both wasted redundant
per-task cost and, worse, risked a permission denial on the large
output indexing zed produces (see `bench/field-log.md`). No task's
token/cost number below includes index-build cost as of that change.

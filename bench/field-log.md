# Field log — real numbers from doing the actual work

This file is not a formal benchmark (that's `bench/tasks.md` +
`bench/run.sh`/`bench/replay/`, still to come). It's a running log of real
`carto` invocations made while building the MCP server and benchmark
harness, per the working instruction to extract numbers from doing the work,
not only from a lab afterward. Misses and non-uses are logged too — a log
with no misses would be evidence of nothing.

## 2026-08-03 — Part 0: feasibility spike

### Index build: carto's own repo vs. zed

`cargo build --release -p carto-cli`, then `/usr/bin/time -l carto index
<repo> --out <dir>`, macOS arm64, 8 cores.

| Repo | Files indexed | Nodes | Edges | Wall time | Peak RSS | `graph.json` |
|---|---:|---:|---:|---:|---:|---:|
| carto (this repo) | 163 | 1,174 | 2,052 | 0.57 s | 30 MB | 1.1 MB |
| zed (`~/playground/zed`) | 3,754 | 58,638 | 142,254 | 13.34 s | 302 MB | 72 MB |

Both well inside `consts::MAX_NODES` (2,000,000) and `MAX_GRAPH_BYTES` (512
MiB) — no scoping fallback needed, zed indexes cleanly at full size.
`carto index` does not itself crash, hang, or need `--subpath` narrowing on a
~1,900-Rust-file repo — first real data point for spec S-2 (target: <30 s
cold on a 5,000-file repo; zed's 3,754 files in 13.34 s is on that curve,
though S-2's corpus is deliberately mixed-language and non-`--release` numbers
would differ).

Reading `walk`'s output: 3,754 "files indexed" vs. 1,791 `.rs` files under
`~/playground/zed` (excluding `target/`) — the walker (via the `ignore`
crate, respecting `.gitignore`) is counting every non-ignored file
(`Cargo.toml`s, docs, JSON fixtures, etc.), not just source. Expected, not a
bug — matches `fixtures/mixed`'s own "noise dirs get walked, non-source
files become opaque `File` nodes" design.

### Call-resolution rate — quantifying the ADR-0008 sparsity risk

The plan flagged, before running anything, that ADR-0008's exclusion of
Rust path-qualified calls (`Type::method()`, `module::func()`) would likely
make zed's `calls` graph sparse, because idiomatic Rust leans on that syntax
heavily. Measured directly from both graphs (`calls` edges vs. total
`unresolved_calls` recorded on `Symbol` nodes):

| Repo | Resolved `calls` edges | Unresolved call sites | Resolution rate |
|---|---:|---:|---:|
| carto (this repo) | 948 | 3,278 | 22.4% |
| zed | 82,911 | 285,936 | 22.5% |

Two things worth noting:

1. **The rate is nearly identical across a 102-file hand-written Rust
   codebase and a 1,900-file production one.** This wasn't obvious in
   advance — it could plausibly have been carto-specific (e.g. more
   free functions, less `impl`-method-heavy style) rather than a general
   property of bare-name call resolution over idiomatic Rust. Instead it
   reads as a structural fact about the resolution policy itself, not
   noise from one small fixture.
2. **A spot check of one real unresolved case** — `find_project_entry` in
   `crates/project_panel/src/project_panel_tests.rs:9408`, a
   `pub(crate) fn` with a `pub(crate)` signature and zero resolved outbound
   `calls` edges — shows exactly the `ADR-0008`-predicted shape: all 7 of
   its call sites (`update`, `read`, `worktrees`, `read`, `strip_prefix`,
   `entry_for_path`, `map`) are chained method calls on typed values
   (`panel.update(cx, |panel, cx| panel.project.read(cx).worktrees()...)`),
   none of them bare free-function names — exactly the call shape
   `resolve.rs`'s bare-name-only tiers can't disambiguate.

This is a real, load-bearing number for the benchmark design: it's why
`bench/tasks.md` (next) splits into Category L (lookup/orientation, where
carto's `where`/`map`/`imports` graph is complete regardless of call
resolution) and Category T (transitive/blast-radius, where this ~78% miss
rate on Rust call sites is expected to directly suppress `deps`'s answer
quality). Category T on zed is the task set most likely to show carto
*losing* to grep — flagged in the plan before this measurement, now backed
by a number instead of a guess.

### `where`/`deps` sanity checks against the zed index

`carto where find_project_entry ~/playground/zed --out <dir> --json` — exact
single match, correct `file:line`, correct signature, in a repo carto had
never seen before this session. Multi-hit case:
`carto where workspace --exact --limit 5` returned 5 same-named methods
across different files/types (`agent`, `agent_ui`, `editor`, …), each with
distinct signatures — this is the substring/exact matching correctly
surfacing *all* candidates rather than silently picking one, which is
exactly the honesty behavior spec §7.1 asks for (no disambiguation
heuristic invented where none is warranted).

No misses logged yet at this stage — Part 0 was infrastructure/measurement
only, not yet real navigation. The next entries (Part A: building
`carto-mcp`) will log real `where`/`deps`/`map` calls made in the course of
navigating carto's own codebase.

## 2026-08-03 — Part A: real MCP client connectivity check

Built `crates/carto-mcp` (hand-rolled JSON-RPC 2.0 stdio server, no
`rmcp` — ADR-0018) and wired `carto serve`. Beyond the 34 in-crate unit
tests and 5 `assert_cmd` end-to-end tests (real binary, real stdin/
stdout, no LLM involved), verified against an actual MCP client: a real
`claude -p` session with `--mcp-config` pointed at `carto serve` and
`--allowedTools mcp__carto__*`.

Prompt: call `selfcheck`, then `index` this repo, then `where redact
--limit 3`. Real result (`claude -p --output-format json`):

- 3 tool calls made, in the requested order, tools correctly discovered
  via `tools/list` and invoked via `tools/call` — no protocol errors, no
  malformed-schema rejections from the client.
- `selfcheck` → carto 0.1.0, schema v1, confinement digest reported.
- `index` → 178 files, 1,276 nodes, 2,228 edges, 17 redactions, at the
  real current commit — a genuinely fresh index, not a cached fixture.
- `where redact --limit 3` → `RedactionCounts`, `redact()`,
  `redact_tainted_string()`, all correctly attributed to
  `crates/carto-core/src/redact/mod.rs` — matches the CLI's own answer
  for the same query (spot-checked separately via `carto where redact`).
- Cost/usage for the whole 3-tool-call, 5-turn session (`sonnet`,
  real API usage block): 10 fresh input tokens, 33,760 cache-creation
  tokens, 130,690 cache-read tokens, 786 output tokens, $0.254 total,
  15.3s wall.

One methodology note worth recording: this check ran *without* `--bare`
(normal OAuth session auth) — `--bare` refuses OAuth/keychain auth by
design, and no `ANTHROPIC_API_KEY` is set in this environment. This
check answers "does a real client work against the server," not "what
does a clean token count look like" — the cache-heavy usage numbers
above (bare session start, no prior conversation) aren't the controlled
baseline the benchmark needs, and shouldn't be read as one. (Update
below: `--bare`'s unavailability turned out to matter for the real S-1
measurement too, not just this one check.)

## 2026-08-03 — Part B: building bench/tasks.md, and a real Rust extractor gap found writing it

Drafting the 8-task S-1 set (spec §11.4), every task was designed with a
*hand-reasoned prediction* of carto's answer before running carto at
all, then checked against real `carto where`/`deps`/`map` output before
locking the task in. That check caught three wrong predictions — see
`bench/tasks.md`'s own "A methodology correction, kept visible rather
than quietly fixed" section for the full accounting. The most
significant: an L3 task about which files import `carto_core::query`
led to discovering, via a minimal one-file reproduction
(`use carto_core::query::{self, MapQuery, QueryGraph};` alone, indexed
in isolation), that carto's Rust extractor produces **zero** import
edges for any grouped `use path::{a, b};` statement — confirmed by
reading `lang/rust.rs`'s own comment and its
`use_list_and_wildcard_are_not_extracted` test: this is
`docs/STATUS.md`'s already-documented "deliberately absent" `use_list`
exclusion (ADR-0008), not a new bug — but a consequence of it hadn't
been traced through before: a file whose *only* import from some
external crate is grouped contributes **nothing** to that crate's fan-in
count, and whether carto's answer includes a file at all becomes a
coincidence of that file's *other*, unrelated single-path imports.
Verified precisely on carto's own repo: 18 files really import from
`carto_core`; carto's `map`/`deps` machinery surfaces 11; the missing 7
are exactly the 7 whose sole `carto_core` import is grouped.

Digging one step further (does an MCP-only agent even have a path to
the 11-file answer, not just the true 18): **no** — `carto deps
carto_core --dir in` fails outright (`no node ID or symbol named
'carto_core' found`), since `where` only searches `Symbol` nodes and no
tool exposes `Module`-node IDs. The only thing an agent can actually
reach is `map`'s aggregate line, `carto_core in=11` — a count with no
way to enumerate members. Reading `graph.json` directly (as this
investigation did) isn't something a real MCP client can do.

**This is exactly the kind of finding the whole exercise exists to
surface** — a real, previously-unconnected consequence of a documented
design decision, found by trying to use the product for something a
user plausibly would ask, not by inventing a synthetic edge case.

## 2026-08-03 — Part B: the deterministic replay arm, run for real

`bench/replay/replay.sh` (free, no LLM) ran successfully against both
corpora. Index-build cost: carto's own repo 1s wall / 237-byte summary;
zed 12s wall / 239-byte summary (both consistent with Part 0's numbers).
Per-task byte counts, carto vs. grep-authored command sequences:

| task | grep bytes | carto bytes | carto vs grep |
|---|---:|---:|---:|
| L1 | 228 | 1,071 | −370% (carto larger) |
| L2 | 905 | 291 | +68% (carto smaller) |
| L3 | 2,357 | 6,901 | −193% |
| L4 | 227 | 6,901 | −2,940% |
| T5 | 1,235 | 2,385 | −93% |
| T6 | 579 | 1,882 | −225% |
| T7 | 3,157 | 2,419 | +23% |
| T8 | 2,731 | 1,238 | +55% |
| **total** | **11,419** | **23,088** | **−102%** |

**Carto uses roughly 2x more bytes than grep overall in this run** — the
opposite direction from S-1's hypothesis. Reported plainly rather than
softened, but with an important caveat spelled out before drawing any
conclusion from it: **I authored both arms' command sequences already
knowing the ground truth**, and the grep sequences in particular
(L1 especially — three quick commands totaling 228 bytes for
"orientation of an unfamiliar 236-crate codebase") are almost certainly
far cheaper than what a real agent would actually explore blind. L1,
L3, and L4's large negative deltas share one real, non-authorial-bias
cause too: `map --json` always returns the *entire* rendered overview
(counts, top modules, external packages, entry points) — there's no way
to request just one section, so even a narrowly-scoped question pays
the full payload. This is a genuine API-shape cost, not just a fair-
baseline artifact, and worth carrying into the real-run arm's
interpretation rather than assuming the replay numbers are simply
"unfair to carto" and dismissing them.

## 2026-08-03 — Part B: the real-run arm's auth path, decided empirically, twice

First attempt: `claude --bare --mcp-config ...` — fails outright
(`Not logged in`), confirming `--bare` needs `ANTHROPIC_API_KEY`
specifically, unavailable here. Second attempt, trying to keep
`--bare`-level isolation some other way: `--safe-mode` (keeps normal
OAuth auth, disables CLAUDE.md/hooks/plugins) combined with
`--mcp-config`/`--strict-mcp-config` — **`--safe-mode` disables MCP
servers too**, `--strict-mcp-config` doesn't override that. The agent
inside that session, finding no `carto__*` tools available, tried
running the locally-built `carto serve` binary itself via Bash as a
workaround, got denied by permissions, and gave up after two attempts —
a real, if minor, illustration of an agent improvising around a missing
tool rather than reporting the gap. **Cost: $0.68, 211s, for a session
that answered nothing** — a concrete, unpleasant data point directly
informing the decision (surfaced to the user) to proceed carefully with
the paid 16-session batch rather than assume the setup was right.

Working setup, confirmed by the earlier successful connectivity check
above (before this session's methodology firmed up): plain `claude -p`
(no `--bare`, no `--safe-mode`) with `--mcp-config`/`--strict-mcp-config`
for real isolation of *which MCP servers* are exposed, invoked from a
neutral temp-directory cwd (not inside either target repo) to avoid
triggering carto's own elaborate `CLAUDE.md` via directory walk-up. Not
as clean as `--bare` would have been — CLAUDE.md/hook auto-discovery
isn't as provably absent as it would be under `--bare`, an accepted,
user-confirmed caveat on every real-run number that follows, not a
silent substitution.

## 2026-08-03 — Part B: the real-run arm's first full batch, half-invalidated, and why

Ran the full 16-session batch under the setup above. `score.py` reported
a real headline number (carto used ~12% *more* combined input tokens
than grep overall) — but reading every session's actual answer text
before trusting that number caught something more serious than "S-1
failed": **5 of the 8 tasks (L3, L4, T5, T6, T8 — every task using
carto's own repo as corpus) failed completely on *both* arms.** Both
grep and carto sessions reported the working directory as empty ("no
Cargo project here at all"), some carto-arm sessions additionally
reported the MCP server "isn't connected in this session," and every
one of these sessions finished in 2–4 turns with zero permission
denials — meaning the model didn't even attempt to explore the given
path, not that it tried and got blocked.

Root cause, confirmed with cheap (non-benchmark) diagnostic sessions
before touching the real batch again: `bench/run.sh` invoked every
session from a neutral `mktemp` directory and communicated the target
repo only as a path *string* inside the prompt text — never via `cd`,
never via `--add-dir`. Claude Code's filesystem tools (Read/Glob/Grep,
and the permission layer even Bash goes through) are scoped to the
session's actual working directory; a path mentioned in prose carries no
access grant. This *happened* to work for zed in the first run — some
tool calls used a bare `find /path/to/zed/...` that the model chose to
issue with an absolute path baked into the command, which apparently
sailed through — while for carto's own repo, the sessions never
attempted the equivalent and simply reported nothing there. (`--add-dir`
would have been the documented fix, but it's a variadic flag —
`--add-dir <directories...>` — and swallows a following positional
prompt argument whole if not carefully ordered; confirmed this exact
failure mode with a diagnostic session too, before choosing the simpler
fix.)

**Fix:** run each session with cwd set to the actual corpus directory
(`cd "$corpus" && claude -p ...`), dropping the neutral-tempdir
indirection entirely. Verified with one cheap diagnostic (L3's grep
arm, $0.11) before re-running the full paid batch: the same prompt that
previously produced "no repo here" now correctly listed 21 real files
importing from `carto_core`, in two turns, zero denials.

**Cost of the mistake:** the first full batch cost $1.51 and is
unusable for 5 of 8 tasks; combined with the earlier `--safe-mode`
dead-end ($0.68), this session spent roughly $2.20 discovering harness
bugs before getting a trustworthy setup. Surfaced to the user before
spending on the re-run — see this repo's commit history for the
check-in — rather than silently absorbing the cost or, worse, reporting
the broken batch's numbers as if they were valid. The fixed batch's
real results are in `bench/results/<timestamp>/` (the invalidated batch
was deleted, not committed — `bench/results/` was never tracked by
git).

## 2026-08-03 — Part B: the fixed batch's real results, one more flake, and the final numbers

Re-ran all 16 sessions under the cwd fix. One more isolated issue
surfaced immediately, checked before trusting anything else: **L1's
carto-arm session again reported the MCP server "wasn't connected in
this session"** — its answer was accurate (drawn from `git ls-files`/
`Cargo.toml` reading instead) but wasn't actually exercising carto at
all, which would have silently mis-measured the one task most likely to
show carto's real value (whole-repo orientation). Confirmed this was an
isolated flake, not systemic, by checking all 8 carto-arm transcripts for
the same "not connected" language — only L1 showed it. Re-ran L1's carto
arm alone ($0.16, cheap given it's an isolated single-session fix): this
time the MCP tools connected correctly and the answer cited exact
figures straight from `map`'s output ("3,754 source files, ~54,554
symbols, 330 modules — per carto's index"), matching Part 0's
independently-measured numbers exactly.

Every one of the 16 final sessions' actual answer text was read and
graded against `bench/tasks.md`'s ground truth — see
`bench/results/20260803T112447/GRADES.md` for the full per-task
reasoning. Grading itself surfaced one more correction: L3's own ground
truth (18 files, from an earlier `rg -l "^use carto_core::"` pass) was
*also* wrong — missing `main.rs` (which references `carto_core::` only
via fully-qualified inline paths, never a `use` statement). Both real
sessions independently said 21, which led to finding the true count is
**19**: both sessions share 2 identical false positives
(`carto-mcp/src/lib.rs` and `tools/mod.rs`, which mention `carto_core::`
only inside `//!` doc comments) — the exact prose-vs-code confusion this
task's very first draft predicted as a risk, just landing on both arms
equally rather than singling out either one.

**Final numbers** (`bench/score.py bench/results/20260803T112447/`,
`GRADES.json`):

| | grep-only | carto | S-1 threshold | met? |
|---|---:|---:|---|---|
| combined input tokens | 1,323,356 | 1,329,234 | carto ≥30% fewer | **no** — carto uses 18.0% *more* |
| accuracy | 93.8% (7.5/8) | 87.5% (7/8) | carto ≥20 points higher | **no** — carto is 6.2 points *lower* |
| total real cost | $1.43 | $1.20 | — | carto ~16% cheaper in dollars despite more tokens (cheaper cache-read pricing) |

**Neither S-1 threshold is met, on this one-trial measurement.** But the
per-task pattern underneath that headline is more informative than the
headline itself:

- **3 of 8 tasks are clean carto wins on tokens** (L1: 9,327 vs. 36,084
  — a 74% reduction; T5: 10,226 vs. 13,658; T7: 14,693 vs. 19,558), and
  L1's margin alone is large enough that a slightly different task mix
  could flip the aggregate sign entirely — this is exactly the "one
  trial per cell, don't over-read the aggregate" caveat `bench/tasks.md`
  built in from the start, demonstrated with real numbers rather than
  asserted in the abstract.
- **The single accuracy loss (T5) has a specific, legible cause**: the
  agent read carto's own tool output *incompletely* (missed 2 of 4 real
  callers, including misreading one as "the definition itself" rather
  than a separate call site in the same file) — not a case where carto's
  underlying data was wrong. `bench/tasks.md`'s own earlier verification
  of `deps render_capped --dir in --depth 1` confirmed all 4 real callers
  *are* present in carto's actual output at that exact query.
- **The three hardest transitive tasks (T6, T7, T8) are all correct for
  carto — via an honest, repeatedly-observed fallback pattern**: each
  transcript explicitly states that carto's own tool has a known,
  named limitation on that specific call shape (path-qualified Rust
  calls, ADR-0008) and falls back to reading source directly, landing on
  the fully correct answer rather than reporting carto's incomplete data
  as if it were the whole picture. This is the single most encouraging
  qualitative finding in the whole exercise: an agent with both tool
  sets available used carto when it helped and correctly stopped relying
  on it when it didn't, rather than trusting a tool's confident-looking
  but incomplete answer.
- **L3's outcome says something about the skill file, not carto's data**:
  both arms made the identical doc-comment-vs-code-import mistake,
  independent of which tools were available — a prompt/skill wording
  issue (or just an inherent ambiguity in "imports from a crate") more
  than a carto-specific one.

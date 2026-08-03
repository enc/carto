# Post-S-1 improvement plan

**Date:** 2026-08-03 · **Basis:** ADR-0019's S-1 result
(`bench/results/20260803T112447/`, `bench/tasks.md`,
`bench/field-log.md`) · **Status:** slices 1–4 of the slice order below
are **implemented** (ADRs 0020–0022; `docs/STATUS.md`'s milestone entry
has the full list). Slice order step 5 (re-running `bench/run.sh` as a
full three-arm batch against these changes) is **not yet run** — a
separate measurement round the user approves independently, per spec
§11.4. Written in response to the S-1 result, at the user's request, to
turn "neither threshold met" into a concrete next-slice plan rather
than a dead end.

## Why this plan exists instead of a straight M3 go/no-go

S-1 failed on a one-trial measurement, but not uniformly and not for
one reason. Three of eight tasks were clean carto wins; the one
accuracy loss traced to a specific, fixable misread, not bad data; and
the three hardest tasks were answered *correctly* precisely because the
agent recognized a known carto limitation and fell back — evidence the
underlying idea works when the agent knows where the edges are. That
pattern points at concrete, scoped fixes, not a redesign. This plan is
those fixes, prioritized by how directly the evidence supports each one.

Every recommendation below cites the specific finding that motivates it
— nothing here is invented; everything traces to a real number, a real
transcript, or a real repro in `bench/`.

---

## 0. Is MCP even the right integration surface? (implemented, not yet re-measured)

A question that came up discussing this plan, and worth settling before
investing further in either surface: `crates/carto-mcp` and
`crates/carto-cli` call the *exact same* underlying functions
(`query::{find,deps,map}`, `indexer::build_and_persist`) — spec §7.1's
"commands = MCP tools, same core functions" holds structurally, per
ADR-0018. So the choice between them is purely about the *path* an agent
takes to reach an identical answer, not about answer quality. Looking
back at where this session actually lost time and money, that path
mattered more than expected: **every real failure in the S-1
measurement was friction of the MCP transport/config layer specifically**
— the neutral-cwd bug, `--bare` needing an unavailable
`ANTHROPIC_API_KEY`, `--safe-mode` silently disabling MCP servers
outright, and one isolated MCP-connection flake that produced a
plausible-looking answer without ever touching carto's tools. None of
that is inherent to a CLI reached through Bash, which has no handshake,
no separate config file, and no connection state to manage.

That reasoning was itself only a prediction — the original S-1 batch
never actually tested carto-via-Bash-CLI as its own arm, only
grep-only vs. carto-via-MCP. **A third `cli` arm has now been added to
`bench/run.sh`** (and `bench/score.py`, `bench/cli-arm-prompt.md`): same
8 tasks, same read tools, but Bash access to the `carto` binary directly
instead of an MCP connection, guided by a CLI-flavored version of
`skill/carto.skill.md`'s honesty contract. `bench/score.py` reports the
spec-defined grep-vs-carto(MCP) comparison unchanged, plus two new,
explicitly-labeled *exploratory, not-S-1* comparisons: cli-vs-grep and
cli-vs-MCP — the latter isolates whatever the MCP transport itself costs
or saves, independent of carto's answers, which are identical either
way. A quick single-task smoke test (`L2`, $0.23) confirmed the new arm
works end-to-end: the agent used the CLI binary correctly, no permission
denials, correct answer.

**This has not yet been run as a full batch.** Per the user's direction,
it's being implemented now and will run as part of the next full
`bench/run.sh` pass — alongside the highest-confidence items from §1–§3
below — rather than as an isolated measurement, so the eventual
`skill/carto.skill.md` and README recommendation (which surface to lead
with) is based on real numbers from the same measurement round, not
argued from this section's reasoning alone.

---

## 1. What features help

### 1.1 Make "zero edges" distinguishable from "we didn't look" (highest priority) — **implemented, ADR-0020**

**Evidence:** T6 is the sharpest example in the whole benchmark.
`deps load --dir in` returns 5 *real but irrelevant* edges (the
function's own unit tests, which call it same-file) and **zero** of the
6 production callers — all path-qualified `graph::load(...)` calls,
excluded by ADR-0008 design. The response contains no signal that
anything was excluded; `root_unresolved_calls` only covers the query
symbol's own *outbound* calls, not inbound calls the extractor never
attempted to capture at all. An agent that trusted this response without
falling back (as the grep arm — and, notably, the carto arm too, by
choosing to verify — both did) would ship a wrong answer with high
apparent confidence.

**Proposal:** `deps`'s `--dir in` response should carry an honest signal
when the target repo contains call syntax the extractor is known not to
capture (e.g., a language-level flag or count: "N path-qualified/
qualified-call-shaped invocations of this symbol's bare name exist in
this repo but were not attempted for resolution — INV-8 honest-omission,
not evidence of zero callers"). This doesn't require solving path-
qualified call resolution (a real, larger project, see §1.3) — it only
requires the extractor to *count* what it's skipping, which it already
walks past today. Directly extends INV-8 ("nothing heuristic is
presented as resolved") to cover *absence*, not just presence.

### 1.2 A `Module`-node discovery path — **implemented, ADR-0021**

**Evidence:** L3. `carto deps carto_core --dir in` fails outright ("no
node ID or symbol named `carto_core` found") — `where` only searches
`Symbol` nodes; there is no tool-level way to enumerate a `Module`
node's ID or its importers. The only reachable signal is `map`'s
aggregate count line (`carto_core in=11`), with no path from that count
to the underlying file list. Every real-run session that answered this
correctly did so by reading `graph.json`/source directly, bypassing the
tool surface entirely — which an MCP client fundamentally cannot do.

**Proposal:** extend `where` (or add a narrowly-scoped new query) to
match `Module.path` in addition to `Symbol.name`, and let `deps` accept
a `Module` node's canonical path as a `target` alongside symbol names
and raw IDs. Small, mechanical change — `where`'s matching loop already
special-cases `NodeData::Symbol`; this is one more arm, not a new
subsystem.

### 1.3 Group-import extraction (the biggest, most surprising finding) — **implemented for Rust, ADR-0022**

**Evidence:** L3's root-cause investigation (`bench/tasks.md`, `bench/
field-log.md`). A minimal one-file repro confirmed: any Rust
`use path::{a, b};` produces **zero** import edges — not reduced
precision, complete invisibility. On carto's own repo, a file whose
*only* `carto_core` import happens to be grouped contributes nothing to
that crate's fan-in count, and whether a file shows up in `deps`'s
answer becomes a coincidence of that file's *other*, unrelated
single-path imports. This is `ADR-0008`'s documented `use_list`
exclusion, but the consequence — silent, coincidence-driven
under-reporting on *any* fan-in/fan-out question — was never connected
to a real scenario before this benchmark.

**Proposal:** this is worth a real slice, not a footnote. Extracting
`use_list`'s member names (each becomes its own `Absolute`/`Relative`
import, same shape as today's single-path form) is bounded, mechanical
tree-sitter work — the query file's own comment already names
`use_list`/`use_wildcard`/`use_as_clause` as the three excluded shapes;
this proposal is only the first of the three, and only because it's the
one demonstrated to produce silent, not just reduced, information loss.
`use_wildcard` (`use a::*`) is a genuinely different problem (no
enumerable member list) and should stay out of scope here. Before
committing to this, check whether TS/JS's `import { a, b } from 'x'`,
Python's `from x import (a, b)`, PHP's `use X\{A, B}`, and C#'s grouped
`using` (if any) have the same all-or-nothing gap — L3 only demonstrated
it for Rust, but the risk is structural to "one query pattern, list
form excluded" and may be repo-wide across languages, not Rust-specific.

### 1.4 Legible edge provenance (motivated by T5) — **implemented, ADR-0021**

**Evidence:** T5. carto's own `deps render_capped --dir in --depth 1`
output (independently verified while writing `bench/tasks.md`) *does*
contain all 4 real production callers — but the real-run carto-arm
agent found only 2, missing `redact_tainted_string` and, notably,
misdescribing `TaintedString`'s own `Serialize` impl as "the method
definition itself" rather than a separate call site that happens to live
in the same file as the definition. This wasn't a data gap; it was a
legibility gap — the response doesn't visually or structurally
distinguish "a call site in the file where the callee is defined" from
"the callee's own definition."

**Proposal:** no data model change needed. `deps`'s human-rendered text
(and, more usefully, a small structured hint) could flag when a caller
symbol and the target symbol share a file, since that's exactly the
condition that produced this specific misread. A cheap, targeted fix for
a specific, now-documented failure mode — not a general "make output
clearer" mandate.

---

## 2. More efficient output

### 2.1 `map`'s all-sections-every-call payload (the clearest, cheapest win) — **implemented**

**Evidence:** the free replay arm (`bench/replay/replay.sh`) measured
this directly: `map --budget 400 --json` on carto's own repo is 6,874
bytes even for narrowly-scoped questions (L3, L4) that only need one
section. `--budget` shrinks the *whole* response but doesn't let a
caller select "just the counts" or "just entry points" — every section
renders in order, capped only by whether it fits the remaining budget,
never by explicit request. This is real: it's not an artifact of an
unfair grep baseline (the replay arm's own methodology notes call this
out explicitly as a genuine API-shape cost, separate from any grep-
baseline generosity).

**Proposal:** add an optional `--section
counts|modules|entry-points|infra` (repeatable) to `map`'s query struct.
Default (no `--section`) keeps today's full-overview behavior — this is
additive, not a breaking change to the existing contract. Directly
targets L1's own real-run finding: the carto arm's *fixed* L1 session
answered from `map`'s full output at 9,327 combined tokens vs. grep's
36,084 — a 74% win *already*, with the full-payload cost baked in.
Section-scoping should widen that margin further on any question that
doesn't need the whole overview.

### 2.2 Structured content isn't capped, only text is

**Evidence:** ADR-0018/the `carto-mcp` implementation itself.
`render.rs::cap_text` enforces `MCP_TEXT_CAP` (8 KiB) on the text
`content` block only; `structuredContent` is deliberately uncapped,
matching spec §7.2's intent that the JSON payload be the "exact" machine
answer. This is correct as designed — flagged here only because it means
§2.1's fix (letting a caller ask for less) is the *only* lever available
today; there is no complementary "trim what you don't need" on the
`--json`/structured side, and shouldn't be one (a client that wants the
full structured answer should get it) — this is a note to future
implementers, not a proposed change.

### 2.3 Per-session re-indexing cost isn't amortized

**Evidence:** L1's fixed carto-arm session (`cache_creation_input_tokens:
9,311`) and L3/T-series carto-arm sessions each carry a real, one-time
index-build cost that a fresh session pays again even when a prior
session in the same benchmark run already indexed the same repo at the
same default out-dir. `bench/tasks.md`'s own methodology section already
flags index-build cost as needing separate accounting; the real-run data
now shows it's a genuinely material fraction of a session's token spend
on any first-touch task.

**Proposal:** not a code change — a **skill-file and MCP tool
description change** (see §3.1): explicitly tell the agent that a
`graph.json` already exists for a given repo+out-dir once `index` has
been called once, and that re-indexing on every task in a multi-question
session is wasteful. `selfcheck` or a lightweight "is this repo already
indexed, and how stale" signal (spec's own `docs/STATUS.md` already
notes "no stale-index detection" as deliberately absent, M5 territory)
would help mechanically, but the cheap, immediate fix is instructional,
not structural.

---

## 3. Stricter usage instructions

### 3.1 The skill file should name carto's specific known gaps, not just its capabilities — **implemented**

**Evidence:** `skill/carto.skill.md`'s current "What carto does not do"
section is generic ("some call shapes... aren't captured at all, by
design"). The benchmark's most encouraging finding — T6/T7/T8's clean
carto wins — happened because the *agent*, not the skill file, recognized
the specific gap (Rust path-qualified calls) in the moment and fell
back. That recognition shouldn't have to be re-derived per-session from
first principles; it's exactly the kind of thing a skill file exists to
pre-load.

**Proposal:** add a short, concrete "known gaps" list to the skill file,
each phrased as an action, not just a fact:
- *"`deps`'s answer for `--dir in` can be empty or incomplete on a
  symbol you know is used — Rust calls written as `Type::method()` or
  `module::func()` are never captured. If a result looks surprisingly
  small, verify with a targeted grep before concluding the symbol is
  unused."* (Directly matches the fallback behavior that produced T6/T7/
  T8's correct answers.)
- *"A file's absence from an import/dependency answer can mean 'no
  import' or 'the only import is a grouped `use path::{a, b}` statement'
  — until §1.3 lands, treat a narrow miss on an import-fan-out question
  as plausible, not certain."*
- *"`map`'s entry-points list is a structural heuristic (zero incoming
  imports, or a `main` symbol) — it will include non-code files
  (READMEs, config, fixtures) alongside real entry points. Filter by
  judgment, the same way L4's own correct answers did."*

This is the cheapest change in this whole plan (a documentation edit,
no code, no re-test of gates.sh) and the evidence most directly supports
it: it formalizes a behavior already observed working, rather than
asking for new behavior.

### 3.2 Encourage blended answers, not either/or — **implemented**

**Evidence:** every task graded "correct" for the carto arm in Category
T did so by combining carto's tool output with source verification, not
by using one exclusively. L3's shared error (both arms) came from the
opposite pattern — trusting a broad grep-style scan without carto's
extractor's own already-correct doc-comment-vs-`use`-statement
distinction to disambiguate it, since the agent went around carto's
tools to double check something the tools would have gotten right on
their own for the *files themselves* (though not for whether an import
was doc-comment-only — that distinction lives in `graph.json`'s edge
data, invisible without calling `deps`).

**Proposal:** skill-file wording that frames carto and grep as
complementary by default for anything transitive/blast-radius-shaped
("Category T"-like), not carto-first-then-grep-as-fallback-only or
grep-only. The current draft already avoids "ALWAYS"/"MANDATORY"
language per spec §9.3's requirement — this refines the framing without
violating that constraint.

---

## 4. What's shaping the path

Three principles, derived directly from what the benchmark actually
showed, not from the original hypothesis:

1. **Fix legible, well-evidenced gaps before adding capability.**
   Every recommendation above traces to a specific transcript or a
   specific repro — none is speculative. M2's infrastructure graph and
   M3's join are real product surface, but they build on the same
   `where`/`deps`/`map` primitives this benchmark just found concrete,
   fixable holes in. Landing §1–§2 first means M3 (if it proceeds) is
   built on a measured-good foundation instead of compounding an
   unmeasured one.
2. **Honesty about absence is as load-bearing as honesty about
   confidence.** INV-8 already guarantees every *present* edge carries a
   confidence. T6 shows the gap isn't there — it's in what happens when
   an edge is silently *absent*. §1.1 is the highest-priority item in
   this plan because it's the most direct extension of a principle the
   product already claims to have, not a new one.
3. **Re-measure, don't re-guess.** This plan's own claims about what
   would improve the S-1 numbers are themselves predictions, in the same
   spirit `bench/tasks.md` used for its per-task predictions — several of
   which turned out wrong on inspection. `bench/run.sh`/`bench/
   replay.sh`/`bench/score.py` already exist and are cheap to re-run
   (~$2.65 for the last full batch). The right way to validate this plan
   is landing §1.1–§2.1 (the highest-confidence, lowest-risk items) and
   re-running S-1 against the same 8 tasks before deciding whether M3
   proceeds — not re-arguing the case from this document alone.

## Suggested slice order (steps 0–4 implemented; step 5 pending)

0. §0 (`cli` benchmark arm) — **implemented** (`bench/run.sh`, `bench/
   score.py`, `bench/cli-arm-prompt.md`), smoke-tested, not yet run as a
   full batch. Runs alongside the next full measurement (step 5), not as
   a separate pass.
1. §1.1 (honest absence signal on `deps`) + §3.1 (skill file known-gaps
   section) together — **implemented** (ADR-0020; skill-file wording
   commit). Cheapest, highest-evidence, no new dependencies, directly
   targets T6's failure mode.
2. §2.1 (`map --section`) — **implemented**. Cheap, clear
   token-efficiency win, additive API change.
3. §1.2 (`Module`-node discovery) — **implemented** (ADR-0021, alongside
   §1.4's same-file caller flag). Closes L3's "correct answer is
   architecturally unreachable" gap.
4. §1.3 (group-import extraction) — **implemented for Rust** (ADR-0022),
   the largest single item here. Scoped to Rust `use_list` as planned;
   the cross-language check (Python/TS-JS/PHP/C#) was done rather than
   deferred — none of the other four extractors shared the failure mode,
   so no further language-specific work followed from it.
5. **Not yet run.** Re-run `bench/run.sh` against the same 8 tasks, now
   with all three arms (grep, cli, carto-MCP) and steps 0–4's changes in
   place. Compare against this session's `bench/results/20260803T112447/`
   baseline (grep/carto only) before deciding on M3 — and let the
   cli-vs-MCP numbers from this run decide which surface `skill/
   carto.skill.md`/README lead with, rather than this document's own
   reasoning in §0. This is a separate measurement round the user
   approves independently, per spec §11.4 — implementing steps 0–4 does
   not itself decide the M3 go/no-go question.

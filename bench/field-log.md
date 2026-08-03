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

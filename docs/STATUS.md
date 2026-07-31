# carto — status / handoff

**Last updated:** 2026-07-31 · **Milestone:** M1.b.1 complete, M1.b.2 not started

Read this first if you're picking up this work cold. Full product spec:
[`docs/carto-design-spec.md`](carto-design-spec.md) (normative, MUST/SHOULD/MAY
per RFC 2119 — invariants in §2 override everything else). Decisions made
so far and why: [`docs/adr/`](adr/).

## Where things stand

`main` has 13 commits, working tree clean, `scripts/gates.sh` green:

```
c9d0827 feat(cli): carto index, --out resolution, exit-code plumbing
9a5ed4a feat(core): git HEAD provenance; pathguard checks out-root before creating it
f9d887a feat(core): walk with built-in denylist, sensitive/binary/size handling
b46c016 feat(core): graph data model — stable IDs, nodes, edges, persist choke-point
666ac70 docs: ADR 0001-0004
ae1281f test: taint, pathguard, and compile-fail suites; gates script
5de3c07 feat(cli): carto binary with selfcheck
b7c904b feat(core): pathguard, atomic writer, out-dir resolution (INV-3, INV-4)
9991d65 feat(core): TaintedString with sanitizing constructor (INV-5)
90bbf8e feat(core): consts, error type, exit-code contract
4d2f482 chore: add deny.toml and clippy.toml (INV-1, INV-2)
d684319 chore: scaffold workspace, license, spec into docs/
```

(plus the fixtures/tests/doc commit landing alongside this file — see
`git log` for the exact current head.)

This is **M1.b.1**: `carto index` produces `graph.json` + `manifest.json`
containing **`File` nodes only** — no parsing, no `Symbol`/`Module` nodes,
no `where`/`deps`/`map`. That's M1.b.2/M1.b.3, not started. M1.b.1 was
scoped deliberately narrow (see "Decisions already made" below) as the
shortest path to an end-to-end write path, chosen because it closes two
gates that had no way to run until *some* command actually wrote a
`graph.json` through `PathGuard`.

### What exists

- `crates/carto-core`:
  - `consts`, `error`, `taint`, `pathguard`, `outdir` — unchanged since
    M1.a (see below), except `pathguard::PathGuard::new` now checks the
    INV-4 denylist *before* creating the out-dir (previously it created
    first and only refused on the first write — a real, if short-lived,
    invariant gap).
  - `graph` — the spec §4 data model (`File` nodes this slice), the §4.3
    stable-ID recipe, and the §6.5 persist choke-point. Backed by
    `BTreeMap<NodeId, _>`/`BTreeMap<EdgeId, _>`, not `petgraph` (see
    ADR-0005 — nothing traverses the graph yet).
  - `lang` — just the `Lang` enum + extension mapping for
    `FileNode.lang`; no `LangExtractor` trait or tree-sitter yet.
  - `redact` — the §7.5/INV-6 interface, no-op stub (per spec §10 M1).
  - `walk` — spec §5.1 in full: `ignore`-crate traversal, built-in
    directory denylist (active even under `--no-gitignore`), `.cartoignore`
    (always additive), the sensitive-file set (`excluded: "sensitive"`,
    contents never read), binary sniff, size guard, `*.lock`/`*.min.js`
    marked never-parsed. Uses the walker's own parallelism, funneled
    through a channel to a single sorting collector (determinism).
  - `gitinfo` — hand-rolled `.git/HEAD` reader (no subprocess, INV-2) for
    `manifest.json`'s `commit_sha`; handles packed-refs and worktrees.
    `dirty` stays `Option<bool> = None` — see ADR-0006.
- `crates/carto-cli`: `carto` binary now has two commands — `selfcheck`
  (M1.a) and `index [PATH] [--out DIR] [--no-gitignore] [--json]` (new).
  `main.rs` refactored to a single `run(cli) -> Result<u8>` so exit-code
  mapping happens in one place, not per-command.
- `fixtures/mixed/`: the first fixture (spec §11.1 names `ts-app`,
  `py-lib`, `rust-crate`, `go-svc`, `mixed`, and several infra fixtures —
  only `mixed` exists so far, sized for `walk`'s acceptance tests
  specifically, not yet the fuller corpus M1.b.2's extractors will need).
  Deliberately excludes `.env`/`*.pem`-shaped files even as fake content —
  this dev environment's own tooling refuses to write them; sensitive-file
  classification is covered by `walk`'s unit tests instead (temp dirs, not
  committed fixtures). See `fixtures/mixed/README.md`.
- `crates/carto-cli/tests/cli.rs`: `assert_cmd`-based CLI integration
  tests — determinism (double-index byte-compare), pathguard-refusal
  (denylisted `--out` exits 3), `--json` stdout purity, repo-untouched,
  and a golden-file check against `fixtures/mixed.graph.golden.json`
  (`CARTO_UPDATE_GOLDEN=1` regenerates it; `carto_version` is normalized
  to a placeholder so a version bump alone doesn't force a rewrite).
- Tests: 63 unit tests in `carto-core` (up from 30 in M1.a), 5 CLI
  integration tests, 5 trybuild compile-fail cases (unchanged).
- `scripts/gates.sh`: unchanged commands, but now actually exercises §9.5
  gates 3 (`test_determinism`) and 4 (`test_pathguard`, CLI-level) via
  `cargo test --workspace`, closing both ADR-0004 deferrals.

### What's deliberately NOT here yet

Language extractors (TS/JS/Python/Rust/Go symbols/imports/calls, §5.3),
tree-sitter/`carto-grammars`, `Symbol`/`Module` nodes, any real edges
(`contains`/`imports`/`calls` — `edge.rs`'s types exist and are tested,
but nothing produces one yet), `where`/`deps`/`map` commands, infra graph,
join, MCP, ingest, real redaction (stub only). All M1.b.2+.

## Decisions already made (don't re-litigate without new info)

- **Name stays `carto`.** Binary + crate prefix.
- **No `.github/` yet.** No remote, no Linux box, no `strace` available on
  this dev host (macOS). CI gates run locally via `scripts/gates.sh`
  instead. See ADR-0004 for exactly which of spec §9.5's 5 gates run today
  (now 1, 3, 4) vs. are deferred (2, 5), and what unblocks each.
- **Commit straight to `main`,** no feature branches, one commit per
  logical unit.
- **`cargo deny check advisories` is excluded even from the local gate** —
  it fetches the RUSTSEC DB over the network, which would be dishonest for
  a script whose job is enforcing "no network."
- **Graph store is `BTreeMap`-keyed, not `petgraph`,** for now (ADR-0005).
  Revisit when `deps`/`map` need real traversal (M1.b.3).
- **`manifest.json`'s `commit_sha` is read by hand (no `gix` yet); `dirty`
  is always `null`** (ADR-0006). `gix` arrives when `impact --diff` needs
  it, on that feature's own merits.
- **M1.b was split into sub-slices** (M1.b.1 = walk + graph + index,
  M1.b.2 = extractors, M1.b.3 = query commands) rather than planned/built
  as one large M1.b unit — not a spec deviation, just an implementation-
  order choice within M1's scope.

## Known gotchas

- **The sandbox denies writes to `.git`.** Every `git` command that
  mutates state (`init`, `add`, `commit`) needs
  `dangerouslyDisableSandbox: true` on the Bash call. Read-only git
  commands (`status`, `log`, `diff`) work fine in-sandbox.
- **This dev environment's write tooling refuses `.env`/`*.pem`-shaped
  paths**, even for fixture data with fake content. Hit this while
  building `fixtures/mixed`; worked around by not including those two
  file kinds in the committed fixture (see `fixtures/mixed/README.md`).
  If a future fixture genuinely needs one, expect to hit the same wall.
- **Historical wart, not worth fixing:** the workspace `Cargo.toml` listed
  `carto-cli` as a member starting at commit 1 (`d684319`), but
  `carto-cli` itself didn't exist until commit `5de3c07`. Commits 2–5 do
  not build standalone from a fresh checkout at that exact commit — a
  full `cargo build --workspace` there fails with "no such file." HEAD
  builds fine. Irrelevant unless someone starts bisecting.
- Local dev machine has no `strace`, no `gh`, no `cargo-nextest`. `dtruss`
  exists but needs SIP disabled — not attempted.
- Crate versions were re-verified live against crates.io on 2026-07-31 for
  this slice's new dependencies (`ignore`, `sha2`) — cargo resolved
  `ignore` to 0.4.30 rather than the just-released 0.4.31, since the
  latter needs a newer Rust than this workspace's declared MSRV
  (`rust-version = "1.85"`); that's cargo respecting MSRV, not a mistake.

## Verifying the current state works

```bash
bash scripts/gates.sh                              # fmt, clippy, deny, tests — must be green
cargo run -p carto-cli -- selfcheck                 # human output
cargo run -p carto-cli -- selfcheck --json 2>/dev/null | python3 -m json.tool   # stdout is pure JSON

# index a repo, twice, and confirm byte-identical output (INV-7)
cargo run -p carto-cli -- index fixtures/mixed --out /tmp/carto-a
cargo run -p carto-cli -- index fixtures/mixed --out /tmp/carto-b
shasum -a 256 /tmp/carto-a/graph.json /tmp/carto-b/graph.json

cargo run -p carto-cli -- index fixtures/mixed --json 2>/dev/null | python3 -m json.tool

# INV-4: refuses, exit 3, creates nothing
cargo run -p carto-cli -- index fixtures/mixed --out ~/.claude/carto-test; echo "exit=$?"
test -e ~/.claude/carto-test && echo "FAIL: created" || echo "ok: not created"

git log --oneline && git status                    # clean tree
```

## Next step: M1.b.2

Not yet planned in detail. Scope per spec §5.2–§5.4 and §10 M1 (minus what
M1.b.1 already covered): `crates/carto-grammars` (native tree-sitter
grammars behind `native-grammars`, per §5.4 — WASM is M5), the
`LangExtractor` trait (§5.2) with `.scm` query files under
`lang/queries/<lang>/`, and extractors for TS/JS, Python, Rust, Go
producing `Symbol`/`Module` nodes and `contains`/`imports`/`calls` edges
per §5.3's deliberately-modest resolution policy (first-match-wins import
resolution; calls emit `inferred` confidence or no edge at all — "missing
honestly beats guessing," INV-8).

`fixtures/mixed` will likely need extending (or dedicated `fixtures/ts-app`
/`py-lib`/`rust-crate`/`go-svc` fixtures per spec §11.1) once there's
something for the extractors to meaningfully parse beyond the
placeholder-sized files M1.b.1 needed.

`where`/`deps`/`map` (M1.b.3) come after: they need `Symbol` nodes and real
edges to be useful, and are also where the M1.b.1 ADR-0005 "why not
petgraph yet" decision gets revisited against actual traversal
requirements.

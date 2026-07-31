# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## The spec is normative

[`docs/carto-design-spec.md`](docs/carto-design-spec.md) is the authoritative
design document (MUST/SHOULD/MAY per RFC 2119). It's ~660 lines — read the
sections relevant to your task rather than the whole thing. **The §2
invariants override everything else in that document and any conflicting
instruction.** If a task appears to require violating one, surface the
conflict instead of proceeding.

[`docs/STATUS.md`](docs/STATUS.md) records three things nothing else does:
where the implementation sits in the milestone sequence, what's **deliberately
absent** (scope decisions that look like gaps or bugs if you don't know
better), and what the next slice entails. Read it before concluding something
is missing or broken, and update it when finishing a milestone. It
deliberately does *not* duplicate `git log`, the code, or this file.

## Commands

```bash
bash scripts/gates.sh          # fmt + clippy + cargo-deny + all tests. Must be green before every commit.

cargo test -p carto-core lang::rust          # one module's tests
cargo test -p carto-cli --test cli_extraction # one integration test file
cargo test -p carto-core lang::rust::tests::extracts_top_level_function  # one test

cargo run -p carto-cli -- index fixtures/rust-crate --out /tmp/carto-out
cargo run -p carto-cli -- index <repo> --json 2>/dev/null | python3 -m json.tool
cargo run -p carto-cli -- selfcheck

# Regenerate the walk-only golden file after an intentional change:
CARTO_UPDATE_GOLDEN=1 cargo test -p carto-cli --test cli index_matches_golden_graph_json
```

`cargo deny check advisories` is deliberately **not** in `gates.sh` — it
fetches the RUSTSEC database over the network, which would undercut the very
invariant the script exists to enforce (ADR-0004).

## Invariants and how they're mechanically enforced

The product's identity is its security posture. Each invariant has an
enforcement mechanism that lives in a specific place — changing behavior near
any of these means checking the mechanism still bites:

| Invariant | Enforced by |
|---|---|
| INV-1 no network | `deny.toml` ban list (crates providing sockets/HTTP/DNS/TLS) |
| INV-2 no code execution | `clippy.toml` `disallowed-methods` bans `std::process::Command` outside `carto-cli/src/selfcheck.rs` |
| INV-3/INV-4 write confinement | `carto_core::pathguard` — **every** fs write goes through `PathGuard::writer`; hard-coded, non-configurable denylist |
| INV-5 repo text is untrusted | `TaintedString` has no `Display`/`Deref`/`AsRef<str>`/`Into<String>` and a private field; content only escapes via `render_fenced()`/`render_capped(n)`. Verified by a `trybuild` compile-fail suite (`carto-core/tests/ui/`) that diffs compiler stderr against checked-in snapshots |
| INV-6 no secrets on disk | `redact::redact()` called from the persist choke-point (currently a no-op stub with the real signature — M2 fills it in) |
| INV-7 deterministic output | `graph.json` must be byte-identical for the same tree. Double-index-and-compare tests in `carto-cli/tests/` |
| INV-8 honest edges | `Edge::new` *requires* confidence + evidence as arguments; there is no constructor without them |

### Determinism is easy to break by accident

Anything reaching `graph.json` must be order-stable: **`BTreeMap`/`BTreeSet`
only, never `HashMap`/`HashSet`**; no `read_dir` order dependence; no
timestamps (those live only in `manifest.json`). `Graph` is `BTreeMap`-keyed
specifically so `into_sorted_parts()` satisfies §4.4's "sorted by id" with no
separate sort step. The double-index tests catch violations.

### Adding a dependency

Spec §13 requires an ADR for anything outside its starting allowlist, and
`deny.toml` sets `multiple-versions = "deny"` — a new dep can fail the gate
by pulling a duplicate version of something already in the tree. Resolve with
a narrowly-scoped `[[bans.skip]]` carrying a reason, **never** by relaxing
`multiple-versions`. Verify versions against crates.io at implementation time
(the spec's version table is explicitly illustrative).

## Architecture

Single static Rust binary. Data flows one direction:

```
walk ──► lang::extract_and_resolve ──► Graph ──► graph::persist ──► <out>/graph.json
(File nodes)  (Symbol/Module nodes,            (the ONE writer:      <out>/manifest.json
               contains/imports/calls edges)    sort → redact → validate
                                                → size caps → atomic write
                                                through PathGuard)
```

- **`crates/carto-core`** — everything above; no clap, no rmcp, no I/O
  besides fs.
- **`crates/carto-cli`** — bin target `carto`. `main.rs` is a thin dispatch:
  one `run(cli) -> Result<u8>`, with `Error::exit_code()` → process exit
  mapped in exactly one place (0 ok / 1 user / 2 data / 3 invariant refusal).
- **`crates/carto-grammars`** — tree-sitter grammar loading. The workspace's
  **one** `unsafe_code` exception (spec §3.1): it cannot use
  `[lints] workspace = true` (inheritance is all-or-nothing and `forbid`
  isn't locally downgradable), so it sets its own lint table and restates the
  clippy lint INV-2 needs. Currently contains zero `unsafe` — see ADR-0007
  for why the carve-out stays anyway.

### Language extraction is two-phase

Per-file extraction and whole-repo resolution are deliberately separate
(`lang/extractor.rs` → `lang/rust.rs` → `lang/resolve.rs`):

1. **Extract** parses one file into `RawSymbol`/`RawImport`/`RawCallSite`
   using tree-sitter queries kept as reviewable `.scm` files under
   `lang/queries/<lang>/`, embedded via `include_str!`.
2. **Resolve** runs whole-repo — a call can't be judged unambiguous until
   every other file's exported symbols are known. Spec §5.3's policy is
   deliberately modest: first-match-wins across three tiers, `calls` edges
   are **always `inferred`, never `certain`**, and zero-or-multiple
   candidates produce **no edge**, recorded in the caller's
   `unresolved_calls` instead. Missing honestly beats guessing.

Adding a language means: a `.scm` query set, a `LangExtractor` impl, and an
ADR mapping §5.3's generic rules onto that language's actual import/call
semantics. [ADR-0008](docs/adr/0008-rust-resolution-policy-mapping.md) does
this for Rust and is the template — but not a rule that transfers verbatim
(TS's `import './x'` is genuinely file-relative; nothing in Rust is).

## Conventions

- **ADRs for every deviation and judgment call.** `docs/adr/NNNN-*.md`.
  Required when deviating from the spec, adding a dependency outside §13's
  list, or resolving something the spec left to the implementer. These are
  load-bearing here — read the relevant ones before changing nearby code.
- **Milestones get sliced.** Spec §10's milestones have been subdivided as
  they proved too large for one plan/execution unit (M1 → M1.a, M1.b → b.1,
  b.2a, b.2b, b.3). Plan and implement one slice at a time; don't pull later
  scope forward.
- **Commit straight to `main`**, one commit per logical unit, `gates.sh`
  green before each. Commit messages explain *why* and record what was
  verified — match the existing `git log` style.
- **Fixtures are small, synthetic, and committed** (`fixtures/`, spec §11.1)
  — never real customer code. Each has a README mapping files to the exact
  behavior they exercise.

## Gotchas that will cost you time

- **The sandbox denies writes to `.git`.** Any state-mutating git command
  (`add`, `commit`) needs `dangerouslyDisableSandbox: true`. Read-only git
  works fine sandboxed.
- **This environment refuses to create `.env`/`*.pem`-shaped files** even
  with fake fixture content. `fixtures/mixed` omits them for this reason;
  sensitive-file classification is covered by `walk`'s unit tests using temp
  dirs instead.
- **`docs.rs` is not on the network allowlist** (crates.io is). Resolve API
  questions empirically — write the code and see if it compiles.
- **A tree-sitter query can match the same node twice** when two patterns
  describe it at different specificity, silently corrupting downstream
  resolution. Eyeball real `carto index` output against a fixture, not just
  unit tests on synthetic snippets — that's how the impl-block-method
  duplicate was caught.
- A fixture's own `.gitignore` makes *git* skip those paths too; noise dirs
  intended to be committed need `git add -f`.

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
cargo run -p carto-cli -- index fixtures/py-lib --out /tmp/carto-out
cargo run -p carto-cli -- index fixtures/php-app --out /tmp/carto-out
cargo run -p carto-cli -- index fixtures/ts-app --out /tmp/carto-out
cargo run -p carto-cli -- index fixtures/go-svc --out /tmp/carto-out
cargo run -p carto-cli -- index fixtures/csharp-app --out /tmp/carto-out
cargo run -p carto-cli -- index fixtures/secrets-corpus --out /tmp/carto-out
cargo run -p carto-cli -- index fixtures/sid-like --out /tmp/carto-out
cargo run -p carto-cli -- index fixtures/monorepo --out /tmp/carto-out
cargo run -p carto-cli -- index <repo> --json 2>/dev/null | python3 -m json.tool
cargo run -p carto-cli -- where <name> <repo> --out /tmp/carto-out
cargo run -p carto-cli -- deps <name|id> <repo> --out /tmp/carto-out --dir out --depth 2
cargo run -p carto-cli -- map <repo> --out /tmp/carto-out --budget 50
cargo run -p carto-cli -- contract <value> <repo> --out /tmp/carto-out
cargo run -p carto-cli -- orphans <repo> --out /tmp/carto-out --category metric_name
cargo run -p carto-cli -- selfcheck
cargo run -p carto-cli -- serve   # MCP stdio server (spec §7.3); see docs/adr/0018

# Regenerate a golden file after an intentional change (walk-only, or
# where/deps/map against fixtures/rust-crate):
CARTO_UPDATE_GOLDEN=1 cargo test -p carto-cli --test cli index_matches_golden_graph_json
CARTO_UPDATE_GOLDEN=1 cargo test -p carto-cli --test cli_query
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
| INV-6 no secrets on disk | `redact::redact()` called from the persist choke-point — pattern + entropy scan over every `TaintedString` field (`crates/carto-core/src/redact/{patterns,entropy}.rs`, ADR-0017) |
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

Single static Rust binary. Write path, then read path:

```
walk ──► lang::extract_and_resolve ──► Graph ──► graph::persist ──► <out>/graph.json
(File nodes)  (Symbol/Module nodes,            (the ONE writer:      <out>/manifest.json
               contains/imports/calls/          sort → redact → validate
               references edges)               → size caps → atomic write
                                                through PathGuard)

<out>/graph.json ──► graph::load ──► QueryGraph ──► query::{find,deps,map} ──┬──► CLI renderer
                     (size-checked                  (adjacency index,       │    (--json: plain
                      before parse,                  BFS/scans,             │     serde; human:
                      schema-checked)                 BTreeMap, not         │     render_fenced()
                                                       petgraph — ADR-0009)  │     once per section,
                                                                             │     ADR-0010)
                                                                             └──► carto-mcp tools/*
                                                                                  (structuredContent:
                                                                                  same struct; content:
                                                                                  same render_fenced(),
                                                                                  capped at 8 KiB —
                                                                                  ADR-0018)
```

`query`'s result structs (`FindResult`, `DepsResult`, …) are plain
serde types over `&QueryGraph` — spec §7.1's "commands = MCP tools, same
core functions" means these are also the MCP server's payloads, so their
serde shape is a compatibility surface both front ends only render, not
an internal detail either reshapes freely. `find`'s `where` also
matches `Module.path` now (`FindResult::module_matches`, a separate
list from `SymbolMatch`), and `deps`'s target resolution matches
`Module.path`/`File.path` in addition to a symbol name — ADR-0021;
previously a `Module`/`File` node's blake3-hashed ID had no other
reachable lookup path at all.

- **`crates/carto-core`** — everything above; no clap, no rmcp, no I/O
  besides fs.
- **`crates/carto-cli`** — bin target `carto`. `main.rs` is a thin dispatch:
  one `run(cli) -> Result<u8>`, with `Error::exit_code()` → process exit
  mapped in exactly one place (0 ok / 1 user / 2 data / 3 invariant refusal).
  Mounts `carto-mcp` behind the `serve` subcommand.
- **`crates/carto-mcp`** — the MCP stdio server (spec §7.3). Hand-rolled
  newline-delimited JSON-RPC 2.0, not `rmcp` — ADR-0018 records why (in
  short: `rmcp`'s dependency tree pulls an async runtime into an
  otherwise fully synchronous codebase for a transport simple enough to
  hand-roll in ~300 LOC with zero new dependencies). Depends on
  `carto-core` only; every tool handler in `tools/` calls the same core
  function the CLI subcommand of the same name calls — no query logic
  lives here.
- **`crates/carto-grammars`** — tree-sitter grammar loading. The workspace's
  **one** `unsafe_code` exception (spec §3.1): it cannot use
  `[lints] workspace = true` (inheritance is all-or-nothing and `forbid`
  isn't locally downgradable), so it sets its own lint table and restates the
  clippy lint INV-2 needs. Currently contains zero `unsafe` — see ADR-0007
  for why the carve-out stays anyway.

### Language extraction is two-phase

Per-file extraction and whole-repo resolution are deliberately separate
(`lang/extractor.rs` → `lang/{rust,python,php,ecma,go,csharp}.rs` →
`lang/resolve.rs`). `ecma.rs` houses three `LangExtractor`s
(TypeScript/TSX/JavaScript) in one module, not one file each — see its
own module doc comment for why (they're dialects of one grammar
family; Rust/Python/PHP/Go/C# aren't). `lang::extract_and_resolve` dispatches
each file to its extractor by `Lang` (a small registry,
`lang/mod.rs::extractors()`), not a hard-coded single language:

1. **Extract** parses one file into `RawSymbol`/`RawImport`/`RawCallSite`
   (plus the optional `RawLiteral`/`RawTypeRef` channels described
   below) using tree-sitter queries kept as reviewable `.scm` files
   under `lang/queries/<lang>/`, embedded via `include_str!`.
2. **Resolve** runs whole-repo — a call can't be judged unambiguous until
   every other file's exported symbols are known. Spec §5.3's policy is
   deliberately modest: first-match-wins across tiers, `calls` edges
   are **always `inferred`, never `certain`**, and zero-or-multiple
   candidates produce **no edge**, recorded in the caller's
   `unresolved_calls` instead. Missing honestly beats guessing — and
   since ADR-0020, that honesty extends to *absence* on the inbound
   side too: an extractor that deliberately never attempts a call shape
   at all (`RawCallSite` vs. `ExtractOut::uncaptured_call_sites` —
   Rust's path-qualified `Type::method()`/`module::func()` is the only
   producer today) has those sites counted, not resolved, into
   `SymbolNode::uncaptured_inbound_calls`, so `deps --dir in` can say
   "N sites here spell this name in an unattempted shape" rather than
   silently looking like the symbol has few callers. The
   tier-resolution logic itself is language-agnostic; only import
   handling (`RawImport::Relative`/`Absolute`/`Qualified`/`PackagePath`/
   `NamespaceImport`, spanning every language today) and each
   extractor's own `is_pub`-equivalent gate are per-language. One tier
   is opt-in per extractor rather than universal: `LangExtractor::
   package_scope_is_directory()` (default `false`) enables a
   directory-scoped tier between same-file and imported, for Go's
   file-independent package visibility — see ADR-0015; every other
   extractor's tiers are exactly spec §5.3's original three. A second
   small per-extractor knob, `namespace_separator()` (default `\`,
   PHP's), lets PHP's `App\Orders` and C#'s `Acme.Orders` share one
   FQN index without cross-matching (ADR-0016). A call (or, since
   ADR-0029, a type reference) is attributed to the *innermost* symbol
   containing it (`assign_to_innermost_symbol`), not every symbol whose
   range contains it — matters for any language whose class-like symbol's
   own range spans its methods' bodies (PHP, Python; not Rust, where
   an `impl` block is never itself a symbol). Tier (b) is alias-aware
   (`ImportedName { bound_name, declared_name }`, `alias_to_declared`)
   — a call site's own spelling and the target's actual declared name
   are tracked separately, not collapsed to one string, so `import {
   foo as bar }`/`from x import foo as bar`/`use Foo as X;` all
   resolve correctly (ADR-0013); Go's `PackagePath` imports never
   populate this map at all, since a Go import binds a package name,
   never a symbol name (ADR-0015).

Adding a language means: a `.scm` query set, a `LangExtractor` impl
(registered in `extractors()`), and an ADR mapping §5.3's generic rules
onto that language's actual import/call semantics.
[ADR-0008](docs/adr/0008-rust-resolution-policy-mapping.md) (Rust — its
`use_list`/grouped-import exclusion was later narrowed by
[ADR-0022](docs/adr/0022-rust-grouped-use-extraction.md); `use_wildcard`/
`use_as_clause` remain excluded, unchanged),
[ADR-0011](docs/adr/0011-python-resolution-policy-mapping.md) (Python),
[ADR-0012](docs/adr/0012-php-resolution-policy-mapping.md) (PHP — also
the ADR that added PHP to spec §5.2's v1 set),
[ADR-0013](docs/adr/0013-typescript-javascript-resolution-policy-mapping.md)
(TypeScript/TSX/JavaScript — also the ADR that fixed the alias-
resolution gap above), and
[ADR-0015](docs/adr/0015-go-resolution-policy-mapping.md) (Go — the
ADR that added `RawImport::PackagePath` and the opt-in directory-scoped
resolution tier above; closes spec §5.2's full v1 language set), and
[ADR-0016](docs/adr/0016-csharp-resolution-policy-mapping.md) (C# —
the first language added *beyond* the v1 set; the ADR that added
`RawImport::NamespaceImport` and `namespace_separator()`, and the one
place a symbol is deliberately *not* extracted to protect edges:
constructors share their type's name, so extracting them would make
every `new Foo()` ambiguous and INV-8 would drop the construction
edge) are the worked examples so far — templates, not rules that
transfer verbatim (Python's attribute-call syntax can't even
distinguish a module-qualified call from an instance call the way
Rust's, PHP's, TS/JS's, Go's, and C#'s grammars all can; PHP, TS/JS,
Go, and C# capture their path-qualified/member/selector calls anyway,
unlike Rust, since they're too common to drop; TS/JS's relative
imports are genuinely file-relative with extension-guessing in a way
none of Rust's, Python's, or PHP's import models are, yet still fit
the existing `RawImport::Relative` shape with no new variant, while
Go's directory-shaped, module-qualified import paths and C#'s
many-files-at-once namespace `using`s each didn't fit any existing
variant and needed their own).

### Cross-language string-literal contracts

A capability outside spec §4/§5/§6 entirely, added on user request
(ADR-0025/0026/0027): a `Contract` node kind plus `produces`/`consumes`
edges represent a bare string literal — a CloudWatch metric name, and
(per the ranked follow-up list in ADR-0026) eventually an env var, a
DynamoDB attribute, a Kafka topic — spelled identically in two or more
files of *different* languages, where nothing in either language's own
grammar connects them. `ExtractOut::literals` (`RawLiteral`) is a
fourth, optional thing a `LangExtractor` can emit alongside
symbols/imports/calls; today only `CSharpExtractor` and the new
`HclExtractor` (`lang/hcl.rs`, the first extractor with no `.scm` query
file — `tree-sitter-hcl`'s grammar carries no named fields, ADR-0025)
populate it. What a literal's `position` string *means* (category,
producer/consumer role, confidence) is decided declaratively by
`crate::contracts::ContractRules` — built-in rules plus an optional
repo-local `.carto/contracts.json` (ADR-0027) — not by the extractor,
keeping per-language code mechanical. `carto contract`/`carto orphans`
(neither in spec §7.1) are the query surface;
`orphans --category metric_name` is the concrete acceptance test this
capability was scoped against — see ADR-0026's Context for the real
finding that motivated it (five CloudWatch alarms referencing metric
names no service emits, permanently unable to fire). `SCHEMA_VERSION`
bumped 3 → 4 for the new node/edge kinds.

### Type references (`EdgeKind::References`)

`EdgeKind::References` existed in the enum from the start with no
producer until real C# field feedback (`bench/field-log.md`) traced a
"who uses this interface" answer's imprecision to a genuine capture
gap, not (as first suspected) an `imports`-granularity problem —
ADR-0029's Context has the full root-cause trace. `ExtractOut::
type_refs` (`RawTypeRef`) is a fifth optional extraction channel,
alongside `literals`; all six typed extractors
(Rust/Go/TS+TSX+JS/PHP/Python/C#) populate it via the same two-step
shape every other query-driven extraction already uses: a
`queries/<lang>/types.scm` file captures only the type-position
*container* node (anchored on a field, never a literal supertype
name — most languages' own `type` grammar rule is a hidden supertype,
so a query naming it directly wouldn't reliably match), and a
`collect_type_names()` function in the extractor's own `.rs` file
walks that subtree down to head identifiers. `resolve` pushes each
resolved type ref through the *identical* tier ladder `calls` edges
use (`resolve_call`, generalized to take a bare `name: &str` instead
of a `&RawCallSite`) — same-file/same-directory/imported/same-package,
first-match-wins, ambiguous-or-zero produces nothing — and emits
`Confidence::Inferred` with `type-reference:<tier>` evidence, never
`Certain`. A resolved self-reference (a field naming its own enclosing
type) is suppressed; an *unresolved* type ref is silently dropped,
deliberately unlike `unresolved_calls` — user-confirmed: overwhelmingly
stdlib/BCL/framework noise, not a signal worth a list or counter (see
ADR-0029). The pre-existing `imports` edge's own namespace/module
fan-out is unchanged — `references` adds precision alongside it, it
doesn't replace or narrow it. `SCHEMA_VERSION` bumped 4 → 5.
ADR-0030 is the six-language survey of exactly which grammar shapes
feed `type_refs` and which are deliberately excluded per language;
three wrong assumptions were caught only by inspecting a real parse
tree, not by the query failing to compile — see that ADR before adding
a seventh language's own `types.scm`. A call or type ref with **no
enclosing symbol at all** (C#'s top-level statements — a `Program.cs`
with no `Main` method, valid since C# 9 and the modern ASP.NET Core
minimal-API style; TS/JS/Python module-level setup code has the same
shape) resolves at *file* scope instead of being silently dropped —
`assign_to_innermost_symbol` returns its unattached leftovers
alongside the usual per-symbol assignment, and a per-file pass resolves
each through the same tier ladder, pushing a `Calls`/`References` edge
from the `File` node on success (resolved case only — an unattached
miss still has no `unresolved_calls`-equivalent; ADR-0031, a same-day
retest fix to ADR-0029/0030).

### Multi-root support (component-aware indexing)

Another capability outside spec §4/§7's original scope, added on user
request (ADR-0034/0035): real repos put several projects that belong
together — microservices, frontends, lambdas, infra — under one root,
and carto's original "same-package = same walked repo" resolution
policy (spec §5.3 rule 2c, every language ADR from 0008 onward) treats
the whole tree as one scope, which both silently drops real
intra-component edges (two services each defining `Handler` makes
either one unresolvable) and produces false cross-component edges at
up to `Certain` confidence (PHP `use`/C# `using`). **One tree, N
components** — not N filesystem roots: `carto index <monorepo>` walks
a single root exactly as before; a new `crates/carto-core/src/
components/` module recognizes the project roots *inside* that tree
from `walk`'s own output (manifest-file markers — `go.mod`,
`package.json`, `Cargo.toml`, etc. — plus an optional
`.carto/roots.json` override, the same built-ins-plus-repo-file shape
ADR-0027 established for `.carto/contracts.json`), stamps each
`FileNode.component` before extraction/resolution runs, and persists a
`components: Vec<Component>` table in `graph.json` alongside it.
`resolve.rs`'s bare-name tiers gain two new component-scoped tiers,
tried *before* their repo-wide counterparts and falling through to
them on ambiguity — provably safe to add, since a component-scoped
candidate set is always a subset of the repo-wide one, so an ambiguous
in-component result is provably still ambiguous on the full set. The
four `Certain`-confidence import indices (`known_modules`/`fqn_to_
file`/`namespace_to_files`/`known_namespace_roots`) each get their own
component treatment, not a single uniform rule — `known_modules` is a
strict per-component partition (semantically required: a Rust `mod`
in one crate is never visible to another), the two PHP/C# FQN indices
prefer same-component with a repo-wide fallback, and
`known_namespace_roots` is deliberately left unchanged (narrowing it
risks misclassifying a genuinely-internal-but-cross-component import
as external). `--component <name>` (repeatable) reaches `where`/
`deps`/`map`/`contract`/`orphans` on both front ends, following
ADR-0014's "restricts what's listed, never what's computed" principle
for the query layer's own dimension — a separate concern from the
index-time resolution narrowing above. `map` gains a `components`
section (per-component file/symbol counts plus a cross-component edge
summary). Verified additive: a repo with no nested components produces
a graph/query diff that is exactly the new `null`/empty-collection
fields, nothing else — checked by diffing goldens, not assumed — and
all 429 pre-existing `resolve.rs` tests pass completely unchanged,
since none of them ever sets a file's component.

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

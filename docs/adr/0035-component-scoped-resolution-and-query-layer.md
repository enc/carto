# 0035 — Component-scoped resolution and `--component` in the query layer

**Status:** accepted · **Date:** 2026-09-21 · **Milestone:** multi-root
support, slice 2 (amends ADR-0008/0011/0012/0013/0015/0016's shared
"same-package = same walked repo" mapping)

## Context

[ADR-0034](0034-component-discovery-and-scoping.md) gives every walked
file a `component` label. This ADR is what actually uses it: narrowing
spec §5.3's bare-name resolution tiers and the `Certain`-confidence
import indices to prefer a caller's own component before falling back
to the repo-wide behavior every language ADR from 0008 onward already
documented, plus a `--component` filter through `where`/`deps`/`map`/
`contract`/`orphans`.

## Decision: resolution

The change lands in `CallResolver` (`crates/carto-core/src/lang/
resolve.rs`), the single ladder shared by `calls` edges, `references`
edges (ADR-0029), the ADR-0031 file-scope leftovers, and ADR-0033's
inbound honesty pre-pass. Two new tiers, **tried before their
repo-wide counterparts, never instead of them**:

| Tier | Candidate set | Evidence when it resolves |
|---|---|---|
| (b1) imported, caller's component | `pub_by_component[(component, declared)]` | `imported` |
| (b2) imported, repo-wide (today's tier b, unchanged code) | `pub_by_name[declared]` | `imported` if caller/target share a bucket, `imported-cross-component` otherwise |
| (c1) exported, caller's component | `pub_by_component[(component, name)]` | `same-component` |
| (c2) exported, repo-wide (today's tier c, unchanged code) | `pub_by_name[name]` | `same-package` if same bucket, `cross-component` otherwise |

Both new tiers only run when the caller's file *has* a component
(`file_component[caller_file_idx]` is `Some`) — a file in the `None`
bucket has no scope narrower than the repo, so it goes straight to the
unchanged repo-wide tier. Both fall through (not `return None`) on 0 or
ambiguous candidates, exactly like every other tier here — a
component-scoped candidate set is always a *subset* of the repo-wide
one (`pub_by_component`'s `(component, name)` entry only ever contains
files that also appear in `pub_by_name[name]`), so an ambiguous
in-component result is provably still ambiguous on the full set: b2/c2
run the identical `disambiguate_by_owner` call on a superset and can
only agree or still fail, never contradict. This is what makes the new
tiers *safe* to add without re-auditing every existing resolution
outcome, not merely convenient.

**`bucket_evidence`** decides the evidence *label* only, after a
candidate is already chosen — never a second filter. `caller_component
== target_component` (an `Option<&str>` comparison, so `None == None`
counts as "same," keeping every string byte-identical to before this
ADR for a repo with no components anywhere) yields the unchanged
`"imported"`/`"same-package"` string; a real difference yields
`"imported-cross-component"`/`"cross-component"`. The
`+owner-type-referenced` suffix (ADR-0032) composes on top of either.

Same component-first/repo-wide-fallback treatment, or a deliberate
exception, for the four repo-global **`Certain`**-confidence import
indices — these deserved more care than the bare-name tiers above,
since a false match there is the most damaging kind:

- **`known_modules`** (Rust `mod` names): **strict per-component
  partition, no fallback** — re-keyed `BTreeMap<Option<&str>,
  BTreeSet<&str>>` by bucket. Semantically correct, not merely a
  heuristic choice: a `mod orders;` declared inside one crate is never
  visible to a *different* crate's `use orders::x` under Rust's own
  module system, so a repo-wide fallback here would resurrect exactly
  the false suppression this ADR exists to fix (component A's `mod
  orders;` wrongly marking component B's external `orders` crate as
  internal, silently dropping the real external-module edge). A repo
  with no components collapses to the single `None` bucket, identical
  to today's flat set.
- **`fqn_to_file`** (PHP `use`/C# `using X = ...`): restructured to
  collect *every* declaring file per FQN (previously `.or_insert` kept
  only the first), then `resolve_fqn` prefers a same-component match
  when the caller has one and any exists, falling back to the first
  file in `extractions`' own order otherwise — exactly preserving the
  pre-existing "arbitrary first-file-wins" behavior for the case
  component-scoping doesn't help. Two components each declaring
  `namespace App\Shared; class Constants` is a real, *not* illegal
  occurrence across independently-compiled monorepo services (unlike
  the single-repo PHP-autoload-violation case ADR-0012's original
  "keeps whichever file is encountered first" comment described);
  preferring the caller's own component is almost always the intended
  target.
- **`namespace_to_files`** (C# `using Acme.Orders;`): the fan-out
  narrows to the caller's own component's declaring files when at
  least one exists, falling back to the full repo-wide fan-out
  otherwise. Prevents exactly the over-broad-fan-out shape ADR-0029's
  field report traced the coarser `imports` edge to in the first
  place — a `using Acme.Orders;` inside service "orders" no longer
  also fans an edge into a different service's own, unrelated
  `Acme.Orders` namespace.
- **`known_namespace_roots`** — **deliberately left repo-wide,
  unchanged.** This index's only job is "internal but unresolvable, so
  stay silent instead of guessing external" (spec §5.3's outcome 2 for
  `Qualified`/`NamespaceImport`); a namespace root declared *anywhere*
  in the repo is still good evidence the root is the repo's own, not a
  third-party package, regardless of which component declared it.
  Narrowing this risks the opposite failure mode this ADR is trying to
  avoid: misclassifying a genuinely-internal-but-cross-component
  import as external and fabricating a spurious `Module` node.

### The one disclosed non-additive case

If the old repo-wide-only owner-type-disambiguated tier (ADR-0032)
would have picked a candidate in a *different* component — because the
caller's own type refs happened to name that candidate's owner, and no
same-component candidate existed to disambiguate against — while the
caller's own component holds a *different*, unambiguous candidate for
the same bare name, tier (c1) now resolves to the same-component
candidate first, before the repo-wide owner-disambiguation tier is
ever reached. This is a real, tested behavior change for that specific
input shape (`resolve.rs`'s
`component_scoping_can_retarget_an_owner_disambiguated_call_to_the_same_component_candidate`
test pins it), not a regression: preferring the same-component match
over a coincidental cross-component owner-type hit is the more
correct answer, and is called out here rather than folded silently
into "purely additive."

## Decision: query layer

`--component <NAME>` (repeatable), independent of `--subpath` (both
given means both apply), through `where`/`deps`/`map`/`contract`/
`orphans` — CLI as a repeatable `clap` flag (`Vec<String>`), MCP as a
comma-separated string (mirroring `map`'s own `sections` param
precedent, ADR from the post-S-1 plan's §2.1 slice).

- **`QueryGraph::component_in_scope(id, filter)`** is the component
  analogue of `path_in_scope` (ADR-0014), including its governing
  principle: restricts what's *listed*, never what a traversal/
  aggregation *computes* — a wholly separate concern from the
  index-time resolution narrowing above. `Module`/`Contract` nodes are
  **always** in scope here, the same "no directory/component of its
  own" rule `path_in_scope` already gives them — deliberately
  different from what `component_of` (the *display* accessor) reports
  for them (`None`): one answers "what component is this," the other
  "should this be hidden by a filter," and for these two kinds those
  are different questions with different honest answers.
- **`QueryGraph::validate_component_filter`** is new, and has no
  `--subpath` analogue: an unknown `--component` name is a typo, not a
  legitimately-empty-result query, unlike `--subpath`, which has no
  enumerable "known directories" list to check a value against.
  Called once at each front-end layer (CLI command / MCP tool, right
  after building `qg`, before constructing the query struct) rather
  than inside the core query functions, which stay infallible — `find`/
  `map` still return their result structs directly, not `Result`.
- **`deps`'s `resolve_target` takes `--component` as a tiebreaker
  only, exactly like `--subpath`** — resolve unscoped first, retry
  scoped by both only when already ambiguous. ADR-0014 records the
  unconditional-scoping version of this as a real bug caught by
  end-to-end testing (`deps handle --subpath src/orders` breaking
  because `handle` itself lives outside `src/orders`); not re-made
  here. Verified end to end against a synthetic two-component fixture:
  `deps Handler --component billing` resolves cleanly where the
  unscoped name alone is ambiguous between two services' own
  `Handler`s.
- **`map` gains a fifth section, `MapSection::Components`** —
  `MapCounts.components: BTreeMap<String, ComponentCounts>` (one row:
  name, path, kind, file/symbol counts) plus a rendered cross-component
  edge summary tallied by `(from-component, to-component, edge kind)`,
  restricted to pairs touching at least one in-scope component. Always
  renders its header even when empty (`"(none)"`), matching
  `top_modules_lines`/`entry_points_lines`'s existing convention rather
  than omitting the section — an empty components section is a real,
  verified fact for a single-project repo, not an "unimplemented"
  placeholder the way `infra`/`join` still are.
- **`contract`'s `--component` filters *sites*, never drops a whole
  match** — `ContractSite` gains `component`; a `ContractMatch` losing
  every producer/consumer row under the filter still appears, with
  empty lists, since that's itself informative for a cross-component
  contract (ADR-0026's entire reason to exist). **`orphans`'s
  `--component` filters the *report*** — `OrphanContract` gains
  `components: Vec<String>` (every distinct component among its
  sites); a contract with no site in the filter set is dropped
  entirely, since an orphan's defining fact (no producer, or no
  consumer, anywhere) is inherently repo-wide, not a per-site listing
  — the opposite design choice from `contract`'s, deliberately, for
  two genuinely different questions.
- **`Truncation.next_call` carries `--component` forward** everywhere
  `--subpath`/`--section`/`--kinds` already do, following the same
  "the exact follow-up call reproduces the same scoped view" precedent
  ADR-0014/the post-S-1 plan's §2.1 slice established.
- **`same_component_as_root` on `DepEdge`** is `same_file_as_root`'s
  exact analogue and gating: `false` whenever either side is a
  `Module`/`Contract` node (`owning_file` is `None`), `true` only when
  both sides have an owning file *and* that file's component matches
  — including both being `None`, so a component-less repo reports
  `true` for every same-file-kind pair, byte-compatible with the
  pre-ADR-0035 absence of the concept. Rendered as a `[cross-component]`
  marker alongside the existing `[same file as root]` one, on both
  front ends.

## Consequences

- 11 new tests in `resolve.rs`'s own suite (84 total, up from 73):
  the motivating same-name-across-components fix, the import-tier
  equivalent, cross-component evidence labeling in both directions
  (into and out of the `None` bucket), the in-component-still-
  ambiguous fallthrough, the disclosed retargeting case, and one
  dedicated test per `Certain`-edge index (`known_modules`'s two-sided
  guarantee, the PHP FQN collision preference, the C# namespace
  fan-out narrowing).
- All 429 pre-existing `resolve.rs` tests pass completely unchanged —
  none of them ever sets `FileExtraction.component`, so every bucket
  comparison is `None == None` and every evidence string is
  byte-identical to before this ADR. This was verified, not assumed:
  the full suite was run after each of the resolution changes above,
  independently.
- New `QueryGraph` tests (13) covering `component_of`/
  `component_in_scope`/`validate_component_filter`'s full matrix
  (files, symbols via owning file, Module/Contract's deliberate
  disagreement between the two predicates, dangling references, empty
  vs. `None` filters, unknown names).
- `fixtures/rust-crate.{where,deps,map}.golden.json` regenerated —
  diffs are exactly the new `"component": null`/`"same_component_as_
  root": true`/`"components": {}`/`"## components\n  (none)"` additions,
  nothing else, checked by hand before accepting.
- **Deliberately out of scope for this slice**: per-hop component
  breakdowns beyond the root-comparison flag `same_component_as_root`
  already gives; a `--component` value that names a *kind* (e.g. "all
  go components") rather than one specific component's name; joining
  `map`'s cross-component edge summary with `deps`'s own BFS output
  format.

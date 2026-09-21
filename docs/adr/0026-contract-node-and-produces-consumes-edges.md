# 0026 — `Contract` node kind and `produces`/`consumes` edges

**Status:** accepted · **Date:** 2026-08-04 · **Milestone:** SID Cloud
cross-language contract slice (spec §4.1/§4.2 amendment)

## Context

See ADR-0025's Context for the motivating repo and its concrete,
verified finding: five CloudWatch alarms whose `metric_name` no
service emits, permanently non-firing, invisible to both a manual code
review and any symbol-graph indexer, because one side of the
dependency is a C# object literal and the other an HCL string. That
repo's own spec ranks ten such dependency classes (metric names, env
vars, DynamoDB attributes, Kafka topics, WS wire-protocol fields,
Parquet/Glue columns, SID protocol subtype IDs, doc-asserted facts, …)
— all structurally the same shape: **a bare string literal, spelled
identically (or near-identically) in two or more files of different
languages, where nothing in either language's own grammar connects
them.**

Spec §4.1/§4.2's node/edge vocabulary has no way to represent this: a
`Symbol` has a `name` (an identifier, not free-text), a `Module` is a
package path, and `calls`/`imports`/`references` are all between two
`Symbol`/`File`/`Module` nodes reached through *language* structure.
This ADR is a genuine amendment, not an implementation detail.

## Decision

**New node kind, `Contract`; two new edge kinds, `produces`/
`consumes`.** This slice implements exactly one category
(`metric_name`, ADR-0025's HCL side + this ADR's C# side) end-to-end,
as a proof that the mechanism generalizes — see the "slice 1 scope"
notes below for what's deliberately deferred, and the SID Cloud spec's
own ranked list (metric names, then env vars, DynamoDB attributes, …)
for the intended follow-on order.

### `ContractNode`

```rust
pub struct ContractNode {
    pub category: String,
    pub qualifier: Option<TaintedString>,
    pub value: TaintedString,
}
```

- **`value`/`qualifier` are `TaintedString`, not `String`.** A string
  literal is arbitrary captured source text — exactly where a
  credential would hide — so INV-6 must scan it. This is *not* the
  `FileNode.path`/`SymbolNode.name` carve-out ADR-0017 documents (those
  are extractor-computed identifiers, never free-form captured text).
  `redact::redact` now has a `NodeData::Contract` match arm alongside
  its existing `Symbol.signature` one, scanning both fields.
- **IDs are computed from the *raw* extracted string, before
  `TaintedString` wraps it, before `persist`'s redact step runs.**
  `graph::contract_id(category, qualifier, value)` — canonical key
  `contract:<category>:<qualifier-or-empty>:<value>`, same blake3-prefix
  recipe as every other node kind (§4.3). Consequence: a later-redacted
  high-entropy value (unlikely for a metric name, plausible for some
  future category) still correctly joins its producer and consumer
  edges — only the *rendered* value in `graph.json`/query output is
  masked, never the graph's connectivity.
- **`category` is an open string, not a closed enum** — ADR-0027's
  `.carto/contracts.json` is exactly how a repo adds a new one; a
  closed enum would defeat that.
- **`qualifier` is part of node identity, not a display-only field.**
  The SID Cloud spec's own worked example is the failure mode this
  guards: `CUSTOMER_SUBSCRIPTIONS_TABLE` (Lambda convention) and
  `DYNAMO_CUSTOMER_SUBSCRIPTIONS_TABLE` (ECS convention) "are both
  correct; an indexer must not fix one to the other." Two same-spelled
  metric names under different CloudWatch namespaces must not collapse
  onto one `Contract` node — verified directly:
  `lang::resolve::tests::same_value_different_qualifier_is_two_distinct_contract_nodes`
  and `fixtures/sid-like`'s `streamer_errors`/`kafkaErrors` pair (same
  bare name, different namespace, deliberately not cross-matched).

### `Produces`/`Consumes` edges

| Edge | From → To | Confidence |
|---|---|---|
| `produces` | Symbol \| File → Contract | `certain` if the syntax states the claim outright; `inferred` if the rule is interpreting the shape |
| `consumes` | Symbol \| File → Contract | same rule |

`Edge::new` already requires confidence + at least one evidence string
(INV-8) — nothing about that constructor changes. This slice's two
built-in rules land on opposite sides of the certain/inferred line for
a principled reason: HCL's `metric_name = "X"` attribute *is* the
claim "this resource references this literal" — the grammar states it
outright, `certain`. C#'s `new { Name = "X", ... }` anonymous-object
member is this rule's own interpretation of what a metric emission
looks like, not something the grammar asserts — `inferred`, the same
distinction ADR-0016 drew for `new Foo()` resolving to a type.

### Honest absence, and where slice 1 narrows the original design

A value that isn't a plain string literal — HCL's `metric_name =
"${local.prefix}Drops"`, C#'s `Name = ComputeDynamicName()` — produces
**no** `RawLiteral`, hence no `Contract` node and no edge: INV-8's
"missing honestly beats guessing" applied to this new node kind.
Verified directly (`lang::hcl::tests::interpolated_metric_name_is_not_extracted`,
`lang::csharp::tests::computed_name_member_is_not_extracted`) and via
the fixture (`infra/alarms.tf`'s `dynamic_alarm`,
`EmfMetricsExportService.cs`'s `dynamicMetric` — neither may ever
appear in `orphans` output, either direction).

**Scope note, narrower than originally planned:** the design this ADR
was scoped against also called for a per-category *count* of these
dropped sites (an `uncaptured_contract_sites` figure, mirroring
ADR-0020/0023's `uncaptured_inbound_calls`/`uncaptured_outbound_calls`
pattern), surfaced on `orphans`. Implementing that honestly requires a
place in `graph.json` to hold a repo-wide or per-file count reachable
from the query layer that isn't itself a node attached to real
producer/consumer edges (a `Contract` node with a count but no edges
would violate this same file's own invariant that every `Contract`
node has at least one `produces`/`consumes` edge — see `orphans`'s
module doc). None of `GraphDocument`'s top-level shape, `FileNode`,
`Manifest` (never read back by any query command today — checked
directly, no call site exists), or a new node kind was a clean fit
within this slice's time budget. **Deliberately deferred, not silently
dropped**: an interpolated/computed literal is currently
indistinguishable from "this position was never looked at" — a real,
disclosed gap, not a claim of zero such sites. Revisit when a second
category (env vars, say) makes the right shape clearer from two data
points instead of one.

### Query-layer additions

Two new commands/tools, neither in spec §7.1 (ADR-0026, same status as
`join --explain` would eventually be for the real infra join):

- **`carto contract <value>`** (`query::contract`) — every producer and
  consumer of one value, exact match. The read that answers "who else
  touches this."
- **`carto orphans`** (`query::orphans`) — every `Contract` with a
  `consumes` edge and no `produces` edge, and the mirror. **This is
  the acceptance-test command** — `orphans --category metric_name`
  against `fixtures/sid-like` returns exactly the fixture's one dead
  alarm (`SequenceGapsTotal`) and one unwatched emission
  (`KafkaErrorsTotal`, `SIDCloud/Ingest`), reproducing the real repo's
  finding at fixture scale
  (`crates/carto-cli/tests/cli_contracts.rs::orphans_finds_exactly_the_dead_alarm_and_the_unwatched_metric`).
  A `Contract` node with *neither* edge is architecturally impossible
  (`resolve.rs` never creates one without immediately attaching an
  edge), so `orphans`'s two-list shape is exhaustive by construction —
  documented in the module's own doc comment rather than handled as a
  silent third case.

Both follow the existing command shape exactly: a plain serde result
struct in `carto-core` (`ContractResult`/`OrphansResult`), a thin CLI
wrapper (`contract_cmd.rs`/`orphans_cmd.rs`), an MCP tool calling the
identical core function (`contract_tool.rs`/`orphans_tool.rs`) — spec
§7.1's "commands = MCP tools, same core functions," even though these
two commands are themselves outside §7.1's named list.

**Regression-tested pitfall:** matching by value must compare
*rendered* content
(`TaintedString::render_capped`), not `TaintedString`'s own derived
`PartialEq` — that also compares `provenance`, and
`TaintedString::deserialize` always re-tags loaded content
`Provenance::Ingested` regardless of how it was indexed (`taint.rs`'s
documented behavior). A query-time needle built with any other
provenance would silently never match real `graph.json` data while
still passing an in-memory-only test. Both `query::contract` and
`query::orphans` are covered by a dedicated round-trip test
(`matches_after_a_real_json_round_trip` /
`survives_a_real_json_round_trip`) that serializes and re-deserializes
the fixture graph before querying, specifically to catch this class of
bug rather than only the in-memory shortcut.

### `SCHEMA_VERSION` 3 → 4

A v3 `graph.json` structurally cannot contain a `Contract` node —
`orphans`/`contract` against one must be rejected with "re-run `carto
index`," not silently answer from a graph that never looked. Same
reasoning ADR-0020/0023 used for their own bumps.

## Consequences

- `NodeData`, `EdgeKind` match arms across `query::{find,map,deps,mod}`
  all gained a `Contract` case — verified exhaustively by the compiler
  (no `_ =>` wildcard was used to paper over any of them); `find`
  doesn't search `Contract` nodes (that's `contract`/`orphans`'s job),
  `map`'s counts/`path_in_scope` treat a `Contract` the same "no
  directory of its own" way a `Module` already is.
- `fixtures/sid-like/` (new, synthetic, spec §11.1) is the committed
  regression fixture; the real SID Cloud repo's acceptance test
  (*"which Terraform CloudWatch metric names are never emitted by any
  service?"* → exactly five names) was not reachable from this
  implementation session — left for the user to run directly, per the
  clarifying round's stated decision.
- Determinism (INV-7) reconfirmed with `Contract` nodes present: a
  double-index-and-compare of `fixtures/sid-like` is byte-identical.
- Everything named "next" in the SID Cloud spec's own ranked list (env
  vars, DynamoDB attributes, Kafka topics, WS wire-protocol fields,
  Parquet/Glue columns, subtype IDs, doc-mention edges, the `.csproj`/
  `implements` graph) is unbuilt — this ADR's node/edge vocabulary is
  designed to need no new node or edge kind for any of them, only new
  `ContractRule`s (ADR-0027) and new per-language literal capture.

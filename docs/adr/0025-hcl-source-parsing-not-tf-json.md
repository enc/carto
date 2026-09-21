# 0025 — HCL source parsing instead of `terraform show -json`

**Status:** accepted · **Date:** 2026-08-04 · **Milestone:** SID Cloud
cross-language contract slice (spec §6 deviation)

## Context

A user request asked whether carto could support a large, real
monorepo (services in C#, TS/React, Go, Rust; infra in Terraform;
docs) whose most-costly dependency edges are **string-keyed contracts
no compiler checks** — a CloudWatch alarm's `metric_name` referencing a
name no service actually emits, an env var whose Terraform-set name and
C#-read name differ by ECS-vs-Lambda convention, a DynamoDB attribute
name duplicated as string literals across services with no shared
type. Scoping that repo surfaced a concrete, verified example: five
`SIDCloud/Ingest` CloudWatch alarms reference metric names no emitter
produces, all `treat_missing_data = "notBreaching"` and therefore
permanently in OK — a silent-loss class that survived a full manual
code review and that no symbol-graph indexer would surface, because one
side is a C# object literal and the other an HCL string.

Spec §6.2 assumes the Terraform *input* is `terraform show -json
plan_or_state` output — "primary, fully resolved." That repo's own
questions can't be answered from a resolved plan:

- *"Unused Terraform locals"* / *"`local.X` referenced but never
  defined"* are source-level facts a resolved plan has already erased
  — `terraform show -json` only ever contains what successfully
  resolved.
- The repo's own two-file-per-environment pattern
  (`locals.tf.simu`/`locals.tf.prod`, copied by tooling into a
  gitignored `locals_env.tf`) means the *source* files carrying the
  facts worth indexing never produce a plan at all in the form carto
  would see it.

This ADR covers only the metric-name slice (ADR-0026's acceptance
test); env vars, DynamoDB attributes, and the rest of the ranked list
are named as follow-up work, not built here.

## Decision

**Parse `.tf`/`.hcl` source directly**, via a new `HclExtractor`
(`crates/carto-core/src/lang/hcl.rs`) added to the `LangExtractor`
registry — the same "per-language extractor, language-agnostic
`resolve`" shape every other language already uses, not a parallel
infra-ingestion path. `Lang::Hcl` already existed in the enum with no
extractor registered; this fills that gap.

- **New dependency: `tree-sitter-hcl = "1.1.0"`** (`crates/
  carto-grammars`), the `tree-sitter-grammars` org's HCL grammar,
  ships a Terraform dialect. Verified via the crates.io sparse index at
  implementation time (spec §13): its only normal dependency is
  `tree-sitter-language ^0.1` (plus a `cc` build dependency) — no
  second `tree-sitter` version, so `deny.toml`'s `multiple-versions =
  "deny"` is satisfied with **no** `[[bans.skip]]` needed. Confirmed
  by running `cargo deny check bans licenses sources` after adding it:
  `bans ok, licenses ok, sources ok`.
- **No `.scm` query files for this extractor** — unlike every other
  language here. `tree-sitter-hcl`'s grammar carries no named fields at
  all (verified empirically by dumping a parse tree's `to_sexp()`;
  `docs.rs` isn't reachable from the implementation sandbox to check
  ahead of time): `block`/`attribute`/`string_lit` are pure
  positional/variadic node lists. A tree-sitter query would have to
  match by node-kind position exactly the way the plain recursive walk
  in `hcl.rs` already does — no more declarative, and one more moving
  part.
- **Provenance is `Declared`, never `Resolved`, for anything HCL-derived
  this slice.** Source HCL is unresolved by construction — a
  `metric_name` attribute's value may be an interpolation
  (`"${local.prefix}Drops"`), which this slice doesn't attempt to
  evaluate (see ADR-0026 for the honest-omission handling of that
  case). Spec §6.2's fully-resolved tf-json path, if ever added
  alongside this one, would be the only producer of `Resolved`
  infra facts.
- **No `IacResource`/infra-graph node this slice.** Spec §4.1's
  `IacResource` (`depends_on`, IAM extraction, the attribute allowlist,
  §6.6) is real M2 infra-graph scope this ADR does not build — HCL's
  only output here is `RawLiteral`s (ADR-0026), attached directly to
  the `File` node. Building `IacResource` properly (with §6.6's
  allowlist enforcement, since attribute values are the main secret-leak
  channel) is real, separate work for whenever the infra-graph slice
  is actually undertaken, not a byproduct of this one.
- **Walk fix, not infra-specific:** `walk::classify` computed a file's
  `Lang` from `Path::extension()` alone, which only ever sees a file
  name's *last* extension segment — `locals.tf.simu` classified as
  `simu` → `Lang::Other`, never reaching any extractor. Fixed generally
  (`walk::lang_for_file_name`, checked ahead of the ordinary
  single-extension lookup): any `<name>.tf.<anything>` classifies
  `Hcl`, not a `simu`/`prod` allowlist. `*.tfvars` stays on spec
  §5.1's sensitive denylist (contents never read) — correct as-is,
  unaffected by this fix.

## Consequences

- `carto index` on an HCL-containing repo now produces real `File`
  nodes with `lang: "hcl"` plus (ADR-0026) `Contract` nodes/edges for
  the one shape this slice recognizes — previously every `.tf` file
  was a bare, uninterpreted `File` node.
- `fixtures/sid-like/infra/locals.tf.simu` exercises the extension
  fix directly (`walk::tests::two_part_tf_extension_classifies_as_hcl`,
  `cli_contracts.rs::two_part_tf_extension_file_is_indexed_as_hcl_with_no_crash`).
- Deliberately **not** done here (see ADR-0026's own scope note and
  the SID Cloud spec's own ranked list for what's next): env var
  producer/consumer matching, `IacResource`/`depends_on`/IAM, Terraform
  reachability (unused locals, undefined `local.X`), tf-json ingestion
  as an *additional*, not alternative, input.

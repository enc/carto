# 0037 — Terraform rollup, real aggregator parsing, `.carto/roots.json` `exclude`

**Status:** accepted · **Date:** 2026-09-21 · **Milestone:** multi-root
support, follow-up slice 2 (amends ADR-0034's marker table)

## Context

The same review that produced ADR-0036 found ADR-0034's terraform
marker — "≥1 HCL file in a directory, checked last, the weakest signal"
— over-fragments a normal infra tree. Built and indexed:

```
infra/main.tf
infra/modules/vpc/main.tf
infra/modules/rds/main.tf
infra/envs/prod/main.tf
infra/envs/dev/main.tf
```

produced five components: `infra`, `vpc`, `rds`, `prod`, `dev` — one per
directory containing a `.tf` file, since ADR-0034's rule operates purely
per-directory with no notion that these directories together represent
one project. This is a real problem, not merely untidy: `prod`/`dev`
are generic names likely to collide with real service names in a larger
repo, and `--component`'s whole purpose — narrowing a query to one
project — is defeated for the flagship `orphans --category metric_name`
use case (ADR-0026) when the infra project's own resources are smeared
across five components instead of one.

Two smaller issues surfaced in the same pass: `is_aggregator`
(ADR-0034) used substring matching (`content.contains("[workspace]")`)
rather than real parsing — for `Cargo.toml` this can false-positive on
a comment or a `[workspace.metadata.*]` nested table containing the
literal text `[workspace]` nowhere, but could plausibly be fooled by
adversarial or unusual formatting; for `package.json`, a
`"workspaces"` substring inside an unrelated string value (e.g. a
`description` field) would wrongly suppress a real component. Neither
recognized pnpm/lerna/turborepo/nx/rush workspace roots at all — those
tools declare their members in a sibling file
(`pnpm-workspace.yaml`, etc.), not inside `package.json`'s own
`"workspaces"` key, so `is_aggregator` had no way to see them.

Separately, `.carto/roots.json` had no way to say "detect everything
except this one directory" — only "replace detection with these
declarations" (`roots` with default `detect: false`) or "detect,
then also add these" (`detect: true` + `roots`). A repo that wants
auto-detection everywhere *except* one path (a `services/x/infra`
subdirectory that should stay part of `x`, say, rather than adding a
`.carto/roots.json` `roots` declaration for every other component just
to work around one bad one) had no way to express that.

## Decision

**Terraform rollup**, in `detect_components`
(`crates/carto-core/src/components/mod.rs`): every directory whose only
marker is "≥1 HCL file" is collected into a separate set (`tf_dirs`)
instead of becoming a candidate immediately, and put through two
passes before candidates are finalized:

1. **Drop any terraform-only directory at or under an exact-marker
   component's own directory.** `services/orders/infra` belongs to
   `orders`, not to a separate terraform component — semantically the
   same "innermost, but not a rival" relationship any other subdirectory
   of a component already has.
2. **Repeatedly merge any two surviving terraform directories to their
   lowest common ancestor**, provided that ancestor is neither the repo
   root nor at-or-under a strong (exact-marker) component's own
   directory, until no further merge applies
   (`merge_terraform_dirs`/`lowest_common_ancestor`). A merge whose
   result already equals one of its two inputs (`infra` absorbing
   `infra/modules/vpc`) collapses a directly-nested `.tf` tree to its
   own topmost directory; a merge between two directories with no
   shared `.tf`-bearing ancestor (`infra/envs/prod` and
   `infra/modules/vpc`, each already collapsed to itself) rolls
   scattered environment/module directories up to the one directory
   that represents the infra project as a whole. Two genuinely
   unrelated terraform trees (no shared ancestor short of the repo
   root) are correctly left unmerged — the algorithm never lets a merge
   reach an empty-string (root) ancestor.

`O(n²)` per merge, `n` = directories containing ≥1 HCL file — bounded
by how terraform-shaped a repo actually is, not by repo size.
Deterministic: `tf_dirs` is a `BTreeSet<String>` throughout, so the same
input always finds the same first mergeable pair in the same order
(INV-7).

**Real aggregator parsing**, `is_aggregator`: `Cargo.toml` is scanned
line-anchored (a line that, after trimming, is *exactly* `[workspace]`
or `[package]` — not a substring, so `[workspace.metadata.foo]` and a
`# ... [workspace] ...` comment no longer count) rather than adding a
`toml` dependency (spec §13 + `deny.toml`'s `multiple-versions =
"deny"`; a line scan is sufficient for the one shape being
distinguished). `package.json` is parsed as real JSON via
`serde_json::Value` (already a `carto-core` dependency) and checked for
a top-level `"workspaces"` key, rather than a substring scan that could
fire on an unrelated string value. `AGGREGATOR_SIBLING_MARKERS`
(`pnpm-workspace.yaml`, `lerna.json`, `turbo.json`, `nx.json`,
`rush.json`) — presence in the directory's own basenames, no YAML/JSON
parsing of the sibling file needed — additionally marks a `package.json`
directory an aggregator regardless of its own content, closing the
pnpm/lerna/turborepo/nx/rush gap. A parse failure or unreadable file
still falls through to "not an aggregator," the same best-effort
treatment ADR-0034 established.

**`.carto/roots.json` gains `exclude: Vec<String>`** — repo-relative
paths dropped from auto-detection (validated like a `roots` entry's
`path`: non-empty, repo-relative, no `..` segment — a config value is
repo-author intent, hard-erroring on a typo rather than silently
ignoring it — except an exclude matching nothing is *not* an error,
since it's a legitimate "belt and braces" entry with nothing to do
yet, unlike a `roots` path that must match a real walked file).
Applied (`apply_excludes`) before `roots`, so a declared root at the
same path as an exclude entry still ends up present — `exclude` only
suppresses auto-detection, a `roots` declaration is unconditional
intent.

## Consequences

- `detect_components` gains `is_at_or_under`/`is_at_or_under_any`/
  `lowest_common_ancestor`/`merge_terraform_dirs`; `is_aggregator`
  gains a `basenames: &[&str]` parameter and `AGGREGATOR_SIBLING_
  MARKERS`; `ConfigDoc` gains `exclude`; a new `apply_excludes`
  function mirrors `apply_declared_roots`' validation style.
- 11 new `components::tests` (35 total, up from 24): T1 (nested
  absorption), T3 (scattered rollup), T2 (nested-under-strong-dir),
  disjoint-trees-stay-separate, pnpm sibling suppression, real-JSON-
  parsing (a `"workspaces"` string value doesn't suppress), line-
  anchored `Cargo.toml` scanning (a `[workspace.metadata]` table
  doesn't suppress), `exclude` (drops, matches-nothing-is-fine,
  invalid-path errors, re-added-by-a-declared-root). All 24
  pre-existing `components` tests pass unchanged — the single-`.tf`-
  file case (`at_least_one_tf_file_is_detected_as_terraform_including_
  two_part_extension`) and both existing aggregator-suppression tests
  are unaffected by either the rollup or the parsing rewrite.
- `fixtures/monorepo` gains an `infra/envs/{prod,dev}/main.tf` +
  `infra/modules/vpc/main.tf` subtree (T3's exact shape, no `.tf` file
  directly in `infra/` itself) — `crates/carto-cli/tests/
  cli_multiroot.rs` updated to expect six components (was five),
  including `component_of_path("infra/envs/prod/main.tf") ==
  Some("infra")`, not a separate `prod` component.
  `fixtures/sid-like/infra` (a single, non-fragmented terraform
  directory — the case ADR-0034 was originally verified against) is
  unaffected: re-indexed and confirmed still exactly one `infra`
  component.
- Verified additive/backward-compatible the same way ADR-0034/ADR-0036
  were: `fixtures/mixed`/`fixtures/rust-crate` `graph.json` confirmed
  byte-identical before/after by diff (neither has any HCL or
  aggregator-shaped manifest). Double-index determinism reverified on
  the synthetic fragmented-terraform tree above and on the updated
  `fixtures/monorepo`.
- `docs/STATUS.md`'s "deliberately absent" list needs no new entry
  retired — the terraform-fragmentation behavior wasn't previously
  documented there as deliberate, only discovered by this review.

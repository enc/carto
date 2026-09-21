# 0027 — Contract classification config (`.carto/contracts.json`)

**Status:** accepted · **Date:** 2026-08-04 · **Milestone:** SID Cloud
cross-language contract slice

## Context

ADR-0026 introduces the `Contract` node kind; this ADR covers how a
captured literal (`RawLiteral::position`, e.g. an HCL
`aws_cloudwatch_metric_alarm.metric_name` attribute or a C#
`object-init:Name` member) gets turned into a `category` + producer/
consumer `role` + `Confidence`. The clarifying round's stated
preference: a **declarative name→category map**, not hard-coded rules
with no repo-level extension point, and not repo-supplied tree-sitter
queries (which would make repo-controlled text steer the extractor
directly — its own, larger INV-5 trust question).

## Decision

`crate::contracts::ContractRules` — built-in default rules, plus an
optional `<repo_root>/.carto/contracts.json` that **extends** them.

```json
{
  "contracts": [
    { "category": "env_var", "role": "consumer", "lang": "csharp",
      "position": "call-arg:GetEnvironmentVariable" }
  ]
}
```

- **JSON, not TOML — zero new dependencies.** `serde_json` is already
  a `carto-core` dependency (spec §13's starting allowlist); `toml` is
  not, and spec §13 requires an ADR to add one. Worth the slightly
  worse ergonomics.
- **`role`/`lang` are a closed, validated vocabulary; `category` is
  open.** A `role` outside `producer`/`consumer`, or a `lang` outside
  the languages carto's registry actually has, is a hard `UserError`
  naming the bad value and the file path — never silently ignored.
  `category`, by contrast, is exactly what this file exists to let a
  repo extend; validating it against anything would defeat the point.
- **First-match-wins, built-in rules checked first.** Repo rules are
  appended after the built-ins, so a repo-declared rule can only *add*
  a new classification over a position the built-in rules don't
  already claim — it can never silently override a built-in rule's
  confidence/evidence for a position carto's own extractors already
  understand.
- **Recognition (does this position matter at all) stays in the
  extractor; classification (what it means) is what this file
  controls.** `RawLiteral::position` is only ever emitted for
  positions an extractor already hard-codes interest in (HCL's
  `metric_name` attribute inside a `resource` block, C#'s `Name`
  member of an anonymous-object literal) — `.carto/contracts.json`
  cannot make an extractor start looking somewhere new; it only labels
  what's already captured. This is deliberately narrower than the
  fully declarative "config controls extraction site *and* meaning"
  design considered during planning — see the "slice 1 narrowing" note
  below.
- **Qualifier derivation is a per-language mechanical concern, not
  config-driven.** How to find a literal's qualifying value (HCL: a
  sibling `namespace` attribute in the same block; C#: the literal's own
  *enclosing type's* `const string Namespace = "...";`, scoped so a
  second class in the same file with its own `Namespace` const can't
  leak onto the first class's emissions) is hand-written in each
  extractor, mirroring how every other per-language judgment call in
  this codebase already lives in the extractor, not a shared config
  schema. A fully generic `QualifierRule` engine (config-declared
  "look at this sibling attribute" / "look at this const field name")
  was part of the original design sketch but added real complexity for
  a two-rule slice with no second data point yet to generalize from;
  revisit once a second category's qualifier shape (env vars' ECS-vs-
  Lambda prefix convention, say) gives two real examples to design
  against instead of one.
- **The config file's own presence/absence is not yet joined into
  `manifest.json`'s `ignore_rule_digest`-style provenance tracking**
  (unlike `walk`'s own `.cartoignore` digest). Two indexes built under
  different `.carto/contracts.json` contents are therefore not
  distinguishable from `manifest.json` alone today — a smaller,
  disclosed gap than the config mechanism itself, deferred for the
  same reason as ADR-0026's `uncaptured_contract_sites` cut.
- **Trust posture, stated explicitly (not merely assumed):** this is
  repo-controlled text steering carto's own classification decisions —
  the same posture `.cartoignore` already has (additive,
  non-configurable-away-from denylist still enforced underneath), and
  it cannot execute anything (INV-2) or widen a write path (INV-3/
  INV-4). It *is* a real, deliberate widening of what repo content
  influences (which categories exist, which `Confidence` a repo-added
  rule gets — always `Inferred`, never `Certain`, since a repo asserting
  its own rule is definitionally not the same as the extractor's
  grammar stating the claim outright), worth saying out loud rather
  than leaving implicit.

## Consequences

- `crates/carto-core/src/contracts/mod.rs` (new module, registered in
  `lib.rs`) — `ContractRules::builtin()` (no repo file consulted, used
  by tests and any caller that doesn't need overrides) and
  `ContractRules::load(repo_root)` (built-ins + repo file, `Result`
  because a malformed file must stop indexing, not silently classify
  nothing).
- `lang::extract_and_resolve` now returns `crate::error::Result<
  ResolvedExtraction>` instead of a bare `ResolvedExtraction` — the one
  signature change this ADR's error path required;
  `indexer::build_and_persist`'s single call site propagates it with
  `?`.
- Verified: a repo-added rule for a new position/category classifies
  correctly (`contracts::tests::repo_config_extends_the_rule_set`); an
  invalid `role` is rejected with a `UserError` naming the file
  (`unknown_role_is_a_user_error`); a repo with no `.carto/
  contracts.json` at all falls back cleanly to built-ins
  (`missing_config_file_falls_back_to_builtins_only`).

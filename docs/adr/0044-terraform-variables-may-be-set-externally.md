# 0044 — A Terraform variable may be set outside the code

**Status:** accepted · **Date:** 2026-09-29 · **Milestone:** Terraform
source support, slice 4 of 4 (closes the ADR-0041 → 0044 sequence) ·
**No `SCHEMA_VERSION` change** (a query payload field and a walk
classification; no persisted graph type changed)

## Context

`deps var.region --dir in` answers "who uses this variable" from
references in the code. But a Terraform variable is routinely given its
value from *outside* the code: a `*.tfvars` / `*.auto.tfvars` file,
`TF_VAR_*` environment variables, `-var` / `-var-file` on the command
line, a caller's module argument, a Terragrunt `inputs` entry. A
variable with zero inbound edges is therefore **not** evidence that it is
unset or unused, and an answer that reads that way would be exactly the
absence-vs-zero confusion INV-8 and every `SCHEMA_VERSION` bump in this
codebase exist to prevent.

Spec §5.1 puts `*.tfvars` on the sensitive denylist (contents never
read), and ADR-0025 affirmed it. The user's direction for this slice: *"it's
not about reading the file, but handle the implications in the code."* So
the file stays unread; the implication is modelled.

## Decision

**tfvars stay unread. Only their existence and path are used.** The walk
already records a sensitive file as a `File` node with `excluded:
sensitive`, `sha256: null`, contents never opened. `*.tfvars.json` — the
JSON form of the same file, previously walked, hashed and handed to the
JSON language — joins the sensitive set (spec §5.1 amendment; walk test
extended).

**`DepsResult::root_may_be_set_externally: Option<ExternalInputs>`**,
`ExternalInputs { tfvars_files: Vec<String> }`:

- `Some` for **every** `tf_variable` root — including when no tfvars file
  exists — so the caveat is never silently absent; `None` for every other
  root (an additive `null` on the JSON/MCP payload).
- `tfvars_files` = sensitive-excluded `File` nodes named `*.tfvars` or
  `*.tfvars.json` in the variable's **own directory** (a root module's
  tfvars sit beside its `variable` blocks), sorted, paths only. Computed
  at **query time** in `query/deps.rs::external_inputs` from data already
  in `graph.json` — no schema change and no new index-time pass.
- `ExternalInputs::note()` is the single source of the one-line wording,
  used by both the CLI (`deps_cmd.rs`) and the MCP tool (`deps_tool.rs`) so
  the two cannot drift. It is printed for `--dir in` and `--dir both`
  only (an outbound question doesn't need it), lists at most ten paths
  (the structured field carries all), and always ends with "Zero inbound
  edges is not evidence it is unset or unused". It names the other
  sources too: `TF_VAR_*`, `-var`/`-var-file`, a caller's module
  argument, a Terragrunt input.

No attempt is made to say *which* source sets a value, to parse a tfvars
file, or to infer a value: that would require reading the file.

## Consequences

- `query/deps.rs`: `ExternalInputs` (exported from `query`),
  `external_inputs`, the new field; `deps_cmd.rs` and `deps_tool.rs`
  render the note. `walk/mod.rs`: `.tfvars.json` sensitivity + denylist
  description. `fixtures/rust-crate.deps.golden.json` regenerated — the
  diff is exactly the one added `"root_may_be_set_externally": null`
  line (checked).
- `fixtures/tf-modules/infra/envs/prod/prod.tfvars` and
  `prod.auto.tfvars.json` carry clearly fake canary values; tests assert
  the canaries appear nowhere in `graph.json` or any output, that both
  files are recorded `excluded: sensitive` with no hash, that the note
  names both paths, that the caveat prints for a module-directory
  variable with no tfvars file, that it is absent for `--dir out` and for
  non-variable roots, that the JSON field is `null` for a non-variable,
  and an MCP round trip (text and `structuredContent`).
- Not built: a `carto unused --kind variable` style report, a
  `.carto/deps.json`-style declaration of externally supplied variables,
  reading tfvars. Any of those would need a new decision about §5.1.
- This closes the Terraform source sequence (ADR-0041 symbols and
  references, 0042 module calls, 0043 Terragrunt, 0044 this). Plan/state
  JSON ingestion (spec §6.2: `IacResource`, resolved `depends_on`, IAM)
  remains the separate, later slice it always was.

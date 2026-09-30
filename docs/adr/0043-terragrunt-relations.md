# 0043 — Terragrunt relations and download-cache pruning

**Status:** accepted · **Date:** 2026-09-29 · **Milestone:** Terraform
source support, slice 3 of 4 (extends ADR-0041/0042) · **No
`SCHEMA_VERSION` change** (`SymKind::TgDependency` was added with
ADR-0041's bump; `RawTerragrunt` is extractor-internal)

## Context

Live infrastructure repos are typically Terragrunt trees: one
`terragrunt.hcl` per unit, each naming a Terraform module (`terraform {
source }`), the units it depends on (`dependency`), a shared root
(`include`) and the values it feeds the module (`inputs`). Before this ADR
those files were literals-only HCL: the unit → module → dependency chain
was invisible to `deps`/`map`. The user asked for the Terragrunt unit
dependency chain, kept as **file-level edges** within ADR-0037's single
rolled-up `infra` component (no component change).

INV-2 forbids running `terragrunt`, so everything here is static:
Terragrunt's functions are never evaluated.

## Decision

**Which files.** Only `terragrunt.hcl` and `root.hcl` are Terragrunt
files. Other `*.hcl` (`common.hcl`, `env.hcl`, Packer/Nomad/Vault,
`.terraform.lock.hcl`, `.tflint.hcl`) stay literals-only: a file's role
cannot be told from its extension, and their values are read through
`read_terragrunt_config(...)`, which carto does not follow. A Terragrunt
file is **its own resolution scope** (not its directory's): its `locals`
are file-local, and a `local.x` there must never match a `.tf` local in
the same directory. Symbols: `locals` attributes (`local.x`) and
`dependency "n"` blocks (`dependency.n`, `SymKind::TgDependency`); no
`variable`/`resource`/… (those aren't Terragrunt).

**Statically decidable path shapes only** (`TgPath`, classified from the
tree; verified on real parse trees):

| Expression | Class |
|---|---|
| plain string `"../vpc"`, `"git::…"` | `Literal` |
| `"${get_terragrunt_dir()}"` + optional one literal | `TerragruntDir(suffix)` |
| `find_in_parent_folders()` / `("name")` | `FindInParentFolders` |
| `get_repo_root()`, `path_relative_to_include()`, `include.x.locals.y`, any other interpolation | `Unresolvable` → no edge, no claim |

**Edges** (all from the Terragrunt file; `certain` only for exact
relative paths, per spec §5.3 rule 1):

| Fact | Edge | Confidence | Evidence |
|---|---|---|---|
| `terraform.source`, local (`./`, `../`, `//` collapsed) or `TerragruntDir` | `imports` → every `.tf` file of the module directory | certain | `tg-terraform-source` |
| `terraform.source`, remote | `imports` → external `Module` (ADR-0042's sanitized key) | certain | `tg-terraform-source:remote` |
| `dependency.config_path` literal / `TerragruntDir` | `imports` → `<dir>/terragrunt.hcl` | certain | `tg-dependency` |
| `dependencies { paths = […] }` | same | certain | `tg-dependencies` |
| `include … path` literal / `TerragruntDir` | `imports` → that file | certain | `tg-include` |
| `include … find_in_parent_folders(name?)` | `imports` → nearest ancestor-directory file of that name among the **walked** files (starting at the parent directory; default name `terragrunt.hcl`) | **inferred** | `tg-include:find_in_parent_folders` |
| `dependency.x…` | `references` → `dependency.x` in the same file | inferred | `tf-ref:same-module` |
| `dependency.x.outputs.y` | `references` → `output "y"` of **the dependency unit's own source module** (dependency → its `terragrunt.hcl` → that unit's `terraform.source`) | inferred | `tg-dependency-output` |
| `inputs = { k = … }` key `k` (bare or plain-string key) | `references` file → the unit's source module's `variable "k"` | inferred | `tg-input:k` |

`find_in_parent_folders` is a pure lookup over the files carto walked —
nothing executed (INV-2) — and only `inferred` because a closer match
could be gitignored and invisible. An `inputs` key with no matching
variable is dropped **silently**: Terragrunt forwards unused inputs as
`TF_VAR_*`, so it is not an error. `root.hcl`'s own `inputs` have no
`terraform.source`, so they link nowhere. Misses that *are* reported (on
the `dependency.n` symbol or the referencing symbol's
`unresolved_calls`): `config_path:<dir>` (no such unit), and
`dependency.x.outputs.y` when the chain resolves to a module that has no
such output. A chain that cannot be followed statically says nothing.
Misses at file scope (an `inputs` attribute is not a symbol) have nowhere
to be recorded — ADR-0031's existing limit.

**`.terraform/` and `.terragrunt-cache/` join the walk's denied
directory names** (spec §5.1 amendment): they hold downloaded third-party
module copies — like `node_modules` — and `.terraform/` can hold backend
state/credentials. Without this, a cached copy of a module would appear as
a second definition of every variable it declares. `.terraform.lock.hcl`
is a file, unaffected (walked, no symbols). The built-in denylist digest
changes (manifest provenance only).

**`map --section infra`** now also lists `terragrunt dependencies (unit
dir -> dependency unit dir)`, counts a unit's `terraform.source` with
module calls, and its remote sources with remote modules.

## Consequences

- `hcl.rs` gains flavor detection, `collect_terragrunt`,
  `classify_tg_path`; `extractor.rs` gains `TgPath`, `RawTgDependency`,
  `RawTerragrunt`; `terraform.rs` gains `scope_of`, `resolve_terragrunt`,
  `resolve_dependency_output`, the `unit_sources` pre-pass, and treats
  `dependency` as a strict root only inside Terragrunt files.
  `walk/mod.rs`: denied names + description.
- `fixtures/terragrunt-live/` (units with local, `${get_terragrunt_dir()}`,
  `${get_repo_root()}` (negative) and remote sources; a missing
  dependency; a cache and a `.terraform/` directory that must be pruned) and
  `cli_terragrunt.rs` (8 tests: fan-out; dependency/include edges and their
  confidence; inputs and cross-unit dependency outputs, unmatched inputs
  silent; misses reported vs silent; file-local `locals`; caches pruned and
  lock file symbol-free; remote source stripped of a fake credential/`?ref`;
  determinism and the `map` chain).
- A `terragrunt.hcl` in a directory that also holds `.tf` files keeps the
  two scopes separate (unit-tested).
- **Not in this slice:** the tfvars implication (ADR-0044); `generate`
  blocks, `read_terragrunt_config`, `include` `expose`/merge semantics
  (a `dependency` defined only in an included file is reported as
  undefined in this file), `terragrunt.stack.hcl`, `unit`/`stack` blocks,
  `mock_outputs`.

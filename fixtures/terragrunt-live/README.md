# terragrunt-live fixture

Synthetic Terragrunt "live" tree. Not real customer code. Exercises
ADR-0043's Terragrunt relations. Everything is under `infra/` so ADR-0037's
rollup gives one `infra` component.

| Path | Exercises |
|---|---|
| `infra/root.hcl` | The include target. `local.region` resolves in **this file only** (Terragrunt `locals` are file-local). Its `inputs` have no `terraform.source`, so link nowhere. |
| `infra/live/prod/vpc/terragrunt.hcl` | `include … find_in_parent_folders("root.hcl")` → `root.hcl` (`inferred`: a pure lookup over walked files, nothing executed). `terraform.source = "../../../modules//vpc"` → `imports` to every `.tf` file in `infra/modules/vpc` (`certain`; `//` collapsed). `inputs`: `cidr`/`region` → the module's `var.cidr`/`var.region` (`tg-input:*`); `unused` has no variable and is dropped silently (Terragrunt forwards unused inputs as `TF_VAR_*`). |
| `infra/live/prod/vpc/.terraform.lock.hcl` | Lock file: literals-only, never symbols. |
| `infra/live/prod/vpc/.terragrunt-cache/`, `.terraform/` | Download caches — pruned from the walk entirely (`var.cache_only`, `var.dot_terraform_only` must not exist). |
| `infra/live/prod/app/terragrunt.hcl` | `${get_terragrunt_dir()}/../../../modules/app` source (statically decidable). `dependency "vpc"` → `../vpc/terragrunt.hcl` (`certain`); `dependencies { paths }` → same file (`tg-dependencies`). `dependency.vpc.outputs.vpc_id` → **the vpc unit's source module's** `output.vpc_id` (`tg-dependency-output`); `…outputs.does_not_exist` → `dependency.vpc.outputs.does_not_exist` in `unresolved_calls` (on `local.missing_out`). `dependency "ghost"` (`../ghost`, no such unit) → `config_path:infra/live/prod/ghost` unresolved. `dependency "abs"` uses `${get_repo_root()}` — not statically decidable: no edge, no claim. |
| `infra/live/prod/remote/terragrunt.hcl` | Remote `git::` source carrying a fake credential and `?ref=`: becomes an external `Module` whose key has both stripped (`git::https://example.com/org/net.git//modules/net`); neither may reach `graph.json`. |
| `infra/live/prod/repo-root/terragrunt.hcl` | `${get_repo_root()}` source: unresolvable, so no edge. |
| `infra/modules/{vpc,app}/*.tf` | The modules the units point at. |

`crates/carto-cli/tests/cli_terragrunt.rs` drives this fixture through the
real `carto` binary.

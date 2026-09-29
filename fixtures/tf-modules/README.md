# tf-modules fixture

Synthetic Terraform tree. Not real customer code. Deliberately small —
exercises ADR-0041's source-level symbols and same-module reference
resolution. Everything sits under `infra/` so ADR-0037's rollup gives one
`infra` component (as in `fixtures/monorepo`) instead of one per
directory.

| Path | Exercises |
|---|---|
| `infra/envs/prod/variables.tf` | `var.region`, `var.name_suffix` — the base declarations. |
| `infra/envs/prod/override.tf` | A second `var.name_suffix`. Terraform merges `override.tf` over the base, so this is **not** a duplicate: references resolve to `variables.tf`'s declaration. |
| `infra/envs/prod/locals.tf.simu`, `locals.tf.prod` | The per-environment variant pattern (ADR-0025). Both define `local.prefix`; `main.tf` (a plain file) references it, so it gets one `env-variant` edge per variant, not one guessed edge. |
| `infra/envs/prod/main.tf` | `local.bucket_name` → `local.prefix` (variants) and `var.name_suffix` (override rule); `local.broken_name` → `local.missing`, which is defined nowhere (recorded in `unresolved_calls`, no edge); resource → resource, resource → `local.bucket_name`, output → resource references; a `dynamic` block whose iterator `statement.value` is silently dropped (not a resource reference). |
| `infra/modules/vpc/*`, `infra/modules/app/*` | Three directories each declare `var.region`. A reference in one must resolve only within its own directory — never to another module's variable. |

`crates/carto-cli/tests/cli_terraform.rs` drives this fixture through the
real `carto` binary.

There is deliberately no `.tfvars` file here: `*.tfvars` is on the
sensitive denylist (spec §5.1) and its contents are never read.

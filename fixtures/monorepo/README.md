# monorepo fixture

Synthetic monorepo-shaped tree. Not real customer code. Deliberately
small — just enough to independently exercise every component-discovery
and component-scoped-resolution outcome ADR-0034/ADR-0035/ADR-0037/
ADR-0039 can reach: manifest-marker auto-detection across four
languages, aggregator suppression, a declared root overriding
auto-detection, the exact same-name-across-components collision that
used to make every one of those symbols unresolvable, the file-scope
(ADR-0031) resolution path under a component, terraform's own rollup
(ADR-0037) — a scattered `infra/envs/*`/`infra/modules/*` tree with no
`.tf` file directly in `infra/` itself collapsing to one `infra`
component instead of fragmenting into `prod`/`dev`/`vpc` — and a real
manifest-declared component dependency (ADR-0039): `orders`'s `go.mod`
`require`s `shared`, so its pre-existing cross-component call is a
*declared* crossing, not an undeclared one.

`crates/carto-cli/tests/cli_multiroot.rs` drives this fixture through
the real `carto` binary — semantic assertions on real CLI stdout and
parsed `graph.json`, not a byte-for-byte golden file (this fixture's
job is proving the end-to-end shape works, which `resolve.rs`'s own
unit tests, verifying the underlying mechanism in isolation, don't
exercise).

## Components and what each exercises

| Path | Marker | Component / kind | Exercises |
|---|---|---|---|
| `Cargo.toml` (repo root) | `[workspace]`, no `[package]` | — | The walked root is never a component candidate (ADR-0034) *and* would be aggregator-suppressed even if it were — two independent reasons this file produces no component. |
| `tools/check.py` | none | `None` bucket | A loose repo-level script belongs to no component — a real, meaningful value, not a gap. |
| `libs/shared/go.mod` + `shared.go` | `go.mod` | `shared` / go | Defines `Shared()`, the unique repo-wide target `services/orders/handler.go` resolves to across a component boundary. |
| `services/orders/go.mod` + `handler.go` + `main.go` | `go.mod` | `orders` / go | Defines `Handler()` (one of three same-named symbols, see below) and `Run()`. `Handler` calls bare `Shared()` with **no Go import at all** — tier (c2), evidence `cross-component`. `Run` calls bare `Handler()` from the *same* component — tier (c1), evidence `same-component`, resolving despite the repo-wide name collision that would otherwise make it ambiguous. `go.mod` also `require`s `shared` (ADR-0039) — a real declared dependency, so the `Handler`→`Shared` cross-component edge does **not** carry `undeclared-dependency` evidence, unlike an otherwise-identical crossing with no manifest declaration. |
| `services/billing/Billing.csproj` + `Handler.cs` | `*.csproj` suffix | `billing` / dotnet | A second, unrelated `Handler` (a C# class) — the collision partner. Never called by `orders`, so must never appear as a candidate for either of `orders`'s bare `Handler()` calls. |
| `web/admin/package.json` + `src/handler.ts` | `package.json` | `admin` / node | A third, unrelated `Handler` (a TS class). `registerHandler(new Handler())` is module-level code with no enclosing symbol — ADR-0031's file-scope fallback, exercised *under* a component: the resolved edge must attach to the `File` node and still carry `admin`'s own component label. |
| `lambdas/ingest/handler.py` | *(none)* | `ingest` / python | No manifest marker anywhere in this directory — only recognized as a component because `.carto/roots.json` declares it explicitly (the declared-root override path, not auto-detection). |
| `.carto/roots.json` | — | — | `{"detect": true, "roots": [{"name": "ingest", "path": "lambdas/ingest", "kind": "python"}]}` — `detect: true` makes the declaration *additive* to auto-detection rather than replacing it, so `shared`/`orders`/`billing`/`admin` are still auto-detected alongside the declared `ingest`. |
| `infra/envs/{prod,dev}/main.tf` + `infra/modules/vpc/main.tf` | ≥1 HCL file per directory, none directly in `infra/` itself | `infra` / terraform | ADR-0037's T3 rollup: three separate `.tf`-bearing directories with no shared `.tf`-bearing ancestor collapse to the one directory that actually represents the infra project (`infra`), not three generically-named components (`prod`/`dev`/`vpc`). |

## Verify with

```bash
cargo run -p carto-cli -- index fixtures/monorepo --out /tmp/carto-monorepo
cargo run -p carto-cli -- map fixtures/monorepo --out /tmp/carto-monorepo --section components
cargo run -p carto-cli -- where Handler fixtures/monorepo --out /tmp/carto-monorepo --exact
cargo run -p carto-cli -- deps Handler fixtures/monorepo --out /tmp/carto-monorepo --dir out
cargo run -p carto-cli -- deps Handler fixtures/monorepo --out /tmp/carto-monorepo --dir out --component orders
```

Expected:

- `map --section components` lists exactly six components (`admin`,
  `billing`, `infra`, `ingest`, `orders`, `shared`), each with its own
  file/symbol counts (`infra` shows `files=3 symbols=0`, not three
  separate `prod`/`dev`/`vpc` rows) and its own `depends_on`
  (`orders`'s row reads `depends_on=shared`; every other component
  reads `depends_on=(none)`), plus a `## cross-component edges` line
  reading `orders -> shared: 1 calls(inferred)` — no `[undeclared]`
  marker, since `orders`'s `go.mod` declares the dependency (ADR-0039).
- `where Handler --exact` (no `--component`) returns all three
  `Handler` symbols, each labeled with its own component
  (`[billing]`/`[orders]`/`[admin]`).
- `deps Handler --dir out` (unscoped) fails with exit code 1 — a
  `UserError` naming all three candidates, not a silent guess (INV-8).
- `deps Handler --dir out --component orders` resolves the target
  unambiguously to `services/orders/handler.go`'s own `Handler`
  (`--component` as a tiebreaker, exactly like `--subpath` — ADR-0014's
  shape, extended by ADR-0035).

## Negative space

No PHP component (no fixture currently exercises PHP's
FQN-collision-across-components case at the CLI level — covered
instead by `resolve.rs`'s own
`colliding_fqn_across_components_prefers_the_callers_own_component`
unit test). No component-name collision requiring the symmetric
extension-by-parent-segment scheme (`components::tests::
colliding_basenames_are_disambiguated_by_parent_segment_symmetrically`
covers that in isolation) — every directory basename here is already
distinct. No terraform-under-a-strong-component (T2) or
single-`.tf`-file-absorbing-a-nested-module-directory (T1) case at the
CLI level either — those are exercised in isolation by
`components::tests::terraform_directory_under_a_strong_component_
belongs_to_that_component` and `components::tests::
terraform_directory_with_its_own_tf_absorbs_a_nested_module_directory`.

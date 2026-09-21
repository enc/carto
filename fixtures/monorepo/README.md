# monorepo fixture

Synthetic monorepo-shaped tree. Not real customer code. Deliberately
small — just enough to independently exercise every component-discovery
and component-scoped-resolution outcome ADR-0034/ADR-0035 can reach:
manifest-marker auto-detection across four languages, aggregator
suppression, a declared root overriding auto-detection, the exact
same-name-across-components collision that used to make every one of
those symbols unresolvable, and the file-scope (ADR-0031) resolution
path under a component.

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
| `services/orders/go.mod` + `handler.go` + `main.go` | `go.mod` | `orders` / go | Defines `Handler()` (one of three same-named symbols, see below) and `Run()`. `Handler` calls bare `Shared()` with **no Go import at all** — tier (c2), evidence `cross-component`. `Run` calls bare `Handler()` from the *same* component — tier (c1), evidence `same-component`, resolving despite the repo-wide name collision that would otherwise make it ambiguous. |
| `services/billing/Billing.csproj` + `Handler.cs` | `*.csproj` suffix | `billing` / dotnet | A second, unrelated `Handler` (a C# class) — the collision partner. Never called by `orders`, so must never appear as a candidate for either of `orders`'s bare `Handler()` calls. |
| `web/admin/package.json` + `src/handler.ts` | `package.json` | `admin` / node | A third, unrelated `Handler` (a TS class). `registerHandler(new Handler())` is module-level code with no enclosing symbol — ADR-0031's file-scope fallback, exercised *under* a component: the resolved edge must attach to the `File` node and still carry `admin`'s own component label. |
| `lambdas/ingest/handler.py` | *(none)* | `ingest` / python | No manifest marker anywhere in this directory — only recognized as a component because `.carto/roots.json` declares it explicitly (the declared-root override path, not auto-detection). |
| `.carto/roots.json` | — | — | `{"detect": true, "roots": [{"name": "ingest", "path": "lambdas/ingest", "kind": "python"}]}` — `detect: true` makes the declaration *additive* to auto-detection rather than replacing it, so `shared`/`orders`/`billing`/`admin` are still auto-detected alongside the declared `ingest`. |

## Verify with

```bash
cargo run -p carto-cli -- index fixtures/monorepo --out /tmp/carto-monorepo
cargo run -p carto-cli -- map fixtures/monorepo --out /tmp/carto-monorepo --section components
cargo run -p carto-cli -- where Handler fixtures/monorepo --out /tmp/carto-monorepo --exact
cargo run -p carto-cli -- deps Handler fixtures/monorepo --out /tmp/carto-monorepo --dir out
cargo run -p carto-cli -- deps Handler fixtures/monorepo --out /tmp/carto-monorepo --dir out --component orders
```

Expected:

- `map --section components` lists exactly five components (`admin`,
  `billing`, `ingest`, `orders`, `shared`), each with its own
  file/symbol counts, plus a `## cross-component edges` line reading
  `orders -> shared: 1 calls`.
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

No HCL/Terraform component in this fixture (that's
`fixtures/sid-like`'s job) and no PHP component (no fixture currently
exercises PHP's FQN-collision-across-components case at the CLI level
— covered instead by `resolve.rs`'s own
`colliding_fqn_across_components_prefers_the_callers_own_component`
unit test). No component-name collision requiring the symmetric
extension-by-parent-segment scheme (`components::tests::
colliding_basenames_are_disambiguated_by_parent_segment_symmetrically`
covers that in isolation) — every directory basename here is already
distinct.

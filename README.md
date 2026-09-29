# carto

Infra-aware code map for coding agents.

`carto` builds a deterministic structural graph of a repository (symbols, files,
modules, imports, calls-best-effort) via tree-sitter, a resolved infrastructure
graph from IaC artifacts you already have (Terraform plan/state JSON,
CloudFormation/SAM, CDK cloud assembly), and joins the two — edges from source
symbols to the deployed resources they become, and onward to the IAM
permissions those resources hold. It serves all of this to agents over MCP
(stdio) and to humans/CI via a CLI.

Full design: [`docs/carto-design-spec.md`](docs/carto-design-spec.md).

## Invariants

These are product identity, not preferences — see spec §2 for the full table
and enforcement mechanism of each:

- **No network.** No crate in the dependency graph may provide sockets/HTTP/DNS.
  Enforced by `deny.toml` + a strace-based integration test.
- **No foreign code execution.** carto never executes repository content or
  build systems.
- **Repo is read-only; output goes to one directory.** Default:
  `${XDG_CACHE_HOME:-~/.cache}/carto/<repo-hash>/`.
- **No agent-config or VCS mutation.** Never writes `~/.claude`, `.claude/`,
  `CLAUDE.md`, `.git/hooks`, git config, or shell rc files.
- **All repo-derived text is untrusted** and is only ever rendered through a
  fencing/capping boundary (`TaintedString`).
- **Secrets never reach disk artifacts.** Redaction pass at serialisation time.
- **Deterministic output.** Same input tree ⇒ byte-identical `graph.json`.
- **Honest edges.** Every edge carries a confidence; nothing heuristic is
  presented as resolved.

carto never calls an LLM and never needs an API key.

## Status

Implementation in progress, milestone by milestone (spec §10). Currently:
**M1.b.2b done** — `carto index`/`where`/`deps`/`map` work
end-to-end against Rust, Python, PHP, Go, C#, and
TypeScript/TSX/JavaScript repos: spec §5.2's full v1 language set,
plus C# added post-v1 (ADR-0016). **M2's first slice, real redaction,
is done** (ADR-0017). **An MCP server slice** (normally M4, pulled ahead
to measure spec §11.4's S-1 benchmark before more capability gets built —
see [`bench/`](bench/)) also exists: `carto serve` exposes
`index`/`where`/`deps`/`map`/`selfcheck` over MCP stdio
([ADR-0018](docs/adr/0018-mcp-transport-hand-rolled-jsonrpc.md)).
Also pulled ahead on request: cross-language string-literal contracts
(`contract`/`orphans`, ADRs 0025–0027),
[monorepo / multi-component indexing](#monorepos-multiple-projects-in-one-tree)
(ADRs 0034–0039) and
[Terraform / Terragrunt source support](#terraform-and-terragrunt)
(ADRs 0041–0044). See [`docs/STATUS.md`](docs/STATUS.md) for the
detailed handoff.

## Building

```
cargo build --workspace
cargo test --workspace
bash scripts/gates.sh   # fmt, clippy, cargo-deny, tests
```

Build a `carto` binary you can actually run against a project:

```
cargo build --release -p carto-cli
# binary at target/release/carto — put it on PATH, or reference the
# full path in the MCP config below.
```

## Monorepos: multiple projects in one tree

carto indexes **one directory tree** at a time. If that tree holds
several projects — services, frontends, lambdas, shared libraries,
Terraform — carto recognizes each of them as a **component** inside
the tree. You still run a single `carto index <repo>`; there is no
separate "multi-repo" mode and no way to index several unrelated
checkouts into one graph.

### What components change

| Without components (single scope) | With components |
|---|---|
| Two services that both define `Handler` make every bare `Handler()` call ambiguous → no edge | A call prefers a match in its **own** component first, then falls back to repo-wide |
| A PHP `use` / C# `using` can resolve to a same-named class in an unrelated service, at `certain` confidence | Same-component (then declared-dependency) targets are preferred; Rust `mod` never resolves across crates |
| Cross-project edges look like any other edge | Edges crossing a component boundary carry `cross-component` evidence, plus `undeclared-dependency` if the caller's manifest doesn't declare the target |
| Queries always cover the whole repo | `--component <name>` narrows `where`/`deps`/`map`/`contract`/`orphans` |

A repo with no nested projects is unaffected: its graph just gets an
empty `components` list and `component: null` on every file.

### How components are found

Automatically, from manifest files in **subdirectories** (the repo
root itself is never a component — a manifest there describes the
whole repo):

| Marker in a directory | Component kind | `depends_on` resolved from |
|---|---|---|
| `go.mod` | `go` | `require` (matched to other components' `module`), local `replace` |
| `package.json` | `node` | `dependencies`/`devDependencies`: `file:`/`workspace:` paths or package-name match |
| `Cargo.toml` | `rust` | `path = "…"` entries in `[dependencies]`/`[dev-dependencies]`/`[build-dependencies]` |
| `pyproject.toml`, `setup.py` | `python` | — (not resolved) |
| `composer.json` | `php` | `"type": "path"` repositories, `require` name match |
| `*.csproj`, `*.fsproj` | `dotnet` | `<ProjectReference>` |
| ≥1 `.tf` file (no other marker) | `terraform` | — (not resolved) |

Rules worth knowing:

- **Name** = the directory's basename (`services/orders` → `orders`).
  If two collide, both get extended by their parent directory
  (`services-orders`, `lambdas-orders`).
- **Workspace roots are skipped**: a `Cargo.toml` with `[workspace]`
  but no `[package]`, or a `package.json` with a `"workspaces"` key or
  a sibling `pnpm-workspace.yaml`/`lerna.json`/`turbo.json`/`nx.json`/
  `rush.json`, is an aggregator, not a component.
- **Terraform rolls up**: `infra/envs/prod`, `infra/envs/dev`,
  `infra/modules/vpc` become one `infra` component, not three. A `.tf`
  directory inside another component (`services/orders/infra`) belongs
  to that component.
- **Nesting**: a file belongs to its **innermost** component.
  `--component api` also includes components nested under `api`'s
  directory (not the other way round).
- **`.gitignore`/`.cartoignore` apply**: a manifest carto doesn't walk
  can't mark a component.

### Overriding detection: `.carto/roots.json`

Needed when auto-detection is wrong or insufficient — most commonly a
repo with a **single top-level manifest** (one `go.mod` at the root
with several `cmd/*` binaries), which yields zero components.
`carto index` prints `components: 0` and a hint in that case.

```json
{
  "detect": true,
  "exclude": ["services/orders/infra"],
  "roots": [
    { "name": "ingest", "path": "lambdas/ingest", "kind": "python" },
    { "name": "api",    "path": "cmd/api" }
  ]
}
```

| Field | Meaning |
|---|---|
| `roots` | Declared components. `name` must match `^[A-Za-z0-9][A-Za-z0-9_.-]*$`; `path` is repo-relative and must contain at least one walked file; `kind` is free text, default `custom`. A declared root replaces an auto-detected one at the same path. |
| `detect` | `true`: declared roots are **added** to auto-detection. `false`: **only** declared roots. Default: `true` if `roots` is empty, otherwise `false`. |
| `exclude` | Repo-relative paths whose auto-detected components (and those nested below) are dropped. Applied before `roots`. |

Invalid entries (bad name, `..` in a path, duplicate name/path, a name
colliding with an auto-detected component) fail `index` with an error
naming the file — they are never silently ignored.

### Using it

```bash
carto index .                                   # prints "components: N"
carto map . --section components                # list components, depends_on, cross-component edges
carto where Handler . --exact                   # each hit labelled [component]
carto deps Handler . --dir out --component orders   # disambiguate by component
carto map . --component orders --component billing  # repeatable
carto orphans . --category metric_name --component infra
```

Real output against [`fixtures/monorepo`](fixtures/monorepo/README.md):

```
## components
  admin  path=web/admin kind=node files=2 symbols=3 depends_on=(none)
  billing  path=services/billing kind=dotnet files=2 symbols=2 depends_on=(none)
  infra  path=infra kind=terraform files=3 symbols=0 depends_on=(none)
  ingest  path=lambdas/ingest kind=python files=1 symbols=2 depends_on=(none)
  orders  path=services/orders kind=go files=3 symbols=2 depends_on=shared
  shared  path=libs/shared kind=go files=2 symbols=1 depends_on=(none)
## cross-component edges
  orders -> shared: 1 calls(inferred)
```

A crossing to a component the caller's manifest does not declare is
marked `[undeclared]` in that list. Over MCP, the same filter is the
`component` argument on the `where`/`deps`/`map`/`contract`/`orphans`
tools, as a comma-separated string (`"orders,billing"`).

`--component` restricts **what is listed**, not what is computed:
edges are resolved once at index time over the whole tree, so the
filter never adds or removes an edge. The one extra effect is in
`deps`: when a name matches several symbols, `--component` acts as a
tiebreaker for picking the target, exactly like `--subpath`
(ADR-0014/0035).

### Not supported (yet)

- **Several separate repositories/checkouts in one graph** — index
  each on its own, or place them under one parent directory and index
  that.
- **One component spanning disjoint directories** (`services/orders`
  + `libs/orders-proto` as a single component) — a component is
  exactly one directory.
- **`go.work` / nested-workspace hierarchies** beyond innermost-
  directory nesting; no per-component `.cartoignore`.
- **`depends_on` for `python`, `terraform` and declared (`custom`)
  components** — always empty, meaning "not analysed", not "has no
  dependencies"; these never get `undeclared-dependency` evidence.
- **Links between components through deployed infrastructure** (a
  service → the queue or Lambda it talks to) — that needs the infra
  graph and code↔infra join (milestones M2/M3), which aren't built yet.
  Today components are connected only by code-level calls/imports/type
  references and string-literal contracts.

Design record: ADRs
[0034](docs/adr/0034-component-discovery-and-scoping.md),
[0035](docs/adr/0035-component-scoped-resolution-and-query-layer.md),
[0036](docs/adr/0036-cross-component-import-evidence-and-discoverability.md),
[0037](docs/adr/0037-terraform-rollup-aggregator-parsing-exclude.md),
[0038](docs/adr/0038-descendant-component-scoping.md),
[0039](docs/adr/0039-component-dependency-graph.md).

## Terraform and Terragrunt

carto reads Terraform **source** — `*.tf` files, per-environment variants
such as `locals.tf.simu`, and Terragrunt's `terragrunt.hcl` / `root.hcl` —
and answers "who uses this?" and "which module calls which?". It is the
code as written, **not a plan**: nothing is executed (no `terraform`, no
`terragrunt`), `count`/`for_each` are not expanded, and there are no
resolved resources or IAM yet (that needs `terraform show -json`
ingestion, a later step; `map` says so under `## infra`).

### What is understood

| Construct | Becomes | Edge |
|---|---|---|
| `variable`, `output`, `resource`, `data`, `module`, each `locals` entry | a symbol named by its Terraform address: `var.region`, `output.vpc_id`, `aws_s3_bucket.logs`, `data.aws_iam_policy_document.x`, `module.vpc`, `local.prefix` | — |
| `var.x`, `local.x`, `module.m`, `data.t.n`, `<type>.<name>` in an expression | a reference **inside the same module directory only** | `references`, `inferred` |
| `module "m" { source = "../modules/vpc" }` | every file of the called directory | `imports`, `certain` |
| `module.m.out`; `foo = …` inside a `module` block | the called module's `output "out"` / `variable "foo"` | `references`, `inferred` |
| registry / `git::` / `https://` module source | an external module — credentials and `?ref=`/query removed | `imports`, `certain` |
| Terragrunt `terraform { source }`, `dependency`, `dependencies`, `include` | the module's files, the dependency unit's `terragrunt.hcl`, the included file | `imports`, `certain` (`find_in_parent_folders` → nearest walked file, `inferred`) |
| Terragrunt `inputs = { k = … }`; `dependency.x.outputs.y` | the unit's source module's `variable "k"`; the dependency unit's module `output "y"` | `references`, `inferred` |

Per-environment files (`locals.tf.simu`, `locals.tf.prod`) that define the
same name give one edge **per variant** instead of a guess; `override.tf`
yields to the base declaration. Terragrunt files are their own scope:
their `locals` never mix with a `.tf` file in the same folder. Paths built
from Terragrunt functions carto cannot evaluate (`get_repo_root()`,
`path_relative_to_include()`) produce no edge, never a wrong one.

### How to use it

```bash
carto index infra/                                   # then, against that index:
carto where var.region --exact                       # every declaration named var.region
carto deps var.region --dir in --subpath envs/prod   # who uses it (subpath picks one module)
carto deps module.vpc --dir both                     # what a module call reads, who reads it
carto deps aws_s3_bucket.logs --dir in
carto map --section infra                            # module-call graph, remote modules, Terragrunt unit chain
```

`map --section infra` on [`fixtures/tf-modules`](fixtures/tf-modules/README.md):

```
## infra
  source-level Terraform (parsed .tf files, not a resolved plan): 23 symbols — 4 tf_local, 5 tf_module, 4 tf_output, 4 tf_resource, 6 tf_variable
  module calls (calling dir -> called dir, file-level imports):
    infra/envs/prod -> infra/modules/vpc  (3)
    infra/envs/prod -> infra/modules/app  (2)
  remote modules (registry/git/http sources, credentials stripped):
    git::https://example.com/org/net.git//modules/net  in=1
    terraform-aws-modules/vpc/aws  in=1
  resolved infrastructure graph (IacResource, depends_on, IAM): none — requires M2 (plan/state JSON ingestion)
```

### Reading the results

- **A variable with no inbound edges is not "unset" or "unused".** Values
  also come from `*.tfvars`, `TF_VAR_*`, `-var`/`-var-file`, a caller's
  module argument or a Terragrunt input. `*.tfvars` / `*.tfvars.json` are
  **never read** (they often hold secrets) — carto only lists their paths
  next to the variable, and `deps` prints this caveat on every variable.
- **`unresolved_calls` means "not found among the parsed files"**
  (`local.missing`, `arg:foo`, `source:<dir>`, `config_path:<dir>`): a
  gitignored generated file or a `*.tf.json` is invisible to carto, so it
  is a lead, not a verdict.
- `.terraform/` and `.terragrunt-cache/` (downloaded module copies) are
  skipped.

### Not supported (yet)

Plan/state JSON, resolved resources, IAM, and the link from code to the
resources it deploys; `count`/`for_each` instances; provider aliases;
`moved`/`import` blocks; Terragrunt `generate`, `read_terragrunt_config`,
`include` expose/merge, `mock_outputs` and stacks; other `*.hcl` files
(`common.hcl`, Packer, Nomad); reading `*.tfvars`.

Design record: ADRs
[0041](docs/adr/0041-terraform-source-symbols-and-references.md),
[0042](docs/adr/0042-terraform-module-calls.md),
[0043](docs/adr/0043-terragrunt-relations.md),
[0044](docs/adr/0044-terraform-variables-may-be-set-externally.md).

## Using with Claude Code

carto runs as a real MCP server (spec §7.3), not a CLI wrapped in a
prompt note. Point Claude Code at the built binary:

```bash
claude mcp add carto -- /path/to/carto serve
```

or add it directly to an MCP config file:

```json
{
  "mcpServers": {
    "carto": { "command": "/path/to/carto", "args": ["serve"] }
  }
}
```

This exposes `index`/`where`/`deps`/`map`/`contract`/`orphans`/
`selfcheck` as tools — the same core functions the CLI's own subcommands
call (`crates/carto-core/src/query/`, `crates/carto-core/src/indexer.rs`),
so answers are identical either way. Copy the `skill/carto/` directory
into your agent's skills directory (spec §9.3's "the one integration
file," packaged as a directory per [ADR-0028](docs/adr/0028-skill-file-packaging.md)
since Claude Code discovers skills at `<skills-dir>/<name>/SKILL.md`, not
a bare file):

```bash
cp -r skill/carto ~/.claude/skills/
```

for a description of what carto answers well and its honesty contract
(confidence per edge, symbol IDs that change when code moves); the file
has no imperative pressure language by design — it describes
capabilities and lets the agent decide when to use them.

The MCP transport is a hand-rolled newline-delimited JSON-RPC 2.0 stdio
loop, not `rmcp` — [ADR-0018](docs/adr/0018-mcp-transport-hand-rolled-jsonrpc.md)
records why (in short: `rmcp`'s dependency tree pulls an async runtime
into an otherwise fully synchronous, dependency-minimal codebase for a
transport simple enough to hand-roll with zero new dependencies).

## License

Apache-2.0 — see [`LICENSE`](LICENSE).

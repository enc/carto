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
(`contract`/`orphans`, ADRs 0025–0027) and
[monorepo / multi-component indexing](#monorepos-multiple-projects-in-one-tree)
(ADRs 0034–0039). See [`docs/STATUS.md`](docs/STATUS.md) for the
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

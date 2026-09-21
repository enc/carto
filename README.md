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
([ADR-0018](docs/adr/0018-mcp-transport-hand-rolled-jsonrpc.md)). See
[`docs/STATUS.md`](docs/STATUS.md) for the detailed handoff.

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

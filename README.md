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
**M1.b.2b in progress** — `carto index`/`where`/`deps`/`map` work
end-to-end against Rust and Python repos; TS/TSX/JS/Go extractors not
started yet. See [`docs/STATUS.md`](docs/STATUS.md) for the detailed
handoff.

## Building

```
cargo build --workspace
cargo test --workspace
bash scripts/gates.sh   # fmt, clippy, cargo-deny, tests
```

## License

Apache-2.0 — see [`LICENSE`](LICENSE).

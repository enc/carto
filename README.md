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
end-to-end against Rust, Python, PHP, and TypeScript/TSX/JavaScript
repos; Go extractor not started yet. See
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
# full path in the CLAUDE.md snippet below.
```

## Using with Claude Code

There's no MCP server yet (that's M4) — for now, point Claude Code at
the CLI by adding a short note to the *target* repo's own `CLAUDE.md`
(the repo you want indexed, not this one):

```markdown
## Code navigation

This repo is indexed with `carto` (github.com/enc/carto). Prefer it
over grep/rg for symbol lookups and call-graph questions:

    carto index . --out /tmp/carto-out           # once per session, or after a large change
    carto where <symbol> . --out /tmp/carto-out   # find a symbol by name (substring or --exact)
    carto deps <symbol> . --out /tmp/carto-out --dir in --depth 2   # what calls this
    carto map . --out /tmp/carto-out --budget 50  # layered overview: top modules, entry points

Every edge carries a confidence (`certain`/`inferred`); `inferred` means
verify before acting on it. Symbol IDs change when code moves — re-run
`where` after edits rather than reusing an old one.
```

Claude Code picks this up automatically and runs the commands itself via
its Bash tool when the note tells it to. This is a stopgap: spec §9.3's
real integration point is a single `skill/carto.skill.md` file the user
copies in themselves (M4), and §7.3's MCP server exposes the same core
functions as the CLI — the manual `CLAUDE.md` pointer above works today
because `where`/`deps`/`map` already *are* those same functions.

## License

Apache-2.0 — see [`LICENSE`](LICENSE).

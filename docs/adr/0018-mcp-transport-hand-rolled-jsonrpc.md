# 0018 — MCP transport: hand-rolled JSON-RPC, not `rmcp` (spec §7.3)

**Status:** accepted · **Date:** 2026-08-03 · **Milestone:** M4 slice (MCP
server), pulled ahead of M2/M3 on user request — to build and benchmark
the actual agent-facing surface (spec §11.4's S-1 gate) before more
capability gets built on an unvalidated value hypothesis. Doesn't change
M2/M3's own scope or ordering; `carto-mcp` only exposes what M1 already
implements (`index`/`where`/`deps`/`map`/`selfcheck`), nothing from M2+.

## Context

Spec §7.3 says: `rmcp` (official Rust SDK — verify current crate
name/version at implementation time), stdio transport only. `deny.toml`
already carries a forward-looking comment pre-authorizing `tokio`'s
non-networking features "for rmcp in M4."

Before writing any code, checked `rmcp` 3.1.0's actual dependency tree
against crates.io's sparse index (`docs.rs` isn't network-reachable in
this environment; crates.io is — CLAUDE.md's own documented gotcha,
followed here). Required (non-optional) normal dependencies: `async-trait`,
`chrono` (two feature sets), `futures`, `pin-project-lite`, `serde`,
`serde_json`, `thiserror`, **`tokio`** (`sync`, `macros`, `rt`, `time`),
`tokio-util`, `tracing`. Optional, feature-gated (so avoidable with
`default-features = false` + `transport-io` only): `hyper`, `reqwest`,
`sse-stream`, `jsonwebtoken`, `oauth2`, `schemars`, and others aimed at
HTTP/SSE transports and OAuth this project doesn't need.

Weighed against that tree:

- `carto-core` and `carto-cli` are entirely synchronous today — no
  crate in the workspace has ever pulled an async runtime. Adding one
  for a single stdio loop is a large capability surface for a small job.
- `deny.toml`'s `[bans] multiple-versions = "deny"` makes a duplicate
  transitive version of something already in the tree (`serde_json`, or
  any of `tokio`'s own dependents) a real, not hypothetical, gate
  failure — `rmcp`'s tree is wide enough that this was a likely
  candidate, not a remote one, and each collision would need a narrowly-
  scoped `[[bans.skip]]` per CLAUDE.md's "Adding a dependency" rule,
  reviewed one at a time.
- MCP's stdio transport, per the protocol spec, is genuinely simple:
  newline-delimited JSON-RPC 2.0 over stdin/stdout. No `Content-Length`
  framing (that's LSP, a different protocol this project doesn't touch),
  no multiplexing, no backpressure beyond what a blocking `BufRead` loop
  already gives for free. There is very little "protocol" to get wrong.
- INV-1 ("no network") is currently *true by construction* — no crate in
  the graph provides sockets/HTTP/DNS, checked mechanically by
  `deny.toml`'s ban list. `rmcp` with `default-features = false` and only
  `transport-io` would keep this *true after auditing feature flags*,
  which is a strictly weaker and more fragile property: a future
  dependency bump or an accidental default-features flip becomes the
  kind of mistake `cargo deny check bans` exists to catch mechanically,
  not defend against by convention.

## Decision

Hand-roll the transport in a new crate, `crates/carto-mcp`, depending on
`carto-core` + `serde`/`serde_json` only — no new dependency, so
`cargo deny check bans licenses sources` needs **zero** new
`[[bans.skip]]` entries (verified after implementation, not just
predicted: see the crate's own commit). `deny.toml`'s existing comment
pre-authorizing `tokio` "for rmcp in M4" is superseded by this decision,
not silently ignored — a future ADR would be needed to actually adopt
`rmcp`, should a real reason (HTTP transport, OAuth, a second protocol
version) emerge.

Implementation shape (`crates/carto-mcp/src/`):

- `jsonrpc.rs` — request/notification parsing (JSON-RPC's
  presence-of-`id`-key rule, not `Content-Length` framing), response
  construction, the standard error codes (`-32700`..`-32602`).
- `server.rs` — a synchronous `serve(reader: impl BufRead, writer: impl
  Write)` loop. `initialize`, `ping`, `tools/list`, `tools/call`, and
  any `notifications/*` handled or safely ignored. A malformed line
  produces an error response (or is silently dropped, if it was a
  notification) and the loop continues — never panics, never kills the
  session over one bad frame.
- `schema.rs` — hand-written JSON Schemas, one per tool, mirroring each
  CLI command's `clap::Args`.
- `tools/` — one handler per tool (`index`/`where`/`deps`/`map`/
  `selfcheck`), each calling the *exact* `carto_core::query::{find,deps,
  map}` / `carto_core::indexer::build_and_persist` function the CLI
  subcommand of the same name calls — spec §7.1's "commands = MCP tools,
  same core functions" holds structurally, not just in spirit.
- `render.rs` — spec §7.2's output discipline: `structuredContent`
  carries the full, uncapped result; the text `content` block is capped
  at `consts::MCP_TEXT_CAP` (8 KiB) on a line boundary — the first real
  enforcement of that constant anywhere in the codebase (`docs/STATUS.md`
  previously recorded it as enforced nowhere).

`carto-cli` gains one new subcommand, `serve`, mounting
`carto_mcp::serve(stdin().lock(), stdout().lock())` — thin dispatch, same
pattern every other subcommand already follows in `main.rs`.

## What this gives up

Named plainly, since choosing not to use the spec's named dependency is a
real trade-off, not a free win:

- **Protocol conformance is now this project's responsibility.** `rmcp`
  is maintained against the evolving MCP spec by people whose job is
  exactly that; a hand-rolled server can drift from a future protocol
  revision in ways only a real client would surface. Mitigated for now
  by testing against an actual `claude --mcp-config` session (this ADR's
  companion commit), not just this crate's own unit tests — but that's
  a one-time manual check, not an ongoing conformance suite.
- **Schemas are hand-written, not generated.** `rmcp` (via `schemars`)
  would generate each tool's `inputSchema` from the same serde types the
  handler already uses, guaranteeing they can't drift. Here, `schema.rs`
  and each `tools/*.rs` handler are two independent sources of truth for
  "what parameters this tool takes" — mitigated by a test
  (`tools/mod.rs::schema_properties_match_each_handlers_declared_params`)
  that pins every schema property name against a handler-declared list,
  but that test only catches drift a human remembered to keep both sides
  of in sync with; it can't catch a parameter neither side lists.
- **No HTTP/SSE transport, ever, without more work.** Not a loss against
  spec §7.3 (stdio only, "No HTTP transport in v1"), but worth stating:
  if a future milestone needs one, this hand-rolled transport doesn't
  extend to it the way `rmcp`'s would have.

## Consequences

- `deny.toml`'s tokio-features-ban comment ("for rmcp in M4") is now
  stale documentation, not an active plan — left in place rather than
  deleted, since removing it would erase the record of what was
  originally intended and why this ADR changed course.
- If `rmcp` is adopted later (a real HTTP/SSE need, or a protocol
  revision this hand-rolled server can't track), it replaces
  `carto-mcp`'s `jsonrpc.rs`/`server.rs`/`schema.rs` wholesale; the
  `tools/` handlers (thin wrappers over `carto_core`) barely change,
  since they never depended on the transport in the first place.
- `carto-mcp` becomes the second crate (`carto-grammars` unsafe carve-out
  is the first, ADR-0007) whose scope was fixed by a specific, checked
  trade-off rather than the spec's literal wording — recorded here per
  CLAUDE.md's "ADRs for every deviation and judgment call."

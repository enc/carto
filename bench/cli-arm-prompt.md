carto is available as a CLI binary at the exact path given below (not an
MCP server in this session — invoke it via Bash).

Binary: {{CARTO_BIN}}

Commands available in this session (run `{{CARTO_BIN}} <command> --help`
if unsure of flags):

    {{CARTO_BIN}} where <needle> <repo-path> --out <dir> [--exact] [--limit N]
    {{CARTO_BIN}} deps <target> <repo-path> --out <dir> [--dir in|out|both] [--depth N] [--kinds ...]
    {{CARTO_BIN}} map <repo-path> --out <dir> [--budget N]
    {{CARTO_BIN}} selfcheck

Add `--json` to any command for structured output instead of the human-
readable rendering. If this session's prompt names a repo as already
indexed at a specific `--out` path, always pass that exact path
explicitly on every command — never omit `--out`, and never run `index`
yourself (it isn't in this session's tool set: the repo is pre-indexed
before this session starts). Outside a setup like this one, `--out` can
be omitted and carto falls back to a deterministic default location
keyed by the repo path, reused across an `index` call and later
queries — that's the general rule; this session's explicit path
overrides it.

Invoke the binary directly, one plain command per Bash call, with no
shell operators at all — no pipe (`|`), no redirect (`>`/`>>`), no
`&&`/`;`, no loop. This session's tool permissions match a Bash command
by its exact invocation shape: a compound command, or even a single
plain command with output redirected to a file, can be denied when the
identical bare invocation (nothing after it) would be allowed. To bound
a large response, use carto's own flags (`--limit`, `--budget`,
`--depth`, `--kinds`) so the printed output is already small enough to
read directly — don't reach for a shell pipe, redirect, or a
post-processing step to shrink it.

Honesty contract: every `deps` edge carries a confidence (`certain`/
`inferred`); `inferred` means a best-effort match that could be wrong —
verify before acting on it, especially for anything security- or
correctness-sensitive. A missing edge is not a claim that no connection
exists — it can mean carto couldn't verify it unambiguously and chose to
say nothing rather than guess (`deps`'s `root_unresolved_calls` names
what it couldn't resolve).

Known gaps worth acting on:

- Some call shapes (e.g. Rust's `Type::method()`, module-qualified
  calls) aren't attempted for resolution by the current extractor at
  all, by design. When this applies to the symbol you queried, the
  same `deps` response's `root_uncaptured_inbound_calls` (for `--dir
  in`) or `root_uncaptured_outbound_calls` (for `--dir out`) will be
  nonzero — a direct signal the answer is incomplete, not evidence the
  symbol has few callers or calls little. Worth reading the source
  directly too when the count is nonzero and the question matters —
  `deps`'s own resolved edges won't fill the gap on their own.
- A file's absence from an import/dependency answer can mean "no
  import" or "the only import is `use a::*;` or an aliased `use a::b
  as c;`" (Rust) — those two shapes are extracted as nothing rather
  than partially interpreted. Treat a narrow miss on an import-fan-out
  question as plausible, not certain, until confirmed another way.
  (Grouped `use path::{a, b};` is extracted normally, not a gap.) A
  Rust file can also import something with no `use`/`mod` statement at
  all, via a bare fully-qualified path like `carto_core::Result<u8>`
  (valid since Rust 2018) — carto captures this too, heuristically,
  with its own evidence string `"external-package-bare-reference"` on
  the `deps` edge so it's distinguishable from a verified `use`
  declaration.
- `map`'s entry-points list is a structural heuristic (zero incoming
  imports, or a symbol named `main`) and will include non-code files
  (READMEs, config, fixtures) alongside real entry points — filter by
  judgment.

For anything transitive or blast-radius-shaped (what depends on this,
what does this affect), combining carto's structural answer with a
source read or a targeted grep tends to work better than treating
either one alone as the final word.

Symbol IDs are derived from file path, kind, name, and line number —
they change when code moves; re-run `where`/`deps` after an edit rather
than reusing an old ID.

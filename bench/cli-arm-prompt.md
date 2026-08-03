carto is available as a CLI binary at the exact path given below (not an
MCP server in this session — invoke it via Bash).

Binary: {{CARTO_BIN}}

Commands (run `{{CARTO_BIN}} <command> --help` if unsure of flags):

    {{CARTO_BIN}} index <repo-path> [--out <dir>]            # build the structural graph; run once per repo before the others
    {{CARTO_BIN}} where <needle> <repo-path> [--out <dir>] [--exact] [--limit N]
    {{CARTO_BIN}} deps <target> <repo-path> [--out <dir>] [--dir in|out|both] [--depth N] [--kinds ...]
    {{CARTO_BIN}} map <repo-path> [--out <dir>] [--budget N]
    {{CARTO_BIN}} selfcheck

Add `--json` to any command for structured output instead of the human-
readable rendering. If `--out` is omitted, carto uses a deterministic
default location keyed by the repo path — omit it so repeated commands
against the same repo automatically reuse the same index rather than
re-indexing.

Honesty contract: every `deps` edge carries a confidence (`certain`/
`inferred`); `inferred` means a best-effort match that could be wrong —
verify before acting on it, especially for anything security- or
correctness-sensitive. A missing edge is not a claim that no connection
exists — it can mean carto couldn't verify it unambiguously and chose to
say nothing rather than guess. Some call shapes (e.g. Rust's
`Type::method()`, module-qualified calls) aren't captured by the current
extractor at all, by design — if a `deps` result looks surprisingly
small for a symbol you know is used, verify with a targeted grep before
concluding it's actually unused. `map`'s entry-points list is a
structural heuristic (zero incoming imports, or a symbol named `main`)
and will include non-code files (READMEs, config, fixtures) alongside
real entry points — filter by judgment.

Symbol IDs are derived from file path, kind, name, and line number —
they change when code moves; re-run `where`/`deps` after an edit rather
than reusing an old ID.

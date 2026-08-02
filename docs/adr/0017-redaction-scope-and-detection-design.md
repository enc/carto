# 0017 — Redaction scope and detection design (spec §7.5, INV-6)

**Status:** accepted · **Date:** 2026-08-02 · **Milestone:** M2 (first slice)

## Context

`redact::redact()` shipped in M1.b.1 as a no-op stub — real signature
(`fn redact(graph: &mut Graph) -> RedactionCounts`), wired into the one
serialisation choke-point (`graph::persist::persist`), scanning
nothing. That was honestly documented as deliberate at the time ("M1
produces no tainted string fields worth scanning yet"), but the
codebase moved on without the stub: `SymbolNode.signature:
Option<TaintedString>` exists and is populated by every one of the (by
now) 8 language extractors. Until this slice, INV-6 ("secrets never
reach disk artifacts") was documented but not actually enforced — a
hardcoded secret in a `const`/`var` initializer reaches `graph.json`
untouched, confirmed by reading `go.rs`/`rust.rs`: a `const_spec`/
`const_item` has no `body` field, so each extractor's own `sig_end`
fallback (`item_node.end_byte()`) captures the *whole* declaration,
initializer included. This is the gap that made carto unsafe to point
at anything with real secrets, and this slice closes it.

## Decisions

### Scope: every `TaintedString` field, not literally every `String` field

Spec §7.5 says "over every String field." The graph today
(`graph/node.rs`) has exactly one **`TaintedString`** field —
`SymbolNode.signature` — and several plain `String` fields that are all
extractor-computed identifiers/paths, never free-form captured source
text: `FileNode.path`, `ModuleNode.path`, `SymbolNode.name`,
`UnresolvedCall.name`. Scanning `TaintedString` fields only is the
faithful reading, not a narrowing done by omission: `TaintedString` is
the type this codebase already invented specifically to mark "content
whose safety is unknown" (INV-5's own module doc). An identifier can't
literally *be* a connection string or a PEM block; pattern/entropy-
scanning paths and bare names would be pure noise with no realistic
true-positive case. This also makes the scan forward-compatible for
free: M4's `Note.text` (agent-ingest, also `TaintedString`) is covered
automatically, since `redact::redact` walks `NodeData` generically, not
field-by-field.

### Reaching into `TaintedString` without weakening INV-5

No changes to `taint.rs`. `redact::mod` reads a field's full sanitized
content via the existing, already-public `render_capped(usize::MAX)`
(deliberately the *full* content, not `SIGNATURE_CAP`-truncated — a
secret starting inside the 300-char cap but extending past it must
still be caught) and `provenance()`, computes the redacted text, and
rebuilds a **new** `TaintedString::new(&redacted, original.provenance())`
to write back. No new accessor, no `pub(crate)` field poke — every
compile-fail guarantee in `tests/ui/` is untouched by this slice.

`Graph` gained one method, `nodes_mut()` — the only in-place mutation
path in the crate. Needed because `redact` runs *before*
`into_sorted_parts()` consumes the graph into `persist`'s output Vecs
(`persist.rs`'s existing call order, unchanged by this slice).

### Detection: hand-rolled, no new dependency

All of spec §7.5(a)'s patterns are fixed-prefix or fixed-structure
matches — cheap to scan by hand over the short (line-sized) strings
this module ever sees. Consistent with this codebase's demonstrated
preference for hand-rolled scanning over a new dependency when the win
is marginal (ADR-0005 "no petgraph", ADR-0015 "no go.mod parsing"). No
`regex` dependency added; `sha2` (already a dependency, used in
`walk/mod.rs`'s file-content hashing) computes the
`«redacted:<sha256[..8]>»` replacement token — sha256 of the exact
matched substring, so the same secret redacted twice in one graph
produces the same token, useful for correlation without ever storing
the value.

Nine categories, `crates/carto-core/src/redact/patterns.rs` (eight,
spec §7.5(a)) plus `entropy.rs` (one, §7.5(b)):
`aws_access_key`, `secret_key_near_keyword`, `private_key_pem`,
`github_token`, `gitlab_token`, `slack_token`, `jwt`,
`connection_string`, `high_entropy`. Patterns run first, collecting
non-overlapping spans (first-match-wins on overlap — same house style
as `resolve.rs`'s ambiguity handling); entropy runs second, skipping
any span a pattern already claimed, so e.g. an AWS key is counted once
under `aws_access_key`, never double-counted under `high_entropy` too.

**"Near" (spec §7.5(a)'s `secret`-adjacent 40-char base64 rule)** is
read as "co-occurring anywhere in the same field," not a byte-distance
window — every field this scans is a single declaration's signature
(line-sized), so whole-field co-occurrence is the natural, and only
practically distinguishable, reading.

**Entropy tokenization** (spec §7.5(b)) scans per-*token*, not the
whole field: entropy over a mixed-punctuation string is diluted by
ordinary low-entropy separators, and the standard approach real secret
scanners (gitleaks, detect-secrets) use is exactly this — split on
token boundaries, score each token's own entropy. The token alphabet
is alnum + `+`/`/`/`=`/`-` (base64/hex "value" characters), explicitly
excluding `.` (dotted paths/versions would glue unrelated words
together) **and excluding `_`** — see the dogfooding finding below.

**The "exempt by field, not by pattern" instruction** (spec §7.5(b):
"our own blake3 IDs, git SHAs, content sha256s are exempt by field")
is satisfied structurally, not by an allowlist: those live in `NodeId`/
`sha256: Option<String>` fields, which are never `TaintedString` and
never reach this module. Separately, and by design rather than by
exemption: a 40-hex-char git SHA's own *maximum possible* entropy is
`log2(16) = 4.0` bits/char (16-symbol alphabet), which is below the 4.2
threshold regardless of how "random" the specific SHA is — hex strings
can never trigger the entropy heuristic at all, alphabet math alone
rules them out.

## A real false positive, found by dogfooding, fixed before shipping

Per-category unit tests (true positive + adjacent false positive) and
`fixtures/secrets-corpus`'s end-to-end CLI test all passed on the first
run — but this is a security-critical pass, so the verification didn't
stop there: `cargo run -p carto-cli -- index .` (indexing carto's own
source) surfaced a real false positive neither the fixture nor the
unit corpus had a case for. A long, descriptive `snake_case` test
function name —
`csharp_namespace_import_with_unknown_root_keys_external_module_by_full_string`
— this codebase's own testing convention (CLAUDE.md) — was flagged
`high_entropy`. Underscore was in the entropy token alphabet at the
time, so a dozen lexically-distinct English words merged into one
76-char token, and their combined lexical diversity alone (a long
identifier with little letter repetition) crossed 4.2 bits/char.

Fixed by removing `_` from the entropy alphabet (`patterns.rs`'s own
matchers, which explicitly need `_` for real token/identifier suffixes
like GitHub's `ghp_...`, are unaffected — this only changes the
*entropy* pass's tokenization). No real base64/hex secret blob uses a
literal underscore as a value character — standard base64 doesn't have
one at all, and base64url's underscore usage is already caught
separately by `patterns::find_jwt`'s explicit three-segment shape — so
false positives on ordinary `snake_case` identifiers (likely in nearly
any real codebase, not just this one) were judged a far more probable
failure mode than a false negative on an unprefixed, underscore-bearing
secret with no other distinguishing shape. Re-running `carto index .`
after the fix: redaction count dropped from 18 to 17, and all 17
remaining hits trace to the deliberately-planted fake secrets in
`fixtures/secrets-corpus/` and `cli_redaction.rs`'s own
`PLANTED_SECRETS` const — zero false positives on carto's actual
source. A regression test pins the exact function name.

## Accepted, not fixed: known false-positive/negative edges

- **A comment mentioning "secret" near an unrelated 40-hex-char git-SHA
  reference** (e.g. `// see commit <sha> for secret handling`) would
  false-positive under `secret_key_near_keyword`, since that pattern's
  "near" is whole-field co-occurrence and doesn't distinguish a SHA
  from a real secret-shaped base64 run by content. Accepted: the
  over-redaction failure mode is the safe one for INV-6; revisit only
  if this shows up in practice, the same "accepted, revisit only if"
  bar STATUS.md already applies elsewhere (e.g. Go's suffix-match false
  positive, ADR-0015).
- **`connection_string` redacts only the credential-bearing prefix**
  (`scheme://user:pass@host[:port]`), not the trailing path — verified
  against `fixtures/secrets-corpus`'s own `dbConnection` case
  (`postgres://admin:hunter2example@db.internal:5432/orders` →
  `postgres://«redacted:...»/orders`... the `/orders` path segment
  survives). Deliberate: the path carries no credential.
- **No cross-run secret-value correlation beyond one graph** — the
  sha256-based token is stable within a single `graph.json`, not
  across separate `carto index` runs of the same repo at different
  commits (each run's own `TaintedString::new` re-sanitizes from
  scratch; nothing is cached). Not a goal for this slice.

## Consequences

- `fixtures/secrets-corpus` (spec §11.1) is genuinely new fixture
  content type: fake secrets as file *content* (const/var
  initializers), not filenames — this environment's documented
  refusal to create `.env`/`*.pem`-*shaped filenames* (CLAUDE.md's own
  gotcha) doesn't apply here, since nothing about the fixture's file
  *names* is sensitive-shaped.
- Query layer (`crates/carto-core/src/query/`) needed zero changes —
  redaction happens entirely inside `persist`, before any query-layer
  code ever sees the graph.
- `manifest.json`'s `redaction.by_category` is the first real,
  non-empty use of `RedactionCounts` — the type itself didn't change,
  only what populates it.

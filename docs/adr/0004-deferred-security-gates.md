# 0004 — Deferred security gates from spec §9.5

**Status:** accepted · **Date:** 2026-07-31 · **Milestone:** M1.a

## Context

Spec §9.5 lists five CI security gates, all nominally due in M1. This
implementation is proceeding without a CI system yet (no remote, no
GitHub Actions) and on a macOS development host, so the gates were
triaged into "runs today via `scripts/gates.sh`" vs. "genuinely needs
something not present yet."

## Decision

| # | Gate (spec §9.5) | Status in M1.a | Why |
|---|---|---|---|
| 1 | `cargo deny check bans` | **Running.** `scripts/gates.sh`. | No blocker — `cargo-deny` is installed locally. |
| 2 | `test_no_network` (strace) | **Deferred.** | `strace` doesn't exist on macOS (this is a Linux syscall tracer); the equivalent macOS tool (`dtruss`) requires disabling SIP, which isn't a reasonable ask of a dev machine. Needs Linux CI. Unblocked when: a Linux CI runner exists (any M1.b+ milestone that sets up `.github/workflows/`). |
| 3 | `test_determinism` (double-index byte-compare) | **Deferred.** | There is no `index` command yet — M1.a is invariant scaffolding only, no parsing. Unblocked when: `carto index` exists (M1.b) and produces a `graph.json`. |
| 4 | `test_pathguard` (CLI-level: attempted denylist write returns exit 3) | **Partially covered.** | `pathguard`'s refusal behavior is fully unit-tested at the library level (`crates/carto-core/src/pathguard/mod.rs` tests) and confirmed to map to `ErrorKind::InvariantRefusal` → exit code 3 (`error.rs` tests). What's missing is an `assert_cmd`-based *CLI* test that actually spawns the `carto` binary and checks its process exit code — there's no command yet that writes anything (`selfcheck` doesn't write to `--out`), so there's nothing to point such a test at. Unblocked when: a command that writes through pathguard exists (M1.b's `index`). |
| 5 | M5 additions (WASM-grammar fuzz smoke, Landlock self-test) | **Not due yet.** | Explicitly M5 scope per spec §9.5 itself. |

`scripts/gates.sh` runs gates 1 and (the library-level half of) 4, plus
`cargo fmt`/`cargo test --workspace` as a baseline this spec doesn't
explicitly ask for but that any commit should pass regardless.

Additionally, `cargo deny check advisories` (fetches the RUSTSEC advisory
database over the network) is deliberately **excluded even from the local
gate**, not just deferred: running it would mean `scripts/gates.sh` — a
script whose entire purpose is enforcing INV-1 — makes a network call
every time it runs. It belongs in real CI, where the network boundary is
the CI runner's, not carto's own claimed guarantee.

## Consequences

- M1.a's acceptance bar (this plan's Verification section) is
  `scripts/gates.sh` green, not literal spec §9.5 compliance — that's
  intentional narrowing, recorded here rather than silently claimed.
- Each deferred gate has a concrete unblocking condition above; none of
  them are deferred indefinitely or "until someone remembers." The three
  gates besides advisories are expected to close out over M1.b (adds
  `index`, unblocking #3 and #4) and whenever CI is stood up (unblocks #2).
- Anyone starting M1.b should treat "wire up `assert_cmd` pathguard-refusal
  test" and "wire up the determinism double-index test" as part of that
  milestone's acceptance criteria, not optional follow-up.

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
| 3 | `test_determinism` (double-index byte-compare) | **Running, as of M1.b.1.** | `crates/carto-cli/tests/cli.rs::index_produces_byte_identical_graph_json_across_two_runs` indexes `fixtures/mixed` into two independent out-dirs via `assert_cmd` and byte-compares `graph.json`. |
| 4 | `test_pathguard` (CLI-level: attempted denylist write returns exit 3) | **Running, as of M1.b.1.** | `crates/carto-cli/tests/cli.rs::index_refuses_a_denylisted_out_dir_with_exit_code_3` spawns `carto index --out <denylisted path>` and asserts exit code 3 and that nothing was created. Library-level coverage (noted below) still stands underneath it. |
| 5 | M5 additions (WASM-grammar fuzz smoke, Landlock self-test) | **Not due yet.** | Explicitly M5 scope per spec §9.5 itself. |

`scripts/gates.sh` runs gates 1, 3, and 4 (as of M1.b.1), plus
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
  them are deferred indefinitely or "until someone remembers." Gates #3
  and #4 closed out in M1.b.1, exactly as anticipated, once `carto index`
  existed; #2 remains blocked on a Linux CI runner existing at all.
- Gate #2 (`test_no_network`/strace) is the only one left with no target
  date: it requires `.github/workflows/` and a remote, which M1.b.1
  deliberately still doesn't set up (see `docs/STATUS.md`).

# 0022 — Rust grouped-`use` extraction (amends ADR-0008)

**Status:** accepted · **Date:** 2026-08-03 · **Milestone:** post-S-1
improvement plan, §1.3

## Context

ADR-0008 recorded, as one of several v1 simplifications, that
`use_list` (`use a::{b, c};`), `use_wildcard` (`use a::*;`), and
`use_as_clause` (`use a::b as c;`) were "not walked this slice" —
`walk_use_tree` returned `None` for all three, extracting nothing
rather than guessing at a partial interpretation.

S-1's L3 root-cause investigation (`bench/tasks.md`, `bench/field-log.md`)
connected the `use_list` exclusion to a real, previously unnoticed
consequence: it isn't reduced precision, it's **complete invisibility**.
A minimal one-file repro confirmed any Rust `use path::{a, b};`
produces zero import edges. On carto's own repo, a file whose *only*
`carto_core` import happens to be grouped (e.g.
`crates/carto-cli/src/deps_cmd.rs`'s `use carto_core::{consts, graph,
target};`) contributed nothing to that crate's fan-in count — whether a
file shows up in a `deps --dir in` answer became a coincidence of that
file's *other*, unrelated single-path imports, not an honest signal.
`docs/post-s1-improvement-plan.md` §1.3 scoped this to `use_list` only
("the demonstrated case"), leaving `use_wildcard` (no enumerable member
list — a genuinely different problem) and `use_as_clause` explicitly
out of scope.

## Decisions

- **`use_list` is now extracted, including nested groups and a `self`
  member; `use_wildcard` and `use_as_clause` remain excluded** — exactly
  the plan's scoping. An aliased member *inside* a group (`use a::{b as
  c, d};`) is now skipped individually rather than dropping the whole
  statement, which is strictly more extraction than before and still
  never a guess about `b as c`'s bound name.
- **Two new grammar shapes, verified empirically against
  tree-sitter-rust's actual parse tree before writing any resolution
  logic** (not assumed from documentation) —
  `crates/carto-core/src/lang/rust.rs`'s `walk_use_clause`/
  `walk_use_list_member`:
  - `scoped_use_list` (`use a::{b, c};`, and nested `use a::{b::{c,
    d}, e};`) — the group's own leftmost segment becomes every
    member's classification root, threaded through unchanged at any
    nesting depth. A member's own *trailing* segment only is kept as
    the imported name (`b::c` inside a group binds as `c`) — the same
    "root + leaf name, middle segments dropped" simplification a
    single-path `use` already made; this ADR doesn't add path-tracking
    that didn't exist before.
  - A bare, path-less `use_list` (`use {crate::a, std::b};`) — reachable
    only when the whole `use` has no shared prefix at all. Each member
    supplies its own complete, independent path via the pre-existing
    `walk_use_tree` base case, rather than inheriting a root that
    doesn't exist here.
- **`self` as a group member** (`use std::io::{self, Write};`) resolves
  to the *immediately enclosing* group's own last path segment as its
  bound name (`io`, not the literal string `self`) — required tracking
  a second piece of state (`group_last_segment`) alongside the
  outermost classification root, since a nested `self`
  (`use a::{b::{self, c}};`) must resolve to `b`, not the outer `a`,
  even though the outer root (`a`) is what both `b` and `c` still
  classify against.
- **Multiple members sharing one root still collapse into a single
  `RawImport::Absolute`**, grouped by root in `extract_imports` before
  emitting — matching the shape a single-path `use` has always
  produced. Only the rare path-less bare-list case (differing roots
  within one `use` statement) produces more than one `RawImport` for a
  single declaration.
- **No `.scm` query change** — `imports.scm` still only captures whole
  `use_declaration` nodes; decomposition (which segments are the root
  vs. members, nested-group/`self` handling) happens in `rust.rs`, the
  same "handled more legibly in plain Rust" precedent every prior
  import-shape decision in this codebase already made.
- **Cross-language check done at ADR-writing time, not deferred**, per
  the plan's own instruction to check before committing further:
  Python (`python.rs`) already walks every `name:` field of an
  `import_from_statement`; TS/JS (`ecma.rs`) already loops
  `named_imports`/`import_specifier`; PHP (`php.rs`) already handles
  the grouped form with its shared prefix
  (`extracts_grouped_use_with_shared_prefix`); C# has no grouped
  `using` syntax at all. The all-or-nothing gap ADR-0008 introduced was
  **Rust-specific**, not a shared "one query pattern, list form
  excluded" bug across languages — none of the other four extractors
  needed a change here.

## Consequences

- Amends ADR-0008's original "not walked this slice" note for
  `use_list` specifically; `use_wildcard`/`use_as_clause` remain
  correctly described by that ADR, unchanged.
- `crates/carto-core/src/lang/extractor.rs`'s `RawImport::Absolute` doc
  comment and `imports.scm`'s header comment both updated to describe
  the new scope.
- No fixture golden changes — `fixtures/rust-crate` has no grouped
  `use` statement (its only import is
  `fixtures/rust-crate/src/handlers.rs`'s single-path `use
  crate::orders::parse_order;`), so this change is invisible to the
  existing golden-file tests and needed dedicated new unit tests
  instead (`lang::rust`'s `nested_grouped_use_extracts_every_leaf_
  under_the_shared_root`, `self_in_a_group_imports_the_groups_own_
  last_segment`, `nested_self_imports_the_nested_groups_own_segment_
  not_the_outer_root`, `aliased_member_inside_a_group_is_skipped_
  individually`, `bare_path_less_group_keeps_each_members_own_
  independent_root`).
- Verified against carto's own repo (not just synthetic snippets, per
  CLAUDE.md's documented tree-sitter gotcha): `carto deps carto_core
  --dir in` now lists `crates/carto-cli/src/deps_cmd.rs` as an
  importer — previously invisible because its only `carto_core` import
  was the grouped form this ADR now extracts.
- No `SCHEMA_VERSION` bump — this only changes which `imports` edges
  get produced from existing extractor output, not the graph schema
  itself.

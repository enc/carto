# 0028 — Skill file packaged as a directory, not a flat file

**Status:** accepted · **Date:** 2026-08-04 · **Milestone:** post-MCP,
real-world-use hardening

## Context

Spec §9.3 and §3's file tree name the skill file literally:
`skill/carto.skill.md`, described as "the ONLY thing a user adds to
their agent, by copying it themselves." That wording predates Claude
Code's actual skill-discovery mechanism, which loads skills from
`<skills-dir>/<name>/SKILL.md` — a directory per skill, not a bare file.
A flat `carto.skill.md` copied into `~/.claude/skills/`, exactly what
`README.md` instructed, is never discovered.

This was not a theoretical concern: `~/.claude/skills/plan-eng-review.md`
(a pre-existing, unrelated skill file on this machine) is in exactly that
flat shape and is absent from a live session's available-skills listing
— direct, empirical confirmation of the discovery gap using a file that
predates this ADR. It had gone unnoticed because `bench/run.sh`'s S-1
harness never exercises real skill discovery at all: it `cat`s the file's
contents directly into `--append-system-prompt` (`bench/run.sh:145`),
bypassing the mechanism entirely. Every prior verification of the skill
file's *content* is real; verification of its *discoverability as a
skill* had never actually happened.

## Decision

`skill/carto.skill.md` → `skill/carto/SKILL.md` (`git mv`). The
frontmatter (`name: carto`, `description: ...`) already matched what a
Claude Code `SKILL.md` needs; only the path/directory shape changes, not
the content's structure.

Spec §9.3's *intent* — "the ONE integration file the user copies
themselves," no build step, no generated output — is preserved and, if
anything, better served: `cp -r skill/carto ~/.claude/skills/` is one
command with no silent-rename failure mode, versus the old two-step
"copy this file, and by the way rename it to `SKILL.md` inside a new
directory named `carto`" that a user would have had to know to do
unprompted.

## Consequences

- Every reference to the old path is updated: `README.md`'s install
  instructions, `bench/run.sh:145`'s `cat`, `docs/STATUS.md`'s ongoing
  (not historical) references.
- A new mechanical guard, `crates/carto-mcp/src/schema.rs`'s
  `skill_file_names_every_advertised_tool` test, `include_str!`s
  `skill/carto/SKILL.md` and asserts every `tool_list()` name appears in
  it — the same class of drift `tools/mod.rs`'s existing
  `schema_properties_match_each_handlers_declared_params` test already
  guards against for schema/handler drift, extended one level further to
  catch schema/skill-file drift. This was a real, live gap independent
  of the packaging issue: the skill file at the time of this ADR
  documented five tools while `schema.rs` advertised seven (`contract`/
  `orphans`, ADR-0026, had shipped with no corresponding skill-file
  update).
- INV-4 is unaffected — this is a path change to a file the user already
  copies manually; carto itself still never writes to `~/.claude/`,
  `.claude/`, or any agent config, in either the old or new shape.
- Not done: no code enforces *where* a user places the copied directory,
  same as before — carto has no opinion on the user's own
  `~/.claude/skills/` layout beyond shipping the shape that directory
  actually requires.

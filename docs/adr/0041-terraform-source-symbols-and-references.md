# 0041 — Terraform source symbols and same-module references

**Status:** accepted · **Date:** 2026-09-29 · **Milestone:** Terraform
source support, slice 1 of 4 (ADR-0041 symbols/references, 0042 module
calls, 0043 Terragrunt, 0044 the tfvars implication) — pulled ahead of
the pending M3 go/no-go on user request, the same precedent as
ADR-0016/0018/0025/0034 · **`SCHEMA_VERSION` 8 → 9**

## Context

ADR-0025 parsed `.tf` source only to lift one literal
(`aws_cloudwatch_metric_alarm.metric_name`). `HclExtractor` produced no
symbols, no references and no module structure, so `where`/`deps` had
nothing to say about Terraform: "who uses this variable / local /
output / resource" was unanswerable, and a `local.X` referenced but
defined nowhere (ADR-0025's own motivating example) was invisible.

The design spec assumes a different route (§6: `terraform show -json`,
`IacResource`, resolved `depends_on`). That route needs the user to run
terraform first and yields the *resolved* graph; it stays a later slice
and is not replaced. This ADR adds what only source can give: how the
code is written — addresses, scopes, references, and what is undefined.

User decisions (2026-09-29): source HCL first; "who uses X" and the
module graph are the priorities; `.tfvars` stays unread (ADR-0044
models the implication); Terragrunt relations are file-level edges
(ADR-0043).

## Decision

**Symbols, named by Terraform address.** For `*.tf` and
environment-variant `*.tf.<suffix>` files, every *top-level* block is a
symbol whose `name` (and qualified name) is the address you would write
in a reference: `variable "region"` → `var.region`; each attribute of a
`locals {}` block → `local.prefix` (range = the attribute's own lines);
`output` → `output.vpc_id`; `resource "t" "n"` → `t.n`; `data "t" "n"` →
`data.t.n`; `module "m"` → `module.m`. So `carto where var.region
--exact` and `carto deps aws_s3_bucket.logs --dir in` work directly, and
the existing "matches multiple nodes; specify by ID" error plus
`--subpath` disambiguate the same name in several modules. Nested blocks
(`dynamic`, `lifecycle`, `content`) belong to their declaration.
`provider`, `terraform`, `moved`, `import`, `check` blocks are not
symbols.

**Seven new `SymKind`s** rather than reusing `Struct`/`Function`:
`tf_variable`, `tf_local`, `tf_output`, `tf_resource`, `tf_data`,
`tf_module` (and `tg_dependency`, reserved for ADR-0043). A deviation
from spec §4.1's list. Nothing in `query`/CLI/MCP branches on
`sym_kind` (display only), so the cost is the enum, the schema bump and
this note.

**Never `is_pub`, no call sites, no type refs.** `resolve.rs`'s
`pub_by_name`/`pub_by_component` indices are language-agnostic. A
Terraform `variable "timeout"` in them would make a unique Python
`timeout()` call ambiguous and delete its `calls` edge (INV-8 working
against us). Keeping HCL symbols private and off the call-site channel
removes them from every tier of the generic ladder. Regression test:
`terraform::tests::terraform_symbols_do_not_disturb_other_languages_
call_resolution`.

**Signatures are the block header only** (`variable "region"`), never an
attribute value — a `variable`'s `default` is where secrets live; this
is defence in depth on top of redaction (INV-6). Labels and `locals`
attribute names must match `[A-Za-z_][A-Za-z0-9_-]*` or the block yields
no symbol: `SymbolNode::name` is a plain `String`, and a label is
arbitrary string content (INV-5).

**References are read from source text, not from sibling nodes.**
Verified by printing real parse trees (ADR-0030's lesson):

- `var.a.b` is a `variable_expr` followed by sibling `get_attr` nodes
  (the grammar's hidden, left-recursive `_expr_term`), and index/splat
  (`index`, `splat`) sit in the same sibling run — *but* inside
  `var.a + var.b` the second operand's `.b` lands **outside** the
  `binary_operation` node, so a sibling-run scan would lose it.
- Therefore each `variable_expr` node is only an *anchor*: the dotted
  path is lexed from the source bytes that follow it, skipping `[...]`
  (string-aware), `.*` and legacy `.0` indexes. `aws_instance.w[0].id` is
  `["aws_instance","w","id"]`.
- `object_elem` does carry `key`/`val` fields (ADR-0025's "no named
  fields at all" was wrong for it). A bare-identifier or literal key is
  a string, not a reference, and is skipped; a parenthesised key
  `(var.k) = …` is walked.
- Interpolations (`template_interpolation → expression → …`) and
  `for`/conditional/function-call arguments need no special handling;
  the walk visits every node.

**A dedicated resolution pass** — `lang/terraform.rs`, called once from
`resolve()` before the per-file loop — not the generic tier ladder,
whose repo-wide fallback is wrong by construction: Terraform scope is
one directory, so `var.region` in `modules/a` must never match
`modules/b`'s. Rules (module doc of `terraform.rs` is authoritative):

| Situation for a reference in file F | Result |
|---|---|
| exactly one definition in F's directory, in a plain `.tf` or F's own variant | `references`, `inferred`, `tf-ref:same-module` |
| definitions exist only in *other* variant files (`locals.tf.simu`/`.prod` pattern) | one `inferred` edge per definition, evidence `tf-ref:env-variant` + `variant:<suffix>` |
| a base and an `override.tf`/`*_override.tf` definition | override dropped, resolves to the base (Terraform merges overrides) |
| two or more real candidates, or none, for `var`/`local`/`module`/`data` | no edge; recorded in the enclosing symbol's `unresolved_calls` |
| none, for a resource-shaped `<type>.<name>` | silently dropped (ADR-0029 policy) — `each.value`, `count.index`, `for`/`dynamic` iterators would flood the list |
| built-in roots `path`/`terraform`/`count`/`each`/`self` | ignored |

The source is the innermost enclosing symbol (`smallest_containing_
symbol`, shared with `resolve.rs`); a reference with no enclosing symbol
is dropped (ADR-0031's existing limit). A declaration naming itself is
suppressed.

**Confidence is always `inferred`, never `certain`.** Spec §4.2 allows
only `inferred` for `references`, and directory scope only approximates
what Terraform really loads: a gitignored generated file
(`locals_env.tf`), a `*.tf.json`, terragrunt `generate` blocks and
override merging are all invisible or approximated. Consequently an
`unresolved_calls` entry means "not found among the parsed files", not
"undefined in Terraform's eyes" — the same honesty boundary INV-8
draws for every other language.

**Side effect, deliberate:** `contract` literals inside a resource now
attach to the resource symbol (`aws_cloudwatch_metric_alarm.<name>`)
instead of only the `File` node, because the existing contract pass
already prefers the innermost containing symbol. `orphans` output is
unchanged (a symbol's component equals its file's); `contract` site
labels improve.

**`SCHEMA_VERSION` 8 → 9.** A v8 graph has HCL files but no Terraform
symbols; `deps var.x --dir in` against it must refuse and ask for a
re-index rather than answer "not found" (absence vs. zero), and an older
binary cannot deserialize the new `sym_kind` spellings.

## Consequences

- New `lang/terraform.rs` (with unit tests), `hcl.rs` gains symbol/reference
  extraction and `terraform_file_kind` (with unit tests),
  `ExtractOut::terraform: Option<TerraformFacts>` (a `None` line in each
  other extractor), `SymKind` +7 variants, `consts.rs` history entry,
  `resolve.rs` hook (+ `smallest_containing_symbol`/`relpath_dir` made
  `pub(super)`). Only `mixed.graph.golden.json`'s `schema_version` line
  changed among goldens (checked by diff).
- `fixtures/tf-modules/` + `crates/carto-cli/tests/cli_terraform.rs`
  (7 tests: symbol names, directory isolation, inferred-only, env-variant
  fan-out, override precedence, undefined local reported and iterators
  not, `where`/`deps` on addresses, determinism and no leaked
  `default`), plus one `cli_contracts` assertion for the new site label.
- `*.hcl` files (Terragrunt) still yield literals only. `.terraform/`
  and `.terragrunt-cache/` are not yet pruned by the walk (ADR-0043).
- **Not in this slice:** module calls and cross-module edges (0042),
  Terragrunt (0043), tfvars caveat on `deps` and `*.tfvars.json`
  sensitivity (0044), counting `count`/`for_each` instances (source-level
  only, by design), provider aliases, `moved`/`import` blocks, plan/state
  JSON ingestion (spec §6.2, later).
- Pre-existing and unrelated: the `trybuild` compile-fail suite
  (`carto-core/tests/compile_fail.rs`) mismatches its snapshots on
  rustc 1.94.1 (an extra "error originates in the macro `println`"
  note) — verified identical on an untouched `origin/main` checkout.

## Follow-up (2026-09-30, review fixes)

- **Variant files never fall back to other variants.** The `env-variant`
  fan-out applies only when the *referencing* file is a plain `*.tf`; a
  reference inside `x.tf.simu` to a name defined only in `y.tf.prod` is a
  genuine miss (recorded, no edge). Before this, it produced a false
  `inferred` edge into another environment's file.
- **`<stem>.tf.<suffix>` is a variant only for a single plain token.**
  Suffixes `bak`, `old`, `orig`, `backup`, `example`, `sample`, `disabled`,
  `tmp`, `swp`, `json` (any case), a suffix ending in `~`, and a
  multi-part suffix (`x.tf.a.b`) are copies or scratch files Terraform
  ignores — they yield no symbols or references, so a backup can't stand
  in as a definition.
- `map --section infra` distinguishes "no Terraform anywhere" (the
  original placeholder) from "Terraform exists but none inside the
  requested `--subpath`/`--component`".


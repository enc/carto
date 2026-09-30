# 0042 — Terraform module calls and the cross-module graph

**Status:** accepted · **Date:** 2026-09-29 · **Milestone:** Terraform
source support, slice 2 of 4 (extends ADR-0041) · **No `SCHEMA_VERSION`
change** (no persisted type changed: `RawTfModuleCall` is
extractor-internal, `Module`/`imports`/`references` already exist)

## Context

ADR-0041 resolves references *inside* one directory. Real Terraform is a
tree of modules calling modules: `module "vpc" { source =
"../../modules/vpc" }`. Without the call edge, `deps` on a module's
variable or output could not see its callers, and `map` could not show
which environment uses which module. The user's two stated priorities
were "who uses X, including across module calls" and "the module graph".

## Decision

**A local source is a directory, imported as a fan-out.** A `source`
starting with `./` or `../` (Terraform's own rule) is normalized against
the calling file's directory (`.`/`..` resolved, `//` collapsed; a path
escaping the repo root is unresolvable). Each Terraform file in the
target directory gets an `imports` edge from the calling **file**,
`certain` (spec §5.3 rule 1: an exact relative path), evidence
`tf-module-source` (+ `cross-component` when the two files' components
differ, ADR-0036). File→file fan-out is the Go `PackagePath` shape
(ADR-0015), chosen so `map`'s "top modules" ranking and "entry points"
need no change: a called module's files gain incoming imports and stop
looking like entry points, root/environment directories stay on the
list. No new `RawImport` variant: every existing variant's resolver
would misinterpret a Terraform source.

**Anything else is remote** (registry `ns/name/provider`, `git::`,
`github.com/…`, `tfr://`, `s3::`, `https://` — including `mod/x` with no
dot-slash, exactly as Terraform reads it). It becomes an external
`Module` node, `imports`/`certain`/`tf-module-remote`. The node key is
the source with **`?query`/`#fragment` dropped and `user[:password]@`
removed** (`git::https://u:tok@host/x.git?ref=v1&sshkey=…` →
`git::https://host/x.git`; `//subdir` kept; scp-style `git@host:org/repo`
→ `host:org/repo`), and is refused (`source:<remote>` miss) unless the
result is plain path-shaped text. Reason: `ModuleNode::path` is a plain
`String` and redaction only scans `TaintedString` (INV-6), so credentials
must be removed *before* the key exists. **Limit, not solved:** a token
embedded in the URL *path* itself cannot be recognized here. A remote
source and another language's import of the same string share one
external node (ID collision is intentional; appended after all other
nodes, deduped by ID).

**Across the call.** With a resolved local target directory *T*:

- `module.m.out` (also through an index, `module.m[0].out`) → *T*'s
  `output "out"`: `references`, `inferred`, `tf-ref:module-output`,
  alongside the ordinary same-module edge to `module.m`.
- each argument `foo = …` inside the block → *T*'s `variable "foo"`:
  `references` from the `module.m` symbol, `inferred`, `tf-module-arg`.
  `source`, `version`, `count`, `for_each`, `providers`, `depends_on` are
  meta-arguments, not inputs.
- If env-variant files disagree on the source of the same-named module,
  the target is ambiguous and crossing edges are skipped, not guessed.

**Misses are recorded on the `module.m` symbol's `unresolved_calls`,
never guessed:** `source:<dir>` (target directory has no walked
Terraform file), `source:<dynamic>` (non-literal `source`),
`source:<outside-repo>`, `source:<remote>` (unsafe remote text),
`arg:<name>` (target declares no such variable). A missing output is
recorded on the *referencing* symbol as `module.m.out`. Path text placed
in a plain-`String` field is gated to `[A-Za-z0-9._/-]` (`<invalid>`
otherwise, INV-5).

**`map --section infra`** stops being an unconditional placeholder: when
Terraform symbols are in scope it prints the count per `tf_*` kind, the
module-call graph (`calling dir -> called dir (file imports)`, top 10),
remote modules with fan-in, and then states plainly that the *resolved*
infrastructure graph (`IacResource`, `depends_on`, IAM) is not present
and needs plan/state-JSON ingestion (M2). Repos without Terraform keep
the old placeholder text exactly. `--subpath`/`--component` restrict the
listed rows only (ADR-0014).

## Consequences

- `terraform.rs` gains `Ctx`, module-call resolution,
  `normalize_rel`, `sanitize_remote_source`; `hcl.rs` gains
  `collect_module_calls`; `extractor.rs` gains
  `RawTfModuleCall`; `resolve.rs` appends the pass's external `Module`
  nodes (deduped by ID); `SymKind::is_terraform`; `map.rs::infra_lines`.
- Map keys inside `terraform.rs` are owned `(String, String)`: a
  tuple-of-`&str` key is invariant under `BTreeMap::get` and could not be
  looked up with shorter-lived borrows.
- `fixtures/tf-modules/infra/envs/prod/modules.tf` + 5 new
  `cli_terraform.rs` tests (fan-out and `certain`; remote nodes with the
  fake `ci-user:tok3n-S3CR3T@…?ref=v1.2.0&sshkey=abc` absent from
  `graph.json`; output/argument crossing into the *called* directory
  rather than the same-named output in the caller's; unresolved pieces
  reported; `map --section infra`). The existing isolation test was
  reworded from a hard count to the property "every same-module edge stays
  inside one directory".
- `deps` on a variable now shows both the caller-side edge (module block →
  caller's `var.x`) and the callee-side edge (module block → callee's
  `var.x`); `deps --dir in` on a called module's file lists its callers via
  the file-level `imports`.
- **Not in this slice:** Terragrunt (ADR-0043), tfvars caveat (ADR-0044),
  `count`/`for_each` expansion, module version pinning, resolving a
  registry/git source to code inside the walked tree.

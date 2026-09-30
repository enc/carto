//! Terraform reference and module-call resolution (ADR-0041/0042) — a
//! dedicated pass, not `resolve.rs`'s generic call/type-ref tier ladder.
//! That ladder ends in a repo-wide fallback, which is wrong by
//! construction here: Terraform scope is exactly one directory (a
//! module), so a `var.region` in `modules/a` must never match the
//! `variable "region"` in `modules/b`.
//!
//! **Scope.** A module is the set of `*.tf` files in one directory plus
//! its environment-variant files (`locals.tf.simu`, `locals.tf.prod` —
//! the per-environment pattern ADR-0025 documents, where the real
//! `locals_env.tf` is generated and gitignored).
//!
//! **Which definition a reference means** (file F, reference name N):
//!
//! 1. Candidates: definitions of N in F's directory that live in a plain
//!    `*.tf` file or in a variant file with F's *own* variant suffix.
//!    If any candidate is not an `override.tf`, override candidates are
//!    dropped (Terraform merges overrides over the base declaration).
//! 2. Exactly one → a `references` edge, `inferred`,
//!    `tf-ref:same-module`.
//! 3. None, but other variant files define N → one `inferred` edge per
//!    such definition, evidence `tf-ref:env-variant` + `variant:<suffix>`
//!    (each variant *is* the definition for its own environment; this is
//!    honest, not a guess).
//! 4. Two or more real candidates, or none at all, for `var`/`local`/
//!    `module`/`data`: no edge; the miss is recorded in the enclosing
//!    symbol's `unresolved_calls` ("not found among parsed files" — a
//!    gitignored generated file or a `*.tf.json` is invisible to us).
//! 5. None at all for a resource-shaped `<type>.<name>`: silently
//!    dropped (ADR-0029's policy for type refs) — `each.value`, `count`,
//!    `for`/`dynamic` iterators would otherwise flood the list.
//!
//! **Module calls (ADR-0042).** A `module "m" { source = … }` whose
//! source starts with `./` or `../` is a directory path relative to the
//! calling file: an `imports` edge, `certain` (spec §5.3 rule 1 — an
//! exact relative path), from the calling file to every Terraform file
//! in the target directory (the Go `PackagePath` fan-out shape), so
//! `map`'s module ranking and entry points stay right with no changes.
//! Any other source (registry, `git::`, `github.com/…`, `tfr://`, …) is
//! an external `Module` node keyed by the source with credentials and
//! query stripped ([`sanitize_remote_source`]). Across the call:
//! `module.m.out` → the target directory's `output "out"`, and each
//! `arg = …` inside the block → the target's `variable "arg"` (both
//! `inferred`, like every other source-level reference). A target
//! directory holding no walked Terraform file, an argument with no
//! matching variable, or an output with no matching declaration is
//! recorded in the `module.m` (or enclosing) symbol's
//! `unresolved_calls`, never guessed.
//!
//! **Terragrunt (ADR-0043).** A Terragrunt file is its own resolution
//! scope (its `locals` are file-local). Statically decidable path shapes
//! only ([`TgPath`]) — carto never runs Terragrunt functions:
//! `terraform.source` → the module directory's files (`certain`) or an
//! external `Module`; `dependency.config_path` / `dependencies.paths` →
//! that directory's `terragrunt.hcl` (`certain`); `include path` → the
//! named file (`certain`), or for `find_in_parent_folders(name?)` the
//! nearest ancestor-directory file of that name among the *walked* files
//! (`inferred`: a closer match could be gitignored and invisible);
//! `inputs = { k = … }` → the unit's source module's `variable "k"`;
//! `dependency.x.outputs.y` → the *dependency unit's* source module's
//! `output "y"`. An unmatched `inputs` key is silently dropped
//! (Terragrunt passes unused inputs as `TF_VAR_*`).
//!
//! Reference edges are always `inferred` (spec §4.2 allows nothing
//! higher for `references`; source-level scope only approximates what
//! Terraform really loads).

use super::extractor::{RawTerragrunt, RawTfModuleCall, RawTfRef, TgPath};
use super::resolve::{
    FileExtraction, cross_component_marker, relpath_dir, smallest_containing_symbol,
};
use crate::graph::{Confidence, Edge, EdgeKind, ModuleNode, Node, NodeId, UnresolvedCall};
use crate::taint::Provenance;
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct TerraformResolved {
    /// External `Module` nodes for remote module sources.
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// `(file index, symbol index)` → misses to append to that symbol's
    /// `unresolved_calls`.
    pub unresolved: BTreeMap<(usize, usize), Vec<UnresolvedCall>>,
}

/// Roots that are never a resource type: Terraform's own namespaces.
const BUILTIN_ROOTS: &[&str] = &["path", "terraform", "count", "each", "self"];

/// What kind of name a reference spells — decides whether a miss is
/// reported (`Strict`) or silently dropped (`ResourceShaped`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Strict,
    ResourceShaped,
}

/// The symbol name (Terraform address) a reference points at, if it is
/// one of the shapes this pass understands.
fn target_name(segments: &[String], is_tg: bool) -> Option<(String, Shape)> {
    let root = segments.first()?.as_str();
    if BUILTIN_ROOTS.contains(&root) {
        return None;
    }
    match root {
        "var" | "local" | "module" => {
            let n = segments.get(1)?;
            Some((format!("{root}.{n}"), Shape::Strict))
        }
        // A Terragrunt `dependency.x` must be declared in the same file.
        "dependency" if is_tg => {
            let n = segments.get(1)?;
            Some((format!("dependency.{n}"), Shape::Strict))
        }
        "data" => {
            let (t, n) = (segments.get(1)?, segments.get(2)?);
            Some((format!("data.{t}.{n}"), Shape::Strict))
        }
        _ => {
            let n = segments.get(1)?;
            Some((format!("{root}.{n}"), Shape::ResourceShaped))
        }
    }
}

/// Terraform's own rule: a local module source starts with `./` or `../`.
fn is_local_source(source: &str) -> bool {
    source.starts_with("./") || source.starts_with("../")
}

/// `rel` applied to the repo-relative directory `base`, `.`/`..`
/// resolved and `//` collapsed. `None` if it escapes the repo root.
fn normalize_rel(base: &str, rel: &str) -> Option<String> {
    let mut segments: Vec<&str> = base.split('/').filter(|s| !s.is_empty()).collect();
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                segments.pop()?;
            }
            other => segments.push(other),
        }
    }
    Some(segments.join("/"))
}

/// Repo-relative paths only ever reach a plain-`String` field
/// (`UnresolvedCall::name`) if they are path-shaped (INV-5: repo text is
/// untrusted, and this string comes from a literal).
fn safe_path(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 200
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'))
}

/// The key of the external `Module` node for a remote module source:
/// the source with any `?query`/`#fragment` dropped (`?ref=…` and
/// `?sshkey=…` are where tokens go) and any `user[:password]@` userinfo
/// removed — `git::https://user:tok@host/x.git?ref=v1` becomes
/// `git::https://host/x.git`. `//subdir` is kept. `None` when the result
/// still isn't plain path-shaped text (`ModuleNode::path` is a plain
/// `String`, never redacted — INV-6/INV-5), so nothing exotic is
/// persisted. A token embedded in the *path* itself cannot be detected
/// here; ADR-0042 records that limit.
pub(super) fn sanitize_remote_source(source: &str) -> Option<String> {
    let s = source.split(['?', '#']).next().unwrap_or("");
    let (getter, rest) = match s.split_once("::") {
        Some((g, r)) => (Some(g), r),
        None => (None, s),
    };
    let cleaned = if let Some((scheme, after)) = rest.split_once("://") {
        let (authority, path) = match after.split_once('/') {
            Some((a, p)) => (a, Some(p)),
            None => (after, None),
        };
        let host = authority.rsplit('@').next().unwrap_or(authority);
        match path {
            Some(p) => format!("{scheme}://{host}/{p}"),
            None => format!("{scheme}://{host}"),
        }
    } else {
        // scp-style `user@host:org/repo` — drop the user part.
        match rest.split_once('@') {
            Some((user, after)) if !user.contains('/') => after.to_string(),
            _ => rest.to_string(),
        }
    };
    let out = match getter {
        Some(g) => format!("{g}::{cleaned}"),
        None => cleaned,
    };
    let ok = !out.is_empty()
        && out.len() <= 200
        && out.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '.' | '_' | '~' | ':' | '/' | '+' | '%' | '=' | '-')
        });
    ok.then_some(out)
}

fn is_terragrunt(fe: &FileExtraction) -> bool {
    fe.extract
        .terraform
        .as_ref()
        .is_some_and(|t| t.terragrunt.is_some())
}

/// A Terraform file's resolution scope is its directory; a Terragrunt
/// file's is the file itself.
fn scope_of(fe: &FileExtraction) -> &str {
    if is_terragrunt(fe) {
        &fe.relpath
    } else {
        relpath_dir(&fe.relpath)
    }
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

/// A relative directory or file a Terragrunt path expression names,
/// resolved against the unit's own directory. `None` for anything not
/// statically decidable (or absolute).
fn tg_target(dir: &str, p: &TgPath) -> Option<String> {
    match p {
        TgPath::Literal(s) if !s.starts_with('/') => normalize_rel(dir, s),
        TgPath::TerragruntDir(suffix) => {
            let rel = suffix.trim_start_matches('/');
            if rel.is_empty() {
                Some(dir.to_string())
            } else {
                normalize_rel(dir, rel)
            }
        }
        _ => None,
    }
}

enum SourceTarget<'p> {
    Local(String),
    Remote(&'p str),
}

/// What a Terragrunt `terraform.source` points at.
fn source_target<'p>(dir: &str, p: &'p TgPath) -> Option<SourceTarget<'p>> {
    match p {
        TgPath::Literal(s) if is_local_source(s) => normalize_rel(dir, s).map(SourceTarget::Local),
        TgPath::Literal(s) if !s.starts_with('/') => Some(SourceTarget::Remote(s)),
        TgPath::TerragruntDir(_) => tg_target(dir, p).map(SourceTarget::Local),
        _ => None,
    }
}

struct Def {
    fi: usize,
    si: usize,
    variant: Option<String>,
    is_override: bool,
}

struct Ctx<'a> {
    extractions: &'a [FileExtraction],
    symbol_ids: &'a [Vec<NodeId>],
    /// (directory, address) -> every definition, in file order.
    defs: BTreeMap<(String, String), Vec<Def>>,
    /// directory -> every Terraform file in it.
    files_by_dir: BTreeMap<&'a str, Vec<usize>>,
    /// (calling directory, module name) -> the local target
    /// directories its `source` resolves to (more than one only when
    /// env-variant files disagree — then module-crossing edges are
    /// skipped rather than guessed).
    module_dirs: BTreeMap<(String, String), BTreeSet<String>>,
    /// repo-relative path -> file index, every extracted file.
    by_relpath: BTreeMap<String, usize>,
    /// Terragrunt file index -> its `terraform.source` module directory,
    /// when that is a local directory holding walked Terraform files.
    unit_sources: BTreeMap<usize, String>,
}

impl<'a> Ctx<'a> {
    /// Definitions of `name` in `dir` usable from a file with `variant`,
    /// with `override.tf` candidates dropped when a base exists.
    fn live(&self, dir: &str, name: &str, variant: Option<&str>) -> Vec<&Def> {
        let mut live: Vec<&Def> = self
            .defs
            .get(&(dir.to_string(), name.to_string()))
            .map(|v| v.iter())
            .into_iter()
            .flatten()
            .filter(|d| d.variant.is_none() || d.variant.as_deref() == variant)
            .collect();
        if live.iter().any(|d| !d.is_override) {
            live.retain(|d| !d.is_override);
        }
        live
    }
}

pub(super) fn resolve(
    extractions: &[FileExtraction],
    symbol_ids: &[Vec<NodeId>],
) -> TerraformResolved {
    let mut ctx = Ctx {
        extractions,
        symbol_ids,
        defs: BTreeMap::new(),
        files_by_dir: BTreeMap::new(),
        module_dirs: BTreeMap::new(),
        by_relpath: BTreeMap::new(),
        unit_sources: BTreeMap::new(),
    };
    for (fi, fe) in extractions.iter().enumerate() {
        ctx.by_relpath.insert(fe.relpath.clone(), fi);
        let Some(tf) = &fe.extract.terraform else {
            continue;
        };
        let scope = scope_of(fe);
        if tf.terragrunt.is_none() {
            ctx.files_by_dir.entry(scope).or_default().push(fi);
        }
        for (si, sym) in fe.extract.symbols.iter().enumerate() {
            ctx.defs
                .entry((scope.to_string(), sym.name.clone()))
                .or_default()
                .push(Def {
                    fi,
                    si,
                    variant: tf.variant.clone(),
                    is_override: tf.is_override,
                });
        }
    }
    // Needs `files_by_dir` complete, hence a second walk.
    let mut module_dirs: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
    for fe in extractions {
        let Some(tf) = &fe.extract.terraform else {
            continue;
        };
        let dir = relpath_dir(&fe.relpath);
        for call in &tf.module_calls {
            let Some(src) = call.source.as_deref().filter(|s| is_local_source(s)) else {
                continue;
            };
            let Some(target) = normalize_rel(dir, src) else {
                continue;
            };
            if ctx.files_by_dir.contains_key(target.as_str()) {
                module_dirs
                    .entry((dir.to_string(), call.name.clone()))
                    .or_default()
                    .insert(target);
            }
        }
    }
    ctx.module_dirs = module_dirs;
    let mut unit_sources = BTreeMap::new();
    for (fi, fe) in extractions.iter().enumerate() {
        let Some(p) = fe
            .extract
            .terraform
            .as_ref()
            .and_then(|t| t.terragrunt.as_ref())
            .and_then(|tg| tg.source.as_ref())
        else {
            continue;
        };
        if let Some(SourceTarget::Local(t)) = source_target(relpath_dir(&fe.relpath), p) {
            if ctx.files_by_dir.contains_key(t.as_str()) {
                unit_sources.insert(fi, t);
            }
        }
    }
    ctx.unit_sources = unit_sources;

    let mut out = TerraformResolved {
        nodes: Vec::new(),
        edges: Vec::new(),
        unresolved: BTreeMap::new(),
    };
    let mut remote_nodes: BTreeMap<String, NodeId> = BTreeMap::new();

    for (fi, fe) in extractions.iter().enumerate() {
        let Some(tf) = &fe.extract.terraform else {
            continue;
        };
        let dir = relpath_dir(&fe.relpath);
        for call in &tf.module_calls {
            resolve_module_call(&mut out, &mut remote_nodes, &ctx, fi, dir, call);
        }
        let scope = scope_of(fe);
        for r in &tf.refs {
            resolve_ref(&mut out, &ctx, fi, scope, tf.variant.as_deref(), r);
        }
        if let Some(tg) = &tf.terragrunt {
            resolve_terragrunt(&mut out, &mut remote_nodes, &ctx, fi, tg);
        }
    }
    out
}

fn record_miss(out: &mut TerraformResolved, at: Option<(usize, usize)>, name: String, line: u32) {
    if let Some(key) = at {
        out.unresolved
            .entry(key)
            .or_default()
            .push(UnresolvedCall { name, line });
    }
}

fn resolve_module_call(
    out: &mut TerraformResolved,
    remote_nodes: &mut BTreeMap<String, NodeId>,
    ctx: &Ctx<'_>,
    fi: usize,
    dir: &str,
    call: &RawTfModuleCall,
) {
    let fe = &ctx.extractions[fi];
    let module_name = format!("module.{}", call.name);
    let module_si = fe
        .extract
        .symbols
        .iter()
        .position(|s| s.start_line == call.line && s.name == module_name);
    let at = module_si.map(|si| (fi, si));

    let Some(source) = call.source.as_deref() else {
        if call.dynamic_source {
            record_miss(out, at, "source:<dynamic>".into(), call.line);
        }
        return;
    };

    if !is_local_source(source) {
        match sanitize_remote_source(source) {
            Some(key) => {
                let id = remote_nodes
                    .entry(key.clone())
                    .or_insert_with(|| {
                        let id = crate::graph::module_id(&key, true);
                        out.nodes.push(Node::module(
                            id.clone(),
                            Provenance::Syntactic,
                            fe.origin,
                            ModuleNode {
                                path: key.clone(),
                                external: true,
                            },
                        ));
                        id
                    })
                    .clone();
                out.edges.push(Edge::new(
                    EdgeKind::Imports,
                    fe.file_id.clone(),
                    id,
                    Confidence::Certain,
                    "tf-module-remote".to_string(),
                ));
            }
            None => record_miss(out, at, "source:<remote>".into(), call.line),
        }
        return;
    }

    let Some(target) = normalize_rel(dir, source) else {
        record_miss(out, at, "source:<outside-repo>".into(), call.line);
        return;
    };
    let Some(target_files) = ctx.files_by_dir.get(target.as_str()) else {
        let shown = if safe_path(&target) {
            target.as_str()
        } else {
            "<invalid>"
        };
        record_miss(out, at, format!("source:{shown}"), call.line);
        return;
    };
    for &tfi in target_files {
        let target_fe = &ctx.extractions[tfi];
        if target_fe.file_id == fe.file_id {
            continue;
        }
        let mut edge = Edge::new(
            EdgeKind::Imports,
            fe.file_id.clone(),
            target_fe.file_id.clone(),
            Confidence::Certain,
            "tf-module-source".to_string(),
        );
        if let Some(marker) =
            cross_component_marker(fe.component.as_deref(), target_fe.component.as_deref())
        {
            edge.evidence.push(marker.to_string());
        }
        out.edges.push(edge);
    }

    // Each argument is an input to the target's `variable` of that name.
    let Some(module_si) = module_si else {
        return;
    };
    for (arg, line) in &call.args {
        let candidates = ctx.live(&target, &format!("var.{arg}"), None);
        match candidates.as_slice() {
            [one] => out.edges.push(Edge::new(
                EdgeKind::References,
                ctx.symbol_ids[fi][module_si].clone(),
                ctx.symbol_ids[one.fi][one.si].clone(),
                Confidence::Inferred,
                "tf-module-arg".to_string(),
            )),
            _ => record_miss(out, at, format!("arg:{arg}"), *line),
        }
    }
}

fn resolve_ref(
    out: &mut TerraformResolved,
    ctx: &Ctx<'_>,
    fi: usize,
    dir: &str,
    variant: Option<&str>,
    r: &RawTfRef,
) {
    let fe = &ctx.extractions[fi];
    let Some((name, shape)) = target_name(&r.segments, is_terragrunt(fe)) else {
        return;
    };
    let enclosing = smallest_containing_symbol(&fe.extract.symbols, r.line);
    let from_id = match enclosing {
        Some(si) => ctx.symbol_ids[fi][si].clone(),
        None => fe.file_id.clone(),
    };

    let all: &[Def] = ctx
        .defs
        .get(&(dir.to_string(), name.clone()))
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let live = ctx.live(dir, &name, variant);

    let mut push_edge = |d: &Def, evidence: Vec<String>| {
        let to_id = ctx.symbol_ids[d.fi][d.si].clone();
        if to_id == from_id {
            return; // a declaration naming itself is not a dependency
        }
        let mut it = evidence.into_iter();
        let mut edge = Edge::new(
            EdgeKind::References,
            from_id.clone(),
            to_id,
            Confidence::Inferred,
            it.next().expect("at least one evidence entry"),
        );
        edge.evidence.extend(it);
        out.edges.push(edge);
    };

    match live.as_slice() {
        [one] => push_edge(one, vec!["tf-ref:same-module".to_string()]),
        // Only a *plain* file falls back to per-environment variants: a
        // variant file (`x.tf.simu`) never sees another variant's files
        // (`y.tf.prod`), so for it this is a genuine miss.
        [] if !all.is_empty() && variant.is_none() => {
            // Defined only in other environment variants.
            for d in all {
                let suffix = d.variant.as_deref().unwrap_or_default();
                push_edge(
                    d,
                    vec![
                        "tf-ref:env-variant".to_string(),
                        format!("variant:{suffix}"),
                    ],
                );
            }
        }
        [] if shape == Shape::ResourceShaped => {}
        // Genuinely undefined, or a real duplicate: no edge, and say so
        // on the enclosing symbol (no enclosing symbol: nowhere to say it).
        _ => record_miss(out, enclosing.map(|si| (fi, si)), name, r.line),
    }

    // `module.m.out`: also cross into the called module's `output "out"`.
    if r.segments.first().map(String::as_str) == Some("module") && r.segments.len() >= 3 {
        resolve_module_output(out, ctx, fi, dir, enclosing, &from_id, r);
    }
    // `dependency.x.outputs.y` (Terragrunt): cross into the dependency
    // unit's source module.
    if is_terragrunt(fe)
        && r.segments.first().map(String::as_str) == Some("dependency")
        && r.segments.len() >= 4
        && r.segments[2] == "outputs"
    {
        resolve_dependency_output(out, ctx, fi, enclosing, &from_id, r);
    }
}

fn resolve_module_output(
    out: &mut TerraformResolved,
    ctx: &Ctx<'_>,
    fi: usize,
    dir: &str,
    enclosing: Option<usize>,
    from_id: &NodeId,
    r: &RawTfRef,
) {
    let (module, output) = (&r.segments[1], &r.segments[2]);
    let Some(dirs) = ctx.module_dirs.get(&(dir.to_string(), module.clone())) else {
        return; // remote, unresolved, or not a module call we saw: nothing to cross into
    };
    let mut it = dirs.iter();
    let (Some(target), None) = (it.next(), it.next()) else {
        return; // env variants disagree on the source: ambiguous, skip
    };
    let candidates = ctx.live(target, &format!("output.{output}"), None);
    match candidates.as_slice() {
        [one] => out.edges.push(Edge::new(
            EdgeKind::References,
            from_id.clone(),
            ctx.symbol_ids[one.fi][one.si].clone(),
            Confidence::Inferred,
            "tf-ref:module-output".to_string(),
        )),
        _ => record_miss(
            out,
            enclosing.map(|si| (fi, si)),
            format!("module.{module}.{output}"),
            r.line,
        ),
    }
}

fn push_import(
    out: &mut TerraformResolved,
    ctx: &Ctx<'_>,
    from: &FileExtraction,
    to_fi: usize,
    confidence: Confidence,
    evidence: &str,
) {
    let to = &ctx.extractions[to_fi];
    if to.file_id == from.file_id {
        return;
    }
    let mut edge = Edge::new(
        EdgeKind::Imports,
        from.file_id.clone(),
        to.file_id.clone(),
        confidence,
        evidence.to_string(),
    );
    if let Some(marker) = cross_component_marker(from.component.as_deref(), to.component.as_deref())
    {
        edge.evidence.push(marker.to_string());
    }
    out.edges.push(edge);
}

fn remote_module_edge(
    out: &mut TerraformResolved,
    remote_nodes: &mut BTreeMap<String, NodeId>,
    fe: &FileExtraction,
    key: String,
    evidence: &str,
) {
    let id = remote_nodes
        .entry(key.clone())
        .or_insert_with(|| {
            let id = crate::graph::module_id(&key, true);
            out.nodes.push(Node::module(
                id.clone(),
                Provenance::Syntactic,
                fe.origin,
                ModuleNode {
                    path: key.clone(),
                    external: true,
                },
            ));
            id
        })
        .clone();
    out.edges.push(Edge::new(
        EdgeKind::Imports,
        fe.file_id.clone(),
        id,
        Confidence::Certain,
        evidence.to_string(),
    ));
}

fn resolve_terragrunt(
    out: &mut TerraformResolved,
    remote_nodes: &mut BTreeMap<String, NodeId>,
    ctx: &Ctx<'_>,
    fi: usize,
    tg: &RawTerragrunt,
) {
    let fe = &ctx.extractions[fi];
    let dir = relpath_dir(&fe.relpath);

    if let Some(p) = &tg.source {
        match source_target(dir, p) {
            Some(SourceTarget::Local(t)) => {
                for &tfi in ctx.files_by_dir.get(t.as_str()).into_iter().flatten() {
                    push_import(
                        out,
                        ctx,
                        fe,
                        tfi,
                        Confidence::Certain,
                        "tg-terraform-source",
                    );
                }
            }
            Some(SourceTarget::Remote(s)) => {
                if let Some(key) = sanitize_remote_source(s) {
                    remote_module_edge(out, remote_nodes, fe, key, "tg-terraform-source:remote");
                }
            }
            None => {}
        }
    }

    for dep in &tg.dependencies {
        let Some(d) = tg_target(dir, &dep.config_path) else {
            continue;
        };
        match ctx.by_relpath.get(&join(&d, "terragrunt.hcl")) {
            Some(&tfi) => push_import(out, ctx, fe, tfi, Confidence::Certain, "tg-dependency"),
            None => {
                let name = format!("dependency.{}", dep.name);
                let at = fe
                    .extract
                    .symbols
                    .iter()
                    .position(|s| s.start_line == dep.line && s.name == name)
                    .map(|si| (fi, si));
                let shown = if safe_path(&d) {
                    d.as_str()
                } else {
                    "<invalid>"
                };
                record_miss(out, at, format!("config_path:{shown}"), dep.line);
            }
        }
    }

    for p in &tg.dependencies_paths {
        if let Some(d) = tg_target(dir, p) {
            if let Some(&tfi) = ctx.by_relpath.get(&join(&d, "terragrunt.hcl")) {
                push_import(out, ctx, fe, tfi, Confidence::Certain, "tg-dependencies");
            }
        }
    }

    for p in &tg.includes {
        match p {
            TgPath::FindInParentFolders(name) => {
                let name = name.as_deref().unwrap_or("terragrunt.hcl");
                // Pure lookup over the *walked* files, starting at the
                // parent directory — nothing is executed (INV-2).
                let mut cur = dir;
                while !cur.is_empty() {
                    cur = relpath_dir(cur);
                    if let Some(&tfi) = ctx.by_relpath.get(&join(cur, name)) {
                        push_import(
                            out,
                            ctx,
                            fe,
                            tfi,
                            Confidence::Inferred,
                            "tg-include:find_in_parent_folders",
                        );
                        break;
                    }
                }
            }
            other => {
                if let Some(t) = tg_target(dir, other) {
                    if let Some(&tfi) = ctx.by_relpath.get(&t) {
                        push_import(out, ctx, fe, tfi, Confidence::Certain, "tg-include");
                    }
                }
            }
        }
    }

    // `inputs = { k = … }` feeds the unit's source module's variable `k`.
    // An unmatched key is dropped silently: Terragrunt passes unused
    // inputs on as `TF_VAR_*`.
    if let Some(target) = ctx.unit_sources.get(&fi) {
        for key in &tg.inputs {
            if let [one] = ctx.live(target, &format!("var.{key}"), None).as_slice() {
                out.edges.push(Edge::new(
                    EdgeKind::References,
                    fe.file_id.clone(),
                    ctx.symbol_ids[one.fi][one.si].clone(),
                    Confidence::Inferred,
                    format!("tg-input:{key}"),
                ));
            }
        }
    }
}

fn resolve_dependency_output(
    out: &mut TerraformResolved,
    ctx: &Ctx<'_>,
    fi: usize,
    enclosing: Option<usize>,
    from_id: &NodeId,
    r: &RawTfRef,
) {
    let fe = &ctx.extractions[fi];
    let Some(tg) = fe
        .extract
        .terraform
        .as_ref()
        .and_then(|t| t.terragrunt.as_ref())
    else {
        return;
    };
    let (dep_name, output) = (&r.segments[1], &r.segments[3]);
    let Some(dep) = tg.dependencies.iter().find(|d| &d.name == dep_name) else {
        return;
    };
    // dependency -> its unit's terragrunt.hcl -> that unit's source module.
    let Some(d) = tg_target(relpath_dir(&fe.relpath), &dep.config_path) else {
        return;
    };
    let Some(&unit_fi) = ctx.by_relpath.get(&join(&d, "terragrunt.hcl")) else {
        return;
    };
    let Some(module_dir) = ctx.unit_sources.get(&unit_fi) else {
        return; // chain not statically resolvable: say nothing
    };
    match ctx
        .live(module_dir, &format!("output.{output}"), None)
        .as_slice()
    {
        [one] => out.edges.push(Edge::new(
            EdgeKind::References,
            from_id.clone(),
            ctx.symbol_ids[one.fi][one.si].clone(),
            Confidence::Inferred,
            "tg-dependency-output".to_string(),
        )),
        _ => record_miss(
            out,
            enclosing.map(|si| (fi, si)),
            format!("dependency.{dep_name}.outputs.{output}"),
            r.line,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::super::extractor::LangExtractor;
    use super::super::hcl::HclExtractor;
    use super::super::resolve::resolve as resolve_all;
    use super::*;
    use crate::contracts::ContractRules;
    use crate::graph::{self, Node, NodeData};
    use crate::lang::Lang;

    fn tf_file(relpath: &str, src: &str) -> FileExtraction {
        FileExtraction {
            file_id: graph::file_id(relpath),
            relpath: relpath.to_string(),
            extract: HclExtractor.extract(src.as_bytes(), relpath),
            origin: "lang-hcl@1",
            dir_scoped: false,
            ns_separator: "\\",
            qualified_external_is_full_fqn: false,
            declares_module: false,
            lang: Lang::Hcl,
            component: None,
        }
    }

    /// `(from, to, evidence)` for every `references` edge, by symbol
    /// name (`<file>` for a file-scope source).
    fn refs(nodes: &[Node], edges: &[Edge]) -> Vec<(String, String, Vec<String>)> {
        let names: BTreeMap<&NodeId, String> = nodes
            .iter()
            .filter_map(|n| match &n.data {
                NodeData::Symbol(s) => Some((&n.id, format!("{}@{}", s.name, s.start_line))),
                _ => None,
            })
            .collect();
        let mut v: Vec<_> = edges
            .iter()
            .filter(|e| e.kind == EdgeKind::References)
            .map(|e| {
                (
                    names.get(&e.from).cloned().unwrap_or("<file>".into()),
                    names.get(&e.to).cloned().unwrap_or("<file>".into()),
                    e.evidence.clone(),
                )
            })
            .collect();
        v.sort();
        v
    }

    fn unresolved(nodes: &[Node], name: &str) -> Vec<String> {
        nodes
            .iter()
            .find_map(|n| match &n.data {
                NodeData::Symbol(s) if s.name == name => {
                    Some(s.unresolved_calls.iter().map(|u| u.name.clone()).collect())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("no symbol {name}"))
    }

    fn run(files: Vec<FileExtraction>) -> (Vec<Node>, Vec<Edge>) {
        let out = resolve_all(files, &ContractRules::builtin(), &[]);
        (out.nodes, out.edges)
    }

    #[test]
    fn same_module_reference_resolves_inferred() {
        let (nodes, edges) = run(vec![
            tf_file("m/variables.tf", "variable \"region\" {}\n"),
            tf_file(
                "m/main.tf",
                "resource \"aws_s3_bucket\" \"b\" {\n  bucket = var.region\n}\n",
            ),
        ]);
        assert_eq!(
            refs(&nodes, &edges),
            vec![(
                "aws_s3_bucket.b@1".into(),
                "var.region@1".into(),
                vec!["tf-ref:same-module".into()]
            )]
        );
        let e = edges
            .iter()
            .find(|e| e.kind == EdgeKind::References)
            .unwrap();
        assert_eq!(e.confidence, Confidence::Inferred);
    }

    #[test]
    fn a_variable_in_another_directory_is_never_matched() {
        let (nodes, edges) = run(vec![
            tf_file("a/variables.tf", "variable \"region\" {}\n"),
            tf_file(
                "b/main.tf",
                "resource \"aws_s3_bucket\" \"x\" {\n  bucket = var.region\n}\n",
            ),
        ]);
        assert!(refs(&nodes, &edges).is_empty());
        assert_eq!(unresolved(&nodes, "aws_s3_bucket.x"), vec!["var.region"]);
    }

    #[test]
    fn same_named_variables_in_two_modules_each_resolve_locally() {
        let (nodes, edges) = run(vec![
            tf_file("a/v.tf", "variable \"region\" {}\n"),
            tf_file("a/m.tf", "output \"o\" {\n  value = var.region\n}\n"),
            tf_file("b/v.tf", "variable \"region\" {}\n"),
            tf_file("b/m.tf", "output \"o\" {\n  value = var.region\n}\n"),
        ]);
        let r = refs(&nodes, &edges);
        assert_eq!(r.len(), 2);
        let ids: Vec<_> = edges
            .iter()
            .filter(|e| e.kind == EdgeKind::References)
            .map(|e| e.to.clone())
            .collect();
        assert_ne!(ids[0], ids[1]);
    }

    #[test]
    fn env_variant_definitions_fan_out_with_variant_evidence() {
        let (nodes, edges) = run(vec![
            tf_file("env/locals.tf.simu", "locals {\n  prefix = \"a\"\n}\n"),
            tf_file("env/locals.tf.prod", "locals {\n  prefix = \"b\"\n}\n"),
            tf_file("env/main.tf", "output \"o\" {\n  value = local.prefix\n}\n"),
        ]);
        let r = refs(&nodes, &edges);
        assert_eq!(r.len(), 2);
        let ev: Vec<_> = r.iter().map(|x| x.2.clone()).collect();
        assert!(ev.contains(&vec!["tf-ref:env-variant".into(), "variant:simu".into()]));
        assert!(ev.contains(&vec!["tf-ref:env-variant".into(), "variant:prod".into()]));
        assert!(unresolved(&nodes, "output.o").is_empty());
    }

    #[test]
    fn a_variant_file_prefers_its_own_variant_and_plain_files() {
        let (nodes, edges) = run(vec![
            tf_file("env/locals.tf.simu", "locals {\n  prefix = \"a\"\n}\n"),
            tf_file("env/locals.tf.prod", "locals {\n  prefix = \"b\"\n}\n"),
            tf_file(
                "env/extra.tf.simu",
                "output \"o\" {\n  value = local.prefix\n}\n",
            ),
        ]);
        let r = refs(&nodes, &edges);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].2, vec!["tf-ref:same-module".to_string()]);
    }

    #[test]
    fn override_file_yields_to_the_base_declaration() {
        let (nodes, edges) = run(vec![
            tf_file("m/v.tf", "variable \"x\" {}\n"),
            tf_file("m/override.tf", "variable \"x\" {}\n"),
            tf_file("m/main.tf", "output \"o\" {\n  value = var.x\n}\n"),
        ]);
        let r = refs(&nodes, &edges);
        assert_eq!(r.len(), 1);
        assert!(unresolved(&nodes, "output.o").is_empty());
    }

    #[test]
    fn real_duplicate_produces_no_edge_and_is_reported() {
        let (nodes, edges) = run(vec![
            tf_file("m/a.tf", "variable \"x\" {}\n"),
            tf_file("m/b.tf", "variable \"x\" {}\n"),
            tf_file("m/main.tf", "output \"o\" {\n  value = var.x\n}\n"),
        ]);
        assert!(refs(&nodes, &edges).is_empty());
        assert_eq!(unresolved(&nodes, "output.o"), vec!["var.x"]);
    }

    #[test]
    fn undefined_local_is_reported_but_unknown_resource_shape_is_not() {
        let (nodes, edges) = run(vec![tf_file(
            "m/main.tf",
            "output \"o\" {\n  value = \"${local.missing}${each.value}${s.id}\"\n}\n",
        )]);
        assert!(refs(&nodes, &edges).is_empty());
        assert_eq!(unresolved(&nodes, "output.o"), vec!["local.missing"]);
    }

    #[test]
    fn resources_and_data_and_modules_resolve() {
        let (nodes, edges) = run(vec![tf_file(
            "m/main.tf",
            "resource \"aws_vpc\" \"main\" {}\n\
             data \"aws_caller_identity\" \"me\" {}\n\
             module \"child\" {}\n\
             output \"o\" {\n  value = [aws_vpc.main.id, data.aws_caller_identity.me.arn, module.child.x]\n}\n",
        )]);
        let r = refs(&nodes, &edges);
        let targets: Vec<_> = r.iter().map(|x| x.1.clone()).collect();
        assert_eq!(targets.len(), 3);
        assert!(targets.iter().any(|t| t.starts_with("aws_vpc.main")));
        assert!(
            targets
                .iter()
                .any(|t| t.starts_with("data.aws_caller_identity.me"))
        );
        assert!(targets.iter().any(|t| t.starts_with("module.child")));
    }

    #[test]
    fn a_local_naming_itself_is_not_an_edge() {
        let (nodes, edges) = run(vec![tf_file("m/main.tf", "locals {\n  a = local.a\n}\n")]);
        assert!(refs(&nodes, &edges).is_empty());
    }

    /// The reason HCL symbols are never `is_pub`: a Terraform
    /// `variable "timeout"` must not make a unique Python `timeout()`
    /// call ambiguous.
    #[test]
    fn terraform_symbols_do_not_disturb_other_languages_call_resolution() {
        use crate::graph::SymKind;
        use crate::lang::extractor::{RawCallSite, RawSymbol};
        let mut py_lib = tf_file("lib.py", "");
        py_lib.origin = "lang-python@1";
        py_lib.lang = Lang::Python;
        py_lib.extract.symbols.push(RawSymbol {
            name: "timeout".into(),
            qualified_name: "timeout".into(),
            sym_kind: SymKind::Function,
            start_line: 1,
            end_line: 2,
            signature: "def timeout()".into(),
            is_pub: true,
            owner: None,
        });
        let mut py_app = tf_file("app.py", "");
        py_app.origin = "lang-python@1";
        py_app.lang = Lang::Python;
        py_app.extract.symbols.push(RawSymbol {
            name: "run".into(),
            qualified_name: "run".into(),
            sym_kind: SymKind::Function,
            start_line: 1,
            end_line: 3,
            signature: "def run()".into(),
            is_pub: true,
            owner: None,
        });
        py_app.extract.call_sites.push(RawCallSite {
            callee_name: "timeout".into(),
            line: 2,
        });
        let (nodes, edges) = run(vec![
            py_lib,
            py_app,
            tf_file("infra/v.tf", "variable \"timeout\" {}\n"),
        ]);
        assert!(
            edges.iter().any(|e| e.kind == EdgeKind::Calls),
            "the Python call must still resolve: {nodes:?}"
        );
    }

    // ---- ADR-0042 -----------------------------------------------------

    #[test]
    fn normalize_rel_resolves_dots_and_refuses_to_escape_the_repo() {
        assert_eq!(
            normalize_rel("infra/envs/prod", "../../modules/vpc").as_deref(),
            Some("infra/modules/vpc")
        );
        assert_eq!(normalize_rel("", "./m").as_deref(), Some("m"));
        assert_eq!(normalize_rel("x", "./a//b").as_deref(), Some("x/a/b"));
        assert_eq!(normalize_rel("a", "../../x"), None);
    }

    #[test]
    fn remote_sources_lose_credentials_and_query_but_keep_the_subdir() {
        for (input, expected) in [
            (
                "terraform-aws-modules/vpc/aws",
                Some("terraform-aws-modules/vpc/aws"),
            ),
            (
                "git::https://user:tok@host.com/x.git?ref=v1",
                Some("git::https://host.com/x.git"),
            ),
            (
                "git::https://host.com/x.git//sub/dir?ref=v1&sshkey=abc",
                Some("git::https://host.com/x.git//sub/dir"),
            ),
            (
                "git@github.com:org/repo.git",
                Some("github.com:org/repo.git"),
            ),
            (
                "git::git@github.com:org/repo.git?ref=x",
                Some("git::github.com:org/repo.git"),
            ),
            (
                "github.com/org/repo//sub?ref=1",
                Some("github.com/org/repo//sub"),
            ),
            (
                "tfr://registry.terraform.io/hashicorp/x/aws",
                Some("tfr://registry.terraform.io/hashicorp/x/aws"),
            ),
            ("https://example.com/a b", None),
            ("", None),
        ] {
            assert_eq!(
                sanitize_remote_source(input).as_deref(),
                expected,
                "{input}"
            );
        }
    }

    fn imports(edges: &[Edge]) -> Vec<&Edge> {
        edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Imports)
            .collect()
    }

    #[test]
    fn module_call_fans_out_to_every_file_and_marks_component_crossings() {
        let mut caller = tf_file("env/main.tf", "module \"m\" {\n  source = \"../mod\"\n}\n");
        // `../mod` lacks the required `./`/`../` prefix? No: it has `../`.
        caller.component = Some("a".into());
        let mut t1 = tf_file("mod/a.tf", "variable \"x\" {}\n");
        let mut t2 = tf_file("mod/b.tf", "variable \"y\" {}\n");
        t1.component = Some("b".into());
        t2.component = Some("b".into());
        let (_, edges) = run(vec![caller, t1, t2]);
        let imp = imports(&edges);
        assert_eq!(imp.len(), 2);
        for e in imp {
            assert_eq!(e.confidence, Confidence::Certain);
            assert_eq!(
                e.evidence,
                vec![
                    "tf-module-source".to_string(),
                    "cross-component".to_string()
                ]
            );
        }
    }

    #[test]
    fn a_source_without_dot_slash_is_remote_not_a_directory() {
        // Terraform's own rule: `mod/x` is a registry/VCS address.
        let (nodes, edges) = run(vec![
            tf_file("env/main.tf", "module \"m\" {\n  source = \"mod/x\"\n}\n"),
            tf_file("env/mod/x/a.tf", "variable \"v\" {}\n"),
        ]);
        let imp = imports(&edges);
        assert_eq!(imp.len(), 1);
        assert_eq!(imp[0].evidence, vec!["tf-module-remote".to_string()]);
        assert!(nodes.iter().any(|n| matches!(
            &n.data,
            NodeData::Module(m) if m.external && m.path == "mod/x"
        )));
    }

    #[test]
    fn an_interpolated_source_is_reported_not_ignored() {
        let (nodes, edges) = run(vec![
            tf_file("env/vars.tf", "variable \"base\" {}\n"),
            tf_file(
                "env/main.tf",
                "module \"m\" {\n  source = \"${var.base}/mod\"\n}\n",
            ),
        ]);
        assert!(imports(&edges).is_empty());
        assert_eq!(unresolved(&nodes, "module.m"), vec!["source:<dynamic>"]);
    }

    #[test]
    fn a_source_escaping_the_repo_root_is_reported() {
        let (nodes, edges) = run(vec![tf_file(
            "env/main.tf",
            "module \"m\" {\n  source = \"../../../elsewhere\"\n}\n",
        )]);
        assert!(imports(&edges).is_empty());
        assert_eq!(
            unresolved(&nodes, "module.m"),
            vec!["source:<outside-repo>"]
        );
    }

    #[test]
    fn variants_that_disagree_on_a_module_source_skip_output_crossing() {
        let (nodes, edges) = run(vec![
            tf_file("env/a.tf.simu", "module \"m\" {\n  source = \"../x\"\n}\n"),
            tf_file("env/a.tf.prod", "module \"m\" {\n  source = \"../y\"\n}\n"),
            tf_file("x/o.tf", "output \"o\" {\n  value = 1\n}\n"),
            tf_file("y/o.tf", "output \"o\" {\n  value = 2\n}\n"),
            tf_file("env/main.tf", "output \"z\" {\n  value = module.m.o\n}\n"),
        ]);
        assert!(
            !edges
                .iter()
                .any(|e| e.evidence.contains(&"tf-ref:module-output".to_string())),
            "ambiguous module target must not be guessed"
        );
        assert!(unresolved(&nodes, "output.z").is_empty());
    }

    #[test]
    fn module_arguments_resolve_against_the_target_and_misses_are_reported() {
        let (nodes, edges) = run(vec![
            tf_file(
                "env/main.tf",
                "module \"m\" {\n  source = \"../mod\"\n  count  = 2\n  a      = 1\n  nope   = 2\n}\n",
            ),
            tf_file("mod/v.tf", "variable \"a\" {}\n"),
        ]);
        let args: Vec<_> = edges
            .iter()
            .filter(|e| e.evidence == vec!["tf-module-arg".to_string()])
            .collect();
        assert_eq!(args.len(), 1);
        // `count` is a meta-argument, not an input.
        assert_eq!(unresolved(&nodes, "module.m"), vec!["arg:nope"]);
    }

    // ---- ADR-0043 -----------------------------------------------------

    fn import_targets<'e>(
        edges: &'e [Edge],
        from: &FileExtraction,
        to: &[&FileExtraction],
        evidence0: &str,
    ) -> Vec<&'e Edge> {
        edges
            .iter()
            .filter(|e| {
                e.kind == EdgeKind::Imports
                    && e.from == from.file_id
                    && to.iter().any(|t| t.file_id == e.to)
                    && e.evidence[0] == evidence0
            })
            .collect()
    }

    #[test]
    fn find_in_parent_folders_picks_the_nearest_walked_ancestor() {
        let far = tf_file("root.hcl", "locals {\n  a = 1\n}\n");
        let near = tf_file("env/root.hcl", "locals {\n  b = 1\n}\n");
        let unit = tf_file(
            "env/prod/x/terragrunt.hcl",
            "include \"r\" {\n  path = find_in_parent_folders(\"root.hcl\")\n}\n",
        );
        let top = tf_file(
            "terragrunt.hcl",
            "include {\n  path = find_in_parent_folders(\"root.hcl\")\n}\n",
        );
        let (far_c, near_c, unit_c, top_c) = (
            tf_file("root.hcl", ""),
            tf_file("env/root.hcl", ""),
            tf_file("env/prod/x/terragrunt.hcl", ""),
            tf_file("terragrunt.hcl", ""),
        );
        let (_, edges) = run(vec![far, near, unit, top]);
        let e = import_targets(
            &edges,
            &unit_c,
            &[&near_c],
            "tg-include:find_in_parent_folders",
        );
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].confidence, Confidence::Inferred);
        assert!(
            import_targets(
                &edges,
                &unit_c,
                &[&far_c],
                "tg-include:find_in_parent_folders"
            )
            .is_empty()
        );
        // A file at the repo root has no parent directory to search.
        assert!(
            import_targets(
                &edges,
                &top_c,
                &[&far_c],
                "tg-include:find_in_parent_folders"
            )
            .is_empty()
        );
    }

    #[test]
    fn a_literal_include_is_certain() {
        let root = tf_file("root.hcl", "");
        let unit = tf_file(
            "u/terragrunt.hcl",
            "include {\n  path = \"../root.hcl\"\n}\n",
        );
        let (_, edges) = run(vec![tf_file("root.hcl", ""), unit]);
        let e = import_targets(
            &edges,
            &tf_file("u/terragrunt.hcl", ""),
            &[&root],
            "tg-include",
        );
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].confidence, Confidence::Certain);
    }

    #[test]
    fn terragrunt_locals_never_see_terraform_locals_in_the_same_directory() {
        let (nodes, edges) = run(vec![
            tf_file("u/main.tf", "locals {\n  a = 1\n}\n"),
            tf_file("u/terragrunt.hcl", "locals {\n  b = local.a\n}\n"),
        ]);
        assert!(refs(&nodes, &edges).is_empty());
        assert_eq!(unresolved(&nodes, "local.b"), vec!["local.a"]);
    }

    #[test]
    fn matched_inputs_link_to_the_source_module_and_unmatched_are_silent() {
        let (nodes, edges) = run(vec![
            tf_file("m/v.tf", "variable \"a\" {}\n"),
            tf_file(
                "u/terragrunt.hcl",
                "terraform {\n  source = \"../m\"\n}\ninputs = {\n  a = 1\n  b = 2\n}\n",
            ),
        ]);
        let tg: Vec<_> = edges
            .iter()
            .filter(|e| e.kind == EdgeKind::References)
            .map(|e| e.evidence.clone())
            .collect();
        assert_eq!(tg, vec![vec!["tg-input:a".to_string()]]);
        // Silent: no unresolved entry anywhere for the unmatched key.
        assert!(nodes.iter().all(|n| match &n.data {
            NodeData::Symbol(s) => s.unresolved_calls.is_empty(),
            _ => true,
        }));
    }

    #[test]
    fn a_variant_file_never_falls_back_to_another_variants_definition() {
        let (nodes, edges) = run(vec![
            tf_file("env/locals.tf.prod", "locals {\n  prefix = \"b\"\n}\n"),
            tf_file(
                "env/extra.tf.simu",
                "output \"o\" {\n  value = local.prefix\n}\n",
            ),
        ]);
        assert!(refs(&nodes, &edges).is_empty(), "no cross-variant edge");
        assert_eq!(unresolved(&nodes, "output.o"), vec!["local.prefix"]);
    }

    #[test]
    fn a_definition_only_in_a_backup_file_is_not_a_definition() {
        let (nodes, edges) = run(vec![
            tf_file("m/vars.tf.bak", "variable \"x\" {}\n"),
            tf_file("m/main.tf", "output \"o\" {\n  value = var.x\n}\n"),
        ]);
        assert!(refs(&nodes, &edges).is_empty());
        assert_eq!(unresolved(&nodes, "output.o"), vec!["var.x"]);
    }
}

//! Cross-file resolution (spec §5.3): turns every file's raw extraction
//! output into graph `Symbol`/`Module` nodes and `contains`/`imports`/
//! `calls` edges. Whole-repo and single-threaded by necessity — a call's
//! resolution needs to see every other file's exported symbols before it
//! can be judged unambiguous (or not).
//!
//! Rust-specific interpretation of spec §5.3's language-agnostic rules
//! (`mod` -> file resolution, `use`-root -> internal/external
//! classification, "same-package" -> "same walked repo") is recorded in
//! `docs/adr/0008-rust-resolution-policy-mapping.md`.

use super::extractor::{ExtractOut, RawCallSite, RawImport, RawSymbol};
use crate::graph::{
    self, Confidence, Edge, EdgeKind, ModuleNode, Node, NodeId, SymKind, SymbolNode, UnresolvedCall,
};
use crate::taint::{Provenance, TaintedString};
use std::collections::{BTreeMap, BTreeSet};

/// One file's raw extraction, paired with the identifying info `resolve`
/// needs but [`super::LangExtractor::extract`] doesn't produce itself.
pub struct FileExtraction {
    pub file_id: NodeId,
    pub relpath: String,
    pub extract: ExtractOut,
    /// The producing extractor's [`super::LangExtractor::origin`] (spec
    /// §4.1's node `origin` field). Per-file rather than a single
    /// module-wide constant since `resolve` processes files from more
    /// than one extractor in a single pass (M1.b.2b).
    pub origin: &'static str,
}

pub struct ResolvedExtraction {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

pub fn resolve(extractions: Vec<FileExtraction>) -> ResolvedExtraction {
    // Every top-level `mod <name>;` declared anywhere — a `use` path
    // whose root matches one of these is treated as internal (spec §5.3
    // rule 1's "package imports become Module nodes" only applies to the
    // ones that don't). `Relative` with non-empty `imported_names` is
    // Python's shape (`from .pkg import a`), not Rust's `mod` shape, so
    // it's excluded here — Rust's `mod foo;` always has empty
    // `imported_names` (see `RawImport::Relative`'s doc comment).
    let known_modules: BTreeSet<&str> = extractions
        .iter()
        .flat_map(|fe| &fe.extract.imports)
        .filter_map(|imp| match imp {
            RawImport::Relative {
                module_path,
                imported_names,
                ..
            } if imported_names.is_empty() => Some(module_path.as_str()),
            _ => None,
        })
        .collect();

    let relpath_to_file_id: BTreeMap<&str, &NodeId> = extractions
        .iter()
        .map(|fe| (fe.relpath.as_str(), &fe.file_id))
        .collect();

    // PHP's `use App\Orders\Order;` (ADR-0012, `RawImport::Qualified`) —
    // a fully-qualified name with no path semantics, resolved against a
    // repo-wide index instead of a directory walk. `known_namespace_roots`
    // is the first `\`-segment of every file's declared namespace (a
    // `use` whose root matches one is internal-but-maybe-unresolvable,
    // same "internal, no guessing" honesty as an unresolvable `mod`).
    // `fqn_to_file` maps each *top-level* declaration's fully-qualified
    // name (namespace + bare name; methods excluded — a `use` never
    // names a method) to its declaring file. A file with no `namespace`
    // declaration contributes its bare names under PHP's global
    // namespace. A colliding key (two files illegally declaring the same
    // FQN) keeps whichever file is encountered first rather than
    // erroring — carto only reads source, it doesn't enforce PHP's own
    // rules.
    let known_namespace_roots: BTreeSet<&str> = extractions
        .iter()
        .filter_map(|fe| fe.extract.declared_namespace.as_deref())
        .filter_map(|ns| ns.split('\\').next())
        .filter(|s| !s.is_empty())
        .collect();

    let mut fqn_to_file: BTreeMap<String, &NodeId> = BTreeMap::new();
    for fe in &extractions {
        let ns = fe.extract.declared_namespace.as_deref().unwrap_or("");
        for sym in &fe.extract.symbols {
            if matches!(sym.sym_kind, SymKind::Method) {
                continue;
            }
            let fqn = if ns.is_empty() {
                sym.name.clone()
            } else {
                format!("{ns}\\{}", sym.name)
            };
            fqn_to_file.entry(fqn).or_insert(&fe.file_id);
        }
    }

    // Each symbol's stable ID, indexed the same way as
    // `extractions[i].extract.symbols[j]` so the two stay in lockstep.
    let symbol_ids: Vec<Vec<NodeId>> = extractions
        .iter()
        .map(|fe| {
            fe.extract
                .symbols
                .iter()
                .map(|s| {
                    graph::sym_id(
                        &fe.relpath,
                        s.sym_kind.as_str(),
                        &s.qualified_name,
                        s.start_line,
                    )
                })
                .collect()
        })
        .collect();

    // Spec §5.3 rule 2 tier (a): same-file, any visibility.
    let same_file_by_name: Vec<BTreeMap<&str, usize>> = extractions
        .iter()
        .map(|fe| {
            fe.extract
                .symbols
                .iter()
                .enumerate()
                .map(|(si, s)| (s.name.as_str(), si))
                .collect()
        })
        .collect();

    // Spec §5.3 rule 2 tiers (b)/(c): every *exported* symbol, by bare
    // name, repo-wide — deliberately not scoped further per-file, since
    // v1 doesn't walk `use` paths deep enough to verify a specific import
    // resolves to a specific file (see ADR-0008). A name with more than
    // one exported candidate is ambiguous for both tiers; only an exactly-
    // one match resolves (INV-8: no guessing).
    let mut pub_by_name: BTreeMap<&str, Vec<(usize, usize)>> = BTreeMap::new();
    for (fi, fe) in extractions.iter().enumerate() {
        for (si, sym) in fe.extract.symbols.iter().enumerate() {
            if sym.is_pub {
                pub_by_name
                    .entry(sym.name.as_str())
                    .or_default()
                    .push((fi, si));
            }
        }
    }

    // Bound-name -> declared-name, per file — tier (b)'s precondition
    // *and* its resolution key in a single map, not two independently-
    // matched strings the way an `imported_names` set + a same-string
    // `pub_by_name.get` used to be. A call site's own identifier is
    // checked against this map's keys; a hit's *value* is what's
    // actually looked up in `pub_by_name` below — the same string only
    // for an unaliased import (`bound_name == declared_name`), which is
    // exactly what makes this alias-aware without changing behavior for
    // every import that isn't aliased. `Relative`/`Absolute` carry
    // `ImportedName` pairs directly; `Qualified`'s FQN's own last
    // segment already *is* the declared name, so nothing extra is
    // needed there. See
    // `docs/adr/0013-typescript-javascript-resolution-policy-mapping.md`.
    let alias_to_declared: Vec<BTreeMap<&str, &str>> = extractions
        .iter()
        .map(|fe| {
            let mut m = BTreeMap::new();
            for imp in &fe.extract.imports {
                match imp {
                    RawImport::Relative { imported_names, .. }
                    | RawImport::Absolute { imported_names, .. } => {
                        for n in imported_names {
                            m.insert(n.bound_name.as_str(), n.declared_name.as_str());
                        }
                    }
                    RawImport::Qualified { fqn, bound_name } => {
                        let declared = fqn.rsplit('\\').next().unwrap_or(fqn.as_str());
                        m.insert(bound_name.as_str(), declared);
                    }
                }
            }
            m
        })
        .collect();

    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut external_modules: BTreeMap<&str, NodeId> = BTreeMap::new();

    for (fi, fe) in extractions.iter().enumerate() {
        let calls_by_symbol =
            assign_calls_to_innermost_symbol(&fe.extract.symbols, &fe.extract.call_sites);

        for (si, sym) in fe.extract.symbols.iter().enumerate() {
            let id = symbol_ids[fi][si].clone();

            let mut unresolved_calls = Vec::new();
            for &call in &calls_by_symbol[si] {
                match resolve_call(
                    fi,
                    call,
                    &same_file_by_name,
                    &alias_to_declared,
                    &pub_by_name,
                ) {
                    Some((target_fi, target_si, evidence)) => {
                        let to_id = symbol_ids[target_fi][target_si].clone();
                        edges.push(Edge::new(
                            EdgeKind::Calls,
                            id.clone(),
                            to_id,
                            Confidence::Inferred,
                            evidence.to_string(),
                        ));
                    }
                    None => unresolved_calls.push(UnresolvedCall {
                        name: call.callee_name.clone(),
                        line: call.line,
                    }),
                }
            }

            let signature = if sym.signature.trim().is_empty() {
                None
            } else {
                Some(TaintedString::new(&sym.signature, Provenance::Syntactic))
            };

            nodes.push(Node::symbol(
                id.clone(),
                Provenance::Syntactic,
                fe.origin,
                SymbolNode {
                    name: sym.name.clone(),
                    sym_kind: sym.sym_kind,
                    file: fe.file_id.clone(),
                    start_line: sym.start_line,
                    end_line: sym.end_line,
                    signature,
                    unresolved_calls,
                },
            ));

            edges.push(Edge::new(
                EdgeKind::Contains,
                fe.file_id.clone(),
                id,
                Confidence::Certain,
                fe.origin.to_string(),
            ));
        }

        for imp in &fe.extract.imports {
            match imp {
                RawImport::Relative {
                    levels_up,
                    module_path,
                    imported_names,
                } => {
                    if imported_names.is_empty() {
                        // Rust's `mod foo;` shape: the import refers to
                        // the module file itself, not a name within it.
                        if let Some(target) = resolve_relative_import(
                            &fe.relpath,
                            *levels_up,
                            module_path,
                            &relpath_to_file_id,
                        ) {
                            edges.push(Edge::new(
                                EdgeKind::Imports,
                                fe.file_id.clone(),
                                target.clone(),
                                Confidence::Certain,
                                "mod-declaration".to_string(),
                            ));
                        }
                    } else if !module_path.is_empty() {
                        // Python's `from .pkg import a, b` — the module
                        // itself is the file-level target; one edge,
                        // certain per spec rule 1, regardless of how
                        // many names are drawn from it.
                        if let Some(target) = resolve_relative_import(
                            &fe.relpath,
                            *levels_up,
                            module_path,
                            &relpath_to_file_id,
                        ) {
                            edges.push(Edge::new(
                                EdgeKind::Imports,
                                fe.file_id.clone(),
                                target.clone(),
                                Confidence::Certain,
                                "relative-import".to_string(),
                            ));
                        }
                    } else {
                        // Python's `from . import pkg[, pkg2]` — each
                        // name IS itself a submodule to resolve relative
                        // to the current directory (no separate
                        // `module_path` to anchor on). Resolved by
                        // `declared_name` (the real submodule/file name),
                        // not `bound_name` — an alias (`from . import
                        // pkg as p`) renames the local binding, not the
                        // file on disk.
                        for name in imported_names {
                            if let Some(target) = resolve_relative_import(
                                &fe.relpath,
                                *levels_up,
                                &name.declared_name,
                                &relpath_to_file_id,
                            ) {
                                edges.push(Edge::new(
                                    EdgeKind::Imports,
                                    fe.file_id.clone(),
                                    target.clone(),
                                    Confidence::Certain,
                                    "relative-import".to_string(),
                                ));
                            }
                        }
                    }
                }
                RawImport::Absolute { root, .. } => {
                    // `known_modules` only ever contains Rust `mod`
                    // names (see its own doc comment) — Python has no
                    // walked-repo equivalent (no explicit module
                    // declaration to collect), so every Python absolute
                    // import is classified external here, even when it
                    // actually names a local top-level package. A known
                    // v1 simplification (ADR-0011), analogous to but
                    // distinct from Rust's own "same-package = same
                    // walked repo" one.
                    let is_internal = root == "crate"
                        || root == "self"
                        || root == "super"
                        || known_modules.contains(root.as_str());
                    if is_internal {
                        continue;
                    }
                    let module_id = external_modules
                        .entry(root.as_str())
                        .or_insert_with(|| {
                            let id = graph::module_id(root, true);
                            nodes.push(Node::module(
                                id.clone(),
                                Provenance::Syntactic,
                                fe.origin,
                                ModuleNode {
                                    path: root.clone(),
                                    external: true,
                                },
                            ));
                            id
                        })
                        .clone();
                    edges.push(Edge::new(
                        EdgeKind::Imports,
                        fe.file_id.clone(),
                        module_id,
                        Confidence::Certain,
                        "external-package".to_string(),
                    ));
                }
                RawImport::Qualified { fqn, .. } => {
                    // PHP's `use App\Orders\Order;` (ADR-0012) — resolved
                    // against the repo-wide FQN index built above, not a
                    // directory walk. Three outcomes, in order:
                    let fqn = fqn.trim_start_matches('\\');
                    if let Some(&target) = fqn_to_file.get(fqn) {
                        // 1. Exact FQN match: certain edge to the
                        //    declaring file.
                        edges.push(Edge::new(
                            EdgeKind::Imports,
                            fe.file_id.clone(),
                            target.clone(),
                            Confidence::Certain,
                            "namespace-import".to_string(),
                        ));
                    } else if fqn
                        .split('\\')
                        .next()
                        .is_some_and(|root| known_namespace_roots.contains(root))
                    {
                        // 2. Internal namespace root, but no file
                        //    declares this exact FQN: honest omission
                        //    (INV-8) rather than a guess — no edge, no
                        //    node.
                    } else {
                        // 3. Unknown root: an external package, same
                        //    dedup-by-root treatment as `Absolute`.
                        let root = fqn.split('\\').next().unwrap_or(fqn);
                        let module_id = external_modules
                            .entry(root)
                            .or_insert_with(|| {
                                let id = graph::module_id(root, true);
                                nodes.push(Node::module(
                                    id.clone(),
                                    Provenance::Syntactic,
                                    fe.origin,
                                    ModuleNode {
                                        path: root.to_string(),
                                        external: true,
                                    },
                                ));
                                id
                            })
                            .clone();
                        edges.push(Edge::new(
                            EdgeKind::Imports,
                            fe.file_id.clone(),
                            module_id,
                            Confidence::Certain,
                            "external-package".to_string(),
                        ));
                    }
                }
            }
        }
    }

    ResolvedExtraction { nodes, edges }
}

/// Assigns each call site to the *innermost* symbol whose line range
/// contains it, rather than every symbol whose range does. A
/// class/interface/trait/enum symbol's own range spans its methods'
/// bodies too (PHP, Python — a Rust `struct`/`impl` doesn't have this
/// shape, since an `impl` block itself is never a symbol), so naive
/// per-symbol containment would attribute a method's call to both the
/// method *and* its enclosing class: two `calls` edges (or two
/// `unresolved_calls` entries) for what is textually one call site —
/// caught by eyeballing real `fixtures/php-app` output (ADR-0012), not
/// by a unit test on an isolated snippet, since `fixtures/py-lib`
/// happens to have no call site inside a class body's methods and never
/// exercised this. Ties (two symbols with identical ranges) fall back to
/// whichever is encountered first — doesn't arise from any current
/// extractor, which never emits two symbols with identical ranges.
fn assign_calls_to_innermost_symbol<'a>(
    symbols: &[RawSymbol],
    call_sites: &'a [RawCallSite],
) -> Vec<Vec<&'a RawCallSite>> {
    let mut by_symbol: Vec<Vec<&RawCallSite>> = vec![Vec::new(); symbols.len()];
    for call in call_sites {
        let mut best: Option<usize> = None;
        for (si, sym) in symbols.iter().enumerate() {
            if call.line < sym.start_line || call.line > sym.end_line {
                continue; // not textually inside this symbol
            }
            let smaller = match best {
                None => true,
                Some(b) => {
                    (sym.end_line - sym.start_line) < (symbols[b].end_line - symbols[b].start_line)
                }
            };
            if smaller {
                best = Some(si);
            }
        }
        if let Some(si) = best {
            by_symbol[si].push(call);
        }
    }
    by_symbol
}

/// Spec §5.3 rule 2, in order, first match wins: (a) same-file, (b)
/// imported into the file, (c) same-package (v1: same walked repo).
/// Returns `(target_file_index, target_symbol_index, evidence)` or
/// `None` if no tier produced exactly one candidate.
fn resolve_call(
    caller_file_idx: usize,
    call: &RawCallSite,
    same_file_by_name: &[BTreeMap<&str, usize>],
    alias_to_declared: &[BTreeMap<&str, &str>],
    pub_by_name: &BTreeMap<&str, Vec<(usize, usize)>>,
) -> Option<(usize, usize, &'static str)> {
    let name = call.callee_name.as_str();

    if let Some(&si) = same_file_by_name[caller_file_idx].get(name) {
        return Some((caller_file_idx, si, "same-file"));
    }

    // Alias-aware: `name` is what the call site spells; the map's
    // value (if any) is the *declared* name `pub_by_name` is keyed by
    // — the same string for an unaliased import, a different one for
    // `import { foo as bar }`/`from x import foo as bar`/`use Foo as
    // X;`. See `ImportedName`'s doc comment.
    if let Some(&declared) = alias_to_declared[caller_file_idx].get(name) {
        if let Some(candidates) = pub_by_name.get(declared) {
            if let [(fi, si)] = candidates[..] {
                return Some((fi, si, "imported"));
            }
        }
    }

    if let Some(candidates) = pub_by_name.get(name) {
        let mut others = candidates.iter().filter(|(fi, _)| *fi != caller_file_idx);
        if let (Some(&(fi, si)), None) = (others.next(), others.next()) {
            return Some((fi, si, "same-package"));
        }
    }

    None
}

/// Resolves a relative import (Rust's `mod <name>;`, `levels_up` always
/// 0; Python's `from <dots><module_path> import ...`, `levels_up` = dot
/// count; TS/JS's `import x from '<dots><module_path>'`, `levels_up` =
/// leading `../` count — ADR-0013) to a sibling/ancestor file. Walks
/// `levels_up` directories up from `declaring_relpath`'s own directory,
/// then looks for `<dir>/<module_path>.<ext>` or
/// `<dir>/<module_path>/<package-marker>` — the common case in each
/// language (`#[path]` overrides, Python namespace packages, and
/// TS/JS's real bundler/tsconfig-driven resolution order are all out of
/// scope, spec §5.3 "deliberately modest"). Which candidate suffixes to
/// try is inferred from `declaring_relpath`'s own extension: a `mod`
/// declaration only ever appears in a `.rs` file, a Python relative
/// import only ever in a `.py` file, and a TS/JS one only ever in a
/// `.ts`/`.tsx`/`.js`/`.jsx`/`.mts`/`.cts`/`.mjs`/`.cjs` file — so the
/// declaring file's extension is sufficient, no need to thread the
/// caller's language through separately. String-joined rather than
/// `std::path::Path`-joined: repo-relative paths are always
/// `/`-separated regardless of host OS (spec §4.1), and `Path::join` on
/// Windows would introduce a `\`-separated key that can't match the
/// `/`-keyed lookup table built from `walk`'s output.
fn resolve_relative_import<'a>(
    declaring_relpath: &str,
    levels_up: u32,
    module_path: &str,
    relpath_to_file_id: &BTreeMap<&str, &'a NodeId>,
) -> Option<&'a NodeId> {
    let mut dir = declaring_relpath
        .rsplit_once('/')
        .map(|(d, _)| d)
        .unwrap_or("");
    for _ in 0..levels_up {
        dir = dir.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    }

    let ext = declaring_relpath.rsplit_once('.').map(|(_, e)| e);
    let candidates: Vec<String> = match ext {
        Some("py") => vec![
            format!("{module_path}.py"),
            format!("{module_path}/__init__.py"),
        ],
        // TS-first priority order (the user-facing language this
        // extractor was built to serve best), first match wins — real
        // bundler resolution can differ per tsconfig/bundler config,
        // out of scope. Tried regardless of the declaring file's own
        // exact extension within this family: a .ts file importing a
        // .jsx sibling (or vice versa) is common in mixed repos.
        Some("ts" | "tsx" | "js" | "jsx" | "mts" | "cts" | "mjs" | "cjs") => vec![
            format!("{module_path}.ts"),
            format!("{module_path}.tsx"),
            format!("{module_path}.js"),
            format!("{module_path}.jsx"),
            format!("{module_path}/index.ts"),
            format!("{module_path}/index.tsx"),
            format!("{module_path}/index.js"),
            format!("{module_path}/index.jsx"),
        ],
        _ => vec![format!("{module_path}.rs"), format!("{module_path}/mod.rs")],
    };

    candidates
        .iter()
        .find_map(|c| relpath_to_file_id.get(join(dir, c).as_str()))
        .copied()
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{NodeData, SymKind};
    use crate::lang::extractor::{ImportedName, RawSymbol};

    fn file(relpath: &str) -> FileExtraction {
        FileExtraction {
            file_id: graph::file_id(relpath),
            relpath: relpath.to_string(),
            extract: ExtractOut::default(),
            origin: "lang-rust@1",
        }
    }

    fn sym(name: &str, kind: SymKind, start: u32, end: u32, is_pub: bool) -> RawSymbol {
        RawSymbol {
            name: name.to_string(),
            qualified_name: name.to_string(),
            sym_kind: kind,
            start_line: start,
            end_line: end,
            signature: format!("fn {name}()"),
            is_pub,
        }
    }

    fn call(name: &str, line: u32) -> RawCallSite {
        RawCallSite {
            callee_name: name.to_string(),
            line,
        }
    }

    /// Test-only shorthand for an unaliased `ImportedName` — bound and
    /// declared name are the same string, the common case for these
    /// tests; the alias-specific tests build `ImportedName` directly.
    fn name(s: &str) -> ImportedName {
        ImportedName {
            bound_name: s.to_string(),
            declared_name: s.to_string(),
        }
    }

    fn find_symbol<'a>(nodes: &'a [Node], name: &str) -> &'a SymbolNode {
        nodes
            .iter()
            .find_map(|n| match &n.data {
                NodeData::Symbol(s) if s.name == name => Some(s),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no symbol node named {name}"))
    }

    fn calls_edges(edges: &[Edge]) -> Vec<&Edge> {
        edges.iter().filter(|e| e.kind == EdgeKind::Calls).collect()
    }

    fn imports_edges(edges: &[Edge]) -> Vec<&Edge> {
        edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Imports)
            .collect()
    }

    #[test]
    fn same_file_call_resolves_with_same_file_evidence() {
        let mut f = file("src/orders.rs");
        f.extract.symbols = vec![
            sym("parse_order", SymKind::Function, 1, 5, true),
            sym("validate", SymKind::Function, 7, 9, false),
        ];
        f.extract.call_sites = vec![call("validate", 3)];

        let out = resolve(vec![f]);
        assert!(
            find_symbol(&out.nodes, "parse_order")
                .unresolved_calls
                .is_empty()
        );

        let edges = calls_edges(&out.edges);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].confidence, Confidence::Inferred);
        assert_eq!(edges[0].evidence, vec!["same-file".to_string()]);
    }

    #[test]
    fn imported_call_resolves_via_use_when_unique() {
        let mut handlers = file("src/handlers.rs");
        handlers.extract.imports = vec![RawImport::Absolute {
            root: "crate".into(),
            imported_names: vec![name("parse_order")],
        }];
        handlers.extract.symbols = vec![sym("handle", SymKind::Function, 1, 5, true)];
        handlers.extract.call_sites = vec![call("parse_order", 3)];

        let mut orders = file("src/orders.rs");
        orders.extract.symbols = vec![sym("parse_order", SymKind::Function, 1, 3, true)];

        let out = resolve(vec![handlers, orders]);
        assert!(
            find_symbol(&out.nodes, "handle")
                .unresolved_calls
                .is_empty()
        );
        assert_eq!(
            calls_edges(&out.edges)[0].evidence,
            vec!["imported".to_string()]
        );
    }

    #[test]
    fn aliased_relative_import_resolves_a_call_written_as_the_alias() {
        // `from .orders import parse_order as po` — the fix this test
        // pins: before `ImportedName`/`alias_to_declared`, tier (b)
        // matched the call site's own identifier ("po") against
        // `pub_by_name`, which is keyed by the *declared* name
        // ("parse_order") — a guaranteed miss for any aliased import.
        // Shared machinery with the PHP case below (both go through
        // `resolve_call`'s single `alias_to_declared` lookup) — see
        // `docs/adr/0013-typescript-javascript-resolution-policy-mapping.md`.
        let mut handlers = file("src/handlers.py");
        handlers.origin = "lang-python@1";
        handlers.extract.imports = vec![RawImport::Relative {
            levels_up: 0,
            module_path: "orders".into(),
            imported_names: vec![ImportedName {
                bound_name: "po".into(),
                declared_name: "parse_order".into(),
            }],
        }];
        handlers.extract.symbols = vec![sym("handle", SymKind::Function, 1, 5, true)];
        handlers.extract.call_sites = vec![call("po", 3)];

        let mut orders = file("src/orders.py");
        orders.origin = "lang-python@1";
        orders.extract.symbols = vec![sym("parse_order", SymKind::Function, 1, 3, true)];

        let out = resolve(vec![handlers, orders]);
        assert!(
            find_symbol(&out.nodes, "handle")
                .unresolved_calls
                .is_empty()
        );
        assert_eq!(
            calls_edges(&out.edges)[0].evidence,
            vec!["imported".to_string()]
        );
    }

    #[test]
    fn aliased_qualified_import_resolves_a_call_written_as_the_alias() {
        // PHP's `use App\Orders\parseOrder as po;` — same fix, exercised
        // through `RawImport::Qualified` instead of `Relative`.
        let mut orders = file("src/Orders.php");
        orders.origin = "lang-php@1";
        orders.extract.declared_namespace = Some("App\\Orders".into());
        orders.extract.symbols = vec![sym("parseOrder", SymKind::Function, 1, 3, true)];

        let mut handlers = file("src/Handlers.php");
        handlers.origin = "lang-php@1";
        handlers.extract.imports = vec![RawImport::Qualified {
            fqn: "App\\Orders\\parseOrder".into(),
            bound_name: "po".into(),
        }];
        handlers.extract.symbols = vec![sym("handle", SymKind::Function, 5, 8, true)];
        handlers.extract.call_sites = vec![call("po", 6)];

        let out = resolve(vec![handlers, orders]);
        assert!(
            find_symbol(&out.nodes, "handle")
                .unresolved_calls
                .is_empty()
        );
        assert_eq!(
            calls_edges(&out.edges)[0].evidence,
            vec!["imported".to_string()]
        );
    }

    #[test]
    fn same_package_call_resolves_when_unimported_and_unique() {
        let mut handlers = file("src/handlers.rs");
        handlers.extract.symbols = vec![sym("handle", SymKind::Function, 1, 5, true)];
        handlers.extract.call_sites = vec![call("audit_order", 3)];

        let mut orders = file("src/orders.rs");
        orders.extract.symbols = vec![sym("audit_order", SymKind::Function, 1, 3, true)];

        let out = resolve(vec![handlers, orders]);
        assert!(
            find_symbol(&out.nodes, "handle")
                .unresolved_calls
                .is_empty()
        );
        assert_eq!(
            calls_edges(&out.edges)[0].evidence,
            vec!["same-package".to_string()]
        );
    }

    #[test]
    fn ambiguous_same_package_candidates_produce_no_edge() {
        let mut caller = file("src/a.rs");
        caller.extract.symbols = vec![sym("run", SymKind::Function, 1, 5, true)];
        caller.extract.call_sites = vec![call("helper", 3)];

        let mut b = file("src/b.rs");
        b.extract.symbols = vec![sym("helper", SymKind::Function, 1, 2, true)];
        let mut c = file("src/c.rs");
        c.extract.symbols = vec![sym("helper", SymKind::Function, 1, 2, true)];

        let out = resolve(vec![caller, b, c]);
        let run = find_symbol(&out.nodes, "run");
        assert_eq!(run.unresolved_calls.len(), 1);
        assert_eq!(run.unresolved_calls[0].name, "helper");
        assert!(calls_edges(&out.edges).is_empty());
    }

    #[test]
    fn unknown_callee_is_recorded_as_unresolved() {
        let mut f = file("src/a.rs");
        f.extract.symbols = vec![sym("run", SymKind::Function, 1, 5, true)];
        f.extract.call_sites = vec![call("mystery", 3)];

        let out = resolve(vec![f]);
        let run = find_symbol(&out.nodes, "run");
        assert_eq!(
            run.unresolved_calls,
            vec![UnresolvedCall {
                name: "mystery".to_string(),
                line: 3
            }]
        );
    }

    #[test]
    fn mod_declaration_resolves_to_sibling_file() {
        let mut lib = file("src/lib.rs");
        lib.extract.imports = vec![RawImport::Relative {
            levels_up: 0,
            module_path: "orders".into(),
            imported_names: vec![],
        }];
        let orders = file("src/orders.rs");

        let out = resolve(vec![lib, orders]);
        let imports = imports_edges(&out.edges);
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].confidence, Confidence::Certain);
        assert_eq!(imports[0].evidence, vec!["mod-declaration".to_string()]);
    }

    #[test]
    fn mod_declaration_resolves_to_mod_rs_in_subdirectory() {
        let mut lib = file("src/lib.rs");
        lib.extract.imports = vec![RawImport::Relative {
            levels_up: 0,
            module_path: "orders".into(),
            imported_names: vec![],
        }];
        let orders = file("src/orders/mod.rs");

        let out = resolve(vec![lib, orders]);
        assert_eq!(imports_edges(&out.edges).len(), 1);
    }

    #[test]
    fn unresolvable_mod_declaration_produces_no_edge() {
        let mut lib = file("src/lib.rs");
        lib.extract.imports = vec![RawImport::Relative {
            levels_up: 0,
            module_path: "missing".into(),
            imported_names: vec![],
        }];

        let out = resolve(vec![lib]);
        assert!(imports_edges(&out.edges).is_empty());
    }

    #[test]
    fn python_relative_import_with_named_symbols_resolves_the_module_file() {
        // `from .orders import parse_order, validate` in pkg/handlers.py
        // -- one dot means "the current package" (levels_up: 0), so
        // `orders` resolves as a same-directory sibling of handlers.py.
        let mut python_file = file("pkg/handlers.py");
        python_file.origin = "lang-python@1";
        python_file.extract.imports = vec![RawImport::Relative {
            levels_up: 0,
            module_path: "orders".into(),
            imported_names: vec![name("parse_order"), name("validate")],
        }];
        let orders = file("pkg/orders.py");

        let out = resolve(vec![python_file, orders]);
        let imports = imports_edges(&out.edges);
        assert_eq!(imports.len(), 1); // one edge to the module, not one per name
        assert_eq!(imports[0].confidence, Confidence::Certain);
        assert_eq!(imports[0].evidence, vec!["relative-import".to_string()]);
    }

    #[test]
    fn python_relative_import_walks_up_one_directory_for_two_dots() {
        // `from ..orders import parse_order` in pkg/sub/handlers.py --
        // two dots means "the parent package" (levels_up: 1): walk up
        // from pkg/sub/ to pkg/, then find orders.py there.
        let mut python_file = file("pkg/sub/handlers.py");
        python_file.origin = "lang-python@1";
        python_file.extract.imports = vec![RawImport::Relative {
            levels_up: 1,
            module_path: "orders".into(),
            imported_names: vec![name("parse_order")],
        }];
        let orders = file("pkg/orders.py");

        let out = resolve(vec![python_file, orders]);
        assert_eq!(imports_edges(&out.edges).len(), 1);
    }

    #[test]
    fn python_bare_relative_submodule_import_resolves_each_name_as_a_submodule() {
        // `from . import a, b` -- each imported name IS itself a
        // submodule to resolve relative to the current directory
        // (no separate module_path to anchor on).
        let mut python_file = file("pkg/__init__.py");
        python_file.origin = "lang-python@1";
        python_file.extract.imports = vec![RawImport::Relative {
            levels_up: 0,
            module_path: "".into(),
            imported_names: vec![name("orders"), name("handlers")],
        }];
        let orders = file("pkg/orders.py");
        let handlers = file("pkg/handlers.py");

        let out = resolve(vec![python_file, orders, handlers]);
        assert_eq!(imports_edges(&out.edges).len(), 2);
    }

    #[test]
    fn php_qualified_import_resolves_via_fqn_index() {
        // `use App\Orders\Order;` in Handlers.php, matched against
        // Orders.php's `namespace App\Orders; class Order { .. }` — the
        // repo-wide FQN index (ADR-0012), not a directory walk.
        let mut orders = file("src/Orders.php");
        orders.origin = "lang-php@1";
        orders.extract.declared_namespace = Some("App\\Orders".into());
        orders.extract.symbols = vec![sym("Order", SymKind::Class, 1, 5, true)];

        let mut handlers = file("src/Handlers.php");
        handlers.origin = "lang-php@1";
        handlers.extract.imports = vec![RawImport::Qualified {
            fqn: "App\\Orders\\Order".into(),
            bound_name: "Order".into(),
        }];

        let out = resolve(vec![handlers, orders]);
        let imports = imports_edges(&out.edges);
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].confidence, Confidence::Certain);
        assert_eq!(imports[0].evidence, vec!["namespace-import".to_string()]);
    }

    #[test]
    fn php_qualified_import_with_known_root_but_no_fqn_match_produces_no_edge() {
        // `App` is a known namespace root (some file declares `namespace
        // App\Orders;`), but no file declares exactly `App\Missing\Thing`
        // -- internal but unresolvable, honest omission (INV-8), not an
        // external module.
        let mut orders = file("src/Orders.php");
        orders.origin = "lang-php@1";
        orders.extract.declared_namespace = Some("App\\Orders".into());
        orders.extract.symbols = vec![sym("Order", SymKind::Class, 1, 5, true)];

        let mut handlers = file("src/Handlers.php");
        handlers.origin = "lang-php@1";
        handlers.extract.imports = vec![RawImport::Qualified {
            fqn: "App\\Missing\\Thing".into(),
            bound_name: "Thing".into(),
        }];

        let out = resolve(vec![handlers, orders]);
        assert!(imports_edges(&out.edges).is_empty());
        assert!(
            out.nodes
                .iter()
                .all(|n| !matches!(n.data, NodeData::Module(_)))
        );
    }

    #[test]
    fn php_qualified_import_with_unknown_root_produces_external_module_node() {
        let mut handlers = file("src/Handlers.php");
        handlers.origin = "lang-php@1";
        handlers.extract.imports = vec![RawImport::Qualified {
            fqn: "Psr\\Log\\LoggerInterface".into(),
            bound_name: "LoggerInterface".into(),
        }];

        let out = resolve(vec![handlers]);
        let module_nodes: Vec<&ModuleNode> = out
            .nodes
            .iter()
            .filter_map(|n| match &n.data {
                NodeData::Module(m) => Some(m),
                _ => None,
            })
            .collect();
        assert_eq!(module_nodes.len(), 1);
        assert_eq!(module_nodes[0].path, "Psr");
        assert!(module_nodes[0].external);

        let imports = imports_edges(&out.edges);
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].confidence, Confidence::Certain);
        assert_eq!(imports[0].evidence, vec!["external-package".to_string()]);
    }

    #[test]
    fn php_qualified_import_resolves_call_via_tier_b() {
        // `use App\Orders\parseOrder;` (unaliased) makes `parseOrder`
        // resolve via tier (b) exactly like Rust's `use` / Python's
        // `from .. import` — the unaliased baseline, `bound_name ==
        // declared_name`, still works after the `alias_to_declared`
        // rework. The *aliased* case (`use ... as X`), once a
        // documented gap here, is now covered by
        // `aliased_qualified_import_resolves_a_call_written_as_the_alias`.
        let mut orders = file("src/Orders.php");
        orders.origin = "lang-php@1";
        orders.extract.declared_namespace = Some("App\\Orders".into());
        orders.extract.symbols = vec![sym("parseOrder", SymKind::Function, 1, 3, true)];

        let mut handlers = file("src/Handlers.php");
        handlers.origin = "lang-php@1";
        handlers.extract.imports = vec![RawImport::Qualified {
            fqn: "App\\Orders\\parseOrder".into(),
            bound_name: "parseOrder".into(),
        }];
        handlers.extract.symbols = vec![sym("handle", SymKind::Function, 5, 8, true)];
        handlers.extract.call_sites = vec![call("parseOrder", 6)];

        let out = resolve(vec![handlers, orders]);
        assert!(
            find_symbol(&out.nodes, "handle")
                .unresolved_calls
                .is_empty()
        );
        assert_eq!(
            calls_edges(&out.edges)[0].evidence,
            vec!["imported".to_string()]
        );
    }

    #[test]
    fn external_use_produces_one_deduped_module_node_with_edges_from_each_file() {
        let mut a = file("src/a.rs");
        a.extract.imports = vec![RawImport::Absolute {
            root: "serde".into(),
            imported_names: vec![name("Deserialize")],
        }];
        let mut b = file("src/b.rs");
        b.extract.imports = vec![RawImport::Absolute {
            root: "serde".into(),
            imported_names: vec![name("Serialize")],
        }];

        let out = resolve(vec![a, b]);
        let module_nodes: Vec<&ModuleNode> = out
            .nodes
            .iter()
            .filter_map(|n| match &n.data {
                NodeData::Module(m) => Some(m),
                _ => None,
            })
            .collect();
        assert_eq!(module_nodes.len(), 1);
        assert_eq!(module_nodes[0].path, "serde");
        assert!(module_nodes[0].external);

        let imports = imports_edges(&out.edges);
        assert_eq!(imports.len(), 2);
        assert!(imports.iter().all(|e| e.confidence == Confidence::Certain));
    }

    #[test]
    fn crate_prefixed_use_does_not_produce_module_node_or_edge() {
        let mut f = file("src/handlers.rs");
        f.extract.imports = vec![RawImport::Absolute {
            root: "crate".into(),
            imported_names: vec![name("parse_order")],
        }];

        let out = resolve(vec![f]);
        assert!(
            out.nodes
                .iter()
                .all(|n| !matches!(n.data, NodeData::Module(_)))
        );
        assert!(imports_edges(&out.edges).is_empty());
    }

    #[test]
    fn known_local_module_use_does_not_produce_module_node() {
        let mut lib = file("src/lib.rs");
        lib.extract.imports = vec![RawImport::Relative {
            levels_up: 0,
            module_path: "orders".into(),
            imported_names: vec![],
        }];
        let mut handlers = file("src/handlers.rs");
        handlers.extract.imports = vec![RawImport::Absolute {
            root: "orders".into(),
            imported_names: vec![name("parse_order")],
        }];

        let out = resolve(vec![lib, handlers]);
        assert!(
            out.nodes
                .iter()
                .all(|n| !matches!(n.data, NodeData::Module(_)))
        );
    }

    #[test]
    fn every_symbol_gets_a_contains_edge_from_its_file() {
        let mut f = file("src/orders.rs");
        f.extract.symbols = vec![sym("parse_order", SymKind::Function, 1, 3, true)];

        let out = resolve(vec![f]);
        let contains: Vec<&Edge> = out
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Contains)
            .collect();
        assert_eq!(contains.len(), 1);
        assert_eq!(contains[0].confidence, Confidence::Certain);
    }
}

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

use super::extractor::{ExtractOut, RawCallSite, RawImport};
use crate::graph::{
    self, Confidence, Edge, EdgeKind, ModuleNode, Node, NodeId, SymbolNode, UnresolvedCall,
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

    // Which bare names this file's imports mention (tier b's
    // precondition — still needs a unique `pub_by_name` match to
    // resolve). Both variants carry `imported_names`; Rust's `Relative`
    // (a `mod` declaration) always has it empty, so only `Absolute`
    // (`use`) contributes for Rust today — same resulting set as
    // before this field became `Vec<String>` instead of
    // `Option<String>`.
    let imported_names: Vec<BTreeSet<&str>> = extractions
        .iter()
        .map(|fe| {
            fe.extract
                .imports
                .iter()
                .flat_map(|imp| match imp {
                    RawImport::Relative { imported_names, .. } => imported_names.iter(),
                    RawImport::Absolute { imported_names, .. } => imported_names.iter(),
                })
                .map(|s| s.as_str())
                .collect()
        })
        .collect();

    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut external_modules: BTreeMap<&str, NodeId> = BTreeMap::new();

    for (fi, fe) in extractions.iter().enumerate() {
        for (si, sym) in fe.extract.symbols.iter().enumerate() {
            let id = symbol_ids[fi][si].clone();

            let mut unresolved_calls = Vec::new();
            for call in &fe.extract.call_sites {
                if call.line < sym.start_line || call.line > sym.end_line {
                    continue; // not textually inside this symbol
                }
                match resolve_call(fi, call, &same_file_by_name, &imported_names, &pub_by_name) {
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
                        // `module_path` to anchor on).
                        for name in imported_names {
                            if let Some(target) = resolve_relative_import(
                                &fe.relpath,
                                *levels_up,
                                name,
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
            }
        }
    }

    ResolvedExtraction { nodes, edges }
}

/// Spec §5.3 rule 2, in order, first match wins: (a) same-file, (b)
/// imported into the file, (c) same-package (v1: same walked repo).
/// Returns `(target_file_index, target_symbol_index, evidence)` or
/// `None` if no tier produced exactly one candidate.
fn resolve_call(
    caller_file_idx: usize,
    call: &RawCallSite,
    same_file_by_name: &[BTreeMap<&str, usize>],
    imported_names: &[BTreeSet<&str>],
    pub_by_name: &BTreeMap<&str, Vec<(usize, usize)>>,
) -> Option<(usize, usize, &'static str)> {
    let name = call.callee_name.as_str();

    if let Some(&si) = same_file_by_name[caller_file_idx].get(name) {
        return Some((caller_file_idx, si, "same-file"));
    }

    if imported_names[caller_file_idx].contains(name) {
        if let Some(candidates) = pub_by_name.get(name) {
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
/// count) to a sibling/ancestor file. Walks `levels_up` directories up
/// from `declaring_relpath`'s own directory, then looks for
/// `<dir>/<module_path>.<ext>` or `<dir>/<module_path>/<package-marker>`
/// — the common case in either language (`#[path]` overrides, Python
/// namespace packages, and other edition/interpreter-version corner
/// cases are out of scope, spec §5.3 "deliberately modest"). Which
/// suffix pair to try is inferred from `declaring_relpath`'s own
/// extension: a `mod` declaration only ever appears in a `.rs` file, and
/// a Python relative import only ever appears in a `.py` file, so the
/// declaring file's extension is sufficient — no need to thread the
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
    let candidates = match ext {
        Some("py") => [
            format!("{module_path}.py"),
            format!("{module_path}/__init__.py"),
        ],
        _ => [format!("{module_path}.rs"), format!("{module_path}/mod.rs")],
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
    use crate::lang::extractor::RawSymbol;

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
            imported_names: vec!["parse_order".into()],
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
            imported_names: vec!["parse_order".into(), "validate".into()],
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
            imported_names: vec!["parse_order".into()],
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
            imported_names: vec!["orders".into(), "handlers".into()],
        }];
        let orders = file("pkg/orders.py");
        let handlers = file("pkg/handlers.py");

        let out = resolve(vec![python_file, orders, handlers]);
        assert_eq!(imports_edges(&out.edges).len(), 2);
    }

    #[test]
    fn external_use_produces_one_deduped_module_node_with_edges_from_each_file() {
        let mut a = file("src/a.rs");
        a.extract.imports = vec![RawImport::Absolute {
            root: "serde".into(),
            imported_names: vec!["Deserialize".into()],
        }];
        let mut b = file("src/b.rs");
        b.extract.imports = vec![RawImport::Absolute {
            root: "serde".into(),
            imported_names: vec!["Serialize".into()],
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
            imported_names: vec!["parse_order".into()],
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
            imported_names: vec!["parse_order".into()],
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

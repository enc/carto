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
}

pub struct ResolvedExtraction {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

/// The extractor name + version recorded on every `Symbol`/`Module` node
/// this pass produces (spec §4.1's `origin` field).
const ORIGIN: &str = "lang-rust@1";

pub fn resolve(extractions: Vec<FileExtraction>) -> ResolvedExtraction {
    // Every top-level `mod <name>;` declared anywhere — a `use` path
    // whose root matches one of these is treated as internal (spec §5.3
    // rule 1's "package imports become Module nodes" only applies to the
    // ones that don't).
    let known_modules: BTreeSet<&str> = extractions
        .iter()
        .flat_map(|fe| &fe.extract.imports)
        .filter_map(|imp| match imp {
            RawImport::ModDecl { name } => Some(name.as_str()),
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

    // Which bare names this file's `use` declarations mention (tier b's
    // precondition — still needs a unique `pub_by_name` match to resolve).
    let imported_names: Vec<BTreeSet<&str>> = extractions
        .iter()
        .map(|fe| {
            fe.extract
                .imports
                .iter()
                .filter_map(|imp| match imp {
                    RawImport::UseDecl {
                        imported_name: Some(n),
                        ..
                    } => Some(n.as_str()),
                    _ => None,
                })
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
                ORIGIN,
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
                "extractor:rust".to_string(),
            ));
        }

        for imp in &fe.extract.imports {
            match imp {
                RawImport::ModDecl { name } => {
                    if let Some(target) = resolve_mod_decl(&fe.relpath, name, &relpath_to_file_id) {
                        edges.push(Edge::new(
                            EdgeKind::Imports,
                            fe.file_id.clone(),
                            target.clone(),
                            Confidence::Certain,
                            "mod-declaration".to_string(),
                        ));
                    }
                }
                RawImport::UseDecl { root, .. } => {
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
                                ORIGIN,
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
                        "external-crate".to_string(),
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

/// Resolves a `mod <name>;` declaration in `declaring_relpath` to a
/// sibling file: `<dir>/<name>.rs` or `<dir>/<name>/mod.rs` (the common
/// case — `#[path]` overrides and other edition-2018+ corner cases are
/// out of scope, spec §5.3 "deliberately modest"). String-joined rather
/// than `std::path::Path`-joined: repo-relative paths are always
/// `/`-separated regardless of host OS (spec §4.1), and `Path::join` on
/// Windows would introduce a `\`-separated key that can't match the
/// `/`-keyed lookup table built from `walk`'s output.
fn resolve_mod_decl<'a>(
    declaring_relpath: &str,
    mod_name: &str,
    relpath_to_file_id: &BTreeMap<&str, &'a NodeId>,
) -> Option<&'a NodeId> {
    let dir = declaring_relpath
        .rsplit_once('/')
        .map(|(d, _)| d)
        .unwrap_or("");
    let candidate_a = join(dir, &format!("{mod_name}.rs"));
    let candidate_b = join(dir, &format!("{mod_name}/mod.rs"));
    relpath_to_file_id
        .get(candidate_a.as_str())
        .or_else(|| relpath_to_file_id.get(candidate_b.as_str()))
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
        handlers.extract.imports = vec![RawImport::UseDecl {
            root: "crate".into(),
            imported_name: Some("parse_order".into()),
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
        lib.extract.imports = vec![RawImport::ModDecl {
            name: "orders".into(),
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
        lib.extract.imports = vec![RawImport::ModDecl {
            name: "orders".into(),
        }];
        let orders = file("src/orders/mod.rs");

        let out = resolve(vec![lib, orders]);
        assert_eq!(imports_edges(&out.edges).len(), 1);
    }

    #[test]
    fn unresolvable_mod_declaration_produces_no_edge() {
        let mut lib = file("src/lib.rs");
        lib.extract.imports = vec![RawImport::ModDecl {
            name: "missing".into(),
        }];

        let out = resolve(vec![lib]);
        assert!(imports_edges(&out.edges).is_empty());
    }

    #[test]
    fn external_use_produces_one_deduped_module_node_with_edges_from_each_file() {
        let mut a = file("src/a.rs");
        a.extract.imports = vec![RawImport::UseDecl {
            root: "serde".into(),
            imported_name: Some("Deserialize".into()),
        }];
        let mut b = file("src/b.rs");
        b.extract.imports = vec![RawImport::UseDecl {
            root: "serde".into(),
            imported_name: Some("Serialize".into()),
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
        f.extract.imports = vec![RawImport::UseDecl {
            root: "crate".into(),
            imported_name: Some("parse_order".into()),
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
        lib.extract.imports = vec![RawImport::ModDecl {
            name: "orders".into(),
        }];
        let mut handlers = file("src/handlers.rs");
        handlers.extract.imports = vec![RawImport::UseDecl {
            root: "orders".into(),
            imported_name: Some("parse_order".into()),
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

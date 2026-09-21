//! `carto where <name>` (spec §7.1: "symbol name (substring/exact)" ->
//! "matches: id, kind, `file:line`, signature"). Named `find` here
//! because `where` is a Rust keyword — the CLI subcommand stays `where`;
//! only the core function and this module are renamed.
//!
//! Also matches [`crate::graph::ModuleNode::path`] (ADR-0021) — a
//! `Module` node's ID is a blake3 hash with no other reachable lookup
//! path (`deps` requires a node ID or an exact symbol name), so without
//! this a package/module was unreachable from the tool surface entirely
//! (spec §7.1 names "symbol name" for `where`, but a purely additive
//! second list alongside it doesn't take that away — see
//! `docs/STATUS.md`'s note on this). `File` nodes are deliberately not
//! searched here — `deps`'s target resolution (not `find`) is where a
//! `File.path` becomes reachable, since a file is a traversal starting
//! point, not a `where`-style name lookup spec §7.1 describes.

use super::{QueryGraph, Truncation};
use crate::graph::{NodeData, NodeId, SymKind};
use crate::taint::{Provenance, TaintedString};
use serde::{Deserialize, Serialize};

/// Default `--limit` when the caller doesn't specify one.
pub const DEFAULT_LIMIT: usize = 50;

#[derive(Debug, Clone)]
pub struct FindQuery {
    pub needle: String,
    /// `false` (default): case-insensitive substring. `true`: exact,
    /// case-sensitive — spec §7.1 says "substring/exact" without
    /// specifying case; case-insensitive substring is the
    /// agent-friendly default, `--exact` the precise escape hatch.
    pub exact: bool,
    pub limit: usize,
    /// Restrict matches to symbols under this repo-relative directory
    /// (spec §7.1's `--subpath`) — see
    /// [`QueryGraph::path_in_scope`]. `None` means no restriction.
    pub subpath: Option<String>,
}

impl FindQuery {
    pub fn new(needle: impl Into<String>) -> Self {
        FindQuery {
            needle: needle.into(),
            exact: false,
            limit: DEFAULT_LIMIT,
            subpath: None,
        }
    }
}

/// One matched symbol (spec §7.1's `where` output row).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolMatch {
    pub id: NodeId,
    pub name: String,
    pub sym_kind: SymKind,
    /// `path:start-end` (spec §7.2).
    pub location: String,
    /// Stays tainted all the way out — the CLI/MCP renderer holds the
    /// only decision about how to surface it (§8.4's fence for human
    /// output, the plain capped string for `--json`).
    pub signature: Option<TaintedString>,
    /// From the node itself, not from `signature`'s own (possibly
    /// re-tagged-on-deserialize) provenance — see
    /// `crate::graph::load`'s round-trip test for why those can differ.
    pub provenance: Provenance,
}

/// One matched module (ADR-0021) — a separate row shape from
/// `SymbolMatch` rather than folding into it, since a module has no
/// `sym_kind`/`location`/`signature` to answer with.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleMatch {
    pub id: NodeId,
    pub path: String,
    /// Whether this module is external (a package carto never parsed)
    /// or an internal package reference — [`crate::graph::ModuleNode::
    /// external`], surfaced since it changes what a caller can do next
    /// with the ID (an internal module has real edges to traverse via
    /// `deps`; an external one is a leaf).
    pub external: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindResult {
    pub matches: Vec<SymbolMatch>,
    /// Modules whose `path` matched the same needle/exact/subpath rule
    /// as `matches` (ADR-0021). `limit`/truncation apply to `matches`
    /// and `module_matches` combined — symbols first, modules filling
    /// whatever's left — rather than each list independently capped at
    /// `limit`, so the response's total size stays bounded the same way
    /// it always has.
    pub module_matches: Vec<ModuleMatch>,
    pub truncation: Truncation,
}

pub fn run(qg: &QueryGraph, query: &FindQuery) -> FindResult {
    let needle_lower = query.needle.to_lowercase();

    let mut matches: Vec<SymbolMatch> = Vec::new();
    let mut module_matches: Vec<ModuleMatch> = Vec::new();
    for node in qg.nodes() {
        match &node.data {
            NodeData::Symbol(sym) => {
                let is_match = if query.exact {
                    sym.name == query.needle
                } else {
                    sym.name.to_lowercase().contains(&needle_lower)
                };
                if !is_match || !qg.path_in_scope(&node.id, query.subpath.as_deref()) {
                    continue;
                }
                matches.push(SymbolMatch {
                    id: node.id.clone(),
                    name: sym.name.clone(),
                    sym_kind: sym.sym_kind,
                    location: qg.location(sym),
                    signature: sym.signature.clone(),
                    provenance: node.provenance,
                });
            }
            NodeData::Module(m) => {
                let is_match = if query.exact {
                    m.path == query.needle
                } else {
                    m.path.to_lowercase().contains(&needle_lower)
                };
                // `path_in_scope` always returns `true` for a `Module`
                // node (ADR-0014: packages have no directory) — called
                // anyway for symmetry with the symbol arm above, not
                // because it can filter anything here.
                if !is_match || !qg.path_in_scope(&node.id, query.subpath.as_deref()) {
                    continue;
                }
                module_matches.push(ModuleMatch {
                    id: node.id.clone(),
                    path: m.path.clone(),
                    external: m.external,
                });
            }
            // Contract nodes have their own lookup surface (`carto
            // contract`/`orphans`, ADR-0026) rather than folding into
            // `where`'s symbol/module matching.
            NodeData::File(_) | NodeData::Contract(_) => {}
        }
    }

    // Deterministic order, not a relevance score — an unexplained score
    // ordering is exactly the kind of silent inference INV-8 exists to
    // avoid.
    matches.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| a.location.cmp(&b.location))
    });
    module_matches.sort_by(|a, b| a.path.cmp(&b.path));

    let total = matches.len() + module_matches.len();
    let truncated = total > query.limit;
    matches.truncate(query.limit);
    let remaining = query.limit.saturating_sub(matches.len());
    module_matches.truncate(remaining);

    let truncation = if truncated {
        Truncation::more(format!(
            "carto where {} --limit {}",
            query.needle,
            query.limit * 2
        ))
    } else {
        Truncation::none()
    };

    FindResult {
        matches,
        module_matches,
        truncation,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{FileNode, GraphDocument, Node, SymbolNode};
    use crate::lang::Lang;

    fn doc_with_symbols(names: &[&str]) -> GraphDocument {
        let file = Node::file(
            crate::graph::file_id("src/lib.rs"),
            Provenance::Syntactic,
            "test@1",
            FileNode {
                path: "src/lib.rs".to_string(),
                lang: Lang::Rust,
                size: 0,
                sha256: None,
                skipped: None,
                excluded: None,
            },
        );
        let mut nodes = vec![file.clone()];
        for (i, name) in names.iter().enumerate() {
            nodes.push(Node::symbol(
                crate::graph::sym_id("src/lib.rs", "function", name, i as u32 + 1),
                Provenance::Syntactic,
                "test@1",
                SymbolNode {
                    name: name.to_string(),
                    sym_kind: SymKind::Function,
                    file: file.id.clone(),
                    start_line: i as u32 + 1,
                    end_line: i as u32 + 2,
                    signature: None,
                    unresolved_calls: vec![],
                    uncaptured_inbound_calls: 0,
                    uncaptured_outbound_calls: 0,
                    unresolved_inbound_calls: vec![],
                    unresolved_inbound_call_count: 0,
                },
            ));
        }
        GraphDocument {
            carto_version: "0.1.0".to_string(),
            schema_version: crate::consts::SCHEMA_VERSION,
            nodes,
            edges: vec![],
        }
    }

    /// A same-named symbol declared in two different files/directories
    /// — `mg_site/orders.rs`, the "live" tree, and
    /// `typo3_v8_delete_me/orders.rs`, a stand-in for vendored/dead
    /// code — for `--subpath` tests.
    fn doc_with_same_name_in_two_dirs() -> GraphDocument {
        let live = Node::file(
            crate::graph::file_id("mg_site/orders.rs"),
            Provenance::Syntactic,
            "test@1",
            FileNode {
                path: "mg_site/orders.rs".to_string(),
                lang: Lang::Rust,
                size: 0,
                sha256: None,
                skipped: None,
                excluded: None,
            },
        );
        let dead = Node::file(
            crate::graph::file_id("typo3_v8_delete_me/orders.rs"),
            Provenance::Syntactic,
            "test@1",
            FileNode {
                path: "typo3_v8_delete_me/orders.rs".to_string(),
                lang: Lang::Rust,
                size: 0,
                sha256: None,
                skipped: None,
                excluded: None,
            },
        );
        let live_sym = Node::symbol(
            crate::graph::sym_id("mg_site/orders.rs", "function", "Div", 1),
            Provenance::Syntactic,
            "test@1",
            SymbolNode {
                name: "Div".to_string(),
                sym_kind: SymKind::Function,
                file: live.id.clone(),
                start_line: 1,
                end_line: 2,
                signature: None,
                unresolved_calls: vec![],
                uncaptured_inbound_calls: 0,
                uncaptured_outbound_calls: 0,
                unresolved_inbound_calls: vec![],
                unresolved_inbound_call_count: 0,
            },
        );
        let dead_sym = Node::symbol(
            crate::graph::sym_id("typo3_v8_delete_me/orders.rs", "function", "Div", 1),
            Provenance::Syntactic,
            "test@1",
            SymbolNode {
                name: "Div".to_string(),
                sym_kind: SymKind::Function,
                file: dead.id.clone(),
                start_line: 1,
                end_line: 2,
                signature: None,
                unresolved_calls: vec![],
                uncaptured_inbound_calls: 0,
                uncaptured_outbound_calls: 0,
                unresolved_inbound_calls: vec![],
                unresolved_inbound_call_count: 0,
            },
        );
        GraphDocument {
            carto_version: "0.1.0".to_string(),
            schema_version: crate::consts::SCHEMA_VERSION,
            nodes: vec![live, dead, live_sym, dead_sym],
            edges: vec![],
        }
    }

    #[test]
    fn subpath_restricts_matches_to_the_given_directory() {
        let qg = QueryGraph::from_document(doc_with_same_name_in_two_dirs());
        let query = FindQuery {
            needle: "Div".to_string(),
            exact: true,
            limit: DEFAULT_LIMIT,
            subpath: Some("mg_site".to_string()),
        };
        let result = run(&qg, &query);
        assert_eq!(result.matches.len(), 1);
        assert!(result.matches[0].location.starts_with("mg_site/"));
    }

    #[test]
    fn subpath_does_not_match_a_sibling_directory_with_a_similar_prefix() {
        // "mg_site" must not match "mg_siteXYZ/..." — segment-boundary
        // safety, not a bare string prefix check.
        let mut doc = doc_with_same_name_in_two_dirs();
        for node in &mut doc.nodes {
            if let crate::graph::NodeData::File(f) = &mut node.data {
                if f.path == "typo3_v8_delete_me/orders.rs" {
                    f.path = "mg_siteXYZ/orders.rs".to_string();
                }
            }
        }
        let qg = QueryGraph::from_document(doc);
        let query = FindQuery {
            needle: "Div".to_string(),
            exact: true,
            limit: DEFAULT_LIMIT,
            subpath: Some("mg_site".to_string()),
        };
        let result = run(&qg, &query);
        assert_eq!(result.matches.len(), 1);
    }

    #[test]
    fn subpath_none_is_unrestricted() {
        let qg = QueryGraph::from_document(doc_with_same_name_in_two_dirs());
        let result = run(&qg, &FindQuery::new("Div"));
        assert_eq!(result.matches.len(), 2);
    }

    #[test]
    fn substring_match_is_case_insensitive_by_default() {
        let qg = QueryGraph::from_document(doc_with_symbols(&["parse_order", "validate"]));
        let result = run(&qg, &FindQuery::new("ORDER"));
        assert_eq!(result.matches.len(), 1);
        assert_eq!(result.matches[0].name, "parse_order");
    }

    #[test]
    fn exact_match_is_case_sensitive() {
        let qg = QueryGraph::from_document(doc_with_symbols(&["parse_order"]));
        let mut query = FindQuery::new("Parse_Order");
        query.exact = true;
        assert!(run(&qg, &query).matches.is_empty());

        let query = FindQuery {
            needle: "parse_order".to_string(),
            exact: true,
            limit: DEFAULT_LIMIT,
            subpath: None,
        };
        assert_eq!(run(&qg, &query).matches.len(), 1);
    }

    #[test]
    fn results_are_sorted_by_name_then_location() {
        let qg = QueryGraph::from_document(doc_with_symbols(&["zeta", "alpha", "beta"]));
        let result = run(&qg, &FindQuery::new(""));
        let names: Vec<&str> = result.matches.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta", "zeta"]);
    }

    #[test]
    fn limit_truncates_and_reports_a_follow_up_call() {
        let qg = QueryGraph::from_document(doc_with_symbols(&["a", "b", "c"]));
        let query = FindQuery {
            needle: "".to_string(),
            exact: false,
            limit: 2,
            subpath: None,
        };
        let result = run(&qg, &query);
        assert_eq!(result.matches.len(), 2);
        assert!(result.truncation.truncated);
        assert!(result.truncation.next_call.is_some());
    }

    #[test]
    fn no_match_is_not_truncated() {
        let qg = QueryGraph::from_document(doc_with_symbols(&["a"]));
        let result = run(&qg, &FindQuery::new("nonexistent"));
        assert!(result.matches.is_empty());
        assert!(!result.truncation.truncated);
    }

    fn doc_with_module(path: &str, external: bool) -> GraphDocument {
        GraphDocument {
            carto_version: "0.1.0".to_string(),
            schema_version: crate::consts::SCHEMA_VERSION,
            nodes: vec![Node::module(
                crate::graph::module_id(path, external),
                Provenance::Syntactic,
                "test@1",
                crate::graph::ModuleNode {
                    path: path.to_string(),
                    external,
                },
            )],
            edges: vec![],
        }
    }

    #[test]
    fn module_path_matches_by_substring_and_carries_external_flag() {
        let qg = QueryGraph::from_document(doc_with_module("carto_core", true));
        let result = run(&qg, &FindQuery::new("carto"));
        assert!(result.matches.is_empty());
        assert_eq!(result.module_matches.len(), 1);
        assert_eq!(result.module_matches[0].path, "carto_core");
        assert!(result.module_matches[0].external);
    }

    #[test]
    fn module_path_exact_match_is_case_sensitive() {
        let qg = QueryGraph::from_document(doc_with_module("carto_core", false));
        let mut query = FindQuery::new("Carto_Core");
        query.exact = true;
        assert!(run(&qg, &query).module_matches.is_empty());

        let query = FindQuery {
            needle: "carto_core".to_string(),
            exact: true,
            limit: DEFAULT_LIMIT,
            subpath: None,
        };
        assert_eq!(run(&qg, &query).module_matches.len(), 1);
    }

    #[test]
    fn module_matches_are_never_excluded_by_subpath() {
        // ADR-0014: modules have no directory, so --subpath never
        // filters them out directly.
        let qg = QueryGraph::from_document(doc_with_module("carto_core", true));
        let query = FindQuery {
            needle: "carto_core".to_string(),
            exact: true,
            limit: DEFAULT_LIMIT,
            subpath: Some("some/unrelated/dir".to_string()),
        };
        assert_eq!(run(&qg, &query).module_matches.len(), 1);
    }

    #[test]
    fn symbols_and_modules_share_a_combined_limit_symbols_first() {
        // Two symbols + one module all match "x", limit 2: symbols win
        // the shared budget, the module is dropped and truncation fires.
        let mut doc = doc_with_symbols(&["x1", "x2"]);
        let module_doc = doc_with_module("x_pkg", true);
        doc.nodes.extend(module_doc.nodes);
        let qg = QueryGraph::from_document(doc);

        let query = FindQuery {
            needle: "x".to_string(),
            exact: false,
            limit: 2,
            subpath: None,
        };
        let result = run(&qg, &query);
        assert_eq!(result.matches.len(), 2);
        assert!(result.module_matches.is_empty());
        assert!(result.truncation.truncated);
    }

    #[test]
    fn symbols_and_modules_both_fit_under_a_generous_combined_limit() {
        let mut doc = doc_with_symbols(&["x1"]);
        let module_doc = doc_with_module("x_pkg", true);
        doc.nodes.extend(module_doc.nodes);
        let qg = QueryGraph::from_document(doc);

        let result = run(&qg, &FindQuery::new("x"));
        assert_eq!(result.matches.len(), 1);
        assert_eq!(result.module_matches.len(), 1);
        assert!(!result.truncation.truncated);
    }
}

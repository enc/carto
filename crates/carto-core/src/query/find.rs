//! `carto where <name>` (spec §7.1: "symbol name (substring/exact)" ->
//! "matches: id, kind, `file:line`, signature"). Named `find` here
//! because `where` is a Rust keyword — the CLI subcommand stays `where`;
//! only the core function and this module are renamed.
//!
//! Matches [`crate::graph::SymbolNode::name`] only — `File`/`Module`
//! nodes are not searched. Spec §7.1 names "symbol name" specifically;
//! broadening this to files/modules is a real decision left to a future
//! milestone, not an oversight (recorded in `docs/STATUS.md`).

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindResult {
    pub matches: Vec<SymbolMatch>,
    pub truncation: Truncation,
}

pub fn run(qg: &QueryGraph, query: &FindQuery) -> FindResult {
    let needle_lower = query.needle.to_lowercase();

    let mut matches: Vec<SymbolMatch> = Vec::new();
    for node in qg.nodes() {
        let NodeData::Symbol(sym) = &node.data else {
            continue;
        };
        let is_match = if query.exact {
            sym.name == query.needle
        } else {
            sym.name.to_lowercase().contains(&needle_lower)
        };
        if !is_match {
            continue;
        }
        if !qg.path_in_scope(&node.id, query.subpath.as_deref()) {
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

    // Deterministic order, not a relevance score — an unexplained score
    // ordering is exactly the kind of silent inference INV-8 exists to
    // avoid.
    matches.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| a.location.cmp(&b.location))
    });

    let truncated = matches.len() > query.limit;
    matches.truncate(query.limit);

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
}

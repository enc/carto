//! `carto contract <value>` — not spec §7.1's command set (ADR-0026):
//! every producer and consumer of a categorised literal, exact-value
//! matched. The companion read to [`super::orphans`]: where `orphans`
//! answers "what's missing," `contract` answers "who else touches this
//! one value" (SID Cloud spec classes 1–6's "for X: who reads/writes
//! it").

use super::{Direction, QueryGraph, Truncation};
use crate::consts;
use crate::graph::{Confidence, Edge, EdgeKind, NodeData, NodeId};
use crate::taint::{Provenance, TaintedString};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const DEFAULT_LIMIT: usize = 50;

#[derive(Debug, Clone)]
pub struct ContractQuery {
    /// Exact value to match — contracts are literal identifiers, not
    /// free text, so (unlike `where`) there's no substring mode.
    pub value: String,
    /// `None`: match this value under every category. `Some`: only the
    /// named category — needed since two different categories could
    /// coincidentally share a spelling (an env var and a metric name
    /// both called `X`).
    pub category: Option<String>,
    pub limit: usize,
    /// Restrict *listed producer/consumer sites* to these components
    /// (ADR-0034/0035, `--component`, repeatable) — see
    /// [`QueryGraph::component_in_scope`]. Deliberately does **not**
    /// drop a whole `ContractMatch` when it has no in-scope sites left —
    /// cross-component contract joins are the entire point of this
    /// capability (ADR-0026), so a match losing all its rows under a
    /// filter is itself informative, not something to hide. `None` or
    /// empty means no restriction.
    pub component: Option<BTreeSet<String>>,
}

/// One site producing or consuming a `Contract` — the `Symbol`/`File`
/// node at a `produces`/`consumes` edge's `from` end.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractSite {
    pub node_id: NodeId,
    /// Symbol name or file path, whichever the node kind has.
    pub label: String,
    /// `path:start-end`, present only for a `Symbol` site.
    pub location: Option<String>,
    pub confidence: Confidence,
    pub evidence: Vec<String>,
    /// The component (ADR-0034/0035) this site's owning file belongs
    /// to, `None` if it's under no recognized project root — which
    /// component produces vs. consumes is exactly the question a
    /// cross-component contract join exists to answer.
    pub component: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractMatch {
    pub contract_id: NodeId,
    pub category: String,
    pub qualifier: Option<String>,
    pub value: String,
    pub producers: Vec<ContractSite>,
    pub consumers: Vec<ContractSite>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractResult {
    pub matches: Vec<ContractMatch>,
    pub truncation: Truncation,
}

pub fn run(qg: &QueryGraph, query: &ContractQuery) -> ContractResult {
    // Compares rendered (sanitized) content, not `TaintedString`'s own
    // derived `PartialEq` — that also compares `provenance`, and every
    // `TaintedString` loaded from a real `graph.json` round-trip is
    // re-tagged `Ingested` on deserialize (`taint.rs`'s documented
    // behavior), regardless of what it was indexed with. A query-time
    // needle has no such provenance to match, so provenance-sensitive
    // equality would silently never match real indexed data.
    let needle = TaintedString::new(&query.value, Provenance::Syntactic).render_capped(usize::MAX);

    let mut matches: Vec<ContractMatch> = Vec::new();
    for node in qg.nodes() {
        let NodeData::Contract(c) = &node.data else {
            continue;
        };
        if let Some(cat) = &query.category {
            if &c.category != cat {
                continue;
            }
        }
        if c.value.render_capped(usize::MAX) != needle {
            continue;
        }

        let incoming = qg.neighbors(&node.id, Direction::In);
        let producers = sites(qg, &incoming, EdgeKind::Produces, query.component.as_ref());
        let consumers = sites(qg, &incoming, EdgeKind::Consumes, query.component.as_ref());

        matches.push(ContractMatch {
            contract_id: node.id.clone(),
            category: c.category.clone(),
            qualifier: c
                .qualifier
                .as_ref()
                .map(|q| q.render_capped(consts::SIGNATURE_CAP)),
            value: c.value.render_capped(consts::SIGNATURE_CAP),
            producers,
            consumers,
        });
    }

    matches.sort_by(|a, b| a.contract_id.as_str().cmp(b.contract_id.as_str()));

    let truncated = matches.len() > query.limit;
    matches.truncate(query.limit);
    let truncation = if truncated {
        let component_flags = QueryGraph::component_flags(query.component.as_ref());
        Truncation::more(format!(
            "carto contract {} --limit {}{component_flags}",
            query.value,
            query.limit * 2
        ))
    } else {
        Truncation::none()
    };

    ContractResult {
        matches,
        truncation,
    }
}

fn sites(
    qg: &QueryGraph,
    incoming: &[&Edge],
    kind: EdgeKind,
    component_filter: Option<&BTreeSet<String>>,
) -> Vec<ContractSite> {
    let mut out: Vec<ContractSite> = incoming
        .iter()
        .filter(|e| e.kind == kind)
        .filter(|e| qg.component_in_scope(&e.from, component_filter))
        .filter_map(|e| {
            let from = qg.node(&e.from)?;
            let (label, location) = match &from.data {
                NodeData::File(f) => (f.path.clone(), None),
                NodeData::Symbol(s) => (s.name.clone(), Some(qg.location(s))),
                // A `produces`/`consumes` edge's `from` is always a
                // Symbol or File (`resolve.rs`'s only two attachment
                // points) — a Module/Contract source here would mean
                // the graph itself is malformed; skip rather than
                // fabricate a row for it.
                NodeData::Module(_) | NodeData::Contract(_) => return None,
            };
            Some(ContractSite {
                node_id: e.from.clone(),
                label,
                location,
                confidence: e.confidence,
                evidence: e.evidence.clone(),
                component: qg.component_of(&e.from).map(str::to_string),
            })
        })
        .collect();
    out.sort_by(|a, b| a.node_id.as_str().cmp(b.node_id.as_str()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{ContractNode, FileNode, GraphDocument, Node};
    use crate::lang::Lang;

    fn doc_with_producer_and_consumer() -> GraphDocument {
        let cs_file = Node::file(
            crate::graph::file_id("Emitter.cs"),
            Provenance::Syntactic,
            "test@1",
            FileNode {
                path: "Emitter.cs".to_string(),
                lang: Lang::CSharp,
                size: 0,
                sha256: None,
                skipped: None,
                excluded: None,
                component: None,
            },
        );
        let tf_file = Node::file(
            crate::graph::file_id("alarms.tf"),
            Provenance::Syntactic,
            "test@1",
            FileNode {
                path: "alarms.tf".to_string(),
                lang: Lang::Hcl,
                size: 0,
                sha256: None,
                skipped: None,
                excluded: None,
                component: None,
            },
        );
        let contract_id = crate::graph::contract_id(
            "metric_name",
            Some("SIDCloud/Ingest"),
            "QuoteDropsPerSecond",
        );
        let contract = Node::contract(
            contract_id.clone(),
            Provenance::Syntactic,
            "lang-hcl@1",
            ContractNode {
                category: "metric_name".to_string(),
                qualifier: Some(TaintedString::new("SIDCloud/Ingest", Provenance::Syntactic)),
                value: TaintedString::new("QuoteDropsPerSecond", Provenance::Syntactic),
            },
        );
        let produces = Edge::new(
            EdgeKind::Produces,
            cs_file.id.clone(),
            contract_id.clone(),
            Confidence::Inferred,
            "csharp-object-init:object-init:Name".to_string(),
        );
        let consumes = Edge::new(
            EdgeKind::Consumes,
            tf_file.id.clone(),
            contract_id,
            Confidence::Certain,
            "hcl-attr:aws_cloudwatch_metric_alarm.metric_name".to_string(),
        );
        GraphDocument {
            carto_version: "0.1.0".to_string(),
            schema_version: crate::consts::SCHEMA_VERSION,
            components: vec![],
            nodes: vec![cs_file, tf_file, contract],
            edges: vec![produces, consumes],
        }
    }

    #[test]
    fn finds_both_sides_of_a_matched_contract() {
        let qg = QueryGraph::from_document(doc_with_producer_and_consumer());
        let result = run(
            &qg,
            &ContractQuery {
                value: "QuoteDropsPerSecond".to_string(),
                category: None,
                limit: DEFAULT_LIMIT,
                component: None,
            },
        );
        assert_eq!(result.matches.len(), 1);
        let m = &result.matches[0];
        assert_eq!(m.producers.len(), 1);
        assert_eq!(m.consumers.len(), 1);
        assert_eq!(m.producers[0].label, "Emitter.cs");
        assert_eq!(m.consumers[0].label, "alarms.tf");
    }

    #[test]
    fn category_filter_excludes_non_matching_categories() {
        let qg = QueryGraph::from_document(doc_with_producer_and_consumer());
        let result = run(
            &qg,
            &ContractQuery {
                value: "QuoteDropsPerSecond".to_string(),
                category: Some("env_var".to_string()),
                limit: DEFAULT_LIMIT,
                component: None,
            },
        );
        assert!(result.matches.is_empty());
    }

    /// Regression guard: matching must survive a real `graph.json`
    /// round-trip, where `TaintedString::deserialize` always re-tags
    /// content `Provenance::Ingested` regardless of how it was indexed —
    /// a naive `TaintedString == TaintedString` comparison (which also
    /// compares provenance) would silently stop matching anything real
    /// while still passing an in-memory-only test.
    #[test]
    fn matches_after_a_real_json_round_trip() {
        let json = serde_json::to_vec(&doc_with_producer_and_consumer()).unwrap();
        let doc: GraphDocument = serde_json::from_slice(&json).unwrap();
        let qg = QueryGraph::from_document(doc);
        let result = run(
            &qg,
            &ContractQuery {
                value: "QuoteDropsPerSecond".to_string(),
                category: None,
                limit: DEFAULT_LIMIT,
                component: None,
            },
        );
        assert_eq!(result.matches.len(), 1);
    }

    #[test]
    fn unmatched_value_is_empty_not_an_error() {
        let qg = QueryGraph::from_document(doc_with_producer_and_consumer());
        let result = run(
            &qg,
            &ContractQuery {
                value: "NoSuchMetric".to_string(),
                category: None,
                limit: DEFAULT_LIMIT,
                component: None,
            },
        );
        assert!(result.matches.is_empty());
        assert!(!result.truncation.truncated);
    }
}

//! `carto orphans` — not spec §7.1's command set (ADR-0026). The
//! acceptance-test command: every `Contract` node with a `consumes`
//! edge but no `produces` edge (SID Cloud spec's five dead CloudWatch
//! alarms — a metric name referenced by an alarm that no service ever
//! emits), and the mirror case (produced but never consumed). Orphan
//! status is well-defined because `resolve.rs` never creates a
//! `Contract` node without at least one `produces`/`consumes` edge —
//! there is no third "neither" case to handle.

use super::{Direction, QueryGraph, Truncation};
use crate::consts;
use crate::graph::{EdgeKind, NodeData, NodeId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const DEFAULT_LIMIT: usize = 200;

#[derive(Debug, Clone)]
pub struct OrphansQuery {
    /// `None`: every category. `Some`: only this one (the acceptance
    /// test's `--category metric_name`).
    pub category: Option<String>,
    pub limit: usize,
    /// Restrict the report to orphan contracts with at least one
    /// producer/consumer site in one of these components
    /// (ADR-0034/0035, `--component`, repeatable) — "which of my
    /// component's contracts are orphaned," a legitimate question for a
    /// service owner in a monorepo. Unlike `contract`'s own
    /// `--component` (which filters sites, never a whole match), this
    /// filters the *contract itself* out of the report, since an
    /// orphan's defining fact (no producer, or no consumer, anywhere)
    /// is inherently repo-wide, not a per-site listing. `None` or empty
    /// means no restriction.
    pub component: Option<BTreeSet<String>>,
}

impl OrphansQuery {
    pub fn new() -> Self {
        OrphansQuery {
            category: None,
            limit: DEFAULT_LIMIT,
            component: None,
        }
    }
}

impl Default for OrphansQuery {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrphanContract {
    pub contract_id: NodeId,
    pub category: String,
    pub qualifier: Option<String>,
    pub value: String,
    /// Every distinct component (ADR-0034/0035) among this contract's
    /// producer/consumer sites, sorted. A component-less site
    /// contributes nothing here (not a `None` entry) — this is a
    /// positive list of "which components touch this," not a row per
    /// site. Usually one entry (the orphan's own producer or consumer
    /// component), but can be several — e.g. a metric several services
    /// each emit, none of which is the one alarm consuming it.
    pub components: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrphansResult {
    /// Has a `consumes` edge, no `produces` edge — referenced but never
    /// emitted/declared (the dead-alarm shape).
    pub consumed_never_produced: Vec<OrphanContract>,
    /// Has a `produces` edge, no `consumes` edge — emitted/declared but
    /// never referenced.
    pub produced_never_consumed: Vec<OrphanContract>,
    pub truncation: Truncation,
}

pub fn run(qg: &QueryGraph, query: &OrphansQuery) -> OrphansResult {
    let mut consumed_never_produced: Vec<OrphanContract> = Vec::new();
    let mut produced_never_consumed: Vec<OrphanContract> = Vec::new();

    for node in qg.nodes() {
        let NodeData::Contract(c) = &node.data else {
            continue;
        };
        if let Some(cat) = &query.category {
            if &c.category != cat {
                continue;
            }
        }

        let incoming = qg.neighbors(&node.id, Direction::In);
        let has_producer = incoming.iter().any(|e| e.kind == EdgeKind::Produces);
        let has_consumer = incoming.iter().any(|e| e.kind == EdgeKind::Consumes);

        if has_producer && has_consumer {
            continue;
        }

        let components: BTreeSet<String> = incoming
            .iter()
            .filter(|e| e.kind == EdgeKind::Produces || e.kind == EdgeKind::Consumes)
            .filter_map(|e| qg.component_of(&e.from).map(str::to_string))
            .collect();
        if let Some(filter) = &query.component {
            // ADR-0038: descendant-aware — a plain `is_disjoint` check
            // would miss a touching component that's nested *under* a
            // filter entry rather than named by it exactly.
            if !filter.is_empty()
                && !components
                    .iter()
                    .any(|c| qg.component_matches_filter(c, filter))
            {
                continue;
            }
        }

        let entry = OrphanContract {
            contract_id: node.id.clone(),
            category: c.category.clone(),
            qualifier: c
                .qualifier
                .as_ref()
                .map(|q| q.render_capped(consts::SIGNATURE_CAP)),
            value: c.value.render_capped(consts::SIGNATURE_CAP),
            components: components.into_iter().collect(),
        };

        if has_consumer {
            consumed_never_produced.push(entry);
        } else if has_producer {
            produced_never_consumed.push(entry);
        }
        // Neither: architecturally unreachable — see module doc.
    }

    consumed_never_produced.sort_by(|a, b| a.contract_id.as_str().cmp(b.contract_id.as_str()));
    produced_never_consumed.sort_by(|a, b| a.contract_id.as_str().cmp(b.contract_id.as_str()));

    let a_truncated = consumed_never_produced.len() > query.limit;
    consumed_never_produced.truncate(query.limit);
    let b_truncated = produced_never_consumed.len() > query.limit;
    produced_never_consumed.truncate(query.limit);

    let truncation = if a_truncated || b_truncated {
        let component_flags = QueryGraph::component_flags(query.component.as_ref());
        Truncation::more(format!(
            "carto orphans --limit {}{component_flags}",
            query.limit * 2
        ))
    } else {
        Truncation::none()
    };

    OrphansResult {
        consumed_never_produced,
        produced_never_consumed,
        truncation,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Confidence, ContractNode, Edge, FileNode, GraphDocument, Node};
    use crate::lang::Lang;
    use crate::taint::{Provenance, TaintedString};

    fn file(path: &str, lang: Lang) -> Node {
        Node::file(
            crate::graph::file_id(path),
            Provenance::Syntactic,
            "test@1",
            FileNode {
                path: path.to_string(),
                lang,
                size: 0,
                sha256: None,
                skipped: None,
                excluded: None,
                component: None,
            },
        )
    }

    fn contract(category: &str, qualifier: Option<&str>, value: &str) -> (NodeId, Node) {
        let id = crate::graph::contract_id(category, qualifier, value);
        let node = Node::contract(
            id.clone(),
            Provenance::Syntactic,
            "lang-hcl@1",
            ContractNode {
                category: category.to_string(),
                qualifier: qualifier.map(|q| TaintedString::new(q, Provenance::Syntactic)),
                value: TaintedString::new(value, Provenance::Syntactic),
            },
        );
        (id, node)
    }

    /// Reproduces the SID Cloud spec's acceptance-test shape: one alarm
    /// (`SequenceGapsTotal`) with no matching emitter — the dead-alarm
    /// case — alongside one properly matched pair
    /// (`QuoteDropsPerSecond`) and one metric a service emits with no
    /// alarm at all (`UnwatchedMetric`). `orphans --category metric_name`
    /// must return exactly the dead alarm under
    /// `consumed_never_produced` and exactly the unwatched metric under
    /// `produced_never_consumed` — the matched pair in neither list.
    fn sid_like_doc() -> GraphDocument {
        let cs = file("Emitter.cs", Lang::CSharp);
        let tf = file("alarms.tf", Lang::Hcl);

        let (matched_id, matched_node) = contract(
            "metric_name",
            Some("SIDCloud/Ingest"),
            "QuoteDropsPerSecond",
        );
        let (dead_id, dead_node) =
            contract("metric_name", Some("SIDCloud/Ingest"), "SequenceGapsTotal");
        let (unwatched_id, unwatched_node) =
            contract("metric_name", Some("SIDCloud/Ingest"), "UnwatchedMetric");

        let edges = vec![
            Edge::new(
                EdgeKind::Produces,
                cs.id.clone(),
                matched_id.clone(),
                Confidence::Inferred,
                "csharp-object-init".to_string(),
            ),
            Edge::new(
                EdgeKind::Consumes,
                tf.id.clone(),
                matched_id,
                Confidence::Certain,
                "hcl-attr".to_string(),
            ),
            Edge::new(
                EdgeKind::Consumes,
                tf.id.clone(),
                dead_id,
                Confidence::Certain,
                "hcl-attr".to_string(),
            ),
            Edge::new(
                EdgeKind::Produces,
                cs.id.clone(),
                unwatched_id,
                Confidence::Inferred,
                "csharp-object-init".to_string(),
            ),
        ];

        GraphDocument {
            carto_version: "0.1.0".to_string(),
            schema_version: crate::consts::SCHEMA_VERSION,
            components: vec![],
            nodes: vec![cs, tf, matched_node, dead_node, unwatched_node],
            edges,
        }
    }

    #[test]
    fn dead_alarm_and_unwatched_metric_are_reported_matched_pair_is_not() {
        let qg = QueryGraph::from_document(sid_like_doc());
        let result = run(
            &qg,
            &OrphansQuery {
                category: Some("metric_name".to_string()),
                limit: DEFAULT_LIMIT,
                component: None,
            },
        );

        assert_eq!(result.consumed_never_produced.len(), 1);
        assert_eq!(result.consumed_never_produced[0].value, "SequenceGapsTotal");

        assert_eq!(result.produced_never_consumed.len(), 1);
        assert_eq!(result.produced_never_consumed[0].value, "UnwatchedMetric");
    }

    fn file_with_component(path: &str, lang: Lang, component: &str) -> Node {
        let mut n = file(path, lang);
        if let crate::graph::NodeData::File(f) = &mut n.data {
            f.component = Some(component.to_string());
        }
        n
    }

    #[test]
    fn component_filter_includes_a_site_whose_component_is_nested_under_the_filtered_one() {
        // ADR-0038: `prod` is nested under `infra` (the same
        // scattered-then-rolled-up shape ADR-0037's terraform rollup
        // avoids for auto-detection, reproduced here as a *declared*
        // nesting to exercise the query-layer side independently).
        // `--component infra` must still surface a contract whose only
        // consumer site's own component is the nested `prod`.
        let tf = file_with_component("infra/envs/prod/alarms.tf", Lang::Hcl, "prod");
        let (metric_id, metric_node) = contract("metric_name", None, "OrdersLag");
        let edges = vec![Edge::new(
            EdgeKind::Consumes,
            tf.id.clone(),
            metric_id,
            Confidence::Certain,
            "hcl-attr".to_string(),
        )];
        let doc = GraphDocument {
            carto_version: "0.1.0".to_string(),
            schema_version: crate::consts::SCHEMA_VERSION,
            components: vec![
                crate::components::Component {
                    name: "infra".to_string(),
                    path: "infra".to_string(),
                    kind: "terraform".to_string(),
                    depends_on: vec![],
                },
                crate::components::Component {
                    name: "prod".to_string(),
                    path: "infra/envs/prod".to_string(),
                    kind: "terraform".to_string(),
                    depends_on: vec![],
                },
            ],
            nodes: vec![tf, metric_node],
            edges,
        };
        let qg = QueryGraph::from_document(doc);
        let filter: std::collections::BTreeSet<String> =
            ["infra".to_string()].into_iter().collect();
        let result = run(
            &qg,
            &OrphansQuery {
                category: Some("metric_name".to_string()),
                limit: DEFAULT_LIMIT,
                component: Some(filter),
            },
        );
        assert_eq!(result.consumed_never_produced.len(), 1);
        assert_eq!(result.consumed_never_produced[0].value, "OrdersLag");
    }

    #[test]
    fn category_filter_scopes_the_report() {
        let qg = QueryGraph::from_document(sid_like_doc());
        let result = run(
            &qg,
            &OrphansQuery {
                category: Some("env_var".to_string()),
                limit: DEFAULT_LIMIT,
                component: None,
            },
        );
        assert!(result.consumed_never_produced.is_empty());
        assert!(result.produced_never_consumed.is_empty());
    }

    #[test]
    fn survives_a_real_json_round_trip() {
        let json = serde_json::to_vec(&sid_like_doc()).unwrap();
        let doc: GraphDocument = serde_json::from_slice(&json).unwrap();
        let qg = QueryGraph::from_document(doc);
        let result = run(&qg, &OrphansQuery::new());
        assert_eq!(result.consumed_never_produced.len(), 1);
        assert_eq!(result.produced_never_consumed.len(), 1);
    }
}

//! Edge kinds and confidence (spec §4.2, INV-8: "every edge carries a
//! confidence; nothing heuristic may be presented as resolved").
//!
//! [`Edge::new`] requires a [`Confidence`] and at least one evidence
//! string — there is no constructor that omits them, so INV-8 is a
//! type-level guarantee, not a convention. This slice's `walk` producer
//! creates no edges yet (`contains` needs `Symbol` nodes); the type lives
//! here now so the extractors that do produce edges compile against a
//! finished shape.

use super::id::{EdgeId, NodeId, edge_id};
use serde::{Deserialize, Serialize};

/// Spec §4.2's edge kinds. Only kinds a `walk`-only index could ever touch
/// are listed as variants that exist elsewhere in this enum's eventual
/// full form; since no edges are produced this slice, this starts as the
/// complete spec §4.2 vocabulary so later producers don't need to touch
/// this enum's definition, only add call sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    Contains,
    Imports,
    Calls,
    References,
    DependsOn,
    HasPolicy,
    DeployedAs,
    TriggeredBy,
    Annotates,
}

impl EdgeKind {
    /// The string used in the edge's canonical key (spec §4.3:
    /// `edge:<kind>:<from>:<to>`) and in serde output.
    pub fn as_str(self) -> &'static str {
        match self {
            EdgeKind::Contains => "contains",
            EdgeKind::Imports => "imports",
            EdgeKind::Calls => "calls",
            EdgeKind::References => "references",
            EdgeKind::DependsOn => "depends_on",
            EdgeKind::HasPolicy => "has_policy",
            EdgeKind::DeployedAs => "deployed_as",
            EdgeKind::TriggeredBy => "triggered_by",
            EdgeKind::Annotates => "annotates",
        }
    }
}

/// Spec §4.2's confidence vocabulary, in the total order the §4.3 merge
/// rule needs ("duplicates merge, keeping highest confidence"):
/// `certain > strong > inferred > weak > ingested`. Declared in that order
/// so derived [`Ord`] *is* the merge order — highest confidence sorts
/// greatest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Ingested,
    Weak,
    Inferred,
    Strong,
    Certain,
}

/// Max evidence strings kept per edge (spec §4.3: "concatenating evidence,
/// capped at 3 entries").
pub const MAX_EVIDENCE_ENTRIES: usize = 3;

/// An edge (spec §4.2). `evidence` is a short machine string per entry
/// (e.g. `handler-path-match:src/handlers/orders.ts`), capped at
/// [`MAX_EVIDENCE_ENTRIES`] by [`Edge::merge`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    pub id: EdgeId,
    pub kind: EdgeKind,
    pub from: NodeId,
    pub to: NodeId,
    pub confidence: Confidence,
    pub evidence: Vec<String>,
}

impl Edge {
    /// Constructs an edge. `confidence` and at least one `evidence` string
    /// are required arguments, not optional builder steps — INV-8 ("every
    /// edge carries a confidence") is therefore enforced by the type
    /// signature: there is no way to build an `Edge` without them.
    pub fn new(
        kind: EdgeKind,
        from: NodeId,
        to: NodeId,
        confidence: Confidence,
        evidence: String,
    ) -> Self {
        let id = edge_id(kind.as_str(), &from, &to);
        Edge {
            id,
            kind,
            from,
            to,
            confidence,
            evidence: vec![evidence],
        }
    }

    /// Merges `other` into `self` per spec §4.3: same `(kind, from, to)` ⇒
    /// duplicates merge, keeping the higher confidence and concatenating
    /// evidence, capped at [`MAX_EVIDENCE_ENTRIES`]. Panics if `other`
    /// isn't actually a duplicate of `self` (same `id`) — callers group by
    /// ID before merging; this function is not the place to silently
    /// merge unrelated edges.
    pub fn merge(&mut self, other: Edge) {
        assert_eq!(
            self.id, other.id,
            "Edge::merge called on edges with different IDs; group by ID first"
        );
        if other.confidence > self.confidence {
            self.confidence = other.confidence;
        }
        for ev in other.evidence {
            if self.evidence.len() >= MAX_EVIDENCE_ENTRIES {
                break;
            }
            if !self.evidence.contains(&ev) {
                self.evidence.push(ev);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nid(s: &str) -> NodeId {
        super::super::id::file_id(s)
    }

    #[test]
    fn confidence_order_matches_spec_merge_rule() {
        assert!(Confidence::Certain > Confidence::Strong);
        assert!(Confidence::Strong > Confidence::Inferred);
        assert!(Confidence::Inferred > Confidence::Weak);
        assert!(Confidence::Weak > Confidence::Ingested);
    }

    #[test]
    fn merge_keeps_highest_confidence() {
        let mut a = Edge::new(
            EdgeKind::Imports,
            nid("a.rs"),
            nid("b.rs"),
            Confidence::Inferred,
            "rule:same-file".into(),
        );
        let b = Edge::new(
            EdgeKind::Imports,
            nid("a.rs"),
            nid("b.rs"),
            Confidence::Certain,
            "rule:relative-path".into(),
        );
        a.merge(b);
        assert_eq!(a.confidence, Confidence::Certain);
        assert_eq!(a.evidence.len(), 2);
    }

    #[test]
    fn merge_caps_evidence_at_three_and_dedupes() {
        let mut a = Edge::new(
            EdgeKind::Calls,
            nid("a.rs"),
            nid("b.rs"),
            Confidence::Inferred,
            "e1".into(),
        );
        for ev in ["e2", "e3", "e4", "e1"] {
            let b = Edge::new(
                EdgeKind::Calls,
                nid("a.rs"),
                nid("b.rs"),
                Confidence::Inferred,
                ev.into(),
            );
            a.merge(b);
        }
        assert_eq!(a.evidence.len(), MAX_EVIDENCE_ENTRIES);
        assert_eq!(a.evidence, vec!["e1", "e2", "e3"]);
    }

    #[test]
    #[should_panic(expected = "different IDs")]
    fn merge_panics_on_mismatched_ids() {
        let mut a = Edge::new(
            EdgeKind::Calls,
            nid("a.rs"),
            nid("b.rs"),
            Confidence::Inferred,
            "e1".into(),
        );
        let b = Edge::new(
            EdgeKind::Calls,
            nid("a.rs"),
            nid("c.rs"),
            Confidence::Inferred,
            "e2".into(),
        );
        a.merge(b);
    }
}

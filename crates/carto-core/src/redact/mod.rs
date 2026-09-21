//! Secret redaction pass (spec §6.5, §7.5, INV-6: "secrets never reach
//! disk artifacts"). Runs at the single serialisation choke-point
//! ([`crate::graph::persist::persist`]), before [`Graph::into_sorted_parts`]
//! — over every [`TaintedString`] field on every node (today: just
//! `SymbolNode.signature`; a future field like `Note.text`, M4's
//! agent-ingest, is covered automatically with no code change here,
//! since the walk below is generic over `NodeData`, not
//! field-by-field). Scoping the scan to `TaintedString` fields rather
//! than literally "every `String` field" (spec §7.5's wording) is a
//! deliberate reading, not a narrowing done by omission — see
//! `docs/adr/0017-redaction-scope-and-detection-design.md`.
//!
//! Detection is hand-rolled (no `regex` dependency — see
//! [`patterns`]'s module doc) across two passes per field: the spec
//! §7.5(a) pattern set ([`patterns::find_all`]), then the §7.5(b)
//! entropy heuristic ([`entropy::find_high_entropy_tokens`]) over
//! whatever the pattern pass didn't already claim. A field is rebuilt
//! into a new [`TaintedString`] only if at least one redaction applied.

mod entropy;
mod patterns;

use crate::graph::{Graph, NodeData};
use crate::taint::TaintedString;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Per-category redaction counts (spec §7.5: "manifest counts per
/// category"), stored in `manifest.json`. `BTreeMap` (not `HashMap`) so
/// a non-empty map still serializes deterministically (INV-7 applies to
/// `graph.json`, not `manifest.json`, but there's no reason to
/// introduce nondeterminism here either).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedactionCounts {
    pub by_category: BTreeMap<String, u64>,
}

impl RedactionCounts {
    pub fn total(&self) -> u64 {
        self.by_category.values().sum()
    }

    fn add(&mut self, category: &str) {
        *self.by_category.entry(category.to_string()).or_insert(0) += 1;
    }

    fn merge(&mut self, other: RedactionCounts) {
        for (category, count) in other.by_category {
            *self.by_category.entry(category).or_insert(0) += count;
        }
    }
}

/// Runs the redaction pass over `graph` in place, returning counts for
/// the manifest.
pub fn redact(graph: &mut Graph) -> RedactionCounts {
    let mut totals = RedactionCounts::default();
    for node in graph.nodes_mut() {
        match &mut node.data {
            NodeData::Symbol(sym) => {
                if let Some(sig) = &sym.signature {
                    if let Some((redacted, counts)) = redact_tainted_string(sig) {
                        sym.signature = Some(redacted);
                        totals.merge(counts);
                    }
                }
            }
            // ADR-0026: a Contract's `value`/`qualifier` are captured
            // source text (a string literal), the same "arbitrary
            // repo-authored content" category as a symbol's signature —
            // unlike `FileNode.path`/`SymbolNode.name`, which stay out
            // of scope (ADR-0017) because they're extractor-computed
            // identifiers, never free-form text. IDs were already
            // computed from the raw (pre-redaction) value in `resolve`
            // (`graph::contract_id`), so redacting the rendered value
            // here doesn't break a `produces`/`consumes` edge's join.
            NodeData::Contract(c) => {
                if let Some((redacted, counts)) = redact_tainted_string(&c.value) {
                    c.value = redacted;
                    totals.merge(counts);
                }
                if let Some(qualifier) = &c.qualifier {
                    if let Some((redacted, counts)) = redact_tainted_string(qualifier) {
                        c.qualifier = Some(redacted);
                        totals.merge(counts);
                    }
                }
            }
            NodeData::File(_) | NodeData::Module(_) => {}
        }
    }
    totals
}

/// Scans one `TaintedString`'s full sanitized content (not just what
/// `Serialize` would cap it to — a secret starting inside the cap but
/// extending past it must still be caught) and, if anything matched,
/// returns a rebuilt `TaintedString` with every match spliced out plus
/// the per-category counts. `None` if the field is clean, so the common
/// case allocates nothing extra in [`redact`]'s caller.
fn redact_tainted_string(t: &TaintedString) -> Option<(TaintedString, RedactionCounts)> {
    let text = t.render_capped(usize::MAX);

    let mut matches = patterns::find_all(&text);
    matches.extend(entropy::find_high_entropy_tokens(&text, &matches));
    matches.sort_by_key(|(r, _)| r.start);

    if matches.is_empty() {
        return None;
    }

    let mut counts = RedactionCounts::default();
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for (range, category) in &matches {
        out.push_str(&text[cursor..range.start]);
        let token_hash = Sha256::digest(text[range.clone()].as_bytes());
        let hex = hex_encode(&token_hash);
        out.push_str(&format!("«redacted:{}»", &hex[..8]));
        counts.add(category);
        cursor = range.end;
    }
    out.push_str(&text[cursor..]);

    Some((TaintedString::new(&out, t.provenance()), counts))
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{SymKind, SymbolNode, file_id, sym_id};
    use crate::taint::Provenance;

    fn symbol_node(signature: &str) -> crate::graph::Node {
        crate::graph::Node::symbol(
            sym_id("a.rs", "const", "KEY", 1),
            Provenance::Syntactic,
            "lang-rust@1",
            SymbolNode {
                name: "KEY".to_string(),
                sym_kind: SymKind::Const,
                file: file_id("a.rs"),
                start_line: 1,
                end_line: 1,
                signature: Some(TaintedString::new(signature, Provenance::Syntactic)),
                unresolved_calls: vec![],
                uncaptured_inbound_calls: 0,
                uncaptured_outbound_calls: 0,
                unresolved_inbound_calls: vec![],
                unresolved_inbound_call_count: 0,
            },
        )
    }

    #[test]
    fn stub_graph_reports_zero_redactions() {
        let mut g = Graph::new();
        let counts = redact(&mut g);
        assert_eq!(counts.total(), 0);
    }

    #[test]
    fn clean_signature_is_untouched_and_uncounted() {
        let mut g = Graph::new();
        g.insert_node(symbol_node("const Port: u16 = 8080"));
        let counts = redact(&mut g);
        assert_eq!(counts.total(), 0);
        let sym = g.node(&sym_id("a.rs", "const", "KEY", 1)).unwrap();
        match &sym.data {
            NodeData::Symbol(s) => {
                assert_eq!(
                    s.signature.as_ref().unwrap().render_capped(100),
                    "const Port: u16 = 8080"
                );
            }
            _ => panic!("expected a symbol node"),
        }
    }

    #[test]
    fn secret_bearing_signature_is_redacted_and_counted() {
        let mut g = Graph::new();
        g.insert_node(symbol_node("const KEY: &str = \"AKIAIOSFODNN7EXAMPLE\""));
        let counts = redact(&mut g);
        assert_eq!(counts.total(), 1);
        assert_eq!(counts.by_category.get("aws_access_key"), Some(&1));

        let sym = g.node(&sym_id("a.rs", "const", "KEY", 1)).unwrap();
        match &sym.data {
            NodeData::Symbol(s) => {
                let rendered = s.signature.as_ref().unwrap().render_capped(200);
                assert!(!rendered.contains("AKIAIOSFODNN7EXAMPLE"));
                assert!(rendered.contains("«redacted:"));
            }
            _ => panic!("expected a symbol node"),
        }
    }

    #[test]
    fn two_occurrences_of_the_same_secret_get_the_same_replacement_token() {
        let mut g = Graph::new();
        g.insert_node(symbol_node(
            "const A: &str = \"AKIAIOSFODNN7EXAMPLE\"; const B: &str = \"AKIAIOSFODNN7EXAMPLE\"",
        ));
        redact(&mut g);
        let sym = g.node(&sym_id("a.rs", "const", "KEY", 1)).unwrap();
        match &sym.data {
            NodeData::Symbol(s) => {
                let rendered = s.signature.as_ref().unwrap().render_capped(300);
                let tokens: Vec<&str> = rendered.split("«redacted:").skip(1).collect();
                assert_eq!(tokens.len(), 2);
                // Same matched substring -> same sha256 prefix.
                assert_eq!(tokens[0].split('»').next(), tokens[1].split('»').next());
            }
            _ => panic!("expected a symbol node"),
        }
    }
}

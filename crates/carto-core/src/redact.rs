//! Secret redaction pass (spec §6.5, §7.5, INV-6: "secrets never reach
//! disk artifacts"). This slice (M1.b.1) implements only the interface,
//! wired into [`crate::graph::persist`] as a no-op pass — spec §10 M1:
//! "redact stub = no-op pass with the interface in place." Real
//! pattern/entropy detection (§7.5) lands in M2, against this same
//! function signature.

use crate::graph::Graph;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Per-category redaction counts (spec §7.5: "manifest counts per
/// category"), stored in `manifest.json`. All-empty until M2 implements
/// real detection; `BTreeMap` (not `HashMap`) so a non-empty map still
/// serializes deterministically (INV-7 applies to `graph.json`, not
/// `manifest.json`, but there's no reason to introduce nondeterminism
/// here either).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedactionCounts {
    pub by_category: BTreeMap<String, u64>,
}

impl RedactionCounts {
    pub fn total(&self) -> u64 {
        self.by_category.values().sum()
    }
}

/// Runs the redaction pass over `graph` in place, returning counts for the
/// manifest. No-op this slice: M1 produces no tainted string fields worth
/// scanning yet (no `Symbol.signature`, no `Note.text`), so there is
/// nothing to redact and pretending otherwise would be dishonest. Takes
/// `&mut Graph` (not `&Graph`) so M2's implementation — which replaces
/// matched substrings with `«redacted:sha256-prefix»` — can land without
/// changing this signature or its call site in
/// [`crate::graph::persist::persist`].
pub fn redact(_graph: &mut Graph) -> RedactionCounts {
    RedactionCounts::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_reports_zero_redactions() {
        let mut g = Graph::new();
        let counts = redact(&mut g);
        assert_eq!(counts.total(), 0);
    }
}

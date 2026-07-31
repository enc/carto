//! Stable ID recipe (spec §4.3): `id = blake3(kind ‖ canonical_key)[..16]`
//! hex.
//!
//! The per-kind `canonical_key` formats spec §4.3 lists (`file:<relpath>`,
//! `sym:<relpath>:<sym_kind>:<qualified_name>:<start_line>`, `iac:…`,
//! `iam:…`, `note:…`, `edge:<kind>:<from>:<to>`) already embed the kind as a
//! literal prefix, so "`kind ‖ canonical_key`" is realized here as hashing
//! that single self-describing string — there is no separate kind value
//! concatenated in front of it. This module owns the one "blake3 → 16 hex
//! chars" helper in the crate; [`crate::outdir`] reuses it for the
//! repo-hash component of the default out-dir, which follows the same
//! convention (spec §4.3's comment on [`crate::outdir::default_out_root`]).
//!
//! Only the `File` canonical-key constructor is implemented this slice
//! (M1.b.1 produces no `Symbol`/`IacResource`/`IamPolicyStmt`/`Note`/`Edge`
//! nodes yet); the others arrive with the node kinds that need them.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Length, in hex chars, of every stable ID this crate mints. Matches
/// [`crate::outdir`]'s `REPO_HASH_HEX_LEN`, which exists as its own constant
/// there (a different concern: a directory-name component, not a graph ID)
/// but is numerically the same convention.
pub(crate) const ID_HEX_LEN: usize = 16;

/// Hashes `bytes` with blake3 and returns the first [`ID_HEX_LEN`] hex
/// characters. The one place this crate turns bytes into a short stable
/// hex ID — reused by [`crate::outdir::default_out_root`] so the recipe
/// lives in exactly one place.
pub(crate) fn blake3_hex_prefix(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex()[..ID_HEX_LEN].to_string()
}

/// A node's stable ID (spec §4.3). Opaque: constructed only via the
/// per-kind functions below (e.g. [`file_id`]), never from an arbitrary
/// string, so a node ID always reflects the recipe.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(String);

impl NodeId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// An edge's stable ID (spec §4.3): `edge:<kind>:<from>:<to>` — one edge per
/// kind per pair; duplicates merge (see [`crate::graph::edge`]).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EdgeId(String);

impl EdgeId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EdgeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Stable ID for a `File` node (spec §4.1, §4.3): canonical key
/// `file:<relpath>`. `relpath` MUST already be repo-relative and
/// `/`-separated (spec §4.1) — this function does not normalize it.
pub fn file_id(relpath: &str) -> NodeId {
    NodeId(blake3_hex_prefix(format!("file:{relpath}").as_bytes()))
}

/// Stable ID for an edge (spec §4.3): canonical key `edge:<kind>:<from>:<to>`.
/// `kind`, `from`, `to` are the edge's `kind` tag and endpoint IDs.
pub fn edge_id(kind: &str, from: &NodeId, to: &NodeId) -> EdgeId {
    EdgeId(blake3_hex_prefix(
        format!("edge:{kind}:{from}:{to}").as_bytes(),
    ))
}

/// Stable ID for a `Symbol` node (spec §4.3): canonical key
/// `sym:<relpath>:<sym_kind>:<qualified_name>:<start_line>`.
/// `qualified_name` is computed by the caller (e.g. `Order::summary` for
/// an impl-block method, spec §4.3's own example of why line is included
/// — "moving a symbol changes its ID," accepted trade-off) — this
/// function has no language-specific qualification logic of its own.
pub fn sym_id(relpath: &str, sym_kind: &str, qualified_name: &str, start_line: u32) -> NodeId {
    NodeId(blake3_hex_prefix(
        format!("sym:{relpath}:{sym_kind}:{qualified_name}:{start_line}").as_bytes(),
    ))
}

/// Stable ID for a `Module` node. Spec §4.3 doesn't give `Module` an
/// explicit canonical-key format (only File/Symbol/IacResource/
/// IamPolicyStmt/Note/Edge are listed) — follows the same `<kind>:<key>`
/// convention as the others. `external` is part of the key so an
/// unresolved external package name can never collide with an internal
/// module path that happens to read the same.
pub fn module_id(path: &str, external: bool) -> NodeId {
    NodeId(blake3_hex_prefix(
        format!("module:{external}:{path}").as_bytes(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_id_is_deterministic() {
        assert_eq!(file_id("src/main.rs"), file_id("src/main.rs"));
    }

    #[test]
    fn file_id_differs_by_path() {
        assert_ne!(file_id("src/main.rs"), file_id("src/lib.rs"));
    }

    #[test]
    fn file_id_is_expected_length() {
        assert_eq!(file_id("src/main.rs").as_str().len(), ID_HEX_LEN);
    }

    #[test]
    fn file_id_golden_vector() {
        // Golden vector pinning the recipe (spec §4.3): a change here means
        // every previously indexed graph.json's node IDs change, which is
        // exactly the kind of accidental drift this test exists to catch.
        assert_eq!(file_id("src/main.rs").as_str(), "219e7e007ca53849");
    }

    #[test]
    fn edge_id_is_deterministic_and_kind_sensitive() {
        let a = file_id("a.rs");
        let b = file_id("b.rs");
        assert_eq!(edge_id("contains", &a, &b), edge_id("contains", &a, &b));
        assert_ne!(edge_id("contains", &a, &b), edge_id("imports", &a, &b));
        assert_ne!(edge_id("contains", &a, &b), edge_id("contains", &b, &a));
    }
}

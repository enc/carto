//! Node kinds (spec §4.1). This slice implements `File` only — `Module`,
//! `Symbol`, `IacResource`, `IamPolicyStmt`, `CloudResourceRef`, `Note`
//! arrive with the extractors/infra ingestion that produce them.

use super::id::NodeId;
use crate::lang::Lang;
use crate::taint::Provenance;
use serde::{Deserialize, Serialize};

/// Common fields on every node (spec §4.1): `id`, `kind` (carried by
/// [`NodeData`]'s serde tag, so it can never drift from the payload),
/// `provenance`, `origin`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub provenance: Provenance,
    /// Extractor name + version that produced this node, e.g. `walk@1`
    /// (spec §4.1). Deliberately independent of the carto crate version
    /// (`consts::…` / `CARGO_PKG_VERSION`) so a carto release bump doesn't
    /// churn every node's `origin` field.
    pub origin: String,
    #[serde(flatten)]
    pub data: NodeData,
}

impl Node {
    pub fn file(
        id: NodeId,
        provenance: Provenance,
        origin: impl Into<String>,
        file: FileNode,
    ) -> Self {
        Node {
            id,
            provenance,
            origin: origin.into(),
            data: NodeData::File(file),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NodeData {
    File(FileNode),
}

/// Why a file's contents were never parsed. Spec §5.1 (walk).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// First 8 KiB contained a NUL byte (spec §5.1's binary sniff).
    Binary,
    /// Larger than [`crate::consts::MAX_FILE_BYTES`] (spec §4.4).
    TooLarge,
    /// A `.lock` file — recorded as a `File` node, never parsed (spec
    /// §5.1), even though it's plain text.
    LockFile,
    /// A `*.min.js` file — recorded as a `File` node, never parsed (spec
    /// §5.1's built-in denylist groups this with `.lock`).
    Minified,
}

/// Why a file's contents were never even opened. Spec §5.1's secret-shaped
/// denylist: `*.pem`, `*.key`, `*.p12`, `.env*`, `*credentials*`,
/// `*.tfstate*`, `*.tfvars`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExclusionReason {
    Sensitive,
}

/// `File` node (spec §4.1). `path` is always repo-relative and
/// `/`-separated, regardless of host OS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileNode {
    pub path: String,
    pub lang: Lang,
    pub size: u64,
    /// sha256 of the file's contents, hex-encoded. `None` when the
    /// contents were never read — [`ExclusionReason::Sensitive`] files'
    /// contents are never opened, so emitting a digest for them would
    /// misrepresent what carto actually did (spec §5.1).
    pub sha256: Option<String>,
    pub skipped: Option<SkipReason>,
    pub excluded: Option<ExclusionReason>,
}

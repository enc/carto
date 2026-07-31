//! Node kinds (spec §4.1). `File`, `Symbol`, and `Module` exist as of
//! M1.b.2a — `IacResource`, `IamPolicyStmt`, `CloudResourceRef`, `Note`
//! arrive with infra ingestion/agent-ingest (M2/M4).

use super::id::NodeId;
use crate::lang::Lang;
use crate::taint::{Provenance, TaintedString};
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

    pub fn symbol(
        id: NodeId,
        provenance: Provenance,
        origin: impl Into<String>,
        symbol: SymbolNode,
    ) -> Self {
        Node {
            id,
            provenance,
            origin: origin.into(),
            data: NodeData::Symbol(symbol),
        }
    }

    pub fn module(
        id: NodeId,
        provenance: Provenance,
        origin: impl Into<String>,
        module: ModuleNode,
    ) -> Self {
        Node {
            id,
            provenance,
            origin: origin.into(),
            data: NodeData::Module(module),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NodeData {
    File(FileNode),
    Symbol(SymbolNode),
    Module(ModuleNode),
}

impl NodeData {
    /// `Some` only for the `File` variant. Producers that only ever
    /// build one variant themselves (e.g. `walk`, which only ever
    /// constructs `File` nodes) use this instead of an irrefutable-
    /// pattern `let`, which stops compiling the moment `NodeData` gains
    /// another variant.
    pub fn as_file(&self) -> Option<&FileNode> {
        match self {
            NodeData::File(f) => Some(f),
            _ => None,
        }
    }

    /// The node kind as a string, matching this enum's own
    /// `#[serde(tag = "kind", ...)]` spelling exactly. Used by the query
    /// layer (`deps`'s `NodeSummary.kind`, `map`'s per-kind counts) so
    /// there's one place that spells out "file"/"symbol"/"module" for
    /// output, not a second match arm per consumer.
    pub fn kind_str(&self) -> &'static str {
        match self {
            NodeData::File(_) => "file",
            NodeData::Symbol(_) => "symbol",
            NodeData::Module(_) => "module",
        }
    }
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

/// Spec §4.1's `Symbol.sym_kind` enum, verbatim. Extractors only ever
/// produce the variants their language actually has — Rust's extractor
/// (M1.b.2a) produces `Function`/`Method`/`Struct`/`Enum`/`Trait`/`Const`/
/// `Type`; `Class`/`Interface`/`Var` wait for languages that have them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymKind {
    Function,
    Method,
    Class,
    Struct,
    Enum,
    Trait,
    Interface,
    Type,
    Const,
    Var,
}

impl SymKind {
    /// The string used in the symbol's canonical key (spec §4.3:
    /// `sym:<relpath>:<sym_kind>:<qualified_name>:<start_line>`) and in
    /// serde output.
    pub fn as_str(self) -> &'static str {
        match self {
            SymKind::Function => "function",
            SymKind::Method => "method",
            SymKind::Class => "class",
            SymKind::Struct => "struct",
            SymKind::Enum => "enum",
            SymKind::Trait => "trait",
            SymKind::Interface => "interface",
            SymKind::Type => "type",
            SymKind::Const => "const",
            SymKind::Var => "var",
        }
    }
}

/// A call site that could not be resolved to a target symbol (spec §5.3
/// rule 2: "If multiple candidates remain: emit NO edge; record the
/// call-site under the caller symbol's `unresolved_calls` list" —
/// extended here to cover the zero-candidate case too, since both are
/// the same honesty principle, INV-8: "missing honestly beats guessing").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnresolvedCall {
    pub name: String,
    pub line: u32,
}

/// `Symbol` node (spec §4.1). `name` is the bare declared identifier —
/// used both for display and as what call-resolution matches against
/// (spec §5.3's matching is name-based, not qualified-path-based). The
/// §4.3 ID recipe's `qualified_name` component (e.g. `Order::summary` for
/// an impl-block method) is computed only when constructing the node's
/// [`NodeId`] — it is not a separate field here, matching spec §4.1's
/// literal field list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolNode {
    pub name: String,
    pub sym_kind: SymKind,
    pub file: NodeId,
    /// 1-based, inclusive (spec §4.1: "range (`start_line..end_line`,
    /// 1-based)").
    pub start_line: u32,
    pub end_line: u32,
    /// ≤300 chars (spec §4.1). [`TaintedString`]'s `Serialize` impl
    /// already caps at [`crate::consts::SIGNATURE_CAP`] on output, so no
    /// separate truncation is needed here.
    pub signature: Option<TaintedString>,
    pub unresolved_calls: Vec<UnresolvedCall>,
}

/// `Module` node (spec §4.1): "logical module/package path". `external`
/// distinguishes a resolved-internal reference (unused this slice — an
/// internal `use` seeds call resolution rather than producing its own
/// `Module` node, see `docs/adr/0008-rust-resolution-policy-mapping.md`)
/// from a package import whose target carto never parses (spec §5.3 rule
/// 1: "certain that it's imported, target unresolved").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleNode {
    pub path: String,
    pub external: bool,
}

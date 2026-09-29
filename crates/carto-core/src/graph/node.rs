//! Node kinds (spec §4.1). `File`, `Symbol`, and `Module` exist as of
//! M1.b.2a — `IacResource`, `IamPolicyStmt`, `CloudResourceRef`, `Note`
//! arrive with infra ingestion/agent-ingest (M2/M4). `Contract` is a
//! spec amendment (not in §4.1) — see
//! `docs/adr/0026-contract-node-and-produces-consumes-edges.md`.

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

    pub fn contract(
        id: NodeId,
        provenance: Provenance,
        origin: impl Into<String>,
        contract: ContractNode,
    ) -> Self {
        Node {
            id,
            provenance,
            origin: origin.into(),
            data: NodeData::Contract(contract),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NodeData {
    File(FileNode),
    Symbol(SymbolNode),
    Module(ModuleNode),
    Contract(ContractNode),
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
            NodeData::Contract(_) => "contract",
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
    /// The innermost [`crate::components::Component`] this file belongs
    /// to (its `name`), or `None` if it's under no recognized project
    /// root — a real, meaningful bucket (repo-root scripts, top-level
    /// docs), not a missing value. Set by `indexer::build_and_persist`
    /// between `walk` and `lang::extract_and_resolve` — `walk` itself
    /// has no component concept, only `FileNode::path`; every `FileNode`
    /// literal built before that assignment step (including every one
    /// `walk` constructs) starts `None`. `#[serde(default)]` so a v6
    /// `graph.json` (built before components existed) still parses —
    /// its absent `component` must not be misread as "confirmed no
    /// components", which is why `SCHEMA_VERSION` still bumps alongside
    /// this field (see `consts.rs`). ADR-0034.
    #[serde(default)]
    pub component: Option<String>,
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
    /// Terraform/Terragrunt declarations (ADR-0041): not in spec §4.1's
    /// original list (amended by that ADR). Display-only — nothing in
    /// `query`/the CLI/MCP branches on `sym_kind`.
    TfVariable,
    TfLocal,
    TfOutput,
    TfResource,
    TfData,
    TfModule,
    TgDependency,
}

impl SymKind {
    /// Terraform/Terragrunt declaration kinds (ADR-0041).
    pub fn is_terraform(self) -> bool {
        matches!(
            self,
            SymKind::TfVariable
                | SymKind::TfLocal
                | SymKind::TfOutput
                | SymKind::TfResource
                | SymKind::TfData
                | SymKind::TfModule
                | SymKind::TgDependency
        )
    }

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
            SymKind::TfVariable => "tf_variable",
            SymKind::TfLocal => "tf_local",
            SymKind::TfOutput => "tf_output",
            SymKind::TfResource => "tf_resource",
            SymKind::TfData => "tf_data",
            SymKind::TfModule => "tf_module",
            SymKind::TgDependency => "tg_dependency",
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

/// One call site elsewhere in the repo that spells a symbol's bare
/// `name` and was *attempted* for resolution but produced no edge —
/// the inbound counterpart to [`UnresolvedCall`], and the third
/// honesty signal alongside `uncaptured_inbound_calls` (ADR-0020, which
/// counts *never-attempted* syntax, not this). `file` is a plain
/// `String`, matching [`FileNode::path`]: a repo-relative path is an
/// extractor-computed identifier, not captured source text, so it
/// carries no taint. Recording this doesn't mean the site targets this
/// symbol — most often it means the bare name is ambiguous (an
/// interface method and its implementation both named `Save`), the
/// same "missing honestly beats guessing" principle INV-8 already
/// applies to the edge itself. See ADR-0033.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct InboundCallSite {
    pub file: String,
    pub line: u32,
    /// The calling site's own component (mirrors `FileNode::component`)
    /// — without this, these rows silently mix components: a symbol's
    /// `unresolved_inbound_calls` could otherwise list a same-named
    /// call site from a *different* component as if it were plausibly
    /// this symbol's own caller. `#[serde(default)]`: same reasoning as
    /// `FileNode::component`. ADR-0034/0035.
    #[serde(default)]
    pub component: Option<String>,
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
    /// How many call sites elsewhere in this repo spell this symbol's
    /// bare `name` in a shape its language's extractor deliberately
    /// never attempts to resolve (Rust's `Type::method()`/
    /// `module::func()`, ADR-0008) — repo-wide, not scoped to any
    /// particular caller. This is a *count of unattempted syntax*, not
    /// evidence any of those sites actually call *this* symbol, and
    /// never implies an edge (INV-8 extended to absence — ADR-0020):
    /// `deps --dir in` returning few/no `calls` edges for a symbol with
    /// a nonzero count here means "carto didn't look here", not "this
    /// symbol has few callers". Zero for every language without such an
    /// exclusion.
    #[serde(default)]
    pub uncaptured_inbound_calls: u32,
    /// How many call sites *inside this symbol's own body* spell a
    /// callee name in a shape its language's extractor deliberately
    /// never attempts to resolve (Rust's `Type::method()`/
    /// `module::func()`, ADR-0008) — the outbound counterpart to
    /// `uncaptured_inbound_calls` (ADR-0023). A count of unattempted
    /// syntax *this symbol itself* contains, not evidence of what those
    /// calls target: `deps --dir out` returning few/no `calls` edges
    /// for a symbol with a nonzero count here means "carto didn't
    /// attempt some of what this symbol calls", not "this symbol calls
    /// little". Zero for every language without such an exclusion.
    #[serde(default)]
    pub uncaptured_outbound_calls: u32,
    /// Call sites elsewhere in the repo that spell this symbol's bare
    /// `name`, *were* attempted for resolution (unlike
    /// `uncaptured_inbound_calls`, which counts syntax an extractor
    /// never even tries), and produced no edge — most often because the
    /// name is ambiguous (an interface method and its implementation
    /// both named the same thing). Capped at
    /// [`crate::consts::UNRESOLVED_INBOUND_SITES_CAP`], sorted by
    /// `(file, line)`; `unresolved_inbound_call_count` below carries the
    /// uncapped total. Not evidence any of these sites actually target
    /// this symbol — same non-claim as `uncaptured_inbound_calls`.
    /// ADR-0033.
    #[serde(default)]
    pub unresolved_inbound_calls: Vec<InboundCallSite>,
    /// Uncapped count backing `unresolved_inbound_calls` above, so the
    /// signal stays honest past the cap (a caller can tell "these are
    /// all of them" from "these are the first 25 of N"). ADR-0033.
    #[serde(default)]
    pub unresolved_inbound_call_count: u32,
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

/// A categorised string literal — a cross-language contract carto's node
/// vocabulary otherwise has no way to represent (a CloudWatch metric
/// name, an env var key, …). Not in spec §4.1; see
/// `docs/adr/0026-contract-node-and-produces-consumes-edges.md` for the
/// amendment and why `value`/`qualifier` are tainted (a string literal is
/// arbitrary captured source text, exactly where a credential would
/// hide — INV-6 must scan it, unlike `FileNode.path`/`SymbolNode.name`,
/// which are extractor-computed identifiers, not captured text).
///
/// `category` is an open, repo-extensible string (`.carto/contracts.json`,
/// ADR-0027) rather than a closed enum — new categories are exactly what
/// that config file is for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractNode {
    pub category: String,
    /// Disambiguates same-spelled values in different scopes (e.g. a
    /// CloudWatch metric namespace) — `None` for a category with no
    /// natural qualifier.
    pub qualifier: Option<TaintedString>,
    pub value: TaintedString,
}

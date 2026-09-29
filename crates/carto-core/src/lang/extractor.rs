//! `LangExtractor` trait and the raw (pre-resolution) extraction shape
//! (spec §5.2: "Per language implement a `LangExtractor` trait"). Each
//! implementation parses one file's bytes into symbols/imports/call-sites;
//! cross-file resolution (spec §5.3) is a separate whole-repo pass — see
//! `super::resolve`.
//!
//! Simplifies spec §5.2's literal `extract(&self, src: &[u8], file:
//! &FileCtx) -> ExtractOut` to a bare `relpath: &str` parameter instead of
//! a `FileCtx` struct: nothing implemented so far needs more than the
//! path (Rust's `mod`/`use` resolution happens whole-repo, in `resolve`,
//! which already has full file context) — see
//! `docs/adr/0008-rust-resolution-policy-mapping.md`. A richer `FileCtx`
//! can replace this if a later language extractor actually needs one.

use crate::graph::SymKind;
use crate::lang::Lang;

/// One extracted symbol, before it becomes a graph `SymbolNode`. Carries
/// `qualified_name` separately from `name` because only the extractor
/// knows how to compute it for its language (e.g. `Type::method` for a
/// Rust impl-block method) — `resolve` uses it solely to build the
/// symbol's stable ID (spec §4.3); `name` (the bare identifier) is what
/// call resolution matches against.
pub struct RawSymbol {
    pub name: String,
    pub qualified_name: String,
    pub sym_kind: SymKind,
    /// 1-based, inclusive.
    pub start_line: u32,
    pub end_line: u32,
    /// Raw (not yet sanitized) signature text — sanitized into a
    /// `TaintedString` when the graph node is built, in `resolve`.
    pub signature: String,
    /// Whether the symbol is exported (Rust: has a `pub` visibility
    /// modifier). Spec §5.3 rule 2 tiers (b)/(c) only match against
    /// "symbols imported into the file" / "exported symbols of
    /// same-package files" — tier (a), same-file, doesn't care, since a
    /// private same-file call is ordinary and Rust itself wouldn't
    /// compile a private cross-file one anyway.
    pub is_pub: bool,
    /// The bare name of the type/class/struct this symbol is declared
    /// in — `None` at top level (a free function, or the type
    /// declaration itself). Explicit rather than parsed back out of
    /// `qualified_name` for the same reason `qualified_name` itself is
    /// explicit: only the extractor knows how to compute it for its
    /// language. Feeds `resolve`'s owner-type disambiguation tier
    /// (ADR-0032) — when a bare callee/type name has more than one
    /// same-named candidate repo-wide (an interface method and its
    /// implementation both named `Save`, say), the candidate whose
    /// owner the caller actually names in a type position (via
    /// `RawTypeRef`) is preferred over leaving the call unresolved.
    pub owner: Option<String>,
}

/// One name brought into scope by an import — the identifier a call
/// site in *this* file actually spells (`bound_name`) paired with the
/// name the target symbol is declared under in *its own* file
/// (`declared_name`, what `resolve`'s `pub_by_name` is keyed by).
/// Equal for an unaliased import (`import { foo } from './x'`, `from
/// .pkg import foo`); differ for an aliased one (`import { foo as bar
/// } from './x'`, `from .pkg import foo as bar`). Carrying both,
/// instead of collapsing to the one name a naive "imported names" set
/// would keep, is what makes spec §5.3 tier (b) alias-aware — a real,
/// pre-existing gap for Python's aliased imports (never fixed when
/// PHP shipped, since PHP's own `RawImport::Qualified` already carried
/// both pieces separately) fixed generically here because TypeScript/
/// JavaScript's `import { foo as bar }` is common enough that shipping
/// the extractor without this would make it far less useful. See
/// `docs/adr/0013-typescript-javascript-resolution-policy-mapping.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedName {
    pub bound_name: String,
    pub declared_name: String,
}

/// A raw import, not yet resolved to a target file or classified as
/// external (spec §5.3 rule 1 — `resolve`'s job, since it needs to know
/// about every other file in the repo, not just this one).
///
/// Generalized (M1.b.2b) from an earlier Rust-only `ModDecl`/`UseDecl`
/// shape to cover Python's genuinely different import model — real
/// dotted-relative imports (`from ..pkg import x`) that nothing in
/// Rust's `mod`/`use` system has — without a third, language-specific
/// variant. See `docs/adr/0011-python-resolution-policy-mapping.md`.
pub enum RawImport {
    /// Resolved relative to the *declaring file's own directory* —
    /// Rust's `mod <name>;` (`levels_up: 0`, `module_path: name`,
    /// `imported_names: vec![]` — the import refers to the module file
    /// itself, not a name within it; an inline `mod foo { .. }` is a
    /// namespace, not a file reference, and isn't extracted at all).
    /// Python's `from .pkg import a, b` (`levels_up: 0` — a single dot
    /// means "the current package", i.e. the declaring file's own
    /// directory; `module_path: "pkg"`, `imported_names: [a, b]`)
    /// and `from . import pkg` (`levels_up: 0`, `module_path: ""`, each
    /// entry of `imported_names` itself a submodule name to resolve
    /// relative to the current directory). Each additional leading dot
    /// adds one more level (`from ..pkg import x` -> `levels_up: 1`).
    /// TypeScript/JavaScript's `import x from './orders'` /
    /// `../shared/utils` decompose the same way: each leading `../`
    /// segment is one `levels_up`, and whatever remains after stripping
    /// a single leading `./`/`../` is `module_path` — which may itself
    /// contain internal slashes (`"shared/utils"`), tolerated as an
    /// opaque suffix by `resolve::resolve_relative_import` already.
    /// See ADR-0013.
    Relative {
        levels_up: u32,
        module_path: String,
        imported_names: Vec<ImportedName>,
    },
    /// Not resolved relative to the declaring file. Rust's `use
    /// crate::a::b;` (`root: "crate"`, `imported_names: [b]`) and
    /// `use serde;` (`root: "serde"`, `imported_names: [serde]`).
    /// Python's `import os` (`root: "os"`, `imported_names: [os]`)
    /// and `from pkg import a, b` (`root: "pkg"`, `imported_names: [a,
    /// b]`). Rust's `use_list` (`use a::{b, c};`, including nested
    /// groups and a `self` member) is extracted the same way, each
    /// member becoming its own `imported_names` entry (§1.3, ADR-0022 —
    /// amends this doc's own original "not extracted" note, which
    /// applied uniformly to all three grouped/wildcard/aliased shapes
    /// until then). `use_wildcard` (`use a::*;`) and `use_as_clause`
    /// (`use a::b as c;`, whether standalone or a member inside a
    /// group) are still not extracted — no enumerable member list for
    /// the former, an accepted smaller judgment call for the latter —
    /// deliberately modest (§5.3), ADR-0008/ADR-0022. Python's wildcard
    /// `from x import *` is excluded the same way, documented in
    /// ADR-0011. TypeScript/JavaScript's bare package specifiers
    /// (`import x from 'lodash'`, `root: "lodash"`; a scoped package
    /// `@scope/pkg` roots at its first *two* `/`-separated segments)
    /// use this too — ADR-0013.
    Absolute {
        root: String,
        imported_names: Vec<ImportedName>,
    },
    /// A fully-qualified-name import with no path semantics at all —
    /// PHP's `use App\Orders\Order;` (ADR-0012). Unlike `Relative`
    /// (path arithmetic from the declaring file) and `Absolute` (a
    /// package root, classified internal/external by name alone), PHP's
    /// `use` names a fully-qualified symbol resolved by composer
    /// autoloading at runtime — there is no directory to walk and no
    /// meaningful "root" to classify by itself. `resolve` matches `fqn`
    /// against a repo-wide index built from every file's own `namespace`
    /// declaration (see `resolve::resolve`'s `fqn_to_file`), not by
    /// walking directories. `bound_name` is the alias if the `use` has
    /// one, else the FQN's last segment — the identifier tier (b) calls
    /// are actually written with in source. No separate `ImportedName`
    /// needed here: the FQN's own last segment already *is* the
    /// declared name, derived where it's used (`resolve::resolve`)
    /// instead of duplicated into this struct.
    Qualified { fqn: String, bound_name: String },
    /// A Go import path — `import "github.com/acme/svc/internal/orders"`
    /// (ADR-0015). None of the three variants above fit: there's no path
    /// arithmetic from the declaring file (`Relative`), no meaningful root
    /// to classify by itself (`Absolute` — `github.com` is useless),
    /// and no FQN-to-single-file index (`Qualified`, PHP's `\`-separated
    /// model, where the *whole* name identifies one declaration). A Go
    /// import path instead names a *directory* — `resolve` matches it
    /// against every walked directory's repo-relative path by longest
    /// suffix (no `go.mod` parsing, same "no manifest parsing" precedent
    /// as Python/PHP/TS's absolute imports) and, on a hit, produces one
    /// edge per Go file in that directory, since Go's import unit is the
    /// package (directory), not a single file the way every other
    /// language's import target is.
    ///
    /// No `imported_names`/`bound_name` here, unlike `Absolute`/
    /// `Qualified`: an import binds the *package* name (`orders`), never
    /// a symbol name, so there is nothing to pair with a call site's own
    /// identifier the way tier (b) needs — a Go import can never feed
    /// tier (b) at all (ADR-0015's "never uses tier (b)" consequence). A
    /// dot import (`import . "fmt"`) and a blank import (`import _ "x"`)
    /// use this same variant with no special casing: a blank import is
    /// still a real dependency edge, and a dot import's local-binding
    /// behavior isn't consumed by anything here anyway.
    PackagePath { path: String },
    /// A C# `using Acme.Orders;` directive (ADR-0016) — an import of a
    /// *namespace*, i.e. potentially many files at once. None of the
    /// variants above fit: it names no single FQN (`Qualified` — a C#
    /// `using` brings a whole namespace's types into scope, not one
    /// declaration), no directory (`PackagePath` — C# namespaces have no
    /// required relationship to the directory tree), involves no path
    /// arithmetic from the declaring file (`Relative`), and its root
    /// segment alone is meaningless (`Absolute` — `System` vs.
    /// `System.Text.Json` are different packages). `resolve` matches
    /// `path` *exactly* against every file's own `declared_namespace`
    /// and, on a hit, fans out one `imports` edge per declaring file —
    /// Go's per-package-file fan-out shape, keyed by namespace instead
    /// of directory. No hit but a known namespace root ⇒ no edge
    /// (INV-8); unknown root ⇒ an external `Module` node keyed by the
    /// full namespace string (Go-style full identity — truncating
    /// `System.Text.Json` to `System` would collapse distinct packages).
    ///
    /// Like `PackagePath`, this carries no `imported_names`/`bound_name`
    /// and never feeds call-resolution tier (b): a namespace `using`
    /// binds no symbol name a call site could be matched against (it
    /// makes *all* of the namespace's types visible — the aliased form
    /// `using F = X.Y.Z;` is different, and maps to `Qualified`).
    NamespaceImport { path: String },
    /// A Rust fully-qualified-path reference with **no** `use`/`mod`
    /// bringing it into scope at all (`carto_core::Result<u8>` needs no
    /// `use carto_core;` — valid since Rust 2018) — ADR-0024. Unlike
    /// every other variant, this is **heuristic, not a verified
    /// declaration**: `root` is only kept when (a) it isn't inside a
    /// `use_declaration` already (that's `Absolute`'s job), (b) it
    /// isn't a `call_expression`'s own callee (`Type::method()`'s exact
    /// shape, ADR-0008's existing exclusion — capturing it here would
    /// reintroduce the ambiguity that exclusion exists to avoid), and
    /// (c) the root starts lowercase, matching Rust's crate-naming
    /// convention (crates.io itself nudges snake_case; types/traits/
    /// generic parameters are conventionally PascalCase) — the only
    /// signal available to tell a plausible crate name apart from a
    /// local `Type::associated_item` path without cross-referencing the
    /// whole symbol table. `resolve` classifies it exactly like
    /// `Absolute` (`known_modules`/`crate`/`self`/`super` internal
    /// check, dedup-by-root external `Module` node) but with a
    /// distinct evidence string, so a caller can tell a verified `use`
    /// declaration apart from this naming-convention heuristic. No
    /// `imported_names` — a bare reference binds no local name, so it
    /// can never feed call-resolution tier (b) either.
    BareReference { root: String },
}

/// A call expression's callee, before resolution. `line` locates it for
/// evidence/`unresolved_calls` reporting; which symbol it's textually
/// inside of is assigned afterward by line-range containment (`resolve`),
/// not captured here.
pub struct RawCallSite {
    pub callee_name: String,
    /// 1-based.
    pub line: u32,
}

/// One identifier appearing in a *type position* — a field or
/// parameter type, a return type, a base/implements/extends clause, a
/// generic type argument (ADR-0029). Distinct from [`RawCallSite`]:
/// referencing a type is not invoking it, so this feeds an
/// `EdgeKind::References` edge, never a `calls` edge, though it goes
/// through the exact same name-resolution tiers in `resolve`. Added to
/// close a real gap the calls-only model left: nothing in any
/// extractor captured `private readonly IFoo _foo;` or
/// `class Impl : IFoo` at all, so `deps IFoo --dir in` could only ever
/// answer from the `imports` edge's file-level granularity — accurate,
/// but far too coarse for "who actually uses this type" in a namespace/
/// module holding more than one exported name.
pub struct RawTypeRef {
    pub name: String,
    /// 1-based.
    pub line: u32,
}

/// One string literal an extractor recognized as sitting in a
/// contract-relevant position — ADR-0026/0027, not spec vocabulary. The
/// extractor decides *where* it looked (`position`, e.g. `"object-init:
/// Name"` for a C# anonymous-object member, `"aws_cloudwatch_metric_
/// alarm.metric_name"` for an HCL attribute); `.carto/contracts.json`
/// (built-in defaults + repo overrides, `crate::contracts`) decides what
/// that position *means* (category, producer/consumer role, confidence)
/// — kept out of the extractor so a repo can extend the vocabulary
/// without touching extraction code. `qualifier`, when the extractor's
/// language has one for this position (a C# metric's `Namespace` const,
/// an HCL alarm's sibling `namespace` attribute), disambiguates
/// same-spelled values in different scopes; deriving it is a per-
/// language mechanical concern, same as `position` itself, not
/// something the config file expresses (a scope narrower than the
/// original design's fully declarative qualifier rules — see ADR-0026's
/// "slice 1" note). An extractor emits nothing at all (no `RawLiteral`)
/// for a value it can't reduce to a plain string (interpolated HCL,
/// computed C#) — INV-8's honesty extended to this new node kind: no
/// value beats a guessed one.
pub struct RawLiteral {
    pub position: String,
    pub value: String,
    pub qualifier: Option<String>,
    /// 1-based.
    pub line: u32,
}

/// One Terraform reference expression found in a `.tf` file
/// (ADR-0041): `var.region`, `local.prefix`, `module.vpc.vpc_id`,
/// `aws_s3_bucket.logs.arn`, `data.aws_iam_policy_document.x.json`.
/// `segments` are the dotted names in order, with index/splat
/// operators (`[0]`, `[*]`, `.*`) skipped, not terminating —
/// `aws_instance.w[0].id` is `["aws_instance", "w", "id"]`.
/// Resolution is a dedicated directory-scoped pass
/// (`super::terraform`), not the generic call-site tier ladder:
/// Terraform's scope is exactly one directory.
pub struct RawTfRef {
    pub segments: Vec<String>,
    /// 1-based line of the reference's first token.
    pub line: u32,
}

/// One `module "name" { source = …, arg = … }` call (ADR-0042).
pub struct RawTfModuleCall {
    pub name: String,
    /// 1-based line of the `module` block's first line — matches the
    /// `module.<name>` symbol's `start_line`, which is how the resolver
    /// finds that symbol again.
    pub line: u32,
    /// The `source` attribute when it is a plain string literal.
    pub source: Option<String>,
    /// A `source` attribute exists but is not a plain literal (an
    /// interpolation Terraform itself would reject) — recorded as an
    /// unresolved miss rather than silently ignored.
    pub dynamic_source: bool,
    /// Argument names (attribute names in the block body other than the
    /// meta-arguments `source`, `version`, `count`, `for_each`,
    /// `providers`, `depends_on`) with their 1-based lines — each is an
    /// input to the called module's `variable` of that name.
    pub args: Vec<(String, u32)>,
}

/// A path-valued Terragrunt expression, classified structurally at
/// extraction time (ADR-0043). Only exact, statically decidable shapes
/// are recognized; everything else is [`TgPath::Unresolvable`] and
/// produces no edge — carto never executes Terragrunt functions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TgPath {
    /// A plain string literal (`"../vpc"`, `"git::https://…//mod"`).
    Literal(String),
    /// `"${get_terragrunt_dir()}<suffix>"` — the unit's own directory
    /// plus a literal suffix (empty when nothing follows).
    TerragruntDir(String),
    /// `find_in_parent_folders()` / `find_in_parent_folders("name")`.
    FindInParentFolders(Option<String>),
    /// `get_repo_root()`, `path_relative_to_include()`,
    /// `include.x.locals.y`, string interpolation of anything else, …
    Unresolvable,
}

/// One `dependency "name" { config_path = … }` block.
pub struct RawTgDependency {
    pub name: String,
    /// 1-based first line of the block — matches the
    /// `dependency.<name>` symbol's `start_line`.
    pub line: u32,
    pub config_path: TgPath,
}

/// What a Terragrunt file (`terragrunt.hcl`, `root.hcl`) says about its
/// relations to other files (ADR-0043).
#[derive(Default)]
pub struct RawTerragrunt {
    /// `terraform { source = … }`.
    pub source: Option<TgPath>,
    pub dependencies: Vec<RawTgDependency>,
    /// `dependencies { paths = [ … ] }`.
    pub dependencies_paths: Vec<TgPath>,
    /// Every `include` / `include "label"` block's `path`.
    pub includes: Vec<TgPath>,
    /// Identifier-shaped keys of the top-level `inputs = { … }` object.
    pub inputs: Vec<String>,
}

/// What a `.tf` file's extraction carries beyond `symbols` (ADR-0041).
/// `None` on [`ExtractOut::terraform`] for every non-Terraform file.
#[derive(Default)]
pub struct TerraformFacts {
    /// `Some(suffix)` for an environment-variant file such as
    /// `locals.tf.simu` (the part after `.tf.`); `None` for a plain
    /// `*.tf` file.
    pub variant: Option<String>,
    /// `override.tf` / `*_override.tf` — Terraform merges these over
    /// same-named declarations, so a duplicate definition here is not
    /// a true duplicate.
    pub is_override: bool,
    pub refs: Vec<RawTfRef>,
    /// ADR-0042.
    pub module_calls: Vec<RawTfModuleCall>,
    /// `Some` for a Terragrunt file (ADR-0043). Such a file is its own
    /// resolution scope (not its directory's): Terragrunt `locals` are
    /// file-local, and a `local.x` there must never match a `.tf` local
    /// that happens to sit in the same directory.
    pub terragrunt: Option<RawTerragrunt>,
}

/// One file's raw extraction output (spec §5.2: "symbols, imports,
/// call-sites").
#[derive(Default)]
pub struct ExtractOut {
    pub symbols: Vec<RawSymbol>,
    pub imports: Vec<RawImport>,
    pub call_sites: Vec<RawCallSite>,
    /// String literals this extractor recognized in a contract-relevant
    /// position (ADR-0026). Empty for every extractor that doesn't look
    /// for any — today, every extractor except C#'s and HCL's.
    pub literals: Vec<RawLiteral>,
    /// Call sites this extractor recognizes but deliberately never
    /// attempts to resolve — Rust's path-qualified `Type::method()`/
    /// `module::func()` (ADR-0008) is the only producer today. Carried
    /// *only* to be counted (`resolve` aggregates these by bare callee
    /// name into `SymbolNode::uncaptured_inbound_calls`, ADR-0020) —
    /// never to produce an edge or feed any resolution tier. Distinct
    /// from `call_sites`, which *are* attempted and either resolve or
    /// land in a symbol's `unresolved_calls`; a call site belongs to
    /// exactly one of the two lists, never both.
    pub uncaptured_call_sites: Vec<RawCallSite>,
    /// Identifiers this extractor found in a type position (ADR-0029).
    /// Empty for an extractor that doesn't look for any yet. Unlike
    /// `uncaptured_call_sites`, there is no "attempted but excluded"
    /// counterpart for type refs — a name that doesn't resolve is
    /// silently dropped in `resolve` (see `RawTypeRef`'s own doc
    /// comment): overwhelmingly stdlib/BCL/third-party noise (`Task`,
    /// `string`, `ILogger`), not a signal worth a counter the way an
    /// unresolved call is.
    pub type_refs: Vec<RawTypeRef>,
    /// The file's declared namespace, PHP's `namespace App\Orders;`
    /// (ADR-0012) — `None` for a file with no namespace declaration
    /// (PHP's global namespace) and always `None` for Rust/Python, which
    /// have no equivalent. Feeds `resolve`'s repo-wide FQN index; not
    /// itself a `RawImport`, since it isn't an import.
    pub declared_namespace: Option<String>,
    /// Terraform-specific facts (ADR-0041) — `Some` only for `.tf`
    /// (and `.tf.<variant>`) files.
    pub terraform: Option<TerraformFacts>,
}

pub trait LangExtractor {
    fn lang(&self) -> Lang;
    fn extensions(&self) -> &'static [&'static str];
    fn extract(&self, src: &[u8], relpath: &str) -> ExtractOut;
    /// Extractor name + version recorded on every `Symbol`/`Module` node
    /// this extractor's output produces (spec §4.1's `origin` field),
    /// e.g. `"lang-rust@1"`. Added in M1.b.2b when `resolve` started
    /// processing files from more than one extractor per repo — a single
    /// hard-coded `ORIGIN` constant stopped being accurate once a
    /// Python symbol could otherwise claim `lang-rust@1`.
    fn origin(&self) -> &'static str;

    /// Whether this language's package scope is the declaring file's own
    /// *directory* rather than the file itself (Go: files in one
    /// directory see each other's unexported symbols with no import at
    /// all — ADR-0015). Enables `resolve`'s directory-scoped resolution
    /// tier (a′) for this extractor's files. Defaults to `false` so every
    /// existing extractor is unaffected — Rust/Python/PHP/TS/TSX/JS all
    /// scope visibility at the file, not the directory.
    fn package_scope_is_directory(&self) -> bool {
        false
    }

    /// The separator this language's `declared_namespace` and
    /// `RawImport::Qualified` FQNs are spelled with — `resolve` composes
    /// its repo-wide FQN index (`fqn_to_file`) and splits namespace
    /// roots using the declaring file's own separator, so PHP's
    /// `App\Orders` and C#'s `Acme.Orders` (ADR-0016) coexist in one
    /// index without ever falsely matching each other. Defaults to
    /// PHP's `\` — the only namespace-based language before C# —
    /// so existing extractors are unaffected; only `CSharpExtractor`
    /// overrides it (to `.`). Irrelevant for languages that never set
    /// `declared_namespace` or emit `Qualified`/`NamespaceImport`.
    fn namespace_separator(&self) -> &'static str {
        "\\"
    }

    /// Whether an unresolved `RawImport::Qualified` FQN's external
    /// `Module` node is keyed by the *whole* FQN string rather than its
    /// first `namespace_separator()`-delimited segment. Defaults to
    /// `false` (PHP's Composer-style root grouping — `Psr\Log\X` and
    /// `Psr\Http\Y` share one `Psr` node, ADR-0012). `CSharpExtractor`
    /// overrides to `true`: `using J = System.Text.Json.JsonSerializer;`
    /// must key the same full-string identity `RawImport::NamespaceImport`
    /// already uses for `using System.Text.Json;` (ADR-0016) — truncating
    /// to `System` would collapse distinct packages together.
    fn qualified_external_is_full_fqn(&self) -> bool {
        false
    }

    /// Whether this language's `RawImport::Relative` with empty
    /// `imported_names` is a *module declaration* — Rust's `mod foo;`,
    /// which makes `foo` a locally known module name other files' `use`
    /// paths can reference (`resolve`'s `known_modules`) — rather than
    /// an ordinary relative import that merely imports nothing by name
    /// (TS/JS's side-effect/default/namespace imports, ADR-0013, which
    /// produce the same empty-names shape but declare no reusable
    /// module name). Defaults to `false`; only `RustExtractor`
    /// overrides it.
    fn relative_import_declares_module(&self) -> bool {
        false
    }
}

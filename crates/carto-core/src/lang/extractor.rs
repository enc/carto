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
    /// b]`). Rust's `use_list` (`use a::{b, c};`), `use_wildcard`
    /// (`use a::*;`), and `use_as_clause` (`use a::b as c;`) are not
    /// extracted — deliberately modest (§5.3), documented in ADR-0008;
    /// Python's wildcard `from x import *` is excluded the same way,
    /// documented in ADR-0011. TypeScript/JavaScript's bare package
    /// specifiers (`import x from 'lodash'`, `root: "lodash"`; a scoped
    /// package `@scope/pkg` roots at its first *two* `/`-separated
    /// segments) use this too — ADR-0013.
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

/// One file's raw extraction output (spec §5.2: "symbols, imports,
/// call-sites").
#[derive(Default)]
pub struct ExtractOut {
    pub symbols: Vec<RawSymbol>,
    pub imports: Vec<RawImport>,
    pub call_sites: Vec<RawCallSite>,
    /// The file's declared namespace, PHP's `namespace App\Orders;`
    /// (ADR-0012) — `None` for a file with no namespace declaration
    /// (PHP's global namespace) and always `None` for Rust/Python, which
    /// have no equivalent. Feeds `resolve`'s repo-wide FQN index; not
    /// itself a `RawImport`, since it isn't an import.
    pub declared_namespace: Option<String>,
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
}
